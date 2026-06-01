//! JSON-RPC 2.0 envelopes used by MCP.
//!
//! MCP uses JSON-RPC 2.0 (https://www.jsonrpc.org/specification) as its
//! wire format, with `jsonrpc: "2.0"` on every envelope. Three envelope
//! kinds:
//!
//! - [`JsonRpcRequest`] — a method call expecting a response.
//! - [`JsonRpcResponse`] — a response (success `result` or error
//!   `JsonRpcError`).
//! - [`JsonRpcNotification`] — a fire-and-forget message (no `id`,
//!   no response).
//!
//! [`JsonRpcMessage`] is the discriminated-union shape used over the
//! wire — a single `serde_json::Value::Object` is parsed once and
//! routed to the right variant by the presence/absence of `id` and
//! `result`/`error`.

use alloc::string::String;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC version this implementation speaks.
pub const JSONRPC_VERSION: &str = "2.0";

/// JSON-RPC 2.0 request-id. Per spec: number or string (or null for
/// notifications — see [`JsonRpcNotification`] for those).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    /// Integer id (the common case in MCP).
    Number(i64),
    /// String id.
    String(String),
}

/// A JSON-RPC 2.0 request envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Request id (echoed in the response).
    pub id: RequestId,
    /// Method name (e.g. `"tools/call"`).
    pub method: String,
    /// Method-specific parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// A JSON-RPC 2.0 response envelope (success).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Echoed request id.
    pub id: RequestId,
    /// Success result; `None` if this is an error response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error payload; `None` on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// Error code (negative integers per JSON-RPC spec; MCP defines its
    /// own range for protocol-level errors).
    pub code: i32,
    /// Human-readable error message.
    pub message: String,
    /// Optional additional payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// A JSON-RPC 2.0 notification envelope (no id, no response expected).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Method name (e.g. `"notifications/progress"`).
    pub method: String,
    /// Method-specific parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// Discriminated-union envelope over the wire.
///
/// A peer receives bytes, parses them once into `serde_json::Value`,
/// and routes by the presence/absence of `id` and `result`/`error`.
/// The `#[serde(untagged)]` attribute lets serde do this routing
/// automatically; the variant order matters — Request is tried first
/// (has `id` AND `method`), then Response (has `id` AND
/// `result`/`error`), then Notification (no `id`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    /// A method-call request.
    Request(JsonRpcRequest),
    /// A response to a prior request.
    Response(JsonRpcResponse),
    /// A fire-and-forget notification.
    Notification(JsonRpcNotification),
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;
    use serde_json::json;

    #[test]
    fn request_round_trips() {
        let req = JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::Number(7),
            method: "tools/call".to_string(),
            params: Some(json!({"name":"get_forecast","arguments":{"lat":40.7,"lon":-74.0}})),
        };
        let bytes = serde_json::to_vec(&req).expect("serialize");
        let decoded: JsonRpcRequest = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(req, decoded);
    }

    #[test]
    fn response_round_trips() {
        let resp = JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::Number(7),
            result: Some(json!({"content":[{"type":"text","text":"Sunny, 72°F"}]})),
            error: None,
        };
        let bytes = serde_json::to_vec(&resp).expect("serialize");
        let decoded: JsonRpcResponse = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(resp, decoded);
    }

    #[test]
    fn notification_round_trips() {
        let n = JsonRpcNotification {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: "notifications/progress".to_string(),
            params: Some(json!({"progressToken":"call-7","progress":50,"total":100})),
        };
        let bytes = serde_json::to_vec(&n).expect("serialize");
        let decoded: JsonRpcNotification = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(n, decoded);
    }

    #[test]
    fn untagged_routing_request() {
        let wire = json!({
            "jsonrpc":"2.0","id":7,"method":"tools/call",
            "params":{"name":"get_forecast"}
        });
        let msg: JsonRpcMessage = serde_json::from_value(wire).expect("route");
        assert!(matches!(msg, JsonRpcMessage::Request(_)));
    }

    #[test]
    fn untagged_routing_response_success() {
        let wire = json!({"jsonrpc":"2.0","id":7,"result":{"content":[]}});
        let msg: JsonRpcMessage = serde_json::from_value(wire).expect("route");
        assert!(matches!(msg, JsonRpcMessage::Response(_)));
    }

    #[test]
    fn untagged_routing_response_error() {
        let wire = json!({"jsonrpc":"2.0","id":7,"error":{"code":-32601,"message":"method not found"}});
        let msg: JsonRpcMessage = serde_json::from_value(wire).expect("route");
        assert!(matches!(msg, JsonRpcMessage::Response(_)));
    }

    #[test]
    fn untagged_routing_notification() {
        let wire = json!({"jsonrpc":"2.0","method":"notifications/progress","params":{}});
        let msg: JsonRpcMessage = serde_json::from_value(wire).expect("route");
        assert!(matches!(msg, JsonRpcMessage::Notification(_)));
    }

    #[test]
    fn request_id_string_round_trips() {
        let id = RequestId::String("req-7".to_string());
        let bytes = serde_json::to_vec(&id).expect("serialize");
        assert_eq!(&bytes, b"\"req-7\"");
        let decoded: RequestId = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(id, decoded);
    }

    #[test]
    fn jsonrpc_message_vec_round_trips() {
        let msgs = vec![
            JsonRpcMessage::Request(JsonRpcRequest {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: RequestId::Number(1),
                method: "initialize".to_string(),
                params: None,
            }),
            JsonRpcMessage::Notification(JsonRpcNotification {
                jsonrpc: JSONRPC_VERSION.to_string(),
                method: "notifications/initialized".to_string(),
                params: None,
            }),
        ];
        let bytes = serde_json::to_vec(&msgs).expect("serialize");
        let decoded: alloc::vec::Vec<JsonRpcMessage> =
            serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(msgs, decoded);
    }
}
