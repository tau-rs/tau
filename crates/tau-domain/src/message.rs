//! Message envelope, addressing, and payload types (G5).

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use chrono::{DateTime, Utc};

use crate::agent::AgentStatus;
use crate::id::{AgentInstanceId, MessageId};
use crate::value::Value;

/// Sender or recipient of a [`Message`].
///
/// # Example
///
/// ```
/// use tau_domain::{Address, AgentInstanceId};
///
/// // A specific agent instance:
/// let agent_addr = Address::Agent(AgentInstanceId::new());
/// assert!(matches!(agent_addr, Address::Agent(_)));
///
/// // Well-known static addresses:
/// assert!(matches!(Address::User, Address::User));
/// assert!(matches!(Address::System, Address::System));
///
/// // Tool address:
/// let tool_addr = Address::Tool("fs-read.read".into());
/// assert!(matches!(tool_addr, Address::Tool(_)));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Address {
    /// A specific agent instance.
    Agent(AgentInstanceId),
    /// A named tool. The runtime resolves name → plugin via its
    /// registration table.
    Tool(String),
    /// A human user (e.g. the operator at the CLI).
    User,
    /// The runtime / observer.
    System,
}

/// Message body. Typed variants for known shapes; `Custom` for
/// plugin-specific.
///
/// # Example
///
/// ```
/// use tau_domain::{MessagePayload, Value};
///
/// // Text payload (most common):
/// let text = MessagePayload::Text { content: "Hello!".into() };
/// assert!(matches!(text, MessagePayload::Text { .. }));
///
/// // Tool call payload:
/// let call = MessagePayload::ToolCall { args: Value::Null };
/// assert!(matches!(call, MessagePayload::ToolCall { .. }));
///
/// // Tool error payload:
/// let err = MessagePayload::ToolError {
///     kind: "not_found".into(),
///     message: "file not found".into(),
///     details: None,
/// };
/// assert!(matches!(err, MessagePayload::ToolError { ref kind, .. } if kind == "not_found"));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum MessagePayload {
    /// Human- or agent-authored text. The envelope's `sender` field
    /// distinguishes origin.
    Text {
        /// Message text.
        content: String,
    },
    /// A tool invocation. The envelope's `recipient: Address::Tool(...)`
    /// names the tool; this carries the arguments.
    ToolCall {
        /// Arguments to pass to the tool.
        args: Value,
    },
    /// Successful tool result.
    ToolResult {
        /// Tool's response body.
        body: Value,
    },
    /// Tool returned an error.
    ToolError {
        /// Error kind (free-form string convention).
        kind: String,
        /// Human-readable error message.
        message: String,
        /// Optional structured detail.
        details: Option<Value>,
    },
    /// Lifecycle event broadcast (System → observers).
    Lifecycle(AgentStatus),
    /// Plugin-specific message kind.
    /// See: [escape-hatches.md#messagepayload-custom](../docs/explanation/escape-hatches.md#messagepayload-custom).
    Custom {
        /// Plugin-specific kind tag (e.g. `"mcp.resource.request"`).
        kind: String,
        /// Plugin-specific body bytes.
        body: Vec<u8>,
    },
}

/// A message envelope (G5).
///
/// # Example
///
/// ```
/// use tau_domain::{Address, Message, MessagePayload};
///
/// // `Message` is `#[non_exhaustive]`; use `Message::new` to construct.
/// let m = Message::new(
///     Address::User,
///     Address::System,
///     MessagePayload::Text { content: "hello".into() },
/// );
/// assert!(matches!(m.payload, MessagePayload::Text { .. }));
/// assert!(m.parent_id.is_none());
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Message {
    /// Globally unique message identifier.
    pub id: MessageId,
    /// Originator.
    pub sender: Address,
    /// Destination.
    pub recipient: Address,
    /// Optional pointer to the message this one replies to.
    pub parent_id: Option<MessageId>,
    /// When the message was created (UTC). Supplied by the caller's
    /// `Clock` port in the no_std kernel; the std `Message::new`
    /// convenience stamps `Utc::now()`.
    ///
    /// `chrono::DateTime<Utc>` is foreign to this crate, so it cannot
    /// carry its own `#[cfg_attr(schema, derive(JsonSchema))]` (orphan
    /// rule). Its serde representation is an RFC3339 string (chrono's
    /// default `Serialize`), so `schemars(with = "String")` describes
    /// the actual wire shape without touching serde.
    #[cfg_attr(feature = "schema", schemars(with = "alloc::string::String"))]
    pub created_at: DateTime<Utc>,
    /// Free-form headers. `BTreeMap` for stable iteration order.
    pub headers: BTreeMap<String, String>,
    /// Message body.
    pub payload: MessagePayload,
}

impl Message {
    /// Construct a new [`Message`] with a fresh [`MessageId`], a
    /// `created_at` of [`Utc::now`], no `parent_id`, and empty `headers`.
    /// Host-only (`std`); the no_std kernel uses [`Message::new_with`] fed by
    /// the `Clock`/`RandomSource` ports.
    ///
    /// `Message` is `#[non_exhaustive]`: external crates (notably
    /// tau-runtime, which assembles every message that flows through
    /// the agent loop) cannot use struct-literal construction, so this
    /// constructor is the canonical way to mint one. Callers wanting to
    /// override `parent_id`, `headers`, or `created_at` mutate the
    /// returned value via the `pub` fields.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_domain::{Address, Message, MessagePayload};
    ///
    /// let m = Message::new(
    ///     Address::User,
    ///     Address::System,
    ///     MessagePayload::Text { content: "hello".into() },
    /// );
    /// assert!(matches!(m.payload, MessagePayload::Text { .. }));
    /// assert!(m.parent_id.is_none());
    /// ```
    #[cfg(feature = "std")]
    pub fn new(sender: Address, recipient: Address, payload: MessagePayload) -> Self {
        Self::new_with(MessageId::new(), Utc::now(), sender, recipient, payload)
    }

    /// no_std-safe constructor: the caller supplies the `id` and
    /// `created_at` (minted from the `Clock`/`RandomSource` ports), with no
    /// `parent_id` and empty `headers`. This is how the kernel assembles
    /// every message inside the agent loop so ids/timestamps are
    /// reproducible under conformance. See
    /// `tau_runtime_core::ids::message_id`.
    pub fn new_with(
        id: MessageId,
        created_at: DateTime<Utc>,
        sender: Address,
        recipient: Address,
        payload: MessagePayload,
    ) -> Self {
        Self {
            id,
            sender,
            recipient,
            parent_id: None,
            created_at,
            headers: BTreeMap::new(),
            payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_payload_holds_status() {
        let m = MessagePayload::Lifecycle(AgentStatus::Ready);
        assert!(matches!(m, MessagePayload::Lifecycle(AgentStatus::Ready)));
    }

    #[test]
    fn new_constructs_with_fresh_id_and_no_parent() {
        let m = Message::new(
            Address::User,
            Address::System,
            MessagePayload::Text {
                content: "hello".into(),
            },
        );
        assert_eq!(m.sender, Address::User);
        assert_eq!(m.recipient, Address::System);
        assert!(m.parent_id.is_none());
        assert!(m.headers.is_empty());
        assert!(matches!(m.payload, MessagePayload::Text { .. }));
    }

    #[test]
    fn new_message_ids_are_unique() {
        let a = Message::new(
            Address::User,
            Address::System,
            MessagePayload::Text {
                content: "a".into(),
            },
        );
        let b = Message::new(
            Address::User,
            Address::System,
            MessagePayload::Text {
                content: "b".into(),
            },
        );
        assert_ne!(a.id, b.id);
    }
}
