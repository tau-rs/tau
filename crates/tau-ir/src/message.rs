//! IR-owned message type used as the inter-node wire.
//!
//! Per the design spec D-2:
//! - A new `tau_ir::Message` type, separate from `tau_domain::Message`.
//! - Conservative migration: includes EVERY semantic field from
//!   `tau_domain::Message`; the only permitted change is type
//!   normalization (`SystemTime` → `i64`-ms).
//! - Bidirectional `From` adapters in both directions, behind the
//!   `with-std-adapters` feature (default-on).

use alloc::collections::BTreeMap;
use alloc::string::String;
use serde::{Deserialize, Serialize};
use tau_domain::message::MessagePayload as DomainMessagePayload;
use tau_domain::{Address, MessageId};

/// The IR-owned message envelope.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Globally unique message identifier.
    pub id: MessageId,
    /// Sender address.
    pub sender: Address,
    /// Recipient address.
    pub recipient: Address,
    /// Optional pointer to the message this one replies to.
    pub parent_id: Option<MessageId>,
    /// When the message was created, in milliseconds since the Unix
    /// epoch. Normalized from `tau_domain::Message::created_at:
    /// SystemTime` per D-2 — matches the β.1 Clock port's i64-ms
    /// convention.
    pub created_at_ms: i64,
    /// Free-form headers (`BTreeMap` for stable iteration).
    pub headers: BTreeMap<String, String>,
    /// Payload.
    pub payload: MessagePayload,
}

/// Mirror of `tau_domain::MessagePayload` adapted for IR storage.
///
/// Variants are 1:1 with `tau_domain::MessagePayload`. If a new variant
/// is added there, the cross-crate shape test will catch the drift.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum MessagePayload {
    /// Plain text content.
    Text {
        /// Message text.
        content: String,
    },
    /// Tool invocation request.
    ToolCall {
        /// Arguments to the tool.
        args: serde_json::Value,
    },
    /// Successful tool result.
    ToolResult {
        /// Tool's response body.
        body: serde_json::Value,
    },
    /// Tool error.
    ToolError {
        /// Error kind tag.
        kind: String,
        /// Human-readable error message.
        message: String,
        /// Optional structured detail.
        details: Option<serde_json::Value>,
    },
    /// Lifecycle broadcast.
    Lifecycle(tau_domain::AgentStatus),
    /// Plugin-custom payload (escape hatch).
    ///
    /// See: [escape-hatches.md#messagepayload-custom](../../docs/explanation/escape-hatches.md#messagepayload-custom).
    Custom {
        /// Kind tag.
        kind: String,
        /// Custom body bytes.
        body: alloc::vec::Vec<u8>,
    },
}

// === Helpers to convert tau_domain::Value ↔ serde_json::Value ===

fn domain_value_to_json(v: tau_domain::Value) -> serde_json::Value {
    // Round-trip through serde. tau_domain::Value's wire format is JSON-shaped
    // (with the @bytes: extension for Bytes variant); serde_json::Value
    // preserves all non-Bytes variants losslessly.
    serde_json::to_value(&v).unwrap_or(serde_json::Value::Null)
}

fn domain_value_opt_to_json(v: Option<tau_domain::Value>) -> Option<serde_json::Value> {
    v.map(domain_value_to_json)
}

fn json_to_domain_value(v: serde_json::Value) -> tau_domain::Value {
    serde_json::from_value(v).unwrap_or(tau_domain::Value::Null)
}

fn json_opt_to_domain_value(v: Option<serde_json::Value>) -> Option<tau_domain::Value> {
    v.map(json_to_domain_value)
}

// === Adapters (always available — no std required for payload conversion) ===

impl From<DomainMessagePayload> for MessagePayload {
    fn from(d: DomainMessagePayload) -> Self {
        match d {
            DomainMessagePayload::Text { content } => Self::Text { content },
            DomainMessagePayload::ToolCall { args } => Self::ToolCall {
                args: domain_value_to_json(args),
            },
            DomainMessagePayload::ToolResult { body } => Self::ToolResult {
                body: domain_value_to_json(body),
            },
            DomainMessagePayload::ToolError {
                kind,
                message,
                details,
            } => Self::ToolError {
                kind,
                message,
                details: domain_value_opt_to_json(details),
            },
            DomainMessagePayload::Lifecycle(status) => Self::Lifecycle(status),
            DomainMessagePayload::Custom { kind, body } => Self::Custom { kind, body },
            // tau_domain::MessagePayload is #[non_exhaustive]; a new variant added
            // upstream will fail to compile here, surfacing the drift loudly.
            _ => panic!(
                "tau_ir::Message: unhandled tau_domain::MessagePayload variant — \
                 update the From impl when tau_domain adds a variant"
            ),
        }
    }
}

impl From<MessagePayload> for DomainMessagePayload {
    fn from(i: MessagePayload) -> Self {
        match i {
            MessagePayload::Text { content } => Self::Text { content },
            MessagePayload::ToolCall { args } => Self::ToolCall {
                args: json_to_domain_value(args),
            },
            MessagePayload::ToolResult { body } => Self::ToolResult {
                body: json_to_domain_value(body),
            },
            MessagePayload::ToolError {
                kind,
                message,
                details,
            } => Self::ToolError {
                kind,
                message,
                details: json_opt_to_domain_value(details),
            },
            MessagePayload::Lifecycle(status) => Self::Lifecycle(status),
            MessagePayload::Custom { kind, body } => Self::Custom { kind, body },
        }
    }
}

// === Message envelope adapters — gated because SystemTime requires std ===

#[cfg(feature = "with-std-adapters")]
impl From<tau_domain::Message> for Message {
    fn from(d: tau_domain::Message) -> Self {
        let created_at_ms = d
            .created_at
            .duration_since(std::time::UNIX_EPOCH)
            .map(|dur| dur.as_millis() as i64)
            .unwrap_or(0); // pre-1970 timestamps clamp to epoch; documented edge case
        Self {
            id: d.id,
            sender: d.sender,
            recipient: d.recipient,
            parent_id: d.parent_id,
            created_at_ms,
            headers: d.headers,
            payload: d.payload.into(),
        }
    }
}

#[cfg(feature = "with-std-adapters")]
impl From<Message> for tau_domain::Message {
    fn from(i: Message) -> Self {
        // Construct via tau_domain::Message::new and overwrite the
        // generated fields; #[non_exhaustive] forbids struct-literal
        // construction.
        let mut m = tau_domain::Message::new(i.sender, i.recipient, i.payload.into());
        m.id = i.id;
        m.parent_id = i.parent_id;
        m.created_at = if i.created_at_ms >= 0 {
            std::time::UNIX_EPOCH + core::time::Duration::from_millis(i.created_at_ms as u64)
        } else {
            std::time::UNIX_EPOCH
        };
        m.headers = i.headers;
        m
    }
}
