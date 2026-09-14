//! The reply half of the bridge: what a model driver sends back, including
//! its own failures, which ride here because `Driver::handle` has no error
//! channel until the M3 error envelope exists.

use serde::{Deserialize, Serialize};

use super::Content;

/// One reply from a model driver, whatever happened.
///
/// A driver that fails still answers with one of these: `stop` says why, and
/// `usage` says what the attempt consumed. "No answer" is reserved for the
/// kernel's own error envelope when supervision arrives (M3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelReply {
    /// The bridge version this reply was written against.
    pub v: u16,
    /// Which model actually answered, as the provider names it. For the
    /// record; the agent did not choose it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// What the model said: text and tool calls. Empty on an error.
    pub content: Vec<Content>,
    /// Why generation stopped.
    pub stop: StopReason,
    /// What the model consumed, for the agent's information.
    ///
    /// Not the accounting. That is the `Consumption` the driver attaches to
    /// the same reply, which the kernel settles against the reservation. The
    /// two come from the same numbers.
    pub usage: Usage,
}

/// Why a reply ended.
///
/// The plain variants serialize as strings (`"end_turn"`); the error carries
/// its detail (`{"error": {...}}`). Exhaustive on purpose: a new reason is a
/// bridge version bump, and a loop matching on it should hear about it from
/// the compiler.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model finished its turn.
    EndTurn,
    /// The model is waiting on one or more tool results.
    ToolCall,
    /// The output cap was hit; the content may be cut mid-thought.
    MaxTokens,
    /// A configured stop sequence was produced.
    StopSequence,
    /// The model declined. A stop reason, not an error: the call happened
    /// and the usage is real. What to do about it is the loop's policy.
    Refusal,
    /// The driver could not get an answer, or refused to ask.
    Error(ModelError),
}

/// A driver-side failure, carried in the reply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelError {
    /// What went wrong.
    pub kind: ErrorKind,
    /// Detail for a human, or the provider's own text.
    pub message: String,
}

/// The kinds of driver-side failure (ADR-0006 §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The driver refused to make the call: the input estimate or
    /// `max_tokens` would exceed what the harness registered. Nothing was
    /// billed.
    OverCeiling,
    /// A version, a sampling field, or a block the driver cannot honour.
    /// Nothing was billed.
    Unsupported,
    /// The provider answered with an error, or with something the driver
    /// could not map.
    Provider,
    /// No answer arrived: connection, timeout, or abandoned by `cancel`.
    Transport,
}

/// What a call consumed, as the provider counts it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Tokens in the prompt.
    pub input_tokens: u64,
    /// Tokens generated.
    pub output_tokens: u64,
}
