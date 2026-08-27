//! MessagePack-RPC frame types (Request / Response / Notification).
//!
//! The wire shape (see spec §4.2) is a top-level MessagePack array
//! whose first element is a small integer type discriminator:
//!
//! * `[0, msgid, method, params]` — Request
//! * `[1, msgid, error, result]`  — Response
//! * `[2, method, params]`         — Notification
//!
//! [`Frame`] keeps `params` and `result` as **raw MessagePack bytes**
//! (the encoded form of the inner value, typically itself an array).
//! Callers decode their concrete request/response types via `rmp-serde`
//! on those bytes; this keeps `Frame` itself generic without an
//! intermediate `serde_json::Value` indirection.

use rmpv::Value;

use crate::error::{ProtocolError, RpcErrorEnvelope};

/// A single MessagePack-RPC frame body.
///
/// `#[non_exhaustive]`: future protocol revisions may add variants
/// without breaking callers.
///
/// # Example
///
/// ```
/// use tau_plugin_protocol::Frame;
/// let frame = Frame::Notification {
///     method: "stream.chunk".into(),
///     params: vec![0x90], // empty MessagePack array
/// };
/// let bytes = frame.clone().encode().unwrap();
/// let decoded = Frame::decode(&bytes).unwrap();
/// assert_eq!(frame, decoded);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// Request frame: `[0, id, method, params]`. The `params` field is
    /// the raw MessagePack-encoded bytes of the inner value (typically
    /// itself an array).
    Request {
        /// Request id; pairs with the matching `Response::id`.
        id: u32,
        /// Method name (e.g. `"llm.complete"`).
        method: String,
        /// MessagePack-encoded parameters.
        params: Vec<u8>,
    },
    /// Response frame: `[1, id, error, result]`. Exactly one of
    /// `error` / `result` is `Some` per the spec, but this type does
    /// not enforce that invariant on the wire — callers should.
    Response {
        /// Matches the `Request::id` this is a reply to.
        id: u32,
        /// `Some` if the call failed; `None` on success.
        error: Option<RpcErrorEnvelope>,
        /// `Some` MessagePack-encoded bytes on success; `None` on error.
        result: Option<Vec<u8>>,
    },
    /// Notification frame: `[2, method, params]`. Fire-and-forget; the
    /// receiver does not reply.
    Notification {
        /// Method name (e.g. `"stream.chunk"`).
        method: String,
        /// MessagePack-encoded parameters.
        params: Vec<u8>,
    },
}

const TYPE_REQUEST: i64 = 0;
const TYPE_RESPONSE: i64 = 1;
const TYPE_NOTIFICATION: i64 = 2;

/// Maximum MessagePack container nesting accepted in a frame body.
///
/// Exactly this many nested containers decode; one more yields
/// [`ProtocolError::BodyTooDeep`].
///
/// The decoder recurses per container level, so this is a **stack**
/// budget spent on behalf of an untrusted plugin, and it is cheap to
/// spend: one input byte of `0x91` (one-element array) buys one level.
/// tau pins it here rather than inheriting `rmpv`'s default of 1024,
/// which is not survivable — measured on this crate, a 1024-deep body
/// needs 64–128 KiB of stack in a release build, and in a debug build
/// it overflows a 2 MiB thread stack (tokio's worker default) somewhere
/// between depth 256 and 384, aborting the process. That made ~1 KiB of
/// attacker-chosen bytes a host-kill primitive (issue #676).
///
/// 128 matches `serde_json`'s recursion limit, so a payload that
/// survives a JSON hop survives this one.
pub const MAX_DECODE_DEPTH: usize = 128;

/// Recursion budget handed to `rmpv`.
///
/// `rmpv` spends two units per container level, so this is derived from
/// [`MAX_DECODE_DEPTH`] rather than equal to it. The exact resulting
/// boundary is pinned by `decode_accepts_nesting_at_the_depth_limit`, so
/// a change in rmpv's accounting fails a test rather than silently
/// moving tau's published limit.
const DECODER_RECURSION_BUDGET: usize = 2 * (MAX_DECODE_DEPTH + 1);

impl Frame {
    /// Decode a frame body (as produced by [`crate::FramedReader`])
    /// into a typed [`Frame`]. Malformed bodies (non-array, wrong
    /// arity, unknown type discriminator, wrong member types) return
    /// [`ProtocolError::BodyDecodeFailed`]; bodies nested deeper than
    /// [`MAX_DECODE_DEPTH`] return [`ProtocolError::BodyTooDeep`].
    ///
    /// # Resource bounds
    ///
    /// `body` is untrusted — it arrives from a plugin subprocess. Two
    /// bounds hold for every input:
    ///
    /// * **Stack**: recursion is capped at [`MAX_DECODE_DEPTH`].
    /// * **Heap**: peak allocation is proportional to `body.len()`, not
    ///   to any length a container header *declares*. A five-byte
    ///   `str32` announcing 4 GiB reserves 64 KiB, not 4 GiB. Pinned by
    ///   `tests/decode_allocation_bound.rs`.
    pub fn decode(body: &[u8]) -> Result<Frame, ProtocolError> {
        let mut cursor = body;
        let value: Value = read_value_bounded(&mut cursor)?;

        let array = match value {
            Value::Array(a) => a,
            _ => return Err(decode_msg("frame body is not a MessagePack array")),
        };

        let ty = array
            .first()
            .and_then(value_as_i64)
            .ok_or_else(|| decode_msg("frame missing integer type discriminator"))?;

        match ty {
            TYPE_REQUEST => decode_request(&array),
            TYPE_RESPONSE => decode_response(&array),
            TYPE_NOTIFICATION => decode_notification(&array),
            other => Err(decode_msg(&format!(
                "unknown frame type discriminator: {other}"
            ))),
        }
    }

    /// Encode this frame to MessagePack-RPC wire bytes.
    ///
    /// Returns [`ProtocolError::EmptyFrameSlot`] if `params`
    /// (Request/Notification) or a `Some(_)` `result` (Response) is
    /// empty — legitimate callers always pass non-empty
    /// rmp-serde-encoded bytes (smallest is `[0x90]` for an empty
    /// array), and accepting empty input would round-trip
    /// asymmetrically through `Value::Nil`.
    pub fn encode(self) -> Result<Vec<u8>, ProtocolError> {
        let value = match self {
            Frame::Request { id, method, params } => {
                if params.is_empty() {
                    return Err(ProtocolError::EmptyFrameSlot { slot: "params" });
                }
                Value::Array(vec![
                    Value::Integer(TYPE_REQUEST.into()),
                    Value::Integer(u64::from(id).into()),
                    Value::String(method.into()),
                    bytes_to_value(&params)?,
                ])
            }
            Frame::Response { id, error, result } => {
                let error_val = match error {
                    Some(env) => rmpv::ext::to_value(&env).map_err(encode_err)?,
                    None => Value::Nil,
                };
                let result_val = match result {
                    Some(bytes) => {
                        if bytes.is_empty() {
                            return Err(ProtocolError::EmptyFrameSlot { slot: "result" });
                        }
                        bytes_to_value(&bytes)?
                    }
                    None => Value::Nil,
                };
                Value::Array(vec![
                    Value::Integer(TYPE_RESPONSE.into()),
                    Value::Integer(u64::from(id).into()),
                    error_val,
                    result_val,
                ])
            }
            Frame::Notification { method, params } => {
                if params.is_empty() {
                    return Err(ProtocolError::EmptyFrameSlot { slot: "params" });
                }
                Value::Array(vec![
                    Value::Integer(TYPE_NOTIFICATION.into()),
                    Value::String(method.into()),
                    bytes_to_value(&params)?,
                ])
            }
        };

        let mut out = Vec::new();
        rmpv::encode::write_value(&mut out, &value).map_err(|e| {
            ProtocolError::BodyEncodeFailed(rmp_serde::encode::Error::InvalidValueWrite(e))
        })?;
        Ok(out)
    }
}

fn decode_request(array: &[Value]) -> Result<Frame, ProtocolError> {
    if array.len() != 4 {
        return Err(decode_msg(&format!(
            "request frame must have 4 elements, got {}",
            array.len()
        )));
    }
    let id = value_as_u32(&array[1])
        .ok_or_else(|| decode_msg("request msgid is not a u32-compatible integer"))?;
    let method =
        value_as_string(&array[2]).ok_or_else(|| decode_msg("request method is not a string"))?;
    let params = value_to_bytes(&array[3])?;
    Ok(Frame::Request { id, method, params })
}

fn decode_response(array: &[Value]) -> Result<Frame, ProtocolError> {
    if array.len() != 4 {
        return Err(decode_msg(&format!(
            "response frame must have 4 elements, got {}",
            array.len()
        )));
    }
    let id = value_as_u32(&array[1])
        .ok_or_else(|| decode_msg("response msgid is not a u32-compatible integer"))?;
    let error = match &array[2] {
        Value::Nil => None,
        v => Some(rmpv::ext::from_value::<RpcErrorEnvelope>(v.clone()).map_err(decode_err)?),
    };
    let result = match &array[3] {
        Value::Nil => None,
        v => Some(value_to_bytes(v)?),
    };
    Ok(Frame::Response { id, error, result })
}

fn decode_notification(array: &[Value]) -> Result<Frame, ProtocolError> {
    if array.len() != 3 {
        return Err(decode_msg(&format!(
            "notification frame must have 3 elements, got {}",
            array.len()
        )));
    }
    let method = value_as_string(&array[1])
        .ok_or_else(|| decode_msg("notification method is not a string"))?;
    let params = value_to_bytes(&array[2])?;
    Ok(Frame::Notification { method, params })
}

/// Re-serialize a single rmpv `Value` back to its MessagePack byte
/// representation. Used to keep `params`/`result` fields opaque on the
/// way out of `decode`.
fn value_to_bytes(value: &Value) -> Result<Vec<u8>, ProtocolError> {
    let mut out = Vec::new();
    rmpv::encode::write_value(&mut out, value).map_err(|e| {
        ProtocolError::BodyEncodeFailed(rmp_serde::encode::Error::InvalidValueWrite(e))
    })?;
    Ok(out)
}

/// Decode raw MessagePack bytes back into an `rmpv::Value`. Used when
/// re-encoding a `Frame` to splice opaque `params`/`result` blobs into
/// the outer array. Callers in [`Frame::encode`] reject empty input
/// before reaching this helper (see [`ProtocolError::EmptyFrameSlot`]),
/// so this function assumes `bytes` is non-empty rmp-serde-encoded
/// MessagePack.
fn bytes_to_value(bytes: &[u8]) -> Result<Value, ProtocolError> {
    let mut cursor = bytes;
    read_value_bounded(&mut cursor)
}

/// `rmpv::decode::read_value` with tau's own recursion cap
/// ([`MAX_DECODE_DEPTH`]) instead of rmpv's default of 1024, mapping
/// rmpv's depth error onto the typed [`ProtocolError::BodyTooDeep`].
///
/// Every path that turns untrusted bytes into an `rmpv::Value` goes
/// through here, so the bound cannot be bypassed by reaching a decoder
/// entry point directly.
fn read_value_bounded(cursor: &mut &[u8]) -> Result<Value, ProtocolError> {
    rmpv::decode::read_value_with_max_depth(cursor, DECODER_RECURSION_BUDGET).map_err(|e| match e {
        rmpv::decode::Error::DepthLimitExceeded => ProtocolError::BodyTooDeep {
            max: MAX_DECODE_DEPTH,
        },
        other => decode_err(other),
    })
}

fn value_as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Integer(i) => i.as_i64(),
        _ => None,
    }
}

fn value_as_u32(value: &Value) -> Option<u32> {
    let n = match value {
        Value::Integer(i) => i.as_u64()?,
        _ => return None,
    };
    u32::try_from(n).ok()
}

fn value_as_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => s.as_str().map(|s| s.to_owned()),
        _ => None,
    }
}

/// Wrap an `rmpv` decode error in [`ProtocolError::BodyDecodeFailed`].
/// Goes through `rmp_serde::decode::Error` to match the existing
/// variant.
fn decode_err<E: std::fmt::Display>(err: E) -> ProtocolError {
    decode_msg(&err.to_string())
}

fn decode_msg(msg: &str) -> ProtocolError {
    ProtocolError::BodyDecodeFailed(rmp_serde::decode::Error::Uncategorized(msg.to_owned()))
}

fn encode_err<E: std::fmt::Display>(err: E) -> ProtocolError {
    ProtocolError::BodyEncodeFailed(rmp_serde::encode::Error::Syntax(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a small MessagePack-encoded payload to use as
    /// `params` / `result`.
    fn sample_params() -> Vec<u8> {
        // Encode `["hello", 42]` as MessagePack.
        let value = Value::Array(vec![
            Value::String("hello".into()),
            Value::Integer(42i64.into()),
        ]);
        let mut out = Vec::new();
        rmpv::encode::write_value(&mut out, &value).unwrap();
        out
    }

    #[test]
    fn request_round_trip() {
        let frame = Frame::Request {
            id: 42,
            method: "llm.complete".into(),
            params: sample_params(),
        };
        let bytes = frame.clone().encode().unwrap();
        let decoded = Frame::decode(&bytes).unwrap();
        assert_eq!(frame, decoded);
    }

    #[test]
    fn response_ok_round_trip() {
        let frame = Frame::Response {
            id: 42,
            error: None,
            result: Some(sample_params()),
        };
        let bytes = frame.clone().encode().unwrap();
        let decoded = Frame::decode(&bytes).unwrap();
        assert_eq!(frame, decoded);
    }

    #[test]
    fn response_error_round_trip() {
        let frame = Frame::Response {
            id: 42,
            error: Some(RpcErrorEnvelope {
                code: -32601,
                message: "method not found".into(),
                data: None,
            }),
            result: None,
        };
        let bytes = frame.clone().encode().unwrap();
        let decoded = Frame::decode(&bytes).unwrap();
        assert_eq!(frame, decoded);
    }

    #[test]
    fn response_error_round_trip_with_data() {
        let envelope = RpcErrorEnvelope {
            code: -32100,
            message: "rate_limited".to_string(),
            data: Some(rmpv::Value::Map(vec![
                (
                    rmpv::Value::String("retry_after".into()),
                    rmpv::Value::Integer(60.into()),
                ),
                (
                    rmpv::Value::String("limit".into()),
                    rmpv::Value::Integer(100.into()),
                ),
            ])),
        };
        let frame = Frame::Response {
            id: 99,
            error: Some(envelope.clone()),
            result: None,
        };
        let bytes = frame.clone().encode().unwrap();
        let decoded = Frame::decode(&bytes).unwrap();
        assert_eq!(decoded, frame);
        // Extra assertion that data round-trips bit-for-bit
        let Frame::Response {
            error: Some(env), ..
        } = decoded
        else {
            panic!()
        };
        assert_eq!(env, envelope);
    }

    #[test]
    fn notification_round_trip() {
        let frame = Frame::Notification {
            method: "stream.chunk".into(),
            params: sample_params(),
        };
        let bytes = frame.clone().encode().unwrap();
        let decoded = Frame::decode(&bytes).unwrap();
        assert_eq!(frame, decoded);
    }

    #[test]
    fn decode_rejects_non_array_body() {
        let body = rmp_serde::to_vec(&"hello").unwrap();
        let err = Frame::decode(&body).unwrap_err();
        assert!(
            matches!(err, ProtocolError::BodyDecodeFailed(_)),
            "expected BodyDecodeFailed, got {err:?}"
        );
    }

    #[test]
    fn decode_rejects_wrong_arity_array() {
        // 5-element request-shaped array.
        let value = Value::Array(vec![
            Value::Integer(0i64.into()),
            Value::Integer(1u64.into()),
            Value::String("m".into()),
            Value::Nil,
            Value::Nil,
        ]);
        let mut body = Vec::new();
        rmpv::encode::write_value(&mut body, &value).unwrap();
        let err = Frame::decode(&body).unwrap_err();
        assert!(
            matches!(err, ProtocolError::BodyDecodeFailed(_)),
            "expected BodyDecodeFailed, got {err:?}"
        );
    }

    #[test]
    fn decode_rejects_unknown_type_discriminator() {
        let value = Value::Array(vec![
            Value::Integer(3i64.into()),
            Value::Integer(1u64.into()),
            Value::String("m".into()),
            Value::Nil,
        ]);
        let mut body = Vec::new();
        rmpv::encode::write_value(&mut body, &value).unwrap();
        let err = Frame::decode(&body).unwrap_err();
        assert!(
            matches!(err, ProtocolError::BodyDecodeFailed(_)),
            "expected BodyDecodeFailed, got {err:?}"
        );
    }

    /// A container header may declare a length far beyond the bytes that
    /// actually follow. Decoding must fail with a typed error — never
    /// reserve capacity for the declared length. Issue #676.
    ///
    /// The allocation side of this contract (peak bytes stay
    /// proportional to the *body* length, not the declared length) is
    /// asserted in `tests/decode_allocation_bound.rs`.
    #[test]
    fn decode_rejects_container_length_beyond_remaining_bytes() {
        // (name, body): a length prefix announcing up to 4 GiB of
        // payload, with no payload at all.
        let cases: [(&str, &[u8]); 7] = [
            ("array32 len=2^32-1", &[0xdd, 0xff, 0xff, 0xff, 0xff]),
            ("map32 len=2^32-1", &[0xdf, 0xff, 0xff, 0xff, 0xff]),
            ("str32 len=2^32-1", &[0xdb, 0xff, 0xff, 0xff, 0xff]),
            ("bin32 len=2^32-1", &[0xc6, 0xff, 0xff, 0xff, 0xff]),
            ("ext32 len=2^32-1", &[0xc9, 0xff, 0xff, 0xff, 0xff, 0x01]),
            ("array16 len=65535", &[0xdc, 0xff, 0xff]),
            ("str16 len=65535", &[0xda, 0xff, 0xff]),
        ];

        for (name, body) in cases {
            let err =
                Frame::decode(body).expect_err(&format!("{name} unexpectedly decoded to a Frame"));
            assert!(
                matches!(err, ProtocolError::BodyDecodeFailed(_)),
                "{name}: expected BodyDecodeFailed, got {err:?}"
            );
        }
    }

    /// Nesting past [`MAX_DECODE_DEPTH`] is rejected with a typed error.
    ///
    /// Before #676 this input reached `rmpv`'s own limit of 1024 instead,
    /// which is deep enough to **abort the process** with a stack
    /// overflow: measured at ~6–8 KiB of stack per level in a debug
    /// build, a 2 MiB thread stack (tokio's worker default) blows
    /// somewhere between depth 256 and 384. That made ~1 KiB of
    /// attacker-chosen bytes a host-kill primitive.
    #[test]
    fn decode_rejects_nesting_past_the_depth_limit() {
        // Far past the limit — this is the input that used to abort.
        let mut body = vec![0x91_u8; 8192];
        body.push(0xc0);

        let err = Frame::decode(&body).unwrap_err();
        assert!(
            matches!(err, ProtocolError::BodyTooDeep { max } if max == MAX_DECODE_DEPTH),
            "expected BodyTooDeep {{ max: {MAX_DECODE_DEPTH} }}, got {err:?}"
        );
    }

    /// The boundary itself: exactly at the limit decodes, one past it
    /// does not. Guards against an off-by-one when the constant moves.
    #[test]
    fn decode_accepts_nesting_at_the_depth_limit() {
        let at_limit = {
            let mut b = vec![0x91_u8; MAX_DECODE_DEPTH];
            b.push(0xc0);
            b
        };
        let past_limit = {
            let mut b = vec![0x91_u8; MAX_DECODE_DEPTH + 1];
            b.push(0xc0);
            b
        };

        // Just inside the limit the body is structurally fine, so it
        // fails later (wrong arity / bad discriminator) — not on depth.
        let err = Frame::decode(&at_limit).unwrap_err();
        assert!(
            matches!(err, ProtocolError::BodyDecodeFailed(_)),
            "just inside the limit, expected a structural error, got {err:?}"
        );

        let err = Frame::decode(&past_limit).unwrap_err();
        assert!(
            matches!(err, ProtocolError::BodyTooDeep { .. }),
            "one past the limit, expected BodyTooDeep, got {err:?}"
        );
    }

    /// The input libFuzzer saved when the `frame_decode` leg crossed its
    /// RSS limit in CI (run 33085192232, issue #676). It is a
    /// well-formed-enough notification carrying a deeply nested opaque
    /// `params` blob; the contract is that `decode` returns normally and
    /// the blob round-trips. Also lives in the fuzz corpus so the
    /// nightly keeps executing it.
    #[test]
    fn decode_handles_the_ci_oom_artifact() {
        let body = include_bytes!("../fuzz/corpus/frame_decode/regress_676_ci_oom_artifact");

        let frame = Frame::decode(body).expect("artifact should decode to a notification");
        let Frame::Notification { method, params } = &frame else {
            panic!("expected a Notification, got {frame:?}");
        };
        assert_eq!(method, "");
        assert!(!params.is_empty());

        // Re-encoding must not blow the depth bound either — the params
        // blob came in under it, so it must go back out under it.
        frame.clone().encode().expect("artifact should re-encode");
    }

    #[test]
    fn encode_rejects_empty_params() {
        let frame = Frame::Request {
            id: 1,
            method: "m".into(),
            params: vec![],
        };
        let err = frame.encode().unwrap_err();
        assert!(
            matches!(err, ProtocolError::EmptyFrameSlot { slot: "params" }),
            "expected EmptyFrameSlot {{ slot: \"params\" }}, got {err:?}"
        );

        let notif = Frame::Notification {
            method: "n".into(),
            params: vec![],
        };
        let err = notif.encode().unwrap_err();
        assert!(
            matches!(err, ProtocolError::EmptyFrameSlot { slot: "params" }),
            "expected EmptyFrameSlot {{ slot: \"params\" }}, got {err:?}"
        );
    }

    #[test]
    fn encode_rejects_empty_result() {
        let frame = Frame::Response {
            id: 1,
            error: None,
            result: Some(vec![]),
        };
        let err = frame.encode().unwrap_err();
        assert!(
            matches!(err, ProtocolError::EmptyFrameSlot { slot: "result" }),
            "expected EmptyFrameSlot {{ slot: \"result\" }}, got {err:?}"
        );
    }
}
