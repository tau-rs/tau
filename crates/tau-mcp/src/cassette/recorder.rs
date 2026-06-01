//! In-memory cassette recorder.
//!
//! Captures `CassetteMessage` records as they're produced (PR-3 wires
//! this into the host loop at the handler-dispatch boundary). The
//! `Recorder` itself is transport-agnostic — it gets called with
//! already-parsed `JsonRpcMessage` values; the transport layer is
//! responsible for handing them to the recorder before they're framed
//! / after they're parsed.
//!
//! File-I/O sink (`save_to_file`) requires `std`; the in-memory record
//! API is `no_std`-compatible.

use alloc::vec::Vec;

use crate::cassette::message::{
    CassetteHeader, CassetteMessage, Direction, MessageKind, CASSETTE_VERSION,
};
use crate::protocol::jsonrpc::{JsonRpcMessage, RequestId};

/// In-memory cassette accumulator.
#[derive(Debug, Default, Clone)]
pub struct Recorder {
    messages: Vec<CassetteMessage>,
}

impl Recorder {
    /// Construct an empty recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one JSON-RPC message with its direction.
    pub fn record(&mut self, dir: Direction, msg: &JsonRpcMessage) {
        let record = match msg {
            JsonRpcMessage::Request(r) => CassetteMessage {
                dir,
                kind: MessageKind::Request,
                id: Some(r.id.clone()),
                method: Some(r.method.clone()),
                payload: r.params.clone().unwrap_or(serde_json::Value::Null),
            },
            JsonRpcMessage::Response(r) => CassetteMessage {
                dir,
                kind: MessageKind::Response,
                id: Some(r.id.clone()),
                method: None,
                payload: if let Some(e) = &r.error {
                    serde_json::json!({"error": e})
                } else {
                    r.result.clone().unwrap_or(serde_json::Value::Null)
                },
            },
            JsonRpcMessage::Notification(n) => CassetteMessage {
                dir,
                kind: MessageKind::Notification,
                id: None,
                method: Some(n.method.clone()),
                payload: n.params.clone().unwrap_or(serde_json::Value::Null),
            },
        };
        self.messages.push(record);
    }

    /// Return the recorded messages.
    pub fn messages(&self) -> &[CassetteMessage] {
        &self.messages
    }

    /// Serialize the cassette to JSONL bytes (header line + one line
    /// per recorded message). Pure-allocator API; no I/O.
    pub fn to_jsonl_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        let mut out: Vec<u8> = Vec::new();
        let header = CassetteHeader {
            version: CASSETTE_VERSION,
        };
        out.extend_from_slice(&serde_json::to_vec(&header)?);
        out.push(b'\n');
        for m in &self.messages {
            out.extend_from_slice(&serde_json::to_vec(m)?);
            out.push(b'\n');
        }
        Ok(out)
    }

    /// Save the cassette to a file. Requires std.
    #[cfg(feature = "with-std-adapters")]
    pub fn save_to_file<P: AsRef<std::path::Path>>(&self, path: P) -> Result<(), String> {
        let bytes = self.to_jsonl_bytes().map_err(|e| alloc::format!("{e}"))?;
        std::fs::write(path, bytes).map_err(|e| alloc::format!("{e}"))?;
        Ok(())
    }

    /// Return how many records are stored, by request id, useful for
    /// asserting in tests.
    pub fn count_for(&self, id: &RequestId) -> usize {
        self.messages
            .iter()
            .filter(|m| m.id.as_ref() == Some(id))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::jsonrpc::{JsonRpcRequest, JsonRpcResponse, JSONRPC_VERSION};
    use alloc::string::ToString;
    use serde_json::json;

    #[test]
    fn records_request_and_response() {
        let mut r = Recorder::new();
        r.record(
            Direction::In,
            &JsonRpcMessage::Request(JsonRpcRequest {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: RequestId::Number(1),
                method: "initialize".to_string(),
                params: Some(json!({"protocolVersion":"2025-03-26"})),
            }),
        );
        r.record(
            Direction::Out,
            &JsonRpcMessage::Response(JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: RequestId::Number(1),
                result: Some(json!({"protocolVersion":"2025-03-26"})),
                error: None,
            }),
        );
        assert_eq!(r.messages().len(), 2);
        assert_eq!(r.count_for(&RequestId::Number(1)), 2);
    }

    #[test]
    fn jsonl_bytes_start_with_version_header() {
        let r = Recorder::new();
        let bytes = r.to_jsonl_bytes().expect("serialize");
        assert!(bytes.starts_with(br#"{"version":1}"#));
    }

    #[test]
    fn jsonl_bytes_per_message_line() {
        let mut r = Recorder::new();
        r.record(
            Direction::In,
            &JsonRpcMessage::Request(JsonRpcRequest {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: RequestId::Number(1),
                method: "x".to_string(),
                params: None,
            }),
        );
        let bytes = r.to_jsonl_bytes().expect("serialize");
        let s = core::str::from_utf8(&bytes).expect("utf8");
        let line_count = s.lines().count();
        assert_eq!(line_count, 2, "header + 1 record");
    }
}
