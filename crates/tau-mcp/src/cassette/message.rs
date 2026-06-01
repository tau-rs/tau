//! Cassette message record — one JSON line per MCP message.
//!
//! Per the β.3 design doc §11. JSONL format with a `{"version":1}` first
//! line followed by per-message records.

use alloc::string::String;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::jsonrpc::RequestId;

/// Cassette format version emitted by this crate.
pub const CASSETTE_VERSION: u32 = 1;

/// Direction of a cassette message (from the cassette's recording POV).
///
/// - [`Direction::In`] — message arrived INTO the cassette from the
///   host side (host sent it to the server).
/// - [`Direction::Out`] — message emitted OUT of the cassette to the
///   host side (server's reply or server-initiated request).
///
/// Mnemonic: replay direction is `Out` — the cassette is the server
/// stand-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Host → server (recorded inbound to the cassette).
    In,
    /// Server → host (cassette emits to host on replay).
    Out,
}

/// Kind of MCP message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageKind {
    /// A method call expecting a response.
    Request,
    /// A response to a prior request.
    Response,
    /// Fire-and-forget notification.
    Notification,
}

/// One cassette record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CassetteMessage {
    /// Direction (see [`Direction`] mnemonic).
    pub dir: Direction,
    /// Message kind.
    pub kind: MessageKind,
    /// Request id (None for notifications).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<RequestId>,
    /// Method name (None for response records).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Raw payload — params for request/notification, result/error for
    /// response.
    pub payload: Value,
}

/// The version-header first line of a cassette file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CassetteHeader {
    /// Format version.
    pub version: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use serde_json::json;

    #[test]
    fn header_round_trips() {
        let h = CassetteHeader { version: 1 };
        let bytes = serde_json::to_vec(&h).expect("serialize");
        assert_eq!(&bytes, br#"{"version":1}"#);
        let decoded: CassetteHeader = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(h, decoded);
    }

    #[test]
    fn message_request_round_trips() {
        let m = CassetteMessage {
            dir: Direction::In,
            kind: MessageKind::Request,
            id: Some(RequestId::Number(7)),
            method: Some("tools/call".to_string()),
            payload: json!({"name":"get_forecast","arguments":{"lat":40.7,"lon":-74.0}}),
        };
        let bytes = serde_json::to_vec(&m).expect("serialize");
        let decoded: CassetteMessage = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(m, decoded);
    }

    #[test]
    fn message_response_round_trips() {
        let m = CassetteMessage {
            dir: Direction::Out,
            kind: MessageKind::Response,
            id: Some(RequestId::Number(7)),
            method: None,
            payload: json!({"content":[{"type":"text","text":"sunny"}]}),
        };
        let bytes = serde_json::to_vec(&m).expect("serialize");
        let decoded: CassetteMessage = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(m, decoded);
    }

    #[test]
    fn message_notification_round_trips() {
        let m = CassetteMessage {
            dir: Direction::Out,
            kind: MessageKind::Notification,
            id: None,
            method: Some("notifications/progress".to_string()),
            payload: json!({"progressToken":"call-7","progress":50,"total":100}),
        };
        let bytes = serde_json::to_vec(&m).expect("serialize");
        let decoded: CassetteMessage = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(m, decoded);
    }
}
