//! The request half of the bridge: what a `send` to a model capability
//! carries, and the content blocks shared with the reply.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::abi::Name;

/// One request to a model: the conversation so far, the tools on offer, and
/// the output cap.
///
/// Which model answers is the driver's configuration, not the request's: a
/// capability is one endpoint at one model, chosen by the harness at
/// registration. The reply names the model that actually answered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    /// The bridge version this request was written against. See
    /// [`VERSION`](super::VERSION).
    pub v: u16,
    /// The system prompt, if any. Top-level rather than a message role,
    /// because the provider that has it top-level cannot recover it from a
    /// message and the provider that has it as a message can fold it in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// Alternating `user` / `assistant` turns.
    pub messages: Vec<Message>,
    /// The projected tool definitions. Empty means the model may only talk.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDef>,
    /// The output cap for this call. The driver clamps or refuses above the
    /// maximum it was registered with (ADR-0006 §7).
    pub max_tokens: u32,
    /// Sampling controls. Absent means the provider's defaults throughout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling: Option<Sampling>,
}

/// One turn of the conversation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Who is speaking.
    pub role: Role,
    /// What was said, as blocks.
    pub content: Vec<Content>,
}

/// Who a [`Message`] is from.
///
/// Only these two. Tool results are `user` content, the way Anthropic frames
/// them; an OpenAI-compatible driver unfolds them into `tool` messages. The
/// system prompt is [`ModelRequest::system`], not a role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The agent, or a tool result it is relaying.
    User,
    /// The model.
    Assistant,
}

/// A block of content inside a [`Message`] or a reply.
///
/// Four kinds. A new kind is a bridge version bump, so the enum is
/// exhaustive on purpose: a loop that matches on it should be told by the
/// compiler when the contract grows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Content {
    /// The model's reasoning, as the provider returned it, sealed
    /// (ADR-0007).
    ///
    /// Opaque: the loop carries it back in the assistant turn without
    /// reading it, because the provider requires its thinking blocks back
    /// unchanged on the next call and rejects a turn that edits or drops
    /// them. A driver of the same `provider` unwraps `data` and sends it
    /// verbatim, in place; a driver of another provider drops the block,
    /// which is what the provider itself does with reasoning its model
    /// cannot read.
    Thinking {
        /// The wire format `data` is written in, named by the driver that
        /// produced it (`anthropic` for the Messages API). A provider, not
        /// a model: the provider decides per model what it can read.
        provider: String,
        /// The provider's block, verbatim.
        data: Value,
    },
    /// Plain text.
    Text {
        /// The text.
        text: String,
    },
    /// The model asks for a tool to be called.
    ToolCall {
        /// The provider's id for this call; the result answers by it.
        id: String,
        /// The tool name *as the model wrote it*. A plain string, not a
        /// [`Name`], on purpose: a model can write `serch`, and the loop
        /// must be able to represent that call in order to answer it with
        /// [`ToolErrorKind::UnknownTool`].
        name: String,
        /// The arguments, parsed. Validated against the driver's schema by
        /// the loop, not by the provider and not by the kernel.
        input: Value,
    },
    /// The answer to one [`Content::ToolCall`], or the reason there is none.
    ToolResult {
        /// Which call this answers.
        call_id: String,
        /// Text the model reads: the tool's reply bytes as UTF-8, or the
        /// failure explained.
        content: String,
        /// Whether the call failed. Always written; absent on the way in
        /// means it succeeded.
        #[serde(default)]
        is_error: bool,
        /// Why it failed, typed. Present only with `is_error`. Provider
        /// drivers drop it (no provider has a slot for it); `libtau`'s
        /// tests, logs, and metrics assert on it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_kind: Option<ToolErrorKind>,
    },
}

/// Why a tool call was answered with an error (ADR-0006 §4).
///
/// Budget refusals are deliberately not here: a `send` refused because the
/// ceiling cannot be reserved is terminal for the loop, not fed back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolErrorKind {
    /// The name resolves to nothing in the projected table.
    UnknownTool,
    /// The input fails validation against the driver's schema.
    BadArgs,
    /// The `send` was refused on authority or policy: the agent does not
    /// hold the capability, or a hook said no.
    Denied,
    /// The `send` happened and the reply says the tool failed, or the loop
    /// could not render it.
    Failed,
}

/// A tool as the model sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDef {
    /// The tool's name: the `DriverId` the harness registered the driver
    /// under (ADR-0006 §5). A validated [`Name`], which is a strict subset of
    /// every provider's tool-name grammar.
    pub name: Name,
    /// What the model reads to decide when to call it.
    pub description: String,
    /// A JSON Schema (draft 2020-12) for the input.
    pub input_schema: Value,
}

/// Sampling controls. Every field is optional; absent means the provider's
/// default.
///
/// A driver that cannot honour a *present* field replies
/// [`ErrorKind::Unsupported`](super::ErrorKind::Unsupported) rather than
/// dropping it: a `seed` that was silently ignored is a branch that was
/// silently not controlled (HANDOFF §4.10).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Sampling {
    /// Sampling temperature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Nucleus sampling threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    /// A seed for pinned sampling, where the provider offers one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Sequences that end generation early.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
}
