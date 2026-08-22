//! LLM-backend port — `kind = "llm-backend"` plugin contracts.
//!
//! This module defines the [`LlmBackend`] trait, the [`CompletionStream`]
//! type alias, and the data types exchanged between tau-runtime and
//! plugin adapters. The `batch_to_stream` / `stream_to_batch` /
//! `ToolUseAccumulator` helpers land in T7.
//!
//! # Layer separation: `LlmProviderMessage` vs `tau_domain::Message`
//!
//! tau distinguishes the agent's **universal message envelope**
//! (`tau_domain::Message`, used for inbox/outbox routing between agents,
//! tools, and the runtime) from the **LLM-call shape**
//! ([`LlmProviderMessage`], used to construct a single completion
//! request to a provider).
//!
//! - `tau_domain::Message` carries an [`AgentInstanceId`] sender, an
//!   [`Address`] recipient, a payload, a timestamp, and a message id.
//!   It is the universal envelope that tau-runtime routes.
//! - [`LlmProviderMessage`] carries only the role
//!   (`User` / `Assistant` / `ToolResult`) and a list of
//!   [`ContentBlock`]s. It is the over-the-wire shape an `LlmBackend`
//!   plugin serialises into a provider-specific completion call.
//!
//! tau-runtime mediates between the two: it consumes
//! `tau_domain::Message`s from agent inboxes, projects them into a
//! `Vec<LlmProviderMessage>`, and hands the result to the
//! `LlmBackend`. Plugins never see `tau_domain::Message` directly.
//!
//! [`AgentInstanceId`]: tau_domain::AgentInstanceId
//! [`Address`]: tau_domain::Address

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::pin::Pin;

use futures_core::Stream;
use tau_domain::Value;

use crate::error::LlmError;

/// Parameters for a single completion request to an `LlmBackend`.
///
/// `CompletionRequest` is `#[non_exhaustive]`: external callers cannot
/// construct it via struct-literal syntax. Construction is gated through
/// a builder (added alongside the trait in T6); fields are `pub` so
/// in-tree code and tests can pattern-match on the parameters.
///
/// # Example
///
/// ```text
/// // Struct-literal construction is forbidden externally because
/// // `CompletionRequest` is `#[non_exhaustive]`. The example here is
/// // illustrative; real callers use the builder added in T6.
/// use tau_ports::llm::{CompletionRequest, ToolChoice};
///
/// let req = CompletionRequest {
///     model: "claude-3-5-sonnet".into(),
///     system: Some("You are helpful.".into()),
///     messages: vec![],
///     tools: vec![],
///     max_tokens: Some(1024),
///     temperature: Some(0.7),
///     top_p: None,
///     seed: None,
///     tool_choice: ToolChoice::Auto,
///     stop_sequences: vec![],
///     provider_specific: Default::default(),
/// };
/// ```
#[non_exhaustive]
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CompletionRequest {
    /// Model identifier (provider-specific; e.g. `"claude-3-5-sonnet"`).
    pub model: String,
    /// Optional system prompt prepended before `messages`.
    pub system: Option<String>,
    /// Conversation history as a sequence of provider-shaped messages.
    pub messages: Vec<LlmProviderMessage>,
    /// Tool specifications advertised to the model.
    pub tools: Vec<ToolSpec>,
    /// Maximum tokens to generate. `None` defers to the provider default.
    pub max_tokens: Option<u32>,
    /// Sampling temperature. `None` defers to the provider default.
    pub temperature: Option<f32>,
    /// Nucleus-sampling cutoff. `None` defers to the provider default.
    pub top_p: Option<f32>,
    /// Deterministic-sampling seed. `None` defers to the provider default.
    pub seed: Option<u64>,
    /// Tool-selection policy. Defaults to [`ToolChoice::Auto`].
    pub tool_choice: ToolChoice,
    /// Custom stop sequences. Empty defers to the provider default.
    pub stop_sequences: Vec<String>,
    /// Provider-specific parameters not yet typed in core (e.g. `top_k`,
    /// `presence_penalty`, `response_format`).
    ///
    /// This is a registered escape hatch. See:
    /// [escape-hatches.md#completionrequest-provider-specific](../docs/explanation/escape-hatches.md#completionrequest-provider-specific).
    pub provider_specific: BTreeMap<String, Value>,
}

impl CompletionRequest {
    /// Construct a [`CompletionRequest`] with all optional fields at
    /// their defaults: `system = None`, `messages` and `tools` empty,
    /// no sampling overrides, [`ToolChoice::Auto`], no stop sequences,
    /// no provider-specific overrides.
    ///
    /// `CompletionRequest` is `#[non_exhaustive]`: external crates
    /// (notably tau-runtime, which mints one of these per turn) cannot
    /// use struct-literal construction. Callers populate the optional
    /// fields by mutating the returned value via the public fields.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_ports::CompletionRequest;
    ///
    /// let req = CompletionRequest::new("claude-3-5-sonnet".into());
    /// assert_eq!(req.model, "claude-3-5-sonnet");
    /// assert!(req.messages.is_empty());
    /// ```
    pub fn new(model: String) -> Self {
        Self {
            model,
            system: None,
            messages: Vec::new(),
            tools: Vec::new(),
            max_tokens: None,
            temperature: None,
            top_p: None,
            seed: None,
            tool_choice: ToolChoice::Auto,
            stop_sequences: Vec::new(),
            provider_specific: BTreeMap::new(),
        }
    }
}

impl LlmProviderMessage {
    /// Construct a [`LlmProviderMessage::User`] with the given content
    /// blocks. `LlmProviderMessage` is `#[non_exhaustive]` so external
    /// crates can't use struct-literal construction.
    pub fn user(content: Vec<ContentBlock>) -> Self {
        Self::User { content }
    }

    /// Construct a [`LlmProviderMessage::Assistant`] with the given
    /// content blocks.
    pub fn assistant(content: Vec<ContentBlock>) -> Self {
        Self::Assistant { content }
    }

    /// Construct a [`LlmProviderMessage::ToolResult`] addressed to the
    /// given `tool_use_id`. Pair with the `id` of the
    /// [`LlmProviderMessage::Assistant`] tool-use this is answering.
    pub fn tool_result(tool_use_id: String, content: Vec<ContentBlock>, is_error: bool) -> Self {
        Self::ToolResult {
            tool_use_id,
            content,
            is_error,
        }
    }
}

impl ToolUse {
    /// Construct a [`ToolUse`]. Provided so external callers — notably
    /// tau-runtime when projecting `MessagePayload::ToolCall` onto an
    /// `LlmProviderMessage` — can build one without struct-literal
    /// syntax (the type is `#[non_exhaustive]`).
    pub fn new(id: String, name: String, input: Value) -> Self {
        Self { id, name, input }
    }
}

impl TokenUsage {
    /// Construct a [`TokenUsage`] from the (input, output) totals
    /// reported by an LLM backend.
    pub fn new(input_tokens: u32, output_tokens: u32) -> Self {
        Self {
            input_tokens,
            output_tokens,
        }
    }
}

/// Tool-selection policy for a [`CompletionRequest`].
///
/// Defaults to [`ToolChoice::Auto`]: the model decides whether to call
/// a tool.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ToolChoice {
    /// Model decides whether to call a tool (default).
    #[default]
    Auto,
    /// Model must call at least one tool.
    Required,
    /// Model must not call any tool.
    None,
    /// Model must call the named tool.
    Specific {
        /// Name of the required tool. Must match a [`ToolSpec::name`]
        /// in [`CompletionRequest::tools`].
        name: String,
    },
}

/// One message in the LLM-call shape, distinct from the agent-runtime
/// `tau_domain::Message` envelope. See module-level docs for the layer
/// separation.
#[non_exhaustive]
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LlmProviderMessage {
    /// User-authored message content.
    User {
        /// Multi-block content; v0.1 admits [`ContentBlock::Text`] and
        /// [`ContentBlock::ToolUse`] only.
        content: Vec<ContentBlock>,
    },
    /// Assistant-authored message content.
    Assistant {
        /// Multi-block content; v0.1 admits [`ContentBlock::Text`] and
        /// [`ContentBlock::ToolUse`] only.
        content: Vec<ContentBlock>,
    },
    /// Result of a previously-issued tool call, fed back to the model.
    ToolResult {
        /// Identifier matching the [`ToolUse::id`] of the call this
        /// result answers.
        tool_use_id: String,
        /// Multi-block content describing the tool result.
        content: Vec<ContentBlock>,
        /// Whether the tool reported an error.
        is_error: bool,
    },
}

/// One content block within an [`LlmProviderMessage`] or
/// [`CompletionResponse`].
///
/// v0.1 admits [`ContentBlock::Text`] and [`ContentBlock::ToolUse`]
/// only. The enum is `#[non_exhaustive]` to admit additive variants for
/// image, audio, document, and reasoning blocks without a major bump.
#[non_exhaustive]
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ContentBlock {
    /// Plain-text content.
    Text(String),
    /// A tool-use request from the assistant.
    ToolUse(ToolUse),
}

/// Batch (non-streaming) completion result.
#[non_exhaustive]
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CompletionResponse {
    /// Concatenated assistant text. May be empty if the model only
    /// emitted tool-use blocks.
    pub text: String,
    /// Tool-use blocks emitted in order.
    pub tool_uses: Vec<ToolUse>,
    /// Why the response stopped.
    pub stop_reason: StopReason,
    /// Token-usage report. `None` if the provider did not return one.
    pub usage: Option<TokenUsage>,
}

impl CompletionResponse {
    /// Construct a response. The struct is `#[non_exhaustive]`, so this
    /// is the supported way for plugins and embedders (EPIC 7.1) to
    /// build one outside this crate.
    pub fn new(
        text: String,
        tool_uses: Vec<ToolUse>,
        stop_reason: StopReason,
        usage: Option<TokenUsage>,
    ) -> Self {
        Self {
            text,
            tool_uses,
            stop_reason,
            usage,
        }
    }
}

/// One streamed event from a `CompletionStream`.
///
/// Plugin authors are responsible for buffering provider-specific
/// streaming representations into the shape below: `Text` deltas are
/// forwarded incrementally, `ToolUse` blocks are emitted only when
/// fully assembled, and exactly one terminal `Finish` is emitted at end
/// of stream. See `ToolUseAccumulator` (added in T7) for the
/// JSON-delta-buffering helper.
#[non_exhaustive]
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CompletionChunk {
    /// Streamed text delta to append to the assistant response.
    Text {
        /// Text fragment to append.
        delta: String,
    },
    /// A tool-use block emitted once fully assembled by the plugin.
    ToolUse(ToolUse),
    /// Final marker. Emitted exactly once at end of stream.
    Finish {
        /// Why the stream ended.
        stop_reason: StopReason,
        /// Token-usage report. `None` if the provider did not return one.
        usage: Option<TokenUsage>,
    },
}

/// One tool-use request emitted by the model.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ToolUse {
    /// Provider-supplied identifier; round-tripped to
    /// [`LlmProviderMessage::ToolResult::tool_use_id`].
    pub id: String,
    /// Name of the tool the model wants to call. Matches a
    /// [`ToolSpec::name`] in the originating request.
    pub name: String,
    /// Arguments to the tool, as a `tau_domain::Value`.
    pub input: Value,
}

/// Specification of a tool the model may call.
#[non_exhaustive]
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ToolSpec {
    /// Tool name. Must be unique within a [`CompletionRequest::tools`].
    pub name: String,
    /// Human-readable description used by the model to decide when to
    /// invoke the tool.
    pub description: String,
    /// JSON Schema describing the tool's input. Round-trips through
    /// `tau_domain::Value`'s serde representation.
    pub input_schema: Value,
}

/// Reason a completion stopped.
///
/// `StopReason::Error` indicates the stream completed but reported an
/// error mid-flight; this is distinct from [`crate::LlmError`], which
/// indicates the trait method itself failed.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum StopReason {
    /// Model finished naturally.
    EndTurn,
    /// Hit the `max_tokens` cap.
    MaxTokens,
    /// Matched one of the configured stop sequences.
    StopSequence,
    /// Model emitted a tool-use block and is awaiting its result.
    ToolUse,
    /// Stream completed but reported an error mid-flight (distinct
    /// from [`crate::LlmError`], which is a trait-method failure).
    Error,
}

/// Token-usage report for a completion.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TokenUsage {
    /// Tokens consumed from the input (system + messages + tools).
    pub input_tokens: u32,
    /// Tokens generated in the response.
    pub output_tokens: u32,
}

/// Boxed dyn-stream type at the runtime registry boundary. Returned from
/// [`LlmBackend::stream`].
pub type CompletionStream = Pin<Box<dyn Stream<Item = Result<CompletionChunk, LlmError>> + Send>>;

/// Trait implemented by `kind = "llm-backend"` plugins.
///
/// Native `async fn in trait` (Rust 1.75+; tau MSRV is 1.91). Both
/// `complete` and `stream` are required to avoid mutual-recursion
/// footguns; helpers in [the helpers section] make the inverse
/// implementation a one-liner for plugin authors who only have one
/// path natively.
///
/// `Send + Sync` so the runtime can store impls in a multi-task plugin
/// registry.
///
/// The `async_fn_in_trait` lint is suppressed: tau-ports intentionally
/// uses native `async fn in trait` (no `async-trait` macro, no boxed
/// future per call). tau-runtime boxes once at the dyn-cast boundary
/// where it stores `Arc<dyn LlmBackend>` in the plugin registry. See
/// spec §3.1 design call "Native `async fn in trait`" and ADR-0003.
#[allow(async_fn_in_trait)]
pub trait LlmBackend: Send + Sync {
    /// Plugin-visible name (matches the package name; for diagnostics).
    fn name(&self) -> &str;

    /// Make a batch completion request.
    ///
    /// Plugin authors with batch-only SDKs implement natively.
    /// Plugin authors with streaming SDKs call
    /// `stream_to_batch(self.stream(req).await?)` (helper in Task 7).
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError>;

    /// Make a streaming completion request.
    ///
    /// Plugin authors with streaming SDKs implement natively.
    /// Plugin authors with batch-only SDKs call
    /// `Ok(batch_to_stream(self.complete(req).await?))` (helper in Task 7).
    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError>;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a batch [`CompletionResponse`] into a [`CompletionStream`] that
/// yields the equivalent chunks: zero-or-one [`CompletionChunk::Text`]
/// (only when `resp.text` is non-empty), one [`CompletionChunk::ToolUse`]
/// per entry in `resp.tool_uses` (in order), and one terminal
/// [`CompletionChunk::Finish`] carrying `resp.stop_reason` and `resp.usage`.
///
/// Plugin authors with batch-only SDKs use this in their
/// [`LlmBackend::stream`] impl: `Ok(batch_to_stream(self.complete(req).await?))`.
///
/// # Example
///
/// ```text
/// // Illustrative; `CompletionResponse` is `#[non_exhaustive]` so external
/// // callers must build it via the data-types builder added in T5.
/// let resp = /* CompletionResponse */;
/// let stream = tau_ports::batch_to_stream(resp);
/// ```
pub fn batch_to_stream(resp: CompletionResponse) -> CompletionStream {
    let CompletionResponse {
        text,
        tool_uses,
        stop_reason,
        usage,
    } = resp;

    let mut chunks: Vec<Result<CompletionChunk, LlmError>> = Vec::new();
    if !text.is_empty() {
        chunks.push(Ok(CompletionChunk::Text { delta: text }));
    }
    for tu in tool_uses {
        chunks.push(Ok(CompletionChunk::ToolUse(tu)));
    }
    chunks.push(Ok(CompletionChunk::Finish { stop_reason, usage }));

    Box::pin(VecStream {
        items: chunks.into_iter(),
    })
}

/// Adapter from a `Vec` of pre-computed items into a `Stream`. Used by
/// [`batch_to_stream`] to avoid pulling in `futures` as a runtime dep.
struct VecStream<T> {
    items: alloc::vec::IntoIter<T>,
}

impl<T> Stream for VecStream<T>
where
    T: Unpin,
{
    type Item = T;

    fn poll_next(
        self: Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Self::Item>> {
        core::task::Poll::Ready(self.get_mut().items.next())
    }
}

/// Consume a [`CompletionStream`] and reassemble it into a
/// [`CompletionResponse`].
///
/// Concatenates [`CompletionChunk::Text`] deltas in order, collects
/// [`CompletionChunk::ToolUse`] blocks in order, and captures the final
/// [`CompletionChunk::Finish`]'s `stop_reason` and `usage`.
///
/// Returns [`LlmError::Stream`] if the stream ends without emitting a
/// `Finish` chunk. Mid-stream errors propagate as-is.
///
/// Plugin authors with streaming-only SDKs use this in their
/// [`LlmBackend::complete`] impl: `stream_to_batch(self.stream(req).await?).await`.
///
/// # Example
///
/// ```text
/// // Illustrative; building a `CompletionStream` requires constructing
/// // `#[non_exhaustive]` types via the data-types builder added in T5.
/// let stream: tau_ports::CompletionStream = /* ... */;
/// let resp = tau_ports::stream_to_batch(stream).await?;
/// ```
pub async fn stream_to_batch(mut stream: CompletionStream) -> Result<CompletionResponse, LlmError> {
    let mut text = String::new();
    let mut tool_uses: Vec<ToolUse> = Vec::new();
    let mut finish: Option<(StopReason, Option<TokenUsage>)> = None;

    loop {
        let next = core::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await;
        match next {
            None => break,
            Some(Err(e)) => return Err(e),
            Some(Ok(CompletionChunk::Text { delta })) => text.push_str(&delta),
            Some(Ok(CompletionChunk::ToolUse(tu))) => tool_uses.push(tu),
            Some(Ok(CompletionChunk::Finish { stop_reason, usage })) => {
                finish = Some((stop_reason, usage));
                // Per spec: Finish is the terminal marker; stop here so any
                // post-Finish items would be a stream-shape bug we don't mask.
                break;
            }
        }
    }

    let (stop_reason, usage) = finish.ok_or_else(|| LlmError::Stream {
        message: "stream ended without Finish chunk".into(),
    })?;

    Ok(CompletionResponse {
        text,
        tool_uses,
        stop_reason,
        usage,
    })
}

/// Helper for plugin authors whose streaming SDKs emit JSON tool-use input
/// deltas (e.g. Anthropic's `input_json_delta` events).
///
/// Call [`ToolUseAccumulator::append`] for each delta event, then
/// [`ToolUseAccumulator::finalize_with`] when the tool-use block closes
/// to obtain a fully-assembled [`ToolUse`].
///
/// `finalize_with` takes a parse callback so plugin authors plug in their
/// preferred JSON parser. tau-ports has no runtime `serde_json` dependency;
/// the callback returns a `Result<Value, String>` so any parser crate can
/// feed in. On `Err(message)` the accumulator returns
/// [`LlmError::Stream`] wrapping the parse-error message.
///
/// # Example
///
/// ```
/// use tau_ports::ToolUseAccumulator;
///
/// let mut acc = ToolUseAccumulator::new("toolu_01".into(), "search".into());
/// acc.append(r#"{"q":"#);
/// acc.append(r#""hello"}"#);
///
/// let tool_use = acc
///     .finalize_with(|s| {
///         serde_json::from_str::<tau_domain::Value>(s).map_err(|e| e.to_string())
///     })
///     .unwrap();
/// ```
#[derive(Debug, Clone)]
pub struct ToolUseAccumulator {
    id: String,
    name: String,
    input_buffer: String,
}

impl ToolUseAccumulator {
    /// Create a fresh accumulator for the tool-use block identified by
    /// `id` (provider-supplied; round-trips to
    /// [`LlmProviderMessage::ToolResult::tool_use_id`]) and named `name`
    /// (must match a [`ToolSpec::name`] in the originating request).
    pub fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            input_buffer: String::new(),
        }
    }

    /// Append a raw JSON delta fragment to the input buffer. Plugin
    /// authors call this once per provider-specific delta event.
    pub fn append(&mut self, json_delta: &str) {
        self.input_buffer.push_str(json_delta);
    }

    /// Borrow the current input buffer (for diagnostics or partial-parse
    /// inspection).
    pub fn input_buffer(&self) -> &str {
        &self.input_buffer
    }

    /// Finalize into a [`ToolUse`] using the supplied JSON parser.
    ///
    /// `parse` is called once with the accumulated buffer; it returns
    /// `Ok(Value)` for a successful parse or `Err(message)` to surface a
    /// parse failure as [`LlmError::Stream`].
    pub fn finalize_with<F>(self, parse: F) -> Result<ToolUse, LlmError>
    where
        F: FnOnce(&str) -> Result<Value, String>,
    {
        let input = parse(&self.input_buffer).map_err(|message| LlmError::Stream {
            message: format!("tool_use input JSON parse failed: {message}"),
        })?;
        Ok(ToolUse {
            id: self.id,
            name: self.name,
            input,
        })
    }
}

#[cfg(test)]
mod helper_tests {
    use super::*;
    #[allow(unused_imports)]
    use alloc::string::ToString;
    use alloc::vec;

    use core::pin::Pin;

    #[test]
    fn completion_response_new_constructs_all_fields() {
        let r = CompletionResponse::new("hi".to_string(), Vec::new(), StopReason::EndTurn, None);
        assert_eq!(r.text, "hi");
        assert!(r.tool_uses.is_empty());
        assert!(matches!(r.stop_reason, StopReason::EndTurn));
        assert!(r.usage.is_none());
    }

    use futures_core::Stream;

    /// Drain a `CompletionStream` into a `Vec<CompletionChunk>` for
    /// assertion. Errors short-circuit.
    async fn drain(mut stream: CompletionStream) -> Result<Vec<CompletionChunk>, LlmError> {
        let mut out = Vec::new();
        loop {
            let next: Option<Result<CompletionChunk, LlmError>> =
                core::future::poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)).await;
            match next {
                None => break,
                Some(Ok(c)) => out.push(c),
                Some(Err(e)) => return Err(e),
            }
        }
        Ok(out)
    }

    #[tokio::test]
    async fn batch_to_stream_to_batch_empty_round_trip() {
        let resp = CompletionResponse {
            text: String::new(),
            tool_uses: vec![],
            stop_reason: StopReason::EndTurn,
            usage: None,
        };

        let stream = batch_to_stream(resp);
        let chunks = drain(stream).await.expect("drain");
        // Empty text => no Text chunk; no tool_uses => no ToolUse chunks;
        // exactly one Finish.
        assert_eq!(chunks.len(), 1);
        let CompletionChunk::Finish { stop_reason, usage } = &chunks[0] else {
            panic!("expected Finish, got {:?}", chunks[0]);
        };
        assert_eq!(*stop_reason, StopReason::EndTurn);
        assert!(usage.is_none());

        // Round-trip via stream_to_batch.
        let resp2 = stream_to_batch(batch_to_stream(CompletionResponse {
            text: String::new(),
            tool_uses: vec![],
            stop_reason: StopReason::EndTurn,
            usage: None,
        }))
        .await
        .expect("stream_to_batch");
        assert_eq!(resp2.text, "");
        assert!(resp2.tool_uses.is_empty());
        assert_eq!(resp2.stop_reason, StopReason::EndTurn);
        assert!(resp2.usage.is_none());
    }

    #[tokio::test]
    async fn batch_to_stream_to_batch_text_round_trip() {
        let resp = CompletionResponse {
            text: "hello world".into(),
            tool_uses: vec![],
            stop_reason: StopReason::MaxTokens,
            usage: Some(TokenUsage {
                input_tokens: 7,
                output_tokens: 11,
            }),
        };

        let stream = batch_to_stream(resp);
        let chunks = drain(stream).await.expect("drain");
        // One Text + one Finish.
        assert_eq!(chunks.len(), 2);
        let CompletionChunk::Text { delta } = &chunks[0] else {
            panic!("expected Text, got {:?}", chunks[0]);
        };
        assert_eq!(delta, "hello world");

        // Round-trip.
        let resp2 = stream_to_batch(batch_to_stream(CompletionResponse {
            text: "hello world".into(),
            tool_uses: vec![],
            stop_reason: StopReason::MaxTokens,
            usage: Some(TokenUsage {
                input_tokens: 7,
                output_tokens: 11,
            }),
        }))
        .await
        .expect("stream_to_batch");
        assert_eq!(resp2.text, "hello world");
        assert!(resp2.tool_uses.is_empty());
        assert_eq!(resp2.stop_reason, StopReason::MaxTokens);
        assert_eq!(
            resp2.usage,
            Some(TokenUsage {
                input_tokens: 7,
                output_tokens: 11,
            })
        );
    }

    #[tokio::test]
    async fn batch_to_stream_to_batch_tool_use_round_trip() {
        let tu1 = ToolUse {
            id: "toolu_a".into(),
            name: "search".into(),
            input: Value::String("hello".into()),
        };
        let tu2 = ToolUse {
            id: "toolu_b".into(),
            name: "fetch".into(),
            input: Value::String("world".into()),
        };

        let resp = CompletionResponse {
            text: "preamble".into(),
            tool_uses: vec![tu1.clone(), tu2.clone()],
            stop_reason: StopReason::ToolUse,
            usage: None,
        };

        let stream = batch_to_stream(resp);
        let chunks = drain(stream).await.expect("drain");
        // Text + ToolUse + ToolUse + Finish.
        assert_eq!(chunks.len(), 4);

        let resp2 = stream_to_batch(batch_to_stream(CompletionResponse {
            text: "preamble".into(),
            tool_uses: vec![tu1.clone(), tu2.clone()],
            stop_reason: StopReason::ToolUse,
            usage: None,
        }))
        .await
        .expect("stream_to_batch");

        assert_eq!(resp2.text, "preamble");
        assert_eq!(resp2.tool_uses.len(), 2);
        assert_eq!(resp2.tool_uses[0].id, "toolu_a");
        assert_eq!(resp2.tool_uses[0].name, "search");
        assert_eq!(resp2.tool_uses[1].id, "toolu_b");
        assert_eq!(resp2.tool_uses[1].name, "fetch");
        assert_eq!(resp2.stop_reason, StopReason::ToolUse);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn tool_use_accumulator_round_trip() {
        let mut acc = ToolUseAccumulator::new("toolu_xyz".into(), "search".into());
        acc.append(r#"{"q":"#);
        acc.append(r#""hello world"}"#);

        assert_eq!(acc.input_buffer(), r#"{"q":"hello world"}"#);

        let tu = acc
            .finalize_with(|s| serde_json::from_str::<Value>(s).map_err(|e| e.to_string()))
            .expect("finalize");

        assert_eq!(tu.id, "toolu_xyz");
        assert_eq!(tu.name, "search");
        // Input is an object with one string field.
        let Value::Object(map) = &tu.input else {
            panic!("expected Value::Object, got {:?}", tu.input);
        };
        assert_eq!(map.get("q"), Some(&Value::String("hello world".into())));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn tool_use_accumulator_invalid_json() {
        let mut acc = ToolUseAccumulator::new("toolu_bad".into(), "search".into());
        acc.append(r#"{"unterminated":"#);

        let err = acc
            .finalize_with(|s| serde_json::from_str::<Value>(s).map_err(|e| e.to_string()))
            .expect_err("should fail to parse");

        assert!(matches!(err, LlmError::Stream { .. }), "got {err:?}");
    }
}
