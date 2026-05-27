//! JSON-RPC 2.0 message types for serve mode.
//!
//! Per spec §5: Request, Response, Notification, ErrorObject.
//! All types use serde for symmetric serialization.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 request id. Per spec, may be integer, string, or null.
/// We accept integer or string; null is treated as a notification
/// (handled separately).
///
/// ```
/// use tau_app::serve::RequestId;
///
/// let int_id = RequestId::Int(42);
/// let str_id = RequestId::Str("uuid-abc".into());
///
/// // Hash + Eq allow use as map keys.
/// let mut map = std::collections::HashMap::new();
/// map.insert(int_id.clone(), "request-42");
/// assert_eq!(map[&RequestId::Int(42)], "request-42");
///
/// // Serde round-trip (untagged: integer → JSON number, string → JSON string).
/// let json_int = serde_json::to_string(&int_id).expect("serialize int id");
/// assert_eq!(json_int, "42");
/// let json_str = serde_json::to_string(&str_id).expect("serialize str id");
/// assert_eq!(json_str, "\"uuid-abc\"");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    /// Integer id (most common).
    Int(i64),
    /// String id (UUIDs, etc.).
    Str(String),
}

/// JSON-RPC 2.0 request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// Always "2.0".
    pub jsonrpc: String,
    /// Request id. Absence means "notification" (handled by [`Notification`]).
    pub id: RequestId,
    /// Method name (e.g. "runtime.run").
    pub method: String,
    /// Method-specific params object. Absent when method takes no args.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC 2.0 successful response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// Always "2.0".
    pub jsonrpc: String,
    /// Matches the originating request id.
    pub id: RequestId,
    /// Method-specific result payload.
    pub result: Value,
}

/// JSON-RPC 2.0 error response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// Always "2.0".
    pub jsonrpc: String,
    /// Matches the originating request id.
    pub id: RequestId,
    /// Error payload.
    pub error: ErrorObject,
}

/// JSON-RPC 2.0 server-initiated notification (no id).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    /// Always "2.0".
    pub jsonrpc: String,
    /// Method name (e.g. "runtime.event").
    pub method: String,
    /// Method-specific params object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC 2.0 error payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorObject {
    /// JSON-RPC error code. See [`super::error_codes`].
    pub code: i32,
    /// Human-readable error message.
    pub message: String,
    /// Structured machine-actionable payload. Shape depends on `code`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Wire-level outbound message (request response, error response, or notification).
///
/// Serializes without a wrapper tag so each variant maps directly to a
/// JSON-RPC 2.0 object on the wire.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Outbound {
    /// Successful response to a request.
    Response(Response),
    /// Error response to a request.
    Error(ErrorResponse),
    /// Server-initiated notification.
    Notification(Notification),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_request_integer_id() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"method":"meta.ping"}"#;
        let req: Request = serde_json::from_str(raw).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.id, RequestId::Int(1));
        assert_eq!(req.method, "meta.ping");
        assert!(req.params.is_none());
    }

    #[test]
    fn parse_request_string_id() {
        let raw = r#"{"jsonrpc":"2.0","id":"abc","method":"meta.ping","params":{}}"#;
        let req: Request = serde_json::from_str(raw).unwrap();
        assert_eq!(req.id, RequestId::Str("abc".into()));
    }

    #[test]
    fn serialize_response_omits_none_data() {
        let out = Outbound::Response(Response {
            jsonrpc: "2.0".into(),
            id: RequestId::Int(1),
            result: json!({"ok": true}),
        });
        let s = serde_json::to_string(&out).unwrap();
        assert_eq!(s, r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#);
    }

    #[test]
    fn serialize_error_with_data() {
        let out = Outbound::Error(ErrorResponse {
            jsonrpc: "2.0".into(),
            id: RequestId::Int(3),
            error: ErrorObject {
                code: -32007,
                message: "Capability denied".into(),
                data: Some(json!({"kind": "CapabilityDenial"})),
            },
        });
        let s = serde_json::to_string(&out).unwrap();
        assert!(s.contains("\"code\":-32007"));
        assert!(s.contains("\"kind\":\"CapabilityDenial\""));
    }

    #[test]
    fn serialize_notification_no_id() {
        let out = Outbound::Notification(Notification {
            jsonrpc: "2.0".into(),
            method: "runtime.event".into(),
            params: Some(json!({"kind": "TextDelta"})),
        });
        let s = serde_json::to_string(&out).unwrap();
        assert!(!s.contains("\"id\":"));
        assert!(s.contains("\"method\":\"runtime.event\""));
    }
}
