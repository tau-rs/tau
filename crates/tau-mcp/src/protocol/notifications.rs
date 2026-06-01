//! Bidirectional notifications + cancellation.
//!
//! Per the β.3 design doc §4 (v0 scope) and §8.4 (cancellation
//! propagation). Notifications are fire-and-forget (no `id`, no
//! response). Cancellation is also a notification per MCP spec.

use alloc::string::String;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::jsonrpc::RequestId;

/// `notifications/progress` — host or server reporting progress on an
/// in-flight request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressNotification {
    /// Progress token (mirrors the request's `_meta.progressToken` if
    /// the caller asked for progress; otherwise free-form).
    #[serde(rename = "progressToken")]
    pub progress_token: Value,
    /// Current progress (units defined by the producer).
    pub progress: f64,
    /// Optional total to compute percentage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
}

/// `notifications/cancelled` — caller is aborting an in-flight request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CancelledNotification {
    /// The request id being cancelled.
    #[serde(rename = "requestId")]
    pub request_id: RequestId,
    /// Optional reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `notifications/initialized` — host signals it has finished processing
/// the `initialize` response.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct InitializedNotification {}

/// `notifications/message` (logging) — server emits a log line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogNotification {
    /// Log level (`"debug"` | `"info"` | `"warn"` | `"error"`).
    pub level: String,
    /// Logger name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logger: Option<String>,
    /// Free-form structured payload (server-defined).
    pub data: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use serde_json::json;

    #[test]
    fn progress_round_trips() {
        let n = ProgressNotification {
            progress_token: json!("call-7"),
            progress: 50.0,
            total: Some(100.0),
        };
        let bytes = serde_json::to_vec(&n).expect("serialize");
        let decoded: ProgressNotification = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(n, decoded);
    }

    #[test]
    fn cancelled_round_trips() {
        let n = CancelledNotification {
            request_id: RequestId::Number(7),
            reason: Some("user abort".to_string()),
        };
        let bytes = serde_json::to_vec(&n).expect("serialize");
        let decoded: CancelledNotification = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(n, decoded);
    }

    #[test]
    fn log_round_trips() {
        let n = LogNotification {
            level: "info".to_string(),
            logger: Some("weather".to_string()),
            data: json!({"msg":"forecast fetched","duration_ms":42}),
        };
        let bytes = serde_json::to_vec(&n).expect("serialize");
        let decoded: LogNotification = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(n, decoded);
    }
}
