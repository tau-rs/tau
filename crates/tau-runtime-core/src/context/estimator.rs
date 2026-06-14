//! Token-count estimators for context-manager budget accounting.

use tau_domain::{Message, MessagePayload, Value};

/// Estimates the token cost of a message. v1 ships [`HeuristicEstimator`];
/// a real per-model tokenizer can replace it behind this trait without
/// changing the transformer contract.
pub trait TokenEstimator: Send + Sync {
    /// Approximate token count for one message.
    fn estimate(&self, msg: &Message) -> u32;
}

/// Deterministic `ceil(bytes / 4)` heuristic plus a fixed per-message
/// structural overhead. Pure arithmetic — identical on every platform, so
/// it is conformance-stable (β.6) and portable (wasm/MCU).
#[derive(Debug, Clone, Copy, Default)]
pub struct HeuristicEstimator;

/// Per-message structural overhead (role tag, delimiters), in tokens.
const MESSAGE_OVERHEAD: u32 = 4;

/// Byte-length proxy for a [`tau_domain::Value`].
///
/// Uses `serde_json::to_string` for a deterministic serialized length.
/// Falls back to 0 on serialization error (should never happen in practice
/// for well-formed Values).
fn json_len(v: &Value) -> usize {
    serde_json::to_string(v).map(|s| s.len()).unwrap_or(0)
}

impl HeuristicEstimator {
    fn payload_bytes(payload: &MessagePayload) -> usize {
        match payload {
            MessagePayload::Text { content } => content.len(),
            MessagePayload::ToolCall { args } => json_len(args),
            MessagePayload::ToolResult { body } => json_len(body),
            MessagePayload::ToolError {
                kind,
                message,
                details,
            } => kind.len() + message.len() + details.as_ref().map(json_len).unwrap_or(0),
            MessagePayload::Lifecycle(_) => 0,
            MessagePayload::Custom { kind, body } => kind.len() + body.len(),
            _ => 0,
        }
    }
}

impl TokenEstimator for HeuristicEstimator {
    fn estimate(&self, msg: &Message) -> u32 {
        let bytes = Self::payload_bytes(&msg.payload);
        // ceil(bytes / 4)
        let approx = u32::try_from(bytes.div_ceil(4)).unwrap_or(u32::MAX);
        approx.saturating_add(MESSAGE_OVERHEAD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_domain::{Address, Message, MessagePayload};

    fn text(content: &str) -> Message {
        Message::new(
            Address::User,
            Address::System,
            MessagePayload::Text {
                content: content.into(),
            },
        )
    }

    #[test]
    fn estimate_is_bytes_over_four_plus_overhead() {
        // 8 bytes -> ceil(8/4)=2, +4 overhead = 6
        assert_eq!(HeuristicEstimator.estimate(&text("12345678")), 6);
    }

    #[test]
    fn estimate_is_deterministic() {
        let m = text("the fan-monitor reads temperature");
        assert_eq!(
            HeuristicEstimator.estimate(&m),
            HeuristicEstimator.estimate(&m)
        );
    }
}
