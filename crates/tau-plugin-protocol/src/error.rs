//! Errors emitted by the framing and codec layers, plus the
//! MessagePack-RPC error envelope used inside [`crate::Frame`] responses.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Failures from the framing and codec layers.
///
/// `#[non_exhaustive]`: additive variants do not break callers.
///
/// # Example
///
/// ```
/// use tau_plugin_protocol::ProtocolError;
/// let err = ProtocolError::FrameTooLarge { len: 1, max: 0 };
/// assert!(format!("{err}").contains("frame too large"));
/// ```
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// Underlying IO error from the transport.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The receiving side observed end-of-stream while expecting more
    /// bytes. For host-side framers, this typically means the plugin
    /// process exited.
    #[error("frame truncated: expected {expected} more bytes, got EOF")]
    FrameTruncated {
        /// How many more bytes were expected.
        expected: usize,
    },

    /// A frame's length-prefix exceeded the configured max.
    #[error("frame too large: {len} bytes (max {max})")]
    FrameTooLarge {
        /// Reported length from the prefix.
        len: usize,
        /// Configured maximum.
        max: usize,
    },

    /// The frame body failed to decode as MessagePack.
    #[error("body decode failed: {0}")]
    BodyDecodeFailed(#[from] rmp_serde::decode::Error),

    /// The frame body nested MessagePack containers more deeply than
    /// [`crate::MAX_DECODE_DEPTH`].
    ///
    /// The decoder recurses once per container level, so nesting depth
    /// is a *stack* budget, and the bytes that set it come from an
    /// untrusted plugin: roughly one input byte buys one stack frame.
    /// This is a distinct variant rather than a
    /// [`ProtocolError::BodyDecodeFailed`] because it reports a limit
    /// tau chose, not malformed input — a host may reasonably log or
    /// alert on it differently.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_plugin_protocol::{Frame, ProtocolError, MAX_DECODE_DEPTH};
    /// // MAX_DECODE_DEPTH + 1 nested one-element arrays, then nil.
    /// let mut body = vec![0x91_u8; MAX_DECODE_DEPTH + 1];
    /// body.push(0xc0);
    /// let err = Frame::decode(&body).unwrap_err();
    /// assert!(matches!(err, ProtocolError::BodyTooDeep { .. }));
    /// ```
    #[error("frame body nests containers more than {max} deep")]
    BodyTooDeep {
        /// The configured maximum nesting depth.
        max: usize,
    },

    /// Body encoding failed.
    #[error("body encode failed: {0}")]
    BodyEncodeFailed(#[from] rmp_serde::encode::Error),

    /// A `Frame` slot that must carry rmp-serde-encoded MessagePack
    /// bytes (e.g. `params` on a request/notification or a non-`None`
    /// `result` on a response) was passed to [`crate::Frame::encode`]
    /// as an empty slice. The smallest legitimate payload is a
    /// one-byte `[0x90]` (empty MessagePack array); empty input would
    /// otherwise round-trip asymmetrically through `Value::Nil`.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_plugin_protocol::{Frame, ProtocolError};
    /// let frame = Frame::Request { id: 1, method: "m".into(), params: vec![] };
    /// let err = frame.encode().unwrap_err();
    /// assert!(matches!(err, ProtocolError::EmptyFrameSlot { slot: "params" }));
    /// ```
    #[error("empty frame slot: {slot} must contain rmp-serde-encoded bytes")]
    EmptyFrameSlot {
        /// Which slot was empty: `"params"` or `"result"`.
        slot: &'static str,
    },
}

/// MessagePack-RPC error envelope carried in the `error` slot of a
/// response frame.
///
/// The `code` follows JSON-RPC 2.0 conventions (see the constants in
/// this module). `message` is a short human-readable summary; `data`
/// carries optional structured payload (e.g. a serialized port-specific
/// error). Spec §4.7.
///
/// # Example
///
/// ```
/// use tau_plugin_protocol::{RpcErrorEnvelope, METHOD_NOT_FOUND};
/// let env = RpcErrorEnvelope::new(METHOD_NOT_FOUND, "method not found".into(), None);
/// assert_eq!(env.code, -32601);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcErrorEnvelope {
    /// Numeric error code. See the `*_ERROR` / `*_DENIED` constants.
    pub code: i32,
    /// Short, human-readable error description.
    pub message: String,
    /// Optional structured payload. For port-specific errors (codes in
    /// the [`PORT_SPECIFIC_ERROR_BASE`] range) this is the serialized
    /// `tau-ports` error type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<rmpv::Value>,
}

impl RpcErrorEnvelope {
    /// Construct an [`RpcErrorEnvelope`].
    pub fn new(code: i32, message: String, data: Option<rmpv::Value>) -> Self {
        Self {
            code,
            message,
            data,
        }
    }
}

/// Standard JSON-RPC parse-error code.
pub const PARSE_ERROR: i32 = -32700;
/// Standard JSON-RPC invalid-request code.
pub const INVALID_REQUEST: i32 = -32600;
/// Standard JSON-RPC method-not-found code.
pub const METHOD_NOT_FOUND: i32 = -32601;
/// Standard JSON-RPC invalid-params code.
pub const INVALID_PARAMS: i32 = -32602;
/// Standard JSON-RPC internal-error code.
pub const INTERNAL_ERROR: i32 = -32603;
/// Tau-specific: plugin contract violation.
pub const PLUGIN_CONTRACT_VIOLATION: i32 = -32000;
/// Tau-specific: capability check denied this method.
pub const CAPABILITY_DENIED: i32 = -32001;

/// Reserved range for port-specific recoverable errors. The `data`
/// field of the envelope carries the serialized tau-ports
/// `LlmError`/`ToolError`/etc. in this range.
pub const PORT_SPECIFIC_ERROR_BASE: i32 = -32100;
