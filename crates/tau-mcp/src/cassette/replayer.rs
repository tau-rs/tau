//! Cassette replayer.
//!
//! Reads a cassette (JSONL bytes), matches inbound (host→server)
//! requests against recorded `Direction::In` entries by
//! (method, normalized args), and emits the matching recorded
//! `Direction::Out` responses + notifications.
//!
//! PR-3 wires this into a `Transport` impl that the in-memory test
//! harness uses (cassette-as-transport).

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use serde_json::Value;
use thiserror::Error;

use crate::cassette::message::{CassetteHeader, CassetteMessage, Direction, MessageKind, CASSETTE_VERSION};

/// Errors during cassette read or replay.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ReplayError {
    /// Cassette bytes are not valid UTF-8 JSONL.
    #[error("cassette parse error: {0}")]
    Parse(String),
    /// Cassette header version is newer than we support.
    #[error("cassette version {found} not supported (max {max})")]
    UnsupportedVersion {
        /// Version we found in the header.
        found: u32,
        /// Maximum version this crate supports.
        max: u32,
    },
    /// No matching inbound entry for an outbound request from the host.
    #[error("no cassette entry matches method {method:?} args {args}")]
    NoMatch {
        /// Method we couldn't match.
        method: String,
        /// Normalized args we tried to match.
        args: String,
    },
}

/// Read-only cassette replayer.
#[derive(Debug, Clone)]
pub struct Replayer {
    /// All messages, in cassette order.
    records: Vec<CassetteMessage>,
    /// Per-record consumption flag (true = already matched once;
    /// matches are one-shot in v0).
    consumed: Vec<bool>,
    /// FIFO queue of outbound (host-bound) records that should be
    /// emitted between matched calls (notifications, server-initiated
    /// requests). Filled when a matching request consumes the records
    /// between it and the matched response.
    pending_outbound: VecDeque<CassetteMessage>,
}

impl Replayer {
    /// Parse a cassette from JSONL bytes.
    pub fn from_jsonl_bytes(bytes: &[u8]) -> Result<Self, ReplayError> {
        let s = core::str::from_utf8(bytes).map_err(|e| ReplayError::Parse(alloc::format!("utf8: {e}")))?;
        let mut lines = s.lines();

        let header_line = lines
            .next()
            .ok_or_else(|| ReplayError::Parse("empty cassette".into()))?;
        let header: CassetteHeader = serde_json::from_str(header_line)
            .map_err(|e| ReplayError::Parse(alloc::format!("header: {e}")))?;
        if header.version > CASSETTE_VERSION {
            return Err(ReplayError::UnsupportedVersion {
                found: header.version,
                max: CASSETTE_VERSION,
            });
        }

        let mut records: Vec<CassetteMessage> = Vec::new();
        for (i, line) in lines.enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let rec: CassetteMessage = serde_json::from_str(line)
                .map_err(|e| ReplayError::Parse(alloc::format!("line {}: {e}", i + 2)))?;
            records.push(rec);
        }

        let consumed = vec![false; records.len()];
        Ok(Self {
            records,
            consumed,
            pending_outbound: VecDeque::new(),
        })
    }

    /// Attempt to match an inbound (host→server) request and return the
    /// recorded outbound response + any notifications/server-initiated
    /// requests that lie between the matched request and its response.
    ///
    /// The matched-request record (`Direction::In`) is consumed; the
    /// outbound records before the matching response are queued for
    /// `next_pending_outbound`; the response itself is returned.
    pub fn match_request(
        &mut self,
        method: &str,
        normalized_args: &Value,
    ) -> Result<CassetteMessage, ReplayError> {
        // Find the first unconsumed inbound request record in cassette
        // order. It MUST match the requested method + args — cassette
        // replay is strictly sequential; skipping over an unconsumed
        // inbound record is not allowed.
        let req_idx = self
            .records
            .iter()
            .enumerate()
            .find(|(i, r)| {
                !self.consumed[*i]
                    && r.dir == Direction::In
                    && r.kind == MessageKind::Request
            })
            .and_then(|(i, r)| {
                if r.method.as_deref() == Some(method)
                    && normalize(&r.payload) == *normalized_args
                {
                    Some(i)
                } else {
                    None
                }
            })
            .ok_or_else(|| ReplayError::NoMatch {
                method: method.into(),
                args: normalized_args.to_string(),
            })?;
        self.consumed[req_idx] = true;

        // Walk forward; queue Direction::Out records until we hit the
        // matching response (Direction::Out, kind=Response, same id).
        let req_id = self.records[req_idx].id.clone();
        let mut response: Option<CassetteMessage> = None;
        for i in (req_idx + 1)..self.records.len() {
            if self.consumed[i] {
                continue;
            }
            let rec = &self.records[i];
            if rec.dir != Direction::Out {
                continue;
            }
            if rec.kind == MessageKind::Response && rec.id == req_id {
                self.consumed[i] = true;
                response = Some(rec.clone());
                break;
            }
            // Notifications or server-initiated requests between the
            // host's request and the server's response.
            self.consumed[i] = true;
            self.pending_outbound.push_back(rec.clone());
        }

        response.ok_or_else(|| {
            ReplayError::NoMatch {
                method: alloc::format!("response to {method}"),
                args: alloc::format!("id={req_id:?}"),
            }
        })
    }

    /// Drain one pending outbound record (notification or
    /// server-initiated request) for the host to consume between
    /// `match_request` calls.
    pub fn next_pending_outbound(&mut self) -> Option<CassetteMessage> {
        self.pending_outbound.pop_front()
    }
}

/// Normalize a JSON value for comparison (deep, key-sorted, whitespace-
/// independent). Implementation reuses serde_json's BTreeMap-backed
/// `Map` ordering by re-serializing to a string and parsing back.
fn normalize(v: &Value) -> Value {
    // Round-trip through bytes — Map is BTreeMap (no preserve_order
    // feature) so keys come out sorted.
    let bytes = serde_json::to_vec(v).unwrap_or_default();
    serde_json::from_slice(&bytes).unwrap_or_else(|_| v.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cassette::recorder::Recorder;
    use crate::protocol::jsonrpc::{
        JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, RequestId,
        JSONRPC_VERSION,
    };
    use alloc::string::ToString;
    use serde_json::json;

    fn build_weather_cassette() -> Vec<u8> {
        let mut r = Recorder::new();
        // initialize handshake
        r.record(
            Direction::In,
            &JsonRpcMessage::Request(JsonRpcRequest {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: RequestId::Number(0),
                method: "initialize".to_string(),
                params: Some(json!({"protocolVersion":"2025-03-26"})),
            }),
        );
        r.record(
            Direction::Out,
            &JsonRpcMessage::Response(JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: RequestId::Number(0),
                result: Some(json!({"protocolVersion":"2025-03-26"})),
                error: None,
            }),
        );
        // tools/list
        r.record(
            Direction::In,
            &JsonRpcMessage::Request(JsonRpcRequest {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: RequestId::Number(1),
                method: "tools/list".to_string(),
                params: None,
            }),
        );
        r.record(
            Direction::Out,
            &JsonRpcMessage::Response(JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: RequestId::Number(1),
                result: Some(json!({"tools":[{"name":"get_forecast","inputSchema":{"type":"object"}}]})),
                error: None,
            }),
        );
        // tools/call (with mid-request progress notification)
        r.record(
            Direction::In,
            &JsonRpcMessage::Request(JsonRpcRequest {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: RequestId::Number(2),
                method: "tools/call".to_string(),
                params: Some(json!({"name":"get_forecast","arguments":{"lat":40.7,"lon":-74.0}})),
            }),
        );
        r.record(
            Direction::Out,
            &JsonRpcMessage::Notification(JsonRpcNotification {
                jsonrpc: JSONRPC_VERSION.to_string(),
                method: "notifications/progress".to_string(),
                params: Some(json!({"progressToken":"call-2","progress":50,"total":100})),
            }),
        );
        r.record(
            Direction::Out,
            &JsonRpcMessage::Response(JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: RequestId::Number(2),
                result: Some(json!({"content":[{"type":"text","text":"Sunny, 72°F"}]})),
                error: None,
            }),
        );
        r.to_jsonl_bytes().expect("serialize")
    }

    #[test]
    fn parses_well_formed_cassette() {
        let bytes = build_weather_cassette();
        let r = Replayer::from_jsonl_bytes(&bytes).expect("parse");
        assert_eq!(r.records.len(), 7);
    }

    #[test]
    fn matches_initialize_and_returns_response() {
        let bytes = build_weather_cassette();
        let mut r = Replayer::from_jsonl_bytes(&bytes).expect("parse");
        let resp = r
            .match_request("initialize", &json!({"protocolVersion":"2025-03-26"}))
            .expect("match");
        assert_eq!(resp.kind, MessageKind::Response);
        assert_eq!(resp.id, Some(RequestId::Number(0)));
    }

    #[test]
    fn matches_tools_call_and_yields_progress_then_response() {
        let bytes = build_weather_cassette();
        let mut r = Replayer::from_jsonl_bytes(&bytes).expect("parse");
        // consume earlier records first
        r.match_request("initialize", &json!({"protocolVersion":"2025-03-26"}))
            .expect("init");
        r.match_request("tools/list", &Value::Null).expect("list");

        let resp = r
            .match_request(
                "tools/call",
                &json!({"name":"get_forecast","arguments":{"lat":40.7,"lon":-74.0}}),
            )
            .expect("call");
        // The progress notification should be queued for the host to
        // consume next.
        let pending = r.next_pending_outbound().expect("progress");
        assert_eq!(pending.kind, MessageKind::Notification);
        assert_eq!(
            pending.method.as_deref(),
            Some("notifications/progress")
        );
        assert_eq!(resp.kind, MessageKind::Response);
    }

    #[test]
    fn unknown_method_errors() {
        let bytes = build_weather_cassette();
        let mut r = Replayer::from_jsonl_bytes(&bytes).expect("parse");
        let err = r
            .match_request("bogus", &Value::Null)
            .expect_err("should not match");
        assert!(matches!(err, ReplayError::NoMatch { .. }));
    }

    #[test]
    fn unsupported_version_errors() {
        let bytes = br#"{"version":99}
"#;
        let err = Replayer::from_jsonl_bytes(bytes).expect_err("should reject");
        assert!(matches!(err, ReplayError::UnsupportedVersion { .. }));
    }

    #[test]
    fn key_order_normalization_independent() {
        let bytes = build_weather_cassette();
        let mut r = Replayer::from_jsonl_bytes(&bytes).expect("parse");
        // Same args, different key order — should still match.
        let resp = r
            .match_request(
                "tools/call",
                &json!({"arguments":{"lon":-74.0,"lat":40.7},"name":"get_forecast"}),
            );
        // First call: not yet matched (must consume earlier records).
        assert!(resp.is_err()); // tools/call comes after init+list, so unmatched at this point

        // Reset by re-parsing
        let mut r = Replayer::from_jsonl_bytes(&bytes).expect("parse");
        r.match_request("initialize", &json!({"protocolVersion":"2025-03-26"}))
            .expect("init");
        r.match_request("tools/list", &Value::Null).expect("list");
        let resp = r
            .match_request(
                "tools/call",
                &json!({"arguments":{"lon":-74.0,"lat":40.7},"name":"get_forecast"}),
            )
            .expect("normalized match");
        assert_eq!(resp.kind, MessageKind::Response);
    }
}
