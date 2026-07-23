//! Streaming agent runs. Realizes ADR-0006 §5 deferral closure
//! (Tier 2 priority 8).
//!
//! `Runtime::run_streaming` (added in Task 6) yields a
//! `Stream<Item = RunEvent>` as the agent loop progresses — text
//! deltas as the LLM types, tool calls as the LLM commits to them,
//! tool results as dispatch finishes. The terminal `RunCompleted`
//! event carries the final `RunOutcome` (success or failure).
//!
//! See `docs/superpowers/specs/2026-04-30-streaming-design.md` and
//! ADR-0011 (added in Task 12).

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use hashbrown::HashMap;

use futures_core::Stream;
use tau_domain::{
    Address, AgentDefinition, Capability, Message, MessagePayload, PackageManifest, Value,
};
// AgentInstanceId::new() (std-only) is only needed in the host-fs branch.
#[cfg(feature = "host-fs")]
use tau_domain::AgentInstanceId;
use tau_ports::{
    Clock, CompletionChunk, CompletionRequest, DenyEntry, LlmError, RandomSource, SessionContext,
    StopReason, TokenUsage, ToolError, ToolResult, ToolSpec,
};
use tracing::{debug, info, info_span, warn, Instrument as _};

use crate::builder::{DynLlmBackend, DynTool};
use crate::options::RunOptions;
use crate::outcome::RunOutcome;
/// Type of the per-tool schema validator passed to `run_streaming_inner`.
///
/// When `tool-validation` is enabled this is the real pre-compiled no_std
/// `ToolArgsValidator`.  Without `tool-validation` (e.g. the wasm guest)
/// schema validation is a no-op and the validator is `()`.
#[cfg(feature = "tool-validation")]
type ValidatorMap = HashMap<String, crate::tool_args::ToolArgsValidator>;
#[cfg(not(feature = "tool-validation"))]
type ValidatorMap = HashMap<String, ()>;
use crate::vocabulary::{
    EV_CONTEXT_STEP_RAN, EV_DISPATCH_TOOL_RESOLVED, EV_LLM_REQUEST_BUILT, EV_LLM_RESPONSE_RECEIVED,
    EV_LLM_STOP_REASON, EV_LLM_TOKEN_USAGE, EV_LLM_TOOL_USE_EMITTED, EV_MESSAGE_ADDED,
    EV_RUNTIME_COMPLETED, EV_RUNTIME_FAILED, EV_RUNTIME_LOOP_TERMINATED,
    EV_RUNTIME_MAX_TURNS_REACHED, EV_RUNTIME_RUN_STARTED, EV_RUNTIME_TURN_STARTED,
    EV_TOOL_INVOKE_FAILED, EV_TOOL_SESSION_CLOSE_FAILED, EV_TOOL_SESSION_OPEN_FAILED,
    SPAN_DISPATCH_TOOL, SPAN_RUNTIME_TURN, SPAN_TOOL_INVOKE, SPAN_TOOL_SESSION_CLOSE,
    SPAN_TOOL_SESSION_OPEN,
};

/// Return the `Clock` from `RunOptions`, panicking if absent.
///
/// # Caller contract
///
/// Every host shell that drives `run_streaming_inner` MUST inject a
/// `Clock` into `RunOptions.clock`.  Until the tokio shell's `drive`
/// entry (Phase β.1.4) sets `TokioClock` automatically, callers are
/// responsible for setting `opts.clock = Some(Arc::new(TokioClock))`.
/// Failing to do so will panic here at the first trace event.
fn clock_ref(opts: &RunOptions) -> &Arc<dyn Clock> {
    opts.clock
        .as_ref()
        .expect("RunOptions.clock must be set by host shell before calling run_streaming_inner")
}

/// Return the `RandomSource` from `RunOptions`, panicking if absent.
///
/// See `clock_ref` for the caller contract.
fn random_ref(opts: &RunOptions) -> &Arc<dyn RandomSource> {
    opts.random
        .as_ref()
        .expect("RunOptions.random must be set by host shell before calling run_streaming_inner")
}

/// Streaming event from `Runtime::run_streaming`.
///
/// Always terminates with exactly one `RunCompleted`; intermediate
/// events are unbounded per agent run. See spec §4.2 for the full
/// pump invariants.
///
/// Per ADR-0011:
/// - Every `ToolCallStarted` is followed by either a matching
///   `ToolCallCompleted` (same `id`) before the next `TurnCompleted`,
///   OR a terminal `RunCompleted { outcome: Failed }` if dispatch
///   crashed mid-flight.
/// - `TurnCompleted` arrives only after the turn's LLM `Finish` AND
///   all that turn's tool dispatches resolved.
/// - Stream order preserves LLM source order; the kernel never
///   reorders events.
///
/// # Example
///
/// ```
/// use tau_runtime_core::stream::RunEvent;
/// use tau_domain::Value;
///
/// // Classify events by variant.
/// fn describe(event: &RunEvent) -> &'static str {
///     match event {
///         RunEvent::TextDelta { .. } => "text",
///         RunEvent::ToolCallStarted { .. } => "tool-started",
///         RunEvent::ToolCallCompleted { .. } => "tool-completed",
///         RunEvent::TurnCompleted { .. } => "turn-completed",
///         RunEvent::RunCompleted { .. } => "run-completed",
///         RunEvent::FatalError { .. } => "fatal",
///         _ => "other",
///     }
/// }
///
/// let text_ev = RunEvent::TextDelta { delta: "hello".into() };
/// assert_eq!(describe(&text_ev), "text");
///
/// let tool_ev = RunEvent::ToolCallStarted {
///     id: "call-1".into(),
///     name: "echo".into(),
///     args: Value::Object(Default::default()),
/// };
/// assert_eq!(describe(&tool_ev), "tool-started");
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum RunEvent {
    /// Run started. Emitted once, before the first turn. Typed
    /// counterpart of the `runtime.run_started` tracing event (β.7.5:
    /// promoted to a typed variant so a no_std wasm guest can produce the
    /// β.6 conformance observable over a single channel — see ADR-0049).
    RunStarted,

    /// A β.4 context-pipeline transform ran. Emitted once per transform
    /// per turn, carrying the step name and the pre/post token estimates.
    /// Typed counterpart of the `runtime.context_step_ran` tracing event.
    ContextStepRan {
        /// Transform name (e.g. `"trim_old"`, `"fit_budget"`).
        step: String,
        /// Token estimate of the view entering this transform.
        tokens_in: u64,
        /// Token estimate of the view leaving this transform.
        tokens_out: u64,
    },

    /// The per-turn LLM request was built and is about to be sent. Typed
    /// counterpart of the `llm.request_built` tracing event.
    InferenceCallStarted,

    /// The per-turn LLM response finished streaming. Typed counterpart of
    /// `llm.response_received` folded with `llm.token_usage`: carries the
    /// turn's `StopReason` and token usage (zero when the provider did not
    /// report usage).
    InferenceCallCompleted {
        /// Why the turn's inference stopped.
        stop_reason: StopReason,
        /// Input tokens reported for this turn (`0` if unreported).
        tokens_in: u64,
        /// Output tokens reported for this turn (`0` if unreported).
        tokens_out: u64,
    },

    /// LLM emitted a text fragment. Concatenate with previous deltas
    /// for the running assistant message text.
    TextDelta {
        /// Text fragment to append.
        delta: String,
    },

    /// LLM emitted a complete `tool_use` block. Fires immediately
    /// when the kernel sees `CompletionChunk::ToolUse` — BEFORE the
    /// tool is dispatched. Display intent: "agent wants to call X
    /// with args Y". The matching `ToolCallCompleted` fires after
    /// dispatch finishes.
    ToolCallStarted {
        /// Provider-supplied tool-use id; correlates with
        /// `ToolCallCompleted.id`.
        id: String,
        /// Tool name.
        name: String,
        /// Args the LLM emitted.
        args: Value,
    },

    /// Tool dispatch finished. Fires after `Tool::invoke` returns,
    /// regardless of success/failure. Carries the tool result OR a
    /// validation/dispatch error message.
    ToolCallCompleted {
        /// Matches the `id` from `ToolCallStarted`.
        id: String,
        /// Tool name.
        name: String,
        /// `Ok(ToolResult)` on success; `Err(reason)` for validation
        /// failures or other recoverable errors. Plugin-crash-class
        /// errors don't surface here — they terminate the run via
        /// `RunCompleted`.
        result: Result<ToolResult, String>,
    },

    /// One turn of the agent loop completed. The LLM's `Finish`
    /// chunk arrived AND any tool calls within the turn finished
    /// dispatching.
    TurnCompleted {
        /// Why the turn ended (per LLM-reported `StopReason`).
        stop_reason: StopReason,
        /// Token usage for this turn. `None` if the provider did
        /// not report.
        usage: Option<TokenUsage>,
        /// Turn number (1-indexed) within the run.
        turn: u32,
    },

    /// Terminal event. Always exactly one per stream. After this
    /// fires, the stream returns `None`.
    RunCompleted {
        /// Final outcome — same shape as `Runtime::run` returns.
        outcome: RunOutcome,
    },

    /// Kernel-level fatal error (ADR-0006 § error dichotomy). In the
    /// batch path (`run_with_history` / `run`), the drainer converts
    /// this to `Err(RuntimeError)`. Streaming callers should treat
    /// this as an unrecoverable run abort.
    ///
    /// Distinct from `RunCompleted { Failed { BackendError } }` which
    /// signals an agent-level failure the caller can handle gracefully.
    FatalError {
        /// `RuntimeError` variant name for structured dispatch.
        /// One of: `"ToolNotRegistered"`, `"Llm"`, `"Tool"`, `"Internal"`.
        kind: String,
        /// Human-readable detail / primary error message.
        detail: String,
        /// Optional extra context (e.g. `tool_name`, `registered` list).
        /// Encoded as a JSON string for simplicity across crate boundaries.
        context_json: Option<String>,
        /// When `kind == "Tool"`, the inner `ToolError` variant name
        /// (`"BadArgs"`, `"Internal"`, `"SessionDead"`, `"DeadlineExceeded"`,
        /// `"CapabilityDenied"`, `"Llm"`, `"Storage"`). `None` for all
        /// other `kind` values. Used by the batch drainer to reconstruct
        /// the typed `ToolError` variant losslessly (Approach A fix).
        tool_error_variant: Option<String>,
    },
}

/// Build the stream of `RunEvent`s for a single agent run.
///
/// Happy path (no tool uses): drains the LLM stream, yields `TextDelta`
/// per chunk, then `TurnCompleted` + `RunCompleted` once `Finish` arrives.
///
/// Tool-dispatch path: for each `ToolUse` chunk, yields
/// `ToolCallStarted` immediately (per spec Q3-A: display intent). After
/// `Finish`, dispatches each tool with capability check + schema
/// validation + session open/invoke/teardown, yielding
/// `ToolCallCompleted` per tool. Then `TurnCompleted` and loops back
/// to the next turn. Terminates with `RunCompleted{Completed}` when
/// the LLM responds with no tool uses.
///
/// Persist a turn-boundary checkpoint for a durable agent (ADR-0053).
///
/// No-op unless BOTH a `checkpoint_store` and a `run_id` are set on
/// `options` (the interpreter sets them only for agents that declare
/// `durable`). A store failure is logged and swallowed, not fatal: a
/// missed checkpoint only means a crash would re-enter from the previous
/// committed turn (at-least-once is preserved), so it must not abort an
/// otherwise-healthy run.
fn persist_checkpoint_if_durable(
    options: &RunOptions,
    turn: u32,
    messages: &[Message],
    tokens: &crate::options::TokenUsage,
    pending_tool_uses: Vec<tau_ports::llm::ToolUse>,
) {
    if let (Some(store), Some(run_id)) =
        (options.checkpoint_store.as_ref(), options.run_id.as_ref())
    {
        let ckpt = tau_ports::orchestration::TurnCheckpoint {
            run_id: run_id.clone(),
            turn,
            history: messages.to_vec(),
            input_tokens: tokens.input_tokens,
            output_tokens: tokens.output_tokens,
            pending_tool_uses,
        };
        if let Err(e) = store.persist(&ckpt) {
            warn!(name = "runtime.checkpoint_failed", turn, error = %e);
        }
    }
}

/// Constructed inputs are pre-validated by the caller in Task 6
/// (`Runtime::run_streaming`); here we trust them.
#[allow(clippy::too_many_arguments)] // 12 params intentional: see Task 4 design doc
pub fn run_streaming_inner(
    backend: Arc<dyn DynLlmBackend>,
    agent_def: AgentDefinition,
    package_manifest: PackageManifest,
    history: Vec<Message>,
    initial_message: Message,
    options: RunOptions,
    tools: HashMap<String, Arc<dyn DynTool>>,
    tool_validators: ValidatorMap,
    granted_capabilities: Vec<Capability>,
    tool_specs: Vec<ToolSpec>,
    deny_entries: Vec<DenyEntry>,
    granted_for_session: Vec<Capability>,
) -> impl Stream<Item = RunEvent> + 'static {
    async_stream::stream! {
        // Use the no_std-safe constructor (clock/random from RunOptions) so
        // this path compiles under the `wasm-interpreter` feature without `std`.
        let agent_instance_id = crate::ids::agent_instance_id(clock_ref(&options), random_ref(&options));
        // ADR-0053: a resumed run rehydrates its committed history + turn
        // counter from the latest checkpoint and re-enters at the next turn,
        // so turns 1..=ckpt.turn are NOT re-billed (no LLM calls are made
        // for them). A fresh run extends the supplied history and pushes the
        // initial user message as turn 0's seed.
        // Carry any mid-turn pending tools (PerToolCall resume).
        // Non-empty only when the checkpoint was written after a per-tool-call
        // commit mid-turn; the first iteration consumes these via `mem::take`
        // and skips the LLM drain so the normal dispatch loop re-runs them.
        let mut resume_pending: Vec<tau_ports::llm::ToolUse> = options
            .resume_from
            .as_ref()
            .map(|c| c.pending_tool_uses.clone())
            .unwrap_or_default();

        let (mut messages, mut total_turns, mut aggregated_tokens) =
            if let Some(ckpt) = options.resume_from.clone() {
                let tokens = crate::options::TokenUsage {
                    input_tokens: ckpt.input_tokens,
                    output_tokens: ckpt.output_tokens,
                    total_tokens: None,
                };
                // Mid-turn resume: re-run the SAME turn (finish its pending
                // tools) before the loop advances. Seed one below `turn` so
                // the first `total_turns += 1` restores `turn`, and the
                // `TurnCompleted` event carries the correct (un-doubled) number.
                // Turn-boundary resume (empty pending) seeds `ckpt.turn` as before.
                let seed_turn = if ckpt.pending_tool_uses.is_empty() {
                    ckpt.turn
                } else {
                    ckpt.turn.saturating_sub(1)
                };
                (ckpt.history, seed_turn, tokens)
            } else {
                let mut messages: Vec<Message> = Vec::with_capacity(history.len() + 1);
                // ADR-0006 §3.9: emit one `message.added` per message pushed
                // onto the run history. No explicit `parent:` here — these
                // fire before any `runtime.turn` span opens, so they attach
                // to the outer `runtime.agent_run` span (or the caller's
                // current span).
                for m in &history {
                    debug!(
                        name = EV_MESSAGE_ADDED,
                        role = ?m.sender,
                    );
                }
                messages.extend(history);
                debug!(
                    name = EV_MESSAGE_ADDED,
                    role = ?initial_message.sender,
                );
                messages.push(initial_message);
                (messages, 0u32, crate::options::TokenUsage::default())
            };

        info!(name = EV_RUNTIME_RUN_STARTED);
        yield RunEvent::RunStarted;

        // max_turns guard: immediately report out-of-resources if 0.
        if options.max_turns == 0 {
            yield make_max_turns_outcome(messages, total_turns, aggregated_tokens, options.max_turns);
            return;
        }

        // Multi-turn loop: continues until LLM responds with no tool uses
        // OR max_turns is reached.
        while total_turns < options.max_turns {
            // Enforce the run budget (tokens / duration / agents) at each turn
            // boundary. On breach, end the run with OutOfResources instead of
            // spending another turn. RunState + budget are threaded via
            // `orchestration_state`; guest/test paths without it skip the check.
            if let Some(state_arc) = options.orchestration_state.as_ref() {
                let now = crate::ids::now_utc(clock_ref(&options));
                let breach = crate::orchestration::budget::BudgetWatchdog
                    .tick(&state_arc.borrow(), now)
                    .err();
                if let Some(err) = breach {
                    yield make_budget_exceeded_outcome(
                        messages,
                        total_turns,
                        aggregated_tokens,
                        err,
                    );
                    return;
                }
            }
            total_turns += 1;
            // Per ADR-0006 §3.9: wrap the turn body in a `runtime.turn`
            // span so child spans/events (llm.complete, dispatch.tool,
            // tool.* sessions, capability.check) attribute correctly to
            // a specific turn. `turn_index` is 1-indexed to match the
            // existing `runtime.turn_started` event's `turn` field.
            //
            // NOTE: we DO NOT call `turn_span.enter()` here — `EnteredSpan`
            // mutates a thread-local span stack, and the tokio multi-thread
            // scheduler can move this task between worker threads at any
            // `.await`, leaving the guard pointing at the wrong thread and
            // mis-parenting any child spans/events created on the new
            // worker. Instead, every event inside the turn body uses
            // `parent: &turn_span` explicitly, and every `.await` that
            // creates a child span (e.g. `llm.complete`, recursive
            // `run_with_history` calls) is wrapped with
            // `.instrument(turn_span.clone())`. This is the official
            // tracing-book pattern for spans whose body straddles awaits.
            let turn_span = info_span!(
                SPAN_RUNTIME_TURN,
                turn_index = u64::from(total_turns),
                messages_len = messages.len(),
            );
            debug!(parent: &turn_span, name = EV_RUNTIME_TURN_STARTED, turn = total_turns);

            // Hoist the four turn-body accumulators above the LLM-drain
            // prologue so both the mid-turn-resume path and the normal
            // LLM-drain path can write into them. The `if mid_turn_resume`
            // block below seeds `pending_tool_uses` directly; the `else`
            // block (the existing prologue) populates all four via the LLM
            // stream. Either way the dispatch loop that follows sees the
            // correct values.
            let mut accumulated_text = String::new();
            let mut turn_stop_reason: Option<StopReason> = None;
            let mut turn_usage: Option<TokenUsage> = None;
            let mut pending_tool_uses: Vec<tau_ports::ToolUse> = Vec::new();

            // PerToolCall mid-turn resume: the first turn after a mid-turn
            // checkpoint finishes the carried tools WITHOUT an LLM call.
            // `core::mem::take` empties `resume_pending` so subsequent
            // turns always fall through to the normal LLM-drain `else` path.
            let mid_turn_resume = !resume_pending.is_empty();
            if mid_turn_resume {
                pending_tool_uses = core::mem::take(&mut resume_pending);
                // Re-emit ToolCallStarted so the "Started precedes Completed"
                // invariant holds on the resumed stream. This seed block
                // pushes no message itself: the dispatch loop below pushes
                // each re-dispatched tool's ToolCall exactly once, which is
                // correct because these pending tools were never recorded in
                // the pre-crash history (the mid-turn checkpoint was taken
                // after the prior tool's result, before these tools started).
                for tu in &pending_tool_uses {
                    yield RunEvent::ToolCallStarted {
                        id: tu.id.clone(),
                        name: tu.name.clone(),
                        args: tu.input.clone(),
                    };
                }
            } else {
                // Normal path: build the request, run the context pipeline,
                // drain the LLM stream, emit gate events, push assistant text.
                let mut request = CompletionRequest::new(agent_def.model.clone());
                request.system = agent_def.system_prompt.clone();
                // β.4: derive a budgeted per-turn VIEW; stored `messages`
                // (full conversation) is never mutated.
                let provider_messages = if options.context_pipeline.is_empty() {
                    crate::run::agent_messages_to_provider_messages(&messages)
                } else {
                    let cx = crate::context::TransformCx::pure(
                        options.token_estimator.as_ref(),
                        agent_def.system_prompt.as_deref(),
                    );
                    let mut view = messages.clone();
                    let mut pipeline_failed: Option<alloc::string::String> = None;
                    for t in &options.context_pipeline {
                        let before: u32 = view.iter().map(|m| cx.estimate_tokens(m)).sum();
                        match t.transform(&cx, view).await {
                            Ok(next) => {
                                let after: u32 = next.iter().map(|m| cx.estimate_tokens(m)).sum();
                                debug!(
                                    parent: &turn_span,
                                    name = EV_CONTEXT_STEP_RAN,
                                    step = t.name(),
                                    tokens_in = before,
                                    tokens_out = after,
                                );
                                yield RunEvent::ContextStepRan {
                                    step: t.name().into(),
                                    tokens_in: u64::from(before),
                                    tokens_out: u64::from(after),
                                };
                                view = next;
                            }
                            Err(e) => {
                                pipeline_failed = Some(alloc::format!("{e}"));
                                view = alloc::vec::Vec::new();
                                break;
                            }
                        }
                    }
                    if let Some(detail) = pipeline_failed {
                        // Terminal kernel error: mirror the make_*_fatal_error
                        // family (e.g. make_llm_fatal_error at the LLM-open seam
                        // just below). FatalError is the batch drainer's signal
                        // to reconstruct Err(RuntimeError::ContextPipeline).
                        warn!(parent: &turn_span, name = EV_RUNTIME_FAILED, detail = %detail);
                        yield make_context_pipeline_fatal_error(detail);
                        return;
                    }
                    crate::run::agent_messages_to_provider_messages(&view)
                };
                request.messages = provider_messages;
                request.tools = tool_specs.clone();
                debug!(
                    parent: &turn_span,
                    name = EV_LLM_REQUEST_BUILT,
                    messages = request.messages.len(),
                    tools = request.tools.len(),
                );
                yield RunEvent::InferenceCallStarted;

                let llm_stream_result = async { backend.stream(request).await }
                    .instrument(info_span!(parent: &turn_span, "llm.complete"))
                    .await;
                let mut llm_stream = match llm_stream_result {
                    Ok(s) => s,
                    Err(llm_err) => {
                        warn!(parent: &turn_span, name = "runtime.streaming_llm_open_failed");
                        yield make_llm_fatal_error(llm_err);
                        return;
                    }
                };

                // Drain the LLM stream for this turn.
                // CompletionStream is Pin<Box<dyn Stream + Send>>; .as_mut() gives Pin<&mut S>.
                // Each chunk-poll is instrumented with `turn_span` so any tracing
                // emitted from inside the provider's stream impl attributes to
                // this turn.
                loop {
                    let next = core::future::poll_fn(|cx| llm_stream.as_mut().poll_next(cx))
                        .instrument(turn_span.clone())
                        .await;
                    match next {
                        None => break,
                        Some(Ok(CompletionChunk::Text { delta })) => {
                            accumulated_text.push_str(&delta);
                            yield RunEvent::TextDelta { delta };
                        }
                        Some(Ok(CompletionChunk::ToolUse(tool_use))) => {
                            // Per spec Q3-A: yield ToolCallStarted immediately on
                            // receipt — display intent BEFORE dispatch.
                            debug!(
                                parent: &turn_span,
                                name = "runtime.streaming_tool_use_received",
                                id = %tool_use.id,
                                tool_name = %tool_use.name,
                            );
                            yield RunEvent::ToolCallStarted {
                                id: tool_use.id.clone(),
                                name: tool_use.name.clone(),
                                args: tool_use.input.clone(),
                            };
                            pending_tool_uses.push(tool_use);
                        }
                        Some(Ok(CompletionChunk::Finish { stop_reason, usage })) => {
                            turn_stop_reason = Some(stop_reason);
                            turn_usage = usage;
                            break;
                        }
                        Some(Err(llm_err)) => {
                            warn!(parent: &turn_span, name = "runtime.streaming_llm_chunk_err");
                            yield make_llm_fatal_error(llm_err);
                            return;
                        }
                        // CompletionChunk is #[non_exhaustive]; ignore unknown variants.
                        Some(Ok(_)) => {}
                    }
                }

                debug!(
                    parent: &turn_span,
                    name = EV_LLM_RESPONSE_RECEIVED,
                    text_len = accumulated_text.len(),
                    tool_uses = pending_tool_uses.len(),
                    stop_reason = ?turn_stop_reason,
                );
                // β.7.5: typed counterpart of `llm.response_received` folded
                // with `llm.token_usage`. The dual-channel design pushed a
                // zero-token InferenceCallCompleted on response_received then
                // patched it on token_usage; reading `turn_usage` directly here
                // yields the identical result (zero when unreported). `turn_usage`
                // (tau_ports::TokenUsage) is `Copy`, so it stays usable below.
                yield RunEvent::InferenceCallCompleted {
                    stop_reason: turn_stop_reason.unwrap_or(StopReason::EndTurn),
                    tokens_in: turn_usage.map_or(0, |u| u64::from(u.input_tokens)),
                    tokens_out: turn_usage.map_or(0, |u| u64::from(u.output_tokens)),
                };

                // ADR-0006 §3.9: emit `llm.token_usage` when the backend
                // reports per-turn usage. Skip when `None` to avoid emitting
                // misleading zeros.
                if let Some(usage) = turn_usage {
                    let input = u64::from(usage.input_tokens);
                    let output = u64::from(usage.output_tokens);
                    info!(
                        parent: &turn_span,
                        name = EV_LLM_TOKEN_USAGE,
                        input_tokens = input,
                        output_tokens = output,
                        total_tokens = input.saturating_add(output),
                    );
                }

                // ADR-0006 §3.9: emit `llm.stop_reason` when the backend
                // reports one (StopReason has Debug, not Display).
                if let Some(reason) = turn_stop_reason {
                    debug!(
                        parent: &turn_span,
                        name = EV_LLM_STOP_REASON,
                        stop_reason = ?reason,
                    );
                }

                // ADR-0006 §3.9: emit one `llm.tool_use_emitted` per ToolUse
                // block returned by the LLM. Fires before dispatch so the
                // event records the model's intent regardless of whether
                // dispatch later succeeds.
                for tu in &pending_tool_uses {
                    debug!(
                        parent: &turn_span,
                        name = EV_LLM_TOOL_USE_EMITTED,
                        tool_name = %tu.name,
                        tool_use_id = %tu.id,
                    );
                }

                // Append assistant text to history if present.
                if !accumulated_text.is_empty() {
                    let agent_addr = Address::Agent(agent_instance_id);
                    let assistant_msg = Message::new_with(
                        crate::ids::message_id(clock_ref(&options), random_ref(&options)),
                        crate::ids::now_utc(clock_ref(&options)),
                        agent_addr,
                        Address::User,
                        MessagePayload::Text {
                            content: accumulated_text.clone(),
                        },
                    );
                    debug!(
                        parent: &turn_span,
                        name = EV_MESSAGE_ADDED,
                        role = ?assistant_msg.sender,
                    );
                    messages.push(assistant_msg);
                }

                // Accumulate token usage.
                if let Some(usage) = turn_usage {
                    aggregated_tokens.input_tokens = aggregated_tokens
                        .input_tokens
                        .saturating_add(u64::from(usage.input_tokens));
                    aggregated_tokens.output_tokens = aggregated_tokens
                        .output_tokens
                        .saturating_add(u64::from(usage.output_tokens));
                    // Feed per-turn usage into the shared RunState so the
                    // BudgetWatchdog (turn-boundary tick above) can enforce
                    // `max_total_tokens`.
                    if let Some(state_arc) = options.orchestration_state.as_ref() {
                        state_arc.borrow().add_tokens(
                            u64::from(usage.input_tokens)
                                .saturating_add(u64::from(usage.output_tokens)),
                        );
                    }
                }
            } // end else (normal LLM-drain prologue)

            // No tool uses → end of run.
            if pending_tool_uses.is_empty() {
                debug!(
                    parent: &turn_span,
                    name = EV_RUNTIME_LOOP_TERMINATED,
                    reason = "end_turn",
                );
                yield RunEvent::TurnCompleted {
                    stop_reason: turn_stop_reason.unwrap_or(StopReason::EndTurn),
                    usage: turn_usage,
                    turn: total_turns,
                };

                // ADR-0053: commit the final turn boundary for durable
                // agents (the assistant's closing message is already in
                // `messages`). A fully-completed turn has no pending tools.
                persist_checkpoint_if_durable(&options, total_turns, &messages, &aggregated_tokens, Vec::new());

                let final_message = messages
                    .last()
                    .cloned()
                    .expect("messages contains at least the initial user message");
                info!(
                    parent: &turn_span,
                    name = EV_RUNTIME_COMPLETED,
                    turn_index = total_turns,
                    all_messages = messages.len(),
                );
                yield RunEvent::RunCompleted {
                    outcome: RunOutcome::Completed {
                        final_message,
                        all_messages: messages,
                        total_turns,
                        token_usage: aggregated_tokens,
                    },
                };
                return;
            }

            // ----- Per-tool dispatch ----------------------------------------
            for (tool_idx, tool_use) in pending_tool_uses.iter().enumerate() {
                // ADR-0006 §3.9: each tool dispatch is wrapped in a
                // `dispatch.tool` span as a child of the turn span.
                // Same straddles-await discipline as `turn_span`: do not
                // call `.enter()`; events use `parent: &dispatch_span`
                // and `.await`s use `.instrument(dispatch_span.clone())`.
                let dispatch_span = info_span!(
                    parent: &turn_span,
                    SPAN_DISPATCH_TOOL,
                    tool_name = %tool_use.name,
                    tool_use_id = %tool_use.id,
                );
                debug!(
                    parent: &dispatch_span,
                    name = "llm.streaming_tool_use_dispatching",
                    id = %tool_use.id,
                    tool_name = %tool_use.name,
                );

                // ----- Orchestration virtual-tool intercept -----------------
                // When a multi-agent run is active and the tool name matches
                // task.* / run.* / agent.<kind>.spawn, dispatch in-kernel via
                // crate::orchestration instead of forwarding to a plugin host.
                // v1: task.* + run.* fully wired; agent.<kind>.spawn returns
                // an is_error=true ToolResult noting deferred-to-follow-up.
                if let Some(state_arc) = options.orchestration_state.as_ref() {
                    if crate::orchestration::is_virtual(&tool_use.name) {
                        // ADR-0006 §3.9: dispatch.tool_resolved fires after
                        // resolution succeeds. Virtual tools resolve to the
                        // in-kernel orchestration dispatcher rather than a
                        // registered plugin; use a sentinel plugin_id.
                        debug!(
                            parent: &dispatch_span,
                            name = EV_DISPATCH_TOOL_RESOLVED,
                            tool_name = %tool_use.name,
                            plugin_id = "kernel:virtual",
                        );
                        // Capability check against agent's grant.
                        // ADR-0006 §3.9: `check_capabilities_for_tool` owns
                        // the `capability.check` span plus the 5 capability
                        // events (required_loaded, granted_loaded,
                        // satisfies_check, allow, deny). Wrap in
                        // `dispatch_span.in_scope` so the span nests under
                        // `dispatch.tool` — `dispatch_span` is never
                        // `.enter()`'d (it straddles awaits), so without
                        // this the span would attach to whatever was
                        // entered higher up.
                        let required_cap = crate::orchestration::required_capability(
                            &tool_use.name,
                        );
                        let required_slice = core::slice::from_ref(&required_cap);
                        let missing = dispatch_span.in_scope(|| {
                            crate::capability::check_capabilities_for_tool(
                                &tool_use.name,
                                &granted_capabilities,
                                required_slice,
                            )
                        });
                        if let Some(cap) = missing {
                            let kind = crate::capability::capability_kind_str(cap);
                            let denial = crate::error::CapabilityDenial::new(
                                agent_def.id.to_string(),
                                agent_def.package.name.to_string(),
                                tool_use.name.clone(),
                                kind,
                                format!("{cap:?}"),
                            );
                            let outcome = crate::run::build_policy_denied_outcome(
                                denial,
                                messages,
                                total_turns,
                                aggregated_tokens,
                            );
                            emit_policy_denied_failure(&turn_span, total_turns, &tool_use.name);
                            yield RunEvent::RunCompleted { outcome };
                            return;
                        }

                        // Append the tool-call message (parallels normal path).
                        let agent_addr = Address::Agent(agent_instance_id);
                        let tool_addr = Address::Tool(tool_use.name.clone());
                        let tool_call_msg = Message::new_with(
                            crate::ids::message_id(clock_ref(&options), random_ref(&options)),
                            crate::ids::now_utc(clock_ref(&options)),
                            agent_addr.clone(),
                            tool_addr.clone(),
                            MessagePayload::ToolCall {
                                args: tool_use.input.clone(),
                            },
                        );
                        debug!(
                            parent: &turn_span,
                            name = EV_MESSAGE_ADDED,
                            role = ?tool_call_msg.sender,
                        );
                        messages.push(tool_call_msg);

                        // Build the ToolResult.
                        let is_agent_spawn = tool_use.name.starts_with("agent.")
                            && tool_use.name.ends_with(".spawn");
                        // `is_skill_spawn` is host-fs-only: skill resolution
                        // goes through the injected SkillResolver port. Shells
                        // without host-fs (embassy/wasm v1) can't reach the
                        // skill dispatch arm; tool_use.name patterns are
                        // dispatched as plain plugin calls there.
                        #[cfg(feature = "host-fs")]
                        let is_skill_spawn = tool_use.name.starts_with("skill.")
                            && tool_use.name.ends_with(".spawn")
                            && tool_use.name.len() > "skill..spawn".len();
                        let agent_id_str = agent_def.id.to_string();
                        // tau_domain::Value -> serde_json::Value via serde
                        // (orchestration handlers parse serde_json::Value).
                        let args_json = serde_json::to_value(&tool_use.input)
                            .unwrap_or(serde_json::Value::Null);

                        // Skills-4: skill.<name>.spawn virtual tool dispatch.
                        // Parallel to is_agent_spawn above, but resolves an
                        // installed skill instead of an agent kind. Uses the
                        // same v1.1 Box::pin(child_runtime.run_with_history)
                        // recursion mechanic — no new kernel infrastructure.
                        //
                        // Early-exit (yield + continue) pattern instead of the
                        // if/else expression form used by is_agent_spawn, so
                        // that we can bail out cleanly on scope/validation
                        // failures without restructuring the rest of the arm.
                        #[cfg(feature = "host-fs")]
                        if is_skill_spawn {
                            // Resolve the skill via the injected SkillResolver
                            // port (host shells supply TauPkgSkillResolver;
                            // guest shells supply NoSkillResolver or None).
                            // A `None` resolver fails gracefully here.
                            let resolver = match options.skill_resolver.as_ref() {
                                Some(r) => r.clone(),
                                None => {
                                    yield make_skill_spawn_error_tool_result(
                                        tool_use,
                                        "no skill resolver available for skill resolution",
                                    );
                                    let err_msg = Message::new(
                                        tool_addr.clone(),
                                        agent_addr.clone(),
                                        MessagePayload::ToolError {
                                            kind: "orchestration_virtual_tool_error"
                                                .into(),
                                            message: "skill spawn failed: no skill \
                                                      resolver available for skill \
                                                      resolution"
                                                .into(),
                                            details: None,
                                        },
                                    );
                                    debug!(
                                        parent: &turn_span,
                                        name = EV_MESSAGE_ADDED,
                                        role = ?err_msg.sender,
                                    );
                                    messages.push(err_msg);
                                    continue;
                                }
                            };

                            // Validate: capability check, authorization, skill
                            // lookup, ${SKILL_DIR} substitution, scope_paths
                            // narrowing, subset law.
                            let skill_req = match crate::orchestration::validate_skill_spawn(
                                &tool_use.name,
                                &args_json,
                                &agent_id_str,
                                &granted_capabilities,
                                resolver.as_ref(),
                            ) {
                                Ok(r) => r,
                                Err(e) => {
                                    let err_msg = format!("{e}");
                                    yield make_skill_spawn_error_tool_result(
                                        tool_use,
                                        &err_msg,
                                    );
                                    let err_event_msg = Message::new(
                                        tool_addr.clone(),
                                        agent_addr.clone(),
                                        MessagePayload::ToolError {
                                            kind: "orchestration_virtual_tool_error"
                                                .into(),
                                            message: format!(
                                                "skill spawn failed: {err_msg}"
                                            ),
                                            details: None,
                                        },
                                    );
                                    debug!(
                                        parent: &turn_span,
                                        name = EV_MESSAGE_ADDED,
                                        role = ?err_event_msg.sender,
                                    );
                                    messages.push(err_event_msg);
                                    continue;
                                }
                            };

                            // If orchestration_runtime is None this is a
                            // single-agent run that somehow received a skill
                            // virtual tool — fail with is_error so the LLM
                            // can recover.
                            let skill_tool_result: ToolResult =
                                match options.orchestration_runtime.as_ref() {
                                    None => ToolResult::new(
                                        vec![tau_ports::ToolContent::Text {
                                            text: "skill.<name>.spawn: no orchestration \
                                                   runtime; this run was not launched \
                                                   via spawn_root_agent."
                                                .into(),
                                        }],
                                        true,
                                    ),
                                    Some(child_runtime) => {
                                        // Record the spawn in the shared
                                        // RunState's counter for budget.
                                        {
                                            let s = state_arc.borrow();
                                            s.record_agent_spawn();
                                        }

                                        // Build child agent id: parent id +
                                        // sanitized skill name suffix.
                                        // skill_name may contain dots — replace
                                        // with '-' for AgentId compliance.
                                        let safe_skill =
                                            skill_req.skill_name.replace('.', "-");
                                        let child_id_str = format!(
                                            "{}-skill-{}",
                                            agent_def.id.as_str(),
                                            safe_skill,
                                        );
                                        let child_id =
                                            core::str::FromStr::from_str(&child_id_str)
                                                .unwrap_or_else(|_| {
                                                    agent_def.id.clone()
                                                });

                                        // Build child def: same package +
                                        // llm_backend as parent; skill's
                                        // system_prompt (from SKILL.md or
                                        // caller override); new id; display
                                        // name identifies the skill.
                                        let mut child_def =
                                            tau_domain::AgentDefinition::new(
                                                child_id,
                                                format!(
                                                    "{} (skill)",
                                                    skill_req.skill_name
                                                ),
                                                agent_def.package.clone(),
                                                agent_def.llm_backend.clone(),
                                            );
                                        child_def = child_def.with_system_prompt(
                                            skill_req.system_prompt.clone(),
                                        );
                                        child_def = child_def
                                            .with_config(agent_def.config.clone());

                                        // Build child opts: share state + runtime
                                        // arc; override grant with validated skill
                                        // grant.
                                        let child_opts = crate::RunOptions {
                                            orchestration_state: Some(state_arc.clone()),
                                            orchestration_runtime: Some(child_runtime.clone()),
                                            granted_capabilities_override: Some(
                                                skill_req.grant.clone(),
                                            ),
                                            clock: options.clock.clone(),
                                            random: options.random.clone(),
                                            scope_root: options.scope_root.clone(),
                                            skill_resolver: options.skill_resolver.clone(),
                                            ..Default::default()
                                        };

                                        // Initial user message for the child.
                                        let child_msg = Message::new(
                                            Address::User,
                                            Address::Agent(AgentInstanceId::new()),
                                            MessagePayload::Text {
                                                content: skill_req.message.clone(),
                                            },
                                        );

                                        // Emit Spawn trace event before
                                        // recursing.
                                        {
                                            let s = state_arc.borrow();
                                            let run_id = s.run_id.clone();
                                            s.trace.emit(tau_ports::TraceEvent {
                                                id: crate::ids::ulid(clock_ref(&options), random_ref(&options)),
                                                ts: crate::ids::now_utc(clock_ref(&options)),
                                                run_id,
                                                agent_id: Some(agent_id_str.clone()),
                                                kind: tau_ports::TraceEventKind::Spawn {
                                                    child_id: child_id_str.clone(),
                                                    agent_kind: skill_req
                                                        .skill_name
                                                        .clone(),
                                                    grant_size: skill_req.grant.len(),
                                                },
                                            });
                                        }

                                        // Recurse. Box::pin for async recursion
                                        // (the future would otherwise be
                                        // infinitely sized). Instrument with
                                        // `turn_span` so the child's
                                        // `runtime.agent_run` span attaches to
                                        // THIS turn rather than the outer
                                        // `runtime.agent_run`.
                                        let child_runtime_clone = child_runtime.clone();
                                        let package_manifest_clone =
                                            package_manifest.clone();
                                        let child_outcome_res: Result<
                                            RunOutcome,
                                            crate::error::RuntimeError,
                                        > = Box::pin(async move {
                                            child_runtime_clone
                                                .run_with_history(
                                                    child_def,
                                                    package_manifest_clone,
                                                    Vec::new(),
                                                    child_msg,
                                                    child_opts,
                                                )
                                                .await
                                        })
                                        .instrument(turn_span.clone())
                                        .await;

                                        match child_outcome_res {
                                            Ok(RunOutcome::Completed {
                                                final_message,
                                                ..
                                            }) => {
                                                let text = match &final_message.payload
                                                {
                                                    MessagePayload::Text {
                                                        content,
                                                    } => content.clone(),
                                                    _ => {
                                                        "<child run completed without \
                                                         text payload>"
                                                            .to_string()
                                                    }
                                                };
                                                ToolResult::new(
                                                    vec![tau_ports::ToolContent::Text {
                                                        text,
                                                    }],
                                                    false,
                                                )
                                            }
                                            Ok(other) => ToolResult::new(
                                                vec![tau_ports::ToolContent::Text {
                                                    text: format!(
                                                        "child run did not complete: \
                                                         {other:?}"
                                                    ),
                                                }],
                                                true,
                                            ),
                                            Err(e) => ToolResult::new(
                                                vec![tau_ports::ToolContent::Text {
                                                    text: format!(
                                                        "child run error: {e}"
                                                    ),
                                                }],
                                                true,
                                            ),
                                        }
                                    }
                                };

                            // Append tool-result message so history is coherent
                            // (mirrors what the non-early-exit path does below).
                            let skill_result_payload = if skill_tool_result.is_error {
                                MessagePayload::ToolError {
                                    kind: "orchestration_virtual_tool_error".into(),
                                    message: crate::run::flatten_content_to_string(
                                        &skill_tool_result.content,
                                    ),
                                    details: None,
                                }
                            } else {
                                MessagePayload::ToolResult {
                                    body: crate::run::content_to_value(
                                        &skill_tool_result.content,
                                    ),
                                }
                            };
                            let skill_result_msg = Message::new(
                                tool_addr.clone(),
                                agent_addr.clone(),
                                skill_result_payload,
                            );
                            debug!(
                                parent: &turn_span,
                                name = EV_MESSAGE_ADDED,
                                role = ?skill_result_msg.sender,
                            );
                            messages.push(skill_result_msg);

                            yield RunEvent::ToolCallCompleted {
                                id: tool_use.id.clone(),
                                name: tool_use.name.clone(),
                                result: Ok(skill_tool_result),
                            };
                            continue; // skip rest of dispatch arm
                        }

                        let tool_result: ToolResult = if is_agent_spawn {
                            // v1.1: recursive agent.<kind>.spawn dispatch.
                            // Validate via validate_agent_spawn (parent's
                            // Agent::Spawn allowed_kinds check + capability
                            // subset law), then build a child agent_def +
                            // child opts with the validated narrowed grant +
                            // the same shared RunState, and recursively call
                            // Runtime::run_with_history. Child's final
                            // assistant text becomes the ToolResult body.
                            //
                            // If orchestration_runtime is None (single-agent
                            // run that somehow saw a multi-agent virtual tool),
                            // fail with is_error so the LLM can recover.
                            match options.orchestration_runtime.as_ref() {
                                None => ToolResult::new(
                                    vec![tau_ports::ToolContent::Text {
                                        text: "agent.<kind>.spawn: no orchestration runtime; \
                                               this run was not launched via spawn_root_agent."
                                            .into(),
                                    }],
                                    true,
                                ),
                                Some(child_runtime) => {
                                    match crate::orchestration::validate_agent_spawn(
                                        &tool_use.name,
                                        &args_json,
                                        &agent_id_str,
                                        &granted_capabilities,
                                    ) {
                                        Err(e) => ToolResult::new(
                                            vec![tau_ports::ToolContent::Text {
                                                text: format!("{e}"),
                                            }],
                                            true,
                                        ),
                                        Ok(req) => {
                                            // Record the spawn in the shared
                                            // RunState's counter for budget.
                                            {
                                                let s = state_arc.borrow();
                                                s.record_agent_spawn();
                                            }

                                            // Build child agent id: parent id +
                                            // 8-char ulid suffix, lowercased.
                                            // Falls back to parent id on
                                            // construction failure (defensive;
                                            // shouldn't happen for compliant
                                            // AgentIds).
                                            let suffix = crate::ids::ulid(clock_ref(&options), random_ref(&options))
                                                .to_lowercase();
                                            let suffix_short: String = suffix
                                                .chars()
                                                .filter(|c| {
                                                    c.is_ascii_lowercase()
                                                        || c.is_ascii_digit()
                                                })
                                                .take(8)
                                                .collect();
                                            let child_id_str = format!(
                                                "{}-{}",
                                                agent_def.id.as_str(),
                                                suffix_short
                                            );
                                            let child_id = core::str::FromStr::from_str(
                                                &child_id_str,
                                            )
                                            .unwrap_or_else(|_| {
                                                agent_def.id.clone()
                                            });

                                            // Build child def: inherit parent's
                                            // package + llm_backend + system
                                            // prompt; new id; display_name
                                            // derived from the spawn kind.
                                            let mut child_def =
                                                tau_domain::AgentDefinition::new(
                                                    child_id,
                                                    format!("{} (spawn)", req.kind),
                                                    agent_def.package.clone(),
                                                    agent_def.llm_backend.clone(),
                                                );
                                            // System prompt resolution (v1.2):
                                            // • Spawn arg `system_prompt`
                                            //   takes precedence (per-kind
                                            //   skill differentiation)
                                            // • Otherwise inherit parent's
                                            //   system_prompt
                                            // • Otherwise None
                                            let child_system_prompt = req
                                                .system_prompt
                                                .clone()
                                                .or_else(|| {
                                                    agent_def.system_prompt.clone()
                                                });
                                            if let Some(sp) = child_system_prompt {
                                                child_def = child_def
                                                    .with_system_prompt(sp);
                                            }
                                            child_def = child_def
                                                .with_config(agent_def.config.clone());

                                            // Build child opts: share state +
                                            // runtime arc; override grant with
                                                // validated child grant.
                                            let child_opts = crate::RunOptions {
                                                orchestration_state: Some(state_arc.clone()),
                                                orchestration_runtime: Some(
                                                    child_runtime.clone(),
                                                ),
                                                granted_capabilities_override: Some(
                                                    req.grant.clone(),
                                                ),
                                                clock: options.clock.clone(),
                                                random: options.random.clone(),
                                                scope_root: options.scope_root.clone(),
                                                skill_resolver: options.skill_resolver.clone(),
                                                ..Default::default()
                                            };

                                            // Initial user message for the child.
                                            let child_msg = Message::new_with(
                                                crate::ids::message_id(clock_ref(&options), random_ref(&options)),
                                                crate::ids::now_utc(clock_ref(&options)),
                                                Address::User,
                                                Address::Agent(crate::ids::agent_instance_id(clock_ref(&options), random_ref(&options))),
                                                MessagePayload::Text {
                                                    content: req.message,
                                                },
                                            );

                                            // Emit a Spawn trace event before
                                            // recursing, so the printer / log
                                            // can pick it up.
                                            {
                                                let s = state_arc.borrow();
                                                let run_id = s.run_id.clone();
                                                s.trace.emit(
                                                    tau_ports::TraceEvent {
                                                        id: crate::ids::ulid(clock_ref(&options), random_ref(&options)),
                                                        ts: crate::ids::now_utc(clock_ref(&options)),
                                                        run_id,
                                                        agent_id: Some(
                                                            agent_id_str.clone(),
                                                        ),
                                                        kind:
                                                            tau_ports::TraceEventKind::Spawn {
                                                                child_id: child_id_str
                                                                    .clone(),
                                                                agent_kind:
                                                                    req.kind.clone(),
                                                                grant_size: req
                                                                    .grant
                                                                    .len(),
                                                            },
                                                    },
                                                );
                                            }

                                            // Recurse. Box::pin for async
                                            // recursion (the future would
                                            // otherwise be infinitely sized).
                                            // Instrument with `turn_span` so
                                            // the child's `runtime.agent_run`
                                            // span attaches to THIS turn
                                            // rather than the outer
                                            // `runtime.agent_run`.
                                            let child_runtime_clone = child_runtime.clone();
                                            let package_manifest_clone = package_manifest.clone();
                                            let child_outcome_res: Result<
                                                RunOutcome,
                                                crate::error::RuntimeError,
                                            > = Box::pin(async move {
                                                child_runtime_clone
                                                    .run_with_history(
                                                        child_def,
                                                        package_manifest_clone,
                                                        Vec::new(),
                                                        child_msg,
                                                        child_opts,
                                                    )
                                                    .await
                                            })
                                            .instrument(turn_span.clone())
                                            .await;

                                            match child_outcome_res {
                                                Ok(RunOutcome::Completed {
                                                    final_message,
                                                    ..
                                                }) => {
                                                    let text = match &final_message
                                                        .payload
                                                    {
                                                        MessagePayload::Text {
                                                            content,
                                                        } => content.clone(),
                                                        _ => {
                                                            "<child run completed \
                                                             without text payload>"
                                                                .to_string()
                                                        }
                                                    };
                                                    ToolResult::new(
                                                        vec![
                                                            tau_ports::ToolContent::Text {
                                                                text,
                                                            },
                                                        ],
                                                        false,
                                                    )
                                                }
                                                Ok(other) => ToolResult::new(
                                                    vec![tau_ports::ToolContent::Text {
                                                        text: format!(
                                                            "child run did not \
                                                             complete: {other:?}"
                                                        ),
                                                    }],
                                                    true,
                                                ),
                                                Err(e) => ToolResult::new(
                                                    vec![tau_ports::ToolContent::Text {
                                                        text: format!(
                                                            "child run error: {e}"
                                                        ),
                                                    }],
                                                    true,
                                                ),
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            let dispatch_res = {
                                let dispatch_now = crate::ids::now_utc(clock_ref(&options));
                                let mut state = state_arc.borrow_mut();
                                crate::orchestration::dispatch(
                                    &tool_use.name,
                                    args_json,
                                    &agent_id_str,
                                    &mut state,
                                    dispatch_now,
                                )
                            };
                            match dispatch_res {
                                Ok(json) => ToolResult::new(
                                    vec![tau_ports::ToolContent::Text {
                                        text: serde_json::to_string(&json)
                                            .unwrap_or_else(|_| "{}".into()),
                                    }],
                                    false,
                                ),
                                Err(e) => ToolResult::new(
                                    vec![tau_ports::ToolContent::Text {
                                        text: format!("{e}"),
                                    }],
                                    true,
                                ),
                            }
                        };

                        // Append the tool-result message (parallels normal path).
                        let result_payload = if tool_result.is_error {
                            MessagePayload::ToolError {
                                kind: "orchestration_virtual_tool_error".into(),
                                message: crate::run::flatten_content_to_string(
                                    &tool_result.content,
                                ),
                                details: None,
                            }
                        } else {
                            MessagePayload::ToolResult {
                                body: crate::run::content_to_value(&tool_result.content),
                            }
                        };
                        let orch_result_msg = Message::new_with(
                            crate::ids::message_id(clock_ref(&options), random_ref(&options)),
                            crate::ids::now_utc(clock_ref(&options)),
                            tool_addr,
                            agent_addr,
                            result_payload,
                        );
                        debug!(
                            parent: &turn_span,
                            name = EV_MESSAGE_ADDED,
                            role = ?orch_result_msg.sender,
                        );
                        messages.push(orch_result_msg);

                        yield RunEvent::ToolCallCompleted {
                            id: tool_use.id.clone(),
                            name: tool_use.name.clone(),
                            result: Ok(tool_result),
                        };
                        continue; // next tool_use; skip plugin dispatch
                    }
                }

                // Resolve the tool from the registry snapshot.
                let tool = match tools.get(tool_use.name.as_str()) {
                    Some(t) => t.clone(),
                    None => {
                        // Tool not in the snapshot — fatal kernel error
                        // (unexpected LLM behavior). The batch drainer
                        // will convert this to RuntimeError::ToolNotRegistered.
                        warn!(
                            parent: &dispatch_span,
                            name = "runtime.streaming_tool_not_found",
                            tool_name = %tool_use.name,
                        );
                        let registered: Vec<String> =
                            tools.keys().cloned().collect();
                        yield make_tool_not_registered_error(
                            &tool_use.name,
                            &registered,
                        );
                        return;
                    }
                };
                // ADR-0006 §3.9: emit dispatch.tool_resolved once the
                // registry lookup returns a concrete plugin.
                debug!(
                    parent: &dispatch_span,
                    name = EV_DISPATCH_TOOL_RESOLVED,
                    tool_name = %tool_use.name,
                    plugin_id = %tool.name(),
                );

                // ----- Capability check -------------------------------------
                // ADR-0006 §3.9: `check_capabilities_for_tool` owns the
                // `capability.check` span + 5 capability events. Wrap
                // in `dispatch_span.in_scope` so the span nests under
                // `dispatch.tool`; `dispatch_span` straddles awaits and
                // is never `.enter()`'d.
                let required: &[Capability] = tool.capabilities();
                let missing = dispatch_span.in_scope(|| {
                    crate::capability::check_capabilities_for_tool(
                        &tool_use.name,
                        &granted_capabilities,
                        required,
                    )
                });
                if let Some(cap) = missing {
                    let kind = crate::capability::capability_kind_str(cap);
                    let denial = crate::error::CapabilityDenial::new(
                        agent_def.id.to_string(),
                        agent_def.package.name.to_string(),
                        tool_use.name.clone(),
                        kind,
                        format!("{cap:?}"),
                    );
                    let outcome = crate::run::build_policy_denied_outcome(
                        denial,
                        messages,
                        total_turns,
                        aggregated_tokens,
                    );
                    emit_policy_denied_failure(&turn_span, total_turns, &tool_use.name);
                    yield RunEvent::RunCompleted { outcome };
                    return;
                }

                // ----- Append the tool-call message -------------------------
                let agent_addr = Address::Agent(agent_instance_id);
                let tool_addr = Address::Tool(tool_use.name.clone());
                let tool_call_msg = Message::new_with(
                    crate::ids::message_id(clock_ref(&options), random_ref(&options)),
                    crate::ids::now_utc(clock_ref(&options)),
                    agent_addr.clone(),
                    tool_addr.clone(),
                    MessagePayload::ToolCall {
                        args: tool_use.input.clone(),
                    },
                );
                debug!(
                    parent: &turn_span,
                    name = EV_MESSAGE_ADDED,
                    role = ?tool_call_msg.sender,
                );
                messages.push(tool_call_msg);

                // ----- Open a session ---------------------------------------
                // SessionContext::new signature differs by `process` feature:
                // with `process`: (agent_id, session_id, deadline: Option<SystemTime>)
                // without `process` (wasm guest): (agent_id, session_id)
                #[cfg(feature = "process")]
                let ctx = SessionContext::new(agent_instance_id, crate::ids::uuid_v4(random_ref(&options)), None)
                    .with_granted_capabilities(granted_for_session.clone())
                    .with_deny_entries(deny_entries.clone());
                #[cfg(not(feature = "process"))]
                let ctx = SessionContext::new(agent_instance_id, crate::ids::uuid_v4(random_ref(&options)))
                    .with_granted_capabilities(granted_for_session.clone())
                    .with_deny_entries(deny_entries.clone());
                // ADR-0006 §3.9: `tool.session_open` span wraps the
                // `init` call AND the FAILED-event emit on the Err
                // branch so the failed event nests inside the span.
                // Parent is `dispatch.tool` (current at this point).
                let session_open_span = info_span!(
                    parent: &dispatch_span,
                    SPAN_TOOL_SESSION_OPEN,
                    tool_name = %tool_use.name,
                    session_id = %ctx.session_id,
                );
                if let Err(err) = tool
                    .init(ctx.clone())
                    .instrument(session_open_span.clone())
                    .await
                {
                    warn!(
                        parent: &session_open_span,
                        name = EV_TOOL_SESSION_OPEN_FAILED,
                        tool_name = %tool_use.name,
                    );
                    yield make_tool_fatal_error(err);
                    return;
                }
                drop(session_open_span);

                // ----- Schema validation (tool-validation feature only) ------
                // When `tool-validation` is enabled, pre-compiled no_std
                // `ToolArgsValidator` instances reject bad args before dispatch. Without it
                // (e.g. the wasm guest), validation is skipped — the
                // `tool_validators` map is `HashMap<String, ()>` and the
                // `crate::tool_args` module is absent.
                #[cfg(feature = "tool-validation")]
                {
                    let validator = tool_validators.get(tool_use.name.as_str()).expect(
                        "tool_validators is in 1:1 correspondence with tools \
                         (Task 4 invariant). If this fires, the registration \
                         pipeline is broken.",
                    );
                    match crate::tool_args::validate_tool_args(
                        &tool_use.input,
                        &tool_use.name,
                        validator,
                    ) {
                        Err(ToolError::BadArgs { reason }) => {
                            // Validation failure is recoverable: write a
                            // ToolError message into the conversation so the
                            // LLM gets to self-correct, then yield
                            // ToolCallCompleted with Err and continue.
                            let close_span = info_span!(
                                parent: &dispatch_span,
                                SPAN_TOOL_SESSION_CLOSE,
                                tool_name = %tool_use.name,
                                session_id = %ctx.session_id,
                            );
                            let _ = tool.teardown(()).instrument(close_span).await; // best-effort
                            warn!(
                                parent: &turn_span,
                                name = "tool.args_validation_failed",
                                tool_name = %tool_use.name,
                            );
                            let validation_err_msg = Message::new_with(
                                crate::ids::message_id(clock_ref(&options), random_ref(&options)),
                                crate::ids::now_utc(clock_ref(&options)),
                                tool_addr.clone(),
                                agent_addr.clone(),
                                MessagePayload::ToolError {
                                    kind: "tool_args_validation".into(),
                                    message: reason.clone(),
                                    details: None,
                                },
                            );
                            debug!(
                                parent: &turn_span,
                                name = EV_MESSAGE_ADDED,
                                role = ?validation_err_msg.sender,
                            );
                            messages.push(validation_err_msg);
                            yield RunEvent::ToolCallCompleted {
                                id: tool_use.id.clone(),
                                name: tool_use.name.clone(),
                                result: Err(reason),
                            };
                            continue; // next tool_use
                        }
                        Err(other) => {
                            // Defensive: validate_tool_args only emits BadArgs
                            // in v0.1 — reach here only if the contract changes.
                            let close_span = info_span!(
                                parent: &dispatch_span,
                                SPAN_TOOL_SESSION_CLOSE,
                                tool_name = %tool_use.name,
                                session_id = %ctx.session_id,
                            );
                            let _ = tool.teardown(()).instrument(close_span).await;
                            yield make_tool_fatal_error(other);
                            return;
                        }
                        Ok(_) => {} // proceed to invoke
                    }
                }
                #[cfg(not(feature = "tool-validation"))]
                let _ = &tool_validators; // suppress unused-variable warning

                // ----- Invoke -----------------------------------------------
                // ADR-0006 §3.9: `tool.invoke` span wraps the `invoke`
                // call AND the FAILED-event emit on Err so the failed
                // event nests inside the span. The `tool.args_received`
                // and `tool.result_received` events emitted inside
                // `IpcTool::invoke` also nest naturally under this span
                // (current-span propagation through `.instrument`).
                let invoke_span = info_span!(
                    parent: &dispatch_span,
                    SPAN_TOOL_INVOKE,
                    tool_name = %tool_use.name,
                    session_id = %ctx.session_id,
                );
                let invoke_outcome = tool
                    .invoke(&ctx, &mut (), tool_use.input.clone())
                    .instrument(invoke_span.clone())
                    .await;
                let tool_result: ToolResult = match invoke_outcome {
                    Ok(r) => r,
                    Err(err) => {
                        warn!(
                            parent: &invoke_span,
                            name = EV_TOOL_INVOKE_FAILED,
                            tool_name = %tool_use.name,
                        );
                        drop(invoke_span);
                        // best-effort teardown; nest under its own span
                        // so any failure inside teardown is observable.
                        let close_span = info_span!(
                            parent: &dispatch_span,
                            SPAN_TOOL_SESSION_CLOSE,
                            tool_name = %tool_use.name,
                            session_id = %ctx.session_id,
                        );
                        let _ = tool.teardown(()).instrument(close_span).await;
                        yield make_tool_fatal_error(err);
                        return;
                    }
                };
                drop(invoke_span);

                // ----- Close the session ------------------------------------
                let close_span = info_span!(
                    parent: &dispatch_span,
                    SPAN_TOOL_SESSION_CLOSE,
                    tool_name = %tool_use.name,
                    session_id = %ctx.session_id,
                );
                if let Err(err) = tool
                    .teardown(())
                    .instrument(close_span.clone())
                    .await
                {
                    warn!(
                        parent: &close_span,
                        name = EV_TOOL_SESSION_CLOSE_FAILED,
                        tool_name = %tool_use.name,
                    );
                    yield make_tool_fatal_error(err);
                    return;
                }
                drop(close_span);

                // ----- Append the tool-result message ----------------------
                let result_payload = if tool_result.is_error {
                    MessagePayload::ToolError {
                        kind: "tool_runtime_error".into(),
                        message: crate::run::flatten_content_to_string(&tool_result.content),
                        details: None,
                    }
                } else {
                    MessagePayload::ToolResult {
                        body: crate::run::content_to_value(&tool_result.content),
                    }
                };
                let tool_result_msg = Message::new_with(
                    crate::ids::message_id(clock_ref(&options), random_ref(&options)),
                    crate::ids::now_utc(clock_ref(&options)),
                    tool_addr,
                    agent_addr,
                    result_payload,
                );
                debug!(
                    parent: &turn_span,
                    name = EV_MESSAGE_ADDED,
                    role = ?tool_result_msg.sender,
                );
                messages.push(tool_result_msg);

                yield RunEvent::ToolCallCompleted {
                    id: tool_use.id.clone(),
                    name: tool_use.name.clone(),
                    result: Ok(tool_result),
                };

                // ADR-0053 follow-up: PerToolCall commits a mid-turn
                // checkpoint after each tool. The remaining (not-yet-run)
                // tools are carried explicitly — they are NOT in `messages`.
                // Overwrites turn-<n>.json (atomic-rename safe); the
                // turn-boundary write later finalizes it with empty pending.
                if options.durable_granularity
                    == Some(tau_ir::durable::CheckpointGranularity::PerToolCall)
                {
                    let remaining: Vec<tau_ports::llm::ToolUse> =
                        pending_tool_uses[tool_idx + 1..].to_vec();
                    persist_checkpoint_if_durable(
                        &options,
                        total_turns,
                        &messages,
                        &aggregated_tokens,
                        remaining,
                    );
                }
            }
            // End of per-tool dispatch for this turn.

            yield RunEvent::TurnCompleted {
                stop_reason: turn_stop_reason.unwrap_or(StopReason::ToolUse),
                usage: turn_usage,
                turn: total_turns,
            };

            // ADR-0053: commit a checkpoint at this turn boundary (after the
            // tool results are in `messages`) for durable agents, before
            // looping back into the next — possibly crashing — turn.
            // A fully-completed turn has no pending tools.
            persist_checkpoint_if_durable(&options, total_turns, &messages, &aggregated_tokens, Vec::new());

            // Loop back for the next turn (LLM will see tool results).
        }

        // ----- max_turns reached -------------------------------------------
        // No `parent:` here — this fires *after* the `while` loop, so the
        // last `turn_span` is no longer in scope; emit under the outer
        // `runtime.agent_run` span instead.
        warn!(
            name = EV_RUNTIME_MAX_TURNS_REACHED,
            turn_index = total_turns,
            max_turns = options.max_turns,
        );
        yield make_max_turns_outcome(messages, total_turns, aggregated_tokens, options.max_turns);
    }
}

// ---------------------------------------------------------------------------
// Outcome helpers
// ---------------------------------------------------------------------------

/// Emit `runtime.failed` with `failure_kind = PolicyDenied` under the given
/// turn span. Dedupes two byte-identical warn! sites (orchestration
/// virtual-tool deny + regular tool-dispatch deny).
fn emit_policy_denied_failure(turn_span: &tracing::Span, turn_index: u32, tool_name: &str) {
    warn!(
        parent: turn_span,
        name = EV_RUNTIME_FAILED,
        turn_index = turn_index,
        failure_kind = ?tau_domain::FailureKind::PolicyDenied,
        detail = %tool_name,
    );
}

// FatalError helpers: emit RunEvent::FatalError so the batch-path
// drainer (run_with_history) can convert them back to Err(RuntimeError),
// preserving the ADR-0006 error/failure dichotomy. Streaming callers
// (run_streaming_with_history) should treat FatalError as a run abort.

/// LLM-side fatal error (backend.stream / chunk error).
/// Drainer converts to Err(RuntimeError::Llm(_)) or Internal.
/// Build a `RunEvent::ToolCallCompleted` with `is_error = true` for a failed
/// `skill.<name>.spawn` virtual tool call. Used in the `is_skill_spawn` branch
/// of `run_streaming_inner` for early-exit error paths (scope resolution
/// failure, validation failure).
#[cfg_attr(not(feature = "host-fs"), allow(dead_code))]
fn make_skill_spawn_error_tool_result(tool_use: &tau_ports::ToolUse, msg: &str) -> RunEvent {
    RunEvent::ToolCallCompleted {
        id: tool_use.id.clone(),
        name: tool_use.name.clone(),
        result: Ok(ToolResult::new(
            vec![tau_ports::ToolContent::Text {
                text: format!("skill spawn failed: {msg}"),
            }],
            true,
        )),
    }
}

fn make_llm_fatal_error(llm_err: LlmError) -> RunEvent {
    RunEvent::FatalError {
        kind: "Llm".to_string(),
        detail: format!("{llm_err}"),
        context_json: None,
        tool_error_variant: None,
    }
}

/// Context-pipeline fatal error (a transformer failed or the budget was
/// unsatisfiable). Drainer converts to
/// `Err(RuntimeError::ContextPipeline { detail })`.
fn make_context_pipeline_fatal_error(detail: String) -> RunEvent {
    RunEvent::FatalError {
        kind: "ContextPipeline".to_string(),
        detail,
        context_json: None,
        tool_error_variant: None,
    }
}

/// Tool-side fatal error (init / invoke / teardown failure).
/// Drainer converts to Err(RuntimeError::Tool(_)).
///
/// Records the `ToolError` variant name in `tool_error_variant` so the
/// batch drainer can reconstruct the typed `ToolError::*` losslessly
/// (Approach A fix for Task 7 regression — see run.rs drainer).
fn make_tool_fatal_error(err: ToolError) -> RunEvent {
    let variant = match &err {
        ToolError::BadArgs { .. } => Some("BadArgs"),
        ToolError::Internal { .. } => Some("Internal"),
        ToolError::SessionDead { .. } => Some("SessionDead"),
        ToolError::DeadlineExceeded => Some("DeadlineExceeded"),
        ToolError::CapabilityDenied { .. } => Some("CapabilityDenied"),
        ToolError::Llm(_) => Some("Llm"),
        ToolError::Storage(_) => Some("Storage"),
        // `ToolError` is `#[non_exhaustive]`; unknown future variants fall
        // back to Internal in the drainer (None → default branch).
        _ => None,
    };
    RunEvent::FatalError {
        kind: "Tool".to_string(),
        detail: format!("{err}"),
        context_json: None,
        tool_error_variant: variant.map(String::from),
    }
}

/// Tool-not-registered fatal error.
/// Drainer converts to Err(RuntimeError::ToolNotRegistered { .. }).
fn make_tool_not_registered_error(tool_name: &str, registered: &[String]) -> RunEvent {
    let context = serde_json::json!({
        "tool_name": tool_name,
        "registered": registered,
    });
    RunEvent::FatalError {
        kind: "ToolNotRegistered".to_string(),
        detail: format!("tool `{tool_name}` not registered; registered: {registered:?}"),
        context_json: Some(context.to_string()),
        tool_error_variant: None,
    }
}

fn make_max_turns_outcome(
    messages: Vec<Message>,
    total_turns: u32,
    token_usage: crate::options::TokenUsage,
    max_turns: u32,
) -> RunEvent {
    use tau_domain::{AgentStatus, FailureKind};
    RunEvent::RunCompleted {
        outcome: RunOutcome::Failed {
            status: AgentStatus::failed(
                FailureKind::OutOfResources,
                Some(format!("max_turns ({max_turns}) reached")),
            ),
            all_messages: messages,
            total_turns,
            token_usage,
        },
    }
}

/// Build the terminal `RunCompleted{Failed{OutOfResources}}` event for a
/// [`crate::orchestration::budget::BudgetWatchdog`] breach (max tokens /
/// duration / agents), mirroring [`make_max_turns_outcome`].
fn make_budget_exceeded_outcome(
    messages: Vec<Message>,
    total_turns: u32,
    token_usage: crate::options::TokenUsage,
    err: crate::orchestration::error::OrchestrationError,
) -> RunEvent {
    use tau_domain::{AgentStatus, FailureKind};
    RunEvent::RunCompleted {
        outcome: RunOutcome::Failed {
            status: AgentStatus::failed(
                FailureKind::OutOfResources,
                Some(format!("run budget exceeded: {err}")),
            ),
            all_messages: messages,
            total_turns,
            token_usage,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use tau_ports::fixtures::{make_token_usage, make_tool_result};
    // Tests that build tool-validator maps use ToolArgsValidator directly.
    // The type is only present when `tool-validation` is enabled (the default).
    #[cfg(feature = "tool-validation")]
    use crate::tool_args::ToolArgsValidator;

    #[test]
    fn run_event_text_delta_clone_preserves_delta() {
        let e = RunEvent::TextDelta {
            delta: "Hello".into(),
        };
        assert_matches!(e.clone(), RunEvent::TextDelta { delta } => {
            assert_eq!(delta, "Hello");
        });
    }

    #[test]
    fn run_event_tool_call_started_clone_preserves_fields() {
        let e = RunEvent::ToolCallStarted {
            id: "call_1".into(),
            name: "fs-read".into(),
            args: Value::Null,
        };
        assert_matches!(e.clone(), RunEvent::ToolCallStarted { id, name, .. } => {
            assert_eq!(id, "call_1");
            assert_eq!(name, "fs-read");
        });
    }

    #[test]
    fn run_event_tool_call_completed_carries_result() {
        let e = RunEvent::ToolCallCompleted {
            id: "call_1".into(),
            name: "fs-read".into(),
            result: Ok(make_tool_result(vec![], false)),
        };
        assert_matches!(e, RunEvent::ToolCallCompleted { result, .. } => {
            assert!(result.is_ok());
        });
    }

    #[test]
    fn run_event_tool_call_completed_carries_error_reason() {
        let e = RunEvent::ToolCallCompleted {
            id: "call_1".into(),
            name: "fs-read".into(),
            result: Err("validation failed".into()),
        };
        assert_matches!(e, RunEvent::ToolCallCompleted { result, .. } => {
            assert_matches!(result, Err(reason) => {
                assert_eq!(reason, "validation failed");
            });
        });
    }

    #[test]
    fn run_event_turn_completed_carries_stop_reason_and_usage() {
        let e = RunEvent::TurnCompleted {
            stop_reason: StopReason::ToolUse,
            usage: Some(make_token_usage(10, 5)),
            turn: 3,
        };
        assert_matches!(
            e,
            RunEvent::TurnCompleted {
                stop_reason,
                usage,
                turn,
            } => {
                assert_eq!(stop_reason, StopReason::ToolUse);
                assert_eq!(turn, 3);
                assert!(usage.is_some());
            }
        );
    }

    #[test]
    fn new_gate_variants_serde_round_trip() {
        use tau_ports::StopReason;
        let evs = alloc::vec![
            RunEvent::RunStarted,
            RunEvent::ContextStepRan {
                step: "trim_old".into(),
                tokens_in: 4,
                tokens_out: 4,
            },
            RunEvent::InferenceCallStarted,
            RunEvent::InferenceCallCompleted {
                stop_reason: StopReason::ToolUse,
                tokens_in: 0,
                tokens_out: 0,
            },
        ];
        for ev in evs {
            let json = serde_json::to_string(&ev).expect("serialize");
            let back: RunEvent = serde_json::from_str(&json).expect("deserialize");
            // RunEvent is not PartialEq; assert the Debug shape round-trips.
            assert_eq!(alloc::format!("{ev:?}"), alloc::format!("{back:?}"));
        }
    }

    // ---- Task 3 tests: run_streaming_inner ----

    use std::sync::Arc;
    use tau_domain::{
        Address, AgentDefinition, AgentId, Message, MessagePayload, PackageId, PackageName, Version,
    };
    use tau_ports::{
        CompletionChunk, CompletionRequest, CompletionResponse, CompletionStream, LlmBackend,
        LlmError, StopReason as PortsStopReason, TokenUsage as PortsTokenUsage, ToolSpec,
    };

    use crate::builder::DynLlmBackend;
    use crate::options::RunOptions;

    /// Scripted LLM that emits a fixed sequence of CompletionChunk via stream().
    /// Supports multiple turns via a `VecDeque` of chunk sequences.
    struct ScriptedLlm {
        turns: std::sync::Mutex<std::collections::VecDeque<Vec<Result<CompletionChunk, LlmError>>>>,
    }

    impl ScriptedLlm {
        /// Single-turn: only one call to stream() is valid.
        fn new(chunks: Vec<Result<CompletionChunk, LlmError>>) -> Self {
            let mut deque = std::collections::VecDeque::new();
            deque.push_back(chunks);
            Self {
                turns: std::sync::Mutex::new(deque),
            }
        }

        /// Multi-turn: each call to stream() pops the next turn's chunks.
        fn multi_turn(turns: Vec<Vec<Result<CompletionChunk, LlmError>>>) -> Self {
            Self {
                turns: std::sync::Mutex::new(turns.into_iter().collect()),
            }
        }
    }

    impl LlmBackend for ScriptedLlm {
        fn name(&self) -> &str {
            "scripted-llm"
        }

        async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
            unimplemented!("ScriptedLlm streams only")
        }

        async fn stream(&self, _req: CompletionRequest) -> Result<CompletionStream, LlmError> {
            let chunks = self
                .turns
                .lock()
                .expect("lock poisoned")
                .pop_front()
                .ok_or_else(|| LlmError::Internal {
                    message: "ScriptedLlm: no more turns configured".into(),
                })?;
            Ok(Box::pin(async_stream::stream! {
                for c in chunks {
                    yield c;
                }
            }))
        }
    }

    fn agent_def() -> AgentDefinition {
        use std::str::FromStr;
        let pkg = PackageId::new(
            PackageName::from_str("test-pkg").unwrap(),
            Version::parse("0.1.0").unwrap(),
        );
        AgentDefinition::new(
            AgentId::from_str("test-agent").unwrap(),
            "test".to_string(),
            pkg,
            PackageName::from_str("scripted-llm").unwrap(),
        )
    }

    fn manifest_with_no_capabilities() -> tau_domain::PackageManifest {
        use tau_domain::UncheckedManifest;
        let toml_str = r#"
            name = "test-pkg"
            version = "0.1.0"
            description = "test package"
            authors = []
            source = "https://example.com/test.git"
            kind = "tool"
            dependencies = []
            capabilities = []
        "#;
        let unchecked: UncheckedManifest = toml::from_str(toml_str).unwrap();
        unchecked.validate().unwrap()
    }

    fn user_msg(text: &str) -> Message {
        Message::new(
            Address::User,
            Address::User,
            MessagePayload::Text {
                content: text.to_string(),
            },
        )
    }

    /// Build a `RunOptions` suitable for unit tests: injects `MockClock`
    /// and `DeterministicRandom` so port-routed id helpers don't panic.
    fn test_run_options() -> RunOptions {
        use tau_ports::{DeterministicRandom, MockClock};
        RunOptions {
            clock: Some(Arc::new(MockClock::new())),
            random: Some(Arc::new(DeterministicRandom::seeded(42))),
            ..Default::default()
        }
    }

    async fn collect_events(
        mut stream: impl futures_core::Stream<Item = RunEvent> + Unpin,
    ) -> Vec<RunEvent> {
        use std::pin::Pin;
        let mut out = Vec::new();
        loop {
            let next = std::future::poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)).await;
            match next {
                None => break,
                Some(e) => out.push(e),
            }
        }
        out
    }

    /// Drop the β.7.5 typed gate lifecycle events (`RunStarted`,
    /// `ContextStepRan`, `InferenceCallStarted`, `InferenceCallCompleted`)
    /// so the pre-existing pump-invariant assertions — which target the
    /// tool/text/turn/run events and their exact indices — keep holding
    /// unchanged. Gate-event ordering is locked separately by the
    /// `gate_events_*` full-sequence tests below and by the β.6
    /// conformance golden.
    fn strip_gate_events(events: Vec<RunEvent>) -> Vec<RunEvent> {
        events
            .into_iter()
            .filter(|e| {
                !matches!(
                    e,
                    RunEvent::RunStarted
                        | RunEvent::ContextStepRan { .. }
                        | RunEvent::InferenceCallStarted
                        | RunEvent::InferenceCallCompleted { .. }
                )
            })
            .collect()
    }

    /// Map a `RunEvent` to a short variant tag for ordering assertions.
    fn event_kind(e: &RunEvent) -> &'static str {
        match e {
            RunEvent::RunStarted => "RunStarted",
            RunEvent::ContextStepRan { .. } => "ContextStepRan",
            RunEvent::InferenceCallStarted => "InferenceCallStarted",
            RunEvent::InferenceCallCompleted { .. } => "InferenceCallCompleted",
            RunEvent::TextDelta { .. } => "TextDelta",
            RunEvent::ToolCallStarted { .. } => "ToolCallStarted",
            RunEvent::ToolCallCompleted { .. } => "ToolCallCompleted",
            RunEvent::TurnCompleted { .. } => "TurnCompleted",
            RunEvent::RunCompleted { .. } => "RunCompleted",
            RunEvent::FatalError { .. } => "FatalError",
        }
    }

    /// Build a ToolArgsValidator that accepts everything (opt-out schema).
    fn make_passthrough_validator() -> crate::tool_args::ToolArgsValidator {
        crate::tool_args::ToolArgsValidator::compile(&Value::Null)
            .expect("null schema must compile")
    }

    // ---- Task 3 tests (updated to pass empty collections) ----

    #[tokio::test]
    async fn happy_path_text_only_yields_text_delta_then_turn_completed_then_run_completed() {
        let llm: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlm::new(vec![
            Ok(CompletionChunk::Text {
                delta: "Hello ".into(),
            }),
            Ok(CompletionChunk::Text {
                delta: "world".into(),
            }),
            Ok(CompletionChunk::Finish {
                stop_reason: PortsStopReason::EndTurn,
                usage: Some(PortsTokenUsage::new(10, 5)),
            }),
        ]));

        let stream = run_streaming_inner(
            llm,
            agent_def(),
            manifest_with_no_capabilities(),
            vec![],
            user_msg("hi"),
            test_run_options(),
            HashMap::new(),
            HashMap::new(),
            vec![],
            vec![],
            vec![],
            vec![],
        );
        let events = strip_gate_events(collect_events(Box::pin(stream)).await);

        assert_eq!(events.len(), 4, "got events: {events:#?}");
        let RunEvent::TextDelta { delta } = &events[0] else {
            panic!("expected TextDelta, got {:?}", events[0])
        };
        assert_eq!(delta, "Hello ");
        let RunEvent::TextDelta { delta } = &events[1] else {
            panic!("expected TextDelta, got {:?}", events[1])
        };
        assert_eq!(delta, "world");
        assert!(matches!(events[2], RunEvent::TurnCompleted { .. }));
        assert!(matches!(events[3], RunEvent::RunCompleted { .. }));
    }

    #[tokio::test]
    async fn llm_error_mid_stream_yields_run_completed_failed() {
        let llm: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlm::new(vec![
            Ok(CompletionChunk::Text {
                delta: "Hello".into(),
            }),
            Err(LlmError::Internal {
                message: "provider blew up".into(),
            }),
        ]));

        let stream = run_streaming_inner(
            llm,
            agent_def(),
            manifest_with_no_capabilities(),
            vec![],
            user_msg("hi"),
            test_run_options(),
            HashMap::new(),
            HashMap::new(),
            vec![],
            vec![],
            vec![],
            vec![],
        );
        let events = strip_gate_events(collect_events(Box::pin(stream)).await);

        assert_eq!(events.len(), 2, "got events: {events:#?}");
        // events[0] = TextDelta("Hello"), events[1] = FatalError{kind="Llm"}
        let RunEvent::FatalError { kind, .. } = &events[1] else {
            panic!("expected FatalError, got {:?}", events[1])
        };
        assert_eq!(kind, "Llm", "expected Llm fatal error kind");
    }

    #[tokio::test]
    async fn llm_open_failure_yields_run_completed_failed_with_no_intermediate_events() {
        struct FailingLlm;
        impl LlmBackend for FailingLlm {
            fn name(&self) -> &str {
                "failing-llm"
            }
            async fn complete(
                &self,
                _r: CompletionRequest,
            ) -> Result<CompletionResponse, LlmError> {
                unimplemented!()
            }
            async fn stream(&self, _r: CompletionRequest) -> Result<CompletionStream, LlmError> {
                Err(LlmError::Internal {
                    message: "open failed".into(),
                })
            }
        }

        let llm: Arc<dyn DynLlmBackend> = Arc::new(FailingLlm);

        let stream = run_streaming_inner(
            llm,
            agent_def(),
            manifest_with_no_capabilities(),
            vec![],
            user_msg("hi"),
            test_run_options(),
            HashMap::new(),
            HashMap::new(),
            vec![],
            vec![],
            vec![],
            vec![],
        );
        let events = strip_gate_events(collect_events(Box::pin(stream)).await);

        assert_eq!(events.len(), 1, "got events: {events:#?}");
        // LLM open failure → FatalError{kind="Llm"} (batch drainer converts to Err(RuntimeError::Llm)).
        let RunEvent::FatalError { kind, .. } = &events[0] else {
            panic!("expected FatalError, got {:?}", events[0])
        };
        assert_eq!(kind, "Llm", "expected Llm fatal error kind");
    }

    // ---- Task 4 tests: tool-dispatch flow ----

    /// Helper to build a tool registry entry with a passthrough validator.
    #[allow(clippy::type_complexity)]
    fn make_tool_entry(
        name: &str,
        tool: Arc<dyn DynTool>,
    ) -> (
        HashMap<String, Arc<dyn DynTool>>,
        HashMap<String, ToolArgsValidator>,
        Vec<ToolSpec>,
    ) {
        let mut tools = HashMap::new();
        let mut validators = HashMap::new();
        let spec = tool.schema();
        tools.insert(name.to_string(), tool);
        validators.insert(name.to_string(), make_passthrough_validator());
        (tools, validators, vec![spec])
    }

    #[tokio::test]
    async fn tool_dispatch_happy_path_yields_tool_call_started_then_completed_then_turn_completed()
    {
        use tau_ports::fixtures::{make_tool_spec, MockTool};

        let spec = make_tool_spec("echo".into(), "echo tool".into(), Value::Null);
        let mock_tool = MockTool::new("echo", spec);
        let tool_arc: Arc<dyn DynTool> = Arc::new(mock_tool);
        let (tools, validators, tool_specs_list) = make_tool_entry("echo", tool_arc);

        // Turn 1: LLM emits ToolUse; Turn 2: LLM emits text + EndTurn.
        let llm: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlm::multi_turn(vec![
            vec![
                Ok(CompletionChunk::ToolUse(
                    tau_ports::fixtures::make_tool_use("call_1".into(), "echo".into(), Value::Null),
                )),
                Ok(CompletionChunk::Finish {
                    stop_reason: PortsStopReason::ToolUse,
                    usage: Some(PortsTokenUsage::new(10, 5)),
                }),
            ],
            vec![
                Ok(CompletionChunk::Text {
                    delta: "Done!".into(),
                }),
                Ok(CompletionChunk::Finish {
                    stop_reason: PortsStopReason::EndTurn,
                    usage: Some(PortsTokenUsage::new(5, 3)),
                }),
            ],
        ]));

        let stream = run_streaming_inner(
            llm,
            agent_def(),
            manifest_with_no_capabilities(),
            vec![],
            user_msg("hi"),
            test_run_options(),
            tools,
            validators,
            vec![],
            tool_specs_list,
            vec![],
            vec![],
        );
        let events = strip_gate_events(collect_events(Box::pin(stream)).await);

        // Expected sequence:
        // ToolCallStarted, ToolCallCompleted, TurnCompleted (turn 1),
        // TextDelta, TurnCompleted (turn 2), RunCompleted
        assert_eq!(events.len(), 6, "got events: {events:#?}");
        assert!(
            matches!(&events[0], RunEvent::ToolCallStarted { name, .. } if name == "echo"),
            "expected ToolCallStarted, got {:?}",
            events[0]
        );
        assert!(
            matches!(&events[1], RunEvent::ToolCallCompleted { name, result: Ok(_), .. } if name == "echo"),
            "expected ToolCallCompleted Ok, got {:?}",
            events[1]
        );
        assert!(
            matches!(&events[2], RunEvent::TurnCompleted { turn: 1, .. }),
            "expected TurnCompleted turn 1, got {:?}",
            events[2]
        );
        assert!(
            matches!(&events[3], RunEvent::TextDelta { delta } if delta == "Done!"),
            "expected TextDelta, got {:?}",
            events[3]
        );
        assert!(
            matches!(&events[4], RunEvent::TurnCompleted { turn: 2, .. }),
            "expected TurnCompleted turn 2, got {:?}",
            events[4]
        );
        assert!(
            matches!(
                &events[5],
                RunEvent::RunCompleted {
                    outcome: RunOutcome::Completed { .. }
                }
            ),
            "expected RunCompleted Completed, got {:?}",
            events[5]
        );
    }

    #[tokio::test]
    async fn tool_dispatch_capability_denial_yields_run_completed_failed() {
        use tau_ports::fixtures::{make_tool_spec, MockTool};

        let spec = make_tool_spec("secured-tool".into(), "needs fs cap".into(), Value::Null);

        // Build a custom tool that requires an fs.read capability.
        struct CapRequiringTool {
            inner: MockTool,
            required: Vec<tau_domain::Capability>,
        }

        impl tau_ports::Tool for CapRequiringTool {
            type Session = ();

            fn name(&self) -> &str {
                tau_ports::Tool::name(&self.inner)
            }

            fn schema(&self) -> tau_ports::ToolSpec {
                tau_ports::Tool::schema(&self.inner)
            }

            fn capabilities(&self) -> &[tau_domain::Capability] {
                &self.required
            }

            async fn init(
                &self,
                ctx: tau_ports::SessionContext,
            ) -> Result<Self::Session, tau_ports::ToolError> {
                tau_ports::Tool::init(&self.inner, ctx).await
            }

            async fn invoke(
                &self,
                session: &mut Self::Session,
                args: tau_domain::Value,
            ) -> Result<tau_ports::ToolResult, tau_ports::ToolError> {
                tau_ports::Tool::invoke(&self.inner, session, args).await
            }

            async fn teardown(&self, session: Self::Session) -> Result<(), tau_ports::ToolError> {
                tau_ports::Tool::teardown(&self.inner, session).await
            }
        }

        // Build the fs.read required capability via TOML.
        #[derive(serde::Deserialize)]
        struct CapWrapper {
            cap: tau_domain::Capability,
        }
        let required_cap: tau_domain::Capability = toml::from_str::<CapWrapper>(
            r#"[cap]
kind = "fs.read"
paths = ["/etc/**"]
"#,
        )
        .unwrap()
        .cap;

        let tool = CapRequiringTool {
            inner: MockTool::new("secured-tool", spec),
            required: vec![required_cap],
        };
        let tool_arc: Arc<dyn DynTool> = Arc::new(tool);
        let (tools, validators, tool_specs_list) = make_tool_entry("secured-tool", tool_arc);

        let llm: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlm::new(vec![
            Ok(CompletionChunk::ToolUse(
                tau_ports::fixtures::make_tool_use(
                    "call_1".into(),
                    "secured-tool".into(),
                    Value::Null,
                ),
            )),
            Ok(CompletionChunk::Finish {
                stop_reason: PortsStopReason::ToolUse,
                usage: None,
            }),
        ]));

        let stream = run_streaming_inner(
            llm,
            agent_def(),
            manifest_with_no_capabilities(),
            vec![],
            user_msg("hi"),
            test_run_options(),
            tools,
            validators,
            vec![], // no granted capabilities → denial
            tool_specs_list,
            vec![],
            vec![],
        );
        let events = strip_gate_events(collect_events(Box::pin(stream)).await);

        // Expected: ToolCallStarted then RunCompleted{Failed}
        assert_eq!(events.len(), 2, "got events: {events:#?}");
        assert!(
            matches!(&events[0], RunEvent::ToolCallStarted { name, .. } if name == "secured-tool"),
            "expected ToolCallStarted, got {:?}",
            events[0]
        );
        let RunEvent::RunCompleted { outcome } = &events[1] else {
            panic!("expected RunCompleted, got {:?}", events[1])
        };
        assert!(
            matches!(outcome, RunOutcome::Failed { .. }),
            "expected Failed outcome, got {:?}",
            outcome
        );
        // Verify it's a PolicyDenied failure.
        if let RunOutcome::Failed { status, .. } = outcome {
            let s = format!("{status:?}");
            assert!(s.contains("PolicyDenied"), "expected PolicyDenied in {s}");
        }
    }

    #[tokio::test]
    async fn tool_dispatch_schema_validation_failure_yields_tool_call_completed_with_err() {
        use tau_ports::fixtures::{make_tool_spec, MockTool};

        // Tool with a strict schema requiring {"x": string}
        let strict_schema = {
            use tau_domain::Value;
            // Build schema via serde_json and deserialize into tau_domain::Value.
            let j = serde_json::json!({
                "type": "object",
                "properties": { "x": { "type": "string" } },
                "required": ["x"],
                "additionalProperties": false
            });
            let s = serde_json::to_string(&j).unwrap();
            serde_json::from_str::<Value>(&s).unwrap()
        };

        let spec = make_tool_spec(
            "strict-tool".into(),
            "strict args".into(),
            strict_schema.clone(),
        );
        let mock_tool = MockTool::new("strict-tool", spec);
        let tool_arc: Arc<dyn DynTool> = Arc::new(mock_tool);

        let mut tools: HashMap<String, Arc<dyn DynTool>> = HashMap::new();
        let mut validators: HashMap<String, ToolArgsValidator> = HashMap::new();
        let tool_specs_list = vec![tool_arc.schema()];
        tools.insert("strict-tool".to_string(), tool_arc);
        // Compile the strict schema validator.
        validators.insert(
            "strict-tool".to_string(),
            crate::tool_args::ToolArgsValidator::compile(&strict_schema)
                .expect("strict schema must compile"),
        );

        // LLM sends invalid args (missing required "x" field):
        // Turn 1: ToolUse with bad args, then Finish.
        // Turn 2: Text + EndTurn (LLM self-corrects).
        let bad_args = {
            let j = serde_json::json!({ "y": 42 }); // missing "x"
            let s = serde_json::to_string(&j).unwrap();
            serde_json::from_str::<Value>(&s).unwrap()
        };

        let llm: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlm::multi_turn(vec![
            vec![
                Ok(CompletionChunk::ToolUse(
                    tau_ports::fixtures::make_tool_use(
                        "call_1".into(),
                        "strict-tool".into(),
                        bad_args,
                    ),
                )),
                Ok(CompletionChunk::Finish {
                    stop_reason: PortsStopReason::ToolUse,
                    usage: None,
                }),
            ],
            vec![
                Ok(CompletionChunk::Text {
                    delta: "Corrected".into(),
                }),
                Ok(CompletionChunk::Finish {
                    stop_reason: PortsStopReason::EndTurn,
                    usage: None,
                }),
            ],
        ]));

        let stream = run_streaming_inner(
            llm,
            agent_def(),
            manifest_with_no_capabilities(),
            vec![],
            user_msg("hi"),
            test_run_options(),
            tools,
            validators,
            vec![],
            tool_specs_list,
            vec![],
            vec![],
        );
        let events = strip_gate_events(collect_events(Box::pin(stream)).await);

        // Expected:
        // ToolCallStarted, ToolCallCompleted{Err(reason)}, TurnCompleted (turn 1),
        // TextDelta, TurnCompleted (turn 2), RunCompleted
        assert_eq!(events.len(), 6, "got events: {events:#?}");
        assert!(
            matches!(&events[0], RunEvent::ToolCallStarted { name, .. } if name == "strict-tool"),
            "expected ToolCallStarted, got {:?}",
            events[0]
        );
        let RunEvent::ToolCallCompleted { result, .. } = &events[1] else {
            panic!("expected ToolCallCompleted, got {:?}", events[1])
        };
        assert!(
            result.is_err(),
            "expected Err result for validation failure, got {:?}",
            result
        );
        assert!(
            matches!(&events[2], RunEvent::TurnCompleted { turn: 1, .. }),
            "expected TurnCompleted turn 1, got {:?}",
            events[2]
        );
        assert!(
            matches!(&events[3], RunEvent::TextDelta { .. }),
            "expected TextDelta turn 2, got {:?}",
            events[3]
        );
        assert!(
            matches!(&events[4], RunEvent::TurnCompleted { turn: 2, .. }),
            "expected TurnCompleted turn 2, got {:?}",
            events[4]
        );
        assert!(
            matches!(
                &events[5],
                RunEvent::RunCompleted {
                    outcome: RunOutcome::Completed { .. }
                }
            ),
            "expected RunCompleted Completed, got {:?}",
            events[5]
        );
    }

    #[tokio::test]
    async fn tool_dispatch_plugin_crash_yields_run_completed_failed() {
        use tau_ports::fixtures::{make_tool_spec, MockTool};

        let spec = make_tool_spec(
            "crashing-tool".into(),
            "crashes on invoke".into(),
            Value::Null,
        );
        let mock_tool =
            MockTool::new("crashing-tool", spec).with_error(tau_ports::ToolError::Internal {
                message: "plugin exploded".into(),
            });
        let tool_arc: Arc<dyn DynTool> = Arc::new(mock_tool);
        let (tools, validators, tool_specs_list) = make_tool_entry("crashing-tool", tool_arc);

        let llm: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlm::new(vec![
            Ok(CompletionChunk::ToolUse(
                tau_ports::fixtures::make_tool_use(
                    "call_1".into(),
                    "crashing-tool".into(),
                    Value::Null,
                ),
            )),
            Ok(CompletionChunk::Finish {
                stop_reason: PortsStopReason::ToolUse,
                usage: None,
            }),
        ]));

        let stream = run_streaming_inner(
            llm,
            agent_def(),
            manifest_with_no_capabilities(),
            vec![],
            user_msg("hi"),
            test_run_options(),
            tools,
            validators,
            vec![],
            tool_specs_list,
            vec![],
            vec![],
        );
        let events = strip_gate_events(collect_events(Box::pin(stream)).await);

        // Expected: ToolCallStarted then FatalError{kind="Tool"}
        // (no ToolCallCompleted — plugin crash terminates the run;
        // the batch drainer converts FatalError to Err(RuntimeError::Tool))
        assert_eq!(events.len(), 2, "got events: {events:#?}");
        assert!(
            matches!(&events[0], RunEvent::ToolCallStarted { name, .. } if name == "crashing-tool"),
            "expected ToolCallStarted, got {:?}",
            events[0]
        );
        let RunEvent::FatalError { kind, .. } = &events[1] else {
            panic!("expected FatalError, got {:?}", events[1])
        };
        assert_eq!(kind, "Tool", "expected Tool fatal error kind");
    }

    #[tokio::test]
    async fn tool_dispatch_two_tools_in_one_turn_emits_both_started_and_both_completed() {
        use tau_ports::fixtures::{make_tool_spec, MockTool};

        let spec_a = make_tool_spec("tool-a".into(), "tool a".into(), Value::Null);
        let spec_b = make_tool_spec("tool-b".into(), "tool b".into(), Value::Null);
        let tool_a: Arc<dyn DynTool> = Arc::new(MockTool::new("tool-a", spec_a));
        let tool_b: Arc<dyn DynTool> = Arc::new(MockTool::new("tool-b", spec_b));

        let mut tools: HashMap<String, Arc<dyn DynTool>> = HashMap::new();
        let mut validators: HashMap<String, ToolArgsValidator> = HashMap::new();
        let spec_a2 = tool_a.schema();
        let spec_b2 = tool_b.schema();
        tools.insert("tool-a".to_string(), tool_a);
        tools.insert("tool-b".to_string(), tool_b);
        validators.insert("tool-a".to_string(), make_passthrough_validator());
        validators.insert("tool-b".to_string(), make_passthrough_validator());
        let tool_specs_list = vec![spec_a2, spec_b2];

        // Turn 1: two ToolUse chunks + Finish.
        // Turn 2: text + EndTurn.
        let llm: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlm::multi_turn(vec![
            vec![
                Ok(CompletionChunk::ToolUse(
                    tau_ports::fixtures::make_tool_use("id_a".into(), "tool-a".into(), Value::Null),
                )),
                Ok(CompletionChunk::ToolUse(
                    tau_ports::fixtures::make_tool_use("id_b".into(), "tool-b".into(), Value::Null),
                )),
                Ok(CompletionChunk::Finish {
                    stop_reason: PortsStopReason::ToolUse,
                    usage: None,
                }),
            ],
            vec![
                Ok(CompletionChunk::Text {
                    delta: "both done".into(),
                }),
                Ok(CompletionChunk::Finish {
                    stop_reason: PortsStopReason::EndTurn,
                    usage: None,
                }),
            ],
        ]));

        let stream = run_streaming_inner(
            llm,
            agent_def(),
            manifest_with_no_capabilities(),
            vec![],
            user_msg("hi"),
            test_run_options(),
            tools,
            validators,
            vec![],
            tool_specs_list,
            vec![],
            vec![],
        );
        let events = strip_gate_events(collect_events(Box::pin(stream)).await);

        // Expected order:
        // ToolCallStarted(a), ToolCallStarted(b)  [during LLM stream drain]
        // ToolCallCompleted(a), ToolCallCompleted(b)  [during dispatch]
        // TurnCompleted(1)
        // TextDelta
        // TurnCompleted(2)
        // RunCompleted
        assert_eq!(events.len(), 8, "got events: {events:#?}");

        assert!(
            matches!(&events[0], RunEvent::ToolCallStarted { name, .. } if name == "tool-a"),
            "expected ToolCallStarted(a), got {:?}",
            events[0]
        );
        assert!(
            matches!(&events[1], RunEvent::ToolCallStarted { name, .. } if name == "tool-b"),
            "expected ToolCallStarted(b), got {:?}",
            events[1]
        );
        assert!(
            matches!(&events[2], RunEvent::ToolCallCompleted { name, result: Ok(_), .. } if name == "tool-a"),
            "expected ToolCallCompleted(a) Ok, got {:?}",
            events[2]
        );
        assert!(
            matches!(&events[3], RunEvent::ToolCallCompleted { name, result: Ok(_), .. } if name == "tool-b"),
            "expected ToolCallCompleted(b) Ok, got {:?}",
            events[3]
        );
        assert!(
            matches!(&events[4], RunEvent::TurnCompleted { turn: 1, .. }),
            "expected TurnCompleted(1), got {:?}",
            events[4]
        );
        assert!(
            matches!(&events[5], RunEvent::TextDelta { .. }),
            "expected TextDelta, got {:?}",
            events[5]
        );
        assert!(
            matches!(&events[6], RunEvent::TurnCompleted { turn: 2, .. }),
            "expected TurnCompleted(2), got {:?}",
            events[6]
        );
        assert!(
            matches!(
                &events[7],
                RunEvent::RunCompleted {
                    outcome: RunOutcome::Completed { .. }
                }
            ),
            "expected RunCompleted Completed, got {:?}",
            events[7]
        );
    }

    // ---- Task 5 tests: failure-mode coverage ----

    #[tokio::test]
    async fn max_turns_reached_yields_run_completed_failed_out_of_resources() {
        use tau_domain::{AgentStatus, FailureKind};
        use tau_ports::fixtures::{make_tool_spec, MockTool};

        let spec = make_tool_spec("echo".into(), "echo tool".into(), Value::Null);
        let mock_tool = MockTool::new("echo", spec);
        let tool_arc: Arc<dyn DynTool> = Arc::new(mock_tool);
        let (tools, validators, tool_specs_list) = make_tool_entry("echo", tool_arc);

        // max_turns = 1: the first turn dispatches a tool use and loops
        // back; the while-condition `total_turns < 1` is now false, so
        // the loop falls through to make_max_turns_outcome.
        let options = {
            let mut o = test_run_options();
            o.max_turns = 1;
            o
        };

        // Turn 1 only: LLM emits ToolUse + Finish(ToolUse).
        // No turn 2 configured — the loop must exit via max_turns guard.
        let llm: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlm::new(vec![
            Ok(CompletionChunk::ToolUse(
                tau_ports::fixtures::make_tool_use("call_1".into(), "echo".into(), Value::Null),
            )),
            Ok(CompletionChunk::Finish {
                stop_reason: PortsStopReason::ToolUse,
                usage: Some(PortsTokenUsage::new(10, 5)),
            }),
        ]));

        let stream = run_streaming_inner(
            llm,
            agent_def(),
            manifest_with_no_capabilities(),
            vec![],
            user_msg("hi"),
            options,
            tools,
            validators,
            vec![],
            tool_specs_list,
            vec![],
            vec![],
        );
        let events = strip_gate_events(collect_events(Box::pin(stream)).await);

        // Expected: ToolCallStarted, ToolCallCompleted, TurnCompleted, RunCompleted{Failed{OutOfResources}}
        assert_eq!(events.len(), 4, "got events: {events:#?}");
        assert!(
            matches!(&events[0], RunEvent::ToolCallStarted { name, .. } if name == "echo"),
            "expected ToolCallStarted(echo), got {:?}",
            events[0]
        );
        assert!(
            matches!(&events[1], RunEvent::ToolCallCompleted { name, result: Ok(_), .. } if name == "echo"),
            "expected ToolCallCompleted(echo) Ok, got {:?}",
            events[1]
        );
        assert!(
            matches!(&events[2], RunEvent::TurnCompleted { turn: 1, .. }),
            "expected TurnCompleted(1), got {:?}",
            events[2]
        );
        let RunEvent::RunCompleted { outcome } = &events[3] else {
            panic!("expected RunCompleted, got {:?}", events[3])
        };
        let RunOutcome::Failed { status, .. } = outcome else {
            panic!("expected Failed outcome, got {:?}", outcome)
        };
        let AgentStatus::Failed { kind, .. } = status else {
            panic!("expected AgentStatus::Failed, got {:?}", status)
        };
        assert_eq!(
            *kind,
            FailureKind::OutOfResources,
            "expected OutOfResources, got {:?}",
            kind
        );
    }

    /// A run that exceeds `RunBudget::max_total_tokens` must abort with
    /// `OutOfResources` at the next turn boundary rather than run on. Turn 1
    /// burns 15 tokens against a 10-token budget; if the budget were NOT
    /// enforced, turn 2's scripted text response would complete the run.
    #[tokio::test]
    async fn run_budget_max_tokens_exceeded_aborts_with_out_of_resources() {
        use crate::orchestration::run_state::RunState;
        use core::cell::RefCell;
        use tau_domain::{AgentStatus, FailureKind};
        use tau_ports::fixtures::{make_tool_spec, MockTool};
        use tau_ports::RunBudget;

        let spec = make_tool_spec("echo".into(), "echo tool".into(), Value::Null);
        let tool_arc: Arc<dyn DynTool> = Arc::new(MockTool::new("echo", spec));
        let (tools, validators, tool_specs_list) = make_tool_entry("echo", tool_arc);

        let state = RunState::new(
            "run-budget".into(),
            "agent-1".into(),
            RunBudget {
                max_total_tokens: Some(10),
                ..Default::default()
            },
            chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).unwrap(),
        );
        #[allow(clippy::arc_with_non_send_sync)]
        let state_arc = Arc::new(RefCell::new(state));

        let options = {
            let mut o = test_run_options();
            o.max_turns = 5; // high enough that max_turns is NOT the trigger
            o.orchestration_state = Some(state_arc.clone());
            o
        };

        // Turn 1: ToolUse + Finish(10 in / 5 out = 15). Turn 2 (only reached
        // if the budget is NOT enforced): plain text that completes the run.
        let llm: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlm::new(vec![
            Ok(CompletionChunk::ToolUse(tau_ports::fixtures::make_tool_use(
                "call_1".into(),
                "echo".into(),
                Value::Null,
            ))),
            Ok(CompletionChunk::Finish {
                stop_reason: PortsStopReason::ToolUse,
                usage: Some(PortsTokenUsage::new(10, 5)),
            }),
            Ok(CompletionChunk::Text {
                delta: "done".into(),
            }),
            Ok(CompletionChunk::Finish {
                stop_reason: PortsStopReason::EndTurn,
                usage: Some(PortsTokenUsage::new(1, 1)),
            }),
        ]));

        let stream = run_streaming_inner(
            llm,
            agent_def(),
            manifest_with_no_capabilities(),
            vec![],
            user_msg("hi"),
            options,
            tools,
            validators,
            vec![],
            tool_specs_list,
            vec![],
            vec![],
        );
        let events = strip_gate_events(collect_events(Box::pin(stream)).await);

        let Some(RunEvent::RunCompleted { outcome }) = events.last() else {
            panic!("expected RunCompleted last, got {:?}", events.last())
        };
        let RunOutcome::Failed { status, .. } = outcome else {
            panic!("budget breach must fail the run, got {outcome:?}")
        };
        let AgentStatus::Failed { kind, .. } = status else {
            panic!("expected AgentStatus::Failed, got {status:?}")
        };
        assert_eq!(
            *kind,
            FailureKind::OutOfResources,
            "expected OutOfResources from budget breach, got {kind:?}"
        );
    }

    #[tokio::test]
    async fn mid_dispatch_crash_after_one_success_yields_started_completed_started_then_run_completed_failed(
    ) {
        use tau_ports::fixtures::{make_tool_spec, MockTool};

        let spec_a = make_tool_spec("tool-ok".into(), "succeeds".into(), Value::Null);
        let spec_b = make_tool_spec("tool-crash".into(), "crashes".into(), Value::Null);

        // tool-ok: default success path.
        let tool_ok: Arc<dyn DynTool> = Arc::new(MockTool::new("tool-ok", spec_a));
        // tool-crash: configured to return Internal error from invoke.
        let tool_crash: Arc<dyn DynTool> = Arc::new(
            MockTool::new("tool-crash", spec_b).with_error(tau_ports::ToolError::Internal {
                message: "plugin exploded mid-dispatch".into(),
            }),
        );

        let mut tools: HashMap<String, Arc<dyn DynTool>> = HashMap::new();
        let mut validators: HashMap<String, ToolArgsValidator> = HashMap::new();
        let spec_ok2 = tool_ok.schema();
        let spec_crash2 = tool_crash.schema();
        tools.insert("tool-ok".to_string(), tool_ok);
        tools.insert("tool-crash".to_string(), tool_crash);
        validators.insert("tool-ok".to_string(), make_passthrough_validator());
        validators.insert("tool-crash".to_string(), make_passthrough_validator());
        let tool_specs_list = vec![spec_ok2, spec_crash2];

        // Single turn: two ToolUse chunks + Finish. No turn 2 needed
        // (the run terminates on the crash).
        let llm: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlm::new(vec![
            Ok(CompletionChunk::ToolUse(
                tau_ports::fixtures::make_tool_use("id_1".into(), "tool-ok".into(), Value::Null),
            )),
            Ok(CompletionChunk::ToolUse(
                tau_ports::fixtures::make_tool_use("id_2".into(), "tool-crash".into(), Value::Null),
            )),
            Ok(CompletionChunk::Finish {
                stop_reason: PortsStopReason::ToolUse,
                usage: None,
            }),
        ]));

        let stream = run_streaming_inner(
            llm,
            agent_def(),
            manifest_with_no_capabilities(),
            vec![],
            user_msg("hi"),
            test_run_options(),
            tools,
            validators,
            vec![],
            tool_specs_list,
            vec![],
            vec![],
        );
        let events = strip_gate_events(collect_events(Box::pin(stream)).await);

        // Expected order per spec §4.3 pump invariants:
        //   ToolCallStarted(id_1)  — during chunk drain
        //   ToolCallStarted(id_2)  — during chunk drain
        //   ToolCallCompleted(id_1, Ok)  — first dispatch succeeds
        //   FatalError{kind="Tool"}  — second dispatch crashes
        // No ToolCallCompleted for id_2 — terminal failure replaces it.
        // The batch drainer converts FatalError → Err(RuntimeError::Tool).
        assert_eq!(events.len(), 4, "got events: {events:#?}");
        assert!(
            matches!(&events[0], RunEvent::ToolCallStarted { id, .. } if id == "id_1"),
            "expected ToolCallStarted(id_1), got {:?}",
            events[0]
        );
        assert!(
            matches!(&events[1], RunEvent::ToolCallStarted { id, .. } if id == "id_2"),
            "expected ToolCallStarted(id_2), got {:?}",
            events[1]
        );
        assert!(
            matches!(&events[2], RunEvent::ToolCallCompleted { id, result: Ok(_), .. } if id == "id_1"),
            "expected ToolCallCompleted(id_1) Ok, got {:?}",
            events[2]
        );
        let RunEvent::FatalError { kind, .. } = &events[3] else {
            panic!("expected FatalError, got {:?}", events[3])
        };
        assert_eq!(kind, "Tool", "expected Tool fatal error kind");
    }

    #[tokio::test]
    async fn empty_llm_stream_yields_turn_completed_then_run_completed() {
        /// LLM whose stream() returns Ok but yields zero chunks.
        struct EmptyLlm;
        impl tau_ports::LlmBackend for EmptyLlm {
            fn name(&self) -> &str {
                "empty-llm"
            }
            async fn complete(
                &self,
                _r: tau_ports::CompletionRequest,
            ) -> Result<tau_ports::CompletionResponse, tau_ports::LlmError> {
                unimplemented!()
            }
            async fn stream(
                &self,
                _r: tau_ports::CompletionRequest,
            ) -> Result<tau_ports::CompletionStream, tau_ports::LlmError> {
                Ok(Box::pin(async_stream::stream! {
                    // yield nothing — empty stream
                    if false {
                        yield Ok(tau_ports::CompletionChunk::Text { delta: String::new() });
                    }
                }))
            }
        }

        let llm: Arc<dyn DynLlmBackend> = Arc::new(EmptyLlm);

        let stream = run_streaming_inner(
            llm,
            agent_def(),
            manifest_with_no_capabilities(),
            vec![],
            user_msg("hi"),
            test_run_options(),
            HashMap::new(),
            HashMap::new(),
            vec![],
            vec![],
            vec![],
            vec![],
        );
        let events = strip_gate_events(collect_events(Box::pin(stream)).await);

        // Drain loop's None arm exits with turn_stop_reason = None,
        // accumulated_text = "", pending_tool_uses = [].
        // Kernel falls back to StopReason::EndTurn (unwrap_or) and takes
        // the happy (no-tools) exit path.
        // Expected: TurnCompleted{EndTurn, usage:None, turn:1}, RunCompleted{Completed}
        assert_eq!(events.len(), 2, "got events: {events:#?}");
        let RunEvent::TurnCompleted {
            stop_reason,
            usage,
            turn,
        } = &events[0]
        else {
            panic!("expected TurnCompleted, got {:?}", events[0])
        };
        assert_eq!(
            *stop_reason,
            StopReason::EndTurn,
            "expected EndTurn fallback"
        );
        assert!(usage.is_none(), "expected no usage for empty stream");
        assert_eq!(*turn, 1, "expected turn 1");
        assert!(
            matches!(
                &events[1],
                RunEvent::RunCompleted {
                    outcome: RunOutcome::Completed { .. }
                }
            ),
            "expected RunCompleted Completed, got {:?}",
            events[1]
        );
    }

    // ---- β.7.5 tests: typed gate-event emission + ordering ----

    #[tokio::test]
    async fn gate_events_full_sequence_text_only() {
        let llm: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlm::new(vec![
            Ok(CompletionChunk::Text {
                delta: "Hello ".into(),
            }),
            Ok(CompletionChunk::Text {
                delta: "world".into(),
            }),
            Ok(CompletionChunk::Finish {
                stop_reason: PortsStopReason::EndTurn,
                usage: None,
            }),
        ]));

        let stream = run_streaming_inner(
            llm,
            agent_def(),
            manifest_with_no_capabilities(),
            vec![],
            user_msg("hi"),
            test_run_options(),
            HashMap::new(),
            HashMap::new(),
            vec![],
            vec![],
            vec![],
            vec![],
        );
        let events = collect_events(Box::pin(stream)).await;

        // Single-turn, no-tool, empty context pipeline → the typed gate
        // events bracket the run and the inference exactly:
        let kinds: Vec<&str> = events.iter().map(event_kind).collect();
        assert_eq!(
            kinds,
            vec![
                "RunStarted",
                "InferenceCallStarted",
                "TextDelta",
                "TextDelta",
                "InferenceCallCompleted",
                "TurnCompleted",
                "RunCompleted",
            ],
            "got events: {events:#?}"
        );

        // InferenceCallCompleted carries the turn's StopReason and zero
        // tokens (provider reported no usage).
        let RunEvent::InferenceCallCompleted {
            stop_reason,
            tokens_in,
            tokens_out,
        } = &events[4]
        else {
            panic!("expected InferenceCallCompleted, got {:?}", events[4])
        };
        assert_eq!(*stop_reason, StopReason::EndTurn);
        assert_eq!(*tokens_in, 0);
        assert_eq!(*tokens_out, 0);
    }

    #[tokio::test]
    async fn gate_events_full_sequence_tool_turn_carries_usage() {
        use tau_ports::fixtures::{make_tool_spec, MockTool};

        let spec = make_tool_spec("echo".into(), "echo tool".into(), Value::Null);
        let mock_tool = MockTool::new("echo", spec);
        let tool_arc: Arc<dyn DynTool> = Arc::new(mock_tool);
        let (tools, validators, tool_specs_list) = make_tool_entry("echo", tool_arc);

        // Turn 1: ToolUse + Finish(ToolUse) WITH usage; Turn 2: text + EndTurn.
        let llm: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlm::multi_turn(vec![
            vec![
                Ok(CompletionChunk::ToolUse(
                    tau_ports::fixtures::make_tool_use("call_1".into(), "echo".into(), Value::Null),
                )),
                Ok(CompletionChunk::Finish {
                    stop_reason: PortsStopReason::ToolUse,
                    usage: Some(PortsTokenUsage::new(10, 5)),
                }),
            ],
            vec![
                Ok(CompletionChunk::Text {
                    delta: "Done!".into(),
                }),
                Ok(CompletionChunk::Finish {
                    stop_reason: PortsStopReason::EndTurn,
                    usage: None,
                }),
            ],
        ]));

        let stream = run_streaming_inner(
            llm,
            agent_def(),
            manifest_with_no_capabilities(),
            vec![],
            user_msg("hi"),
            test_run_options(),
            tools,
            validators,
            vec![],
            tool_specs_list,
            vec![],
            vec![],
        );
        let events = collect_events(Box::pin(stream)).await;

        let kinds: Vec<&str> = events.iter().map(event_kind).collect();
        // Per-turn the typed events interleave: InferenceCallStarted fires
        // before the drain (so before ToolCallStarted), InferenceCallCompleted
        // fires after the drain (so after ToolCallStarted, before dispatch's
        // ToolCallCompleted).
        assert_eq!(
            kinds,
            vec![
                "RunStarted",
                "InferenceCallStarted",
                "ToolCallStarted",
                "InferenceCallCompleted",
                "ToolCallCompleted",
                "TurnCompleted",
                "InferenceCallStarted",
                "TextDelta",
                "InferenceCallCompleted",
                "TurnCompleted",
                "RunCompleted",
            ],
            "got events: {events:#?}"
        );

        // Turn 1's InferenceCallCompleted folds the reported token usage.
        let RunEvent::InferenceCallCompleted {
            stop_reason,
            tokens_in,
            tokens_out,
        } = &events[3]
        else {
            panic!("expected InferenceCallCompleted, got {:?}", events[3])
        };
        assert_eq!(*stop_reason, StopReason::ToolUse);
        assert_eq!(*tokens_in, 10);
        assert_eq!(*tokens_out, 5);
    }

    // ---- ADR-0053: durable checkpoint emit + resume ----

    /// A durable agent commits one checkpoint per completed turn, exercising
    /// BOTH boundary sites: the tool-loop site (turn 1, a tool_use turn) and
    /// the end-turn site (turn 2). The latest checkpoint carries the full
    /// accumulated history.
    #[tokio::test]
    async fn durable_agent_persists_a_checkpoint_per_turn() {
        use tau_ports::fixtures::{make_tool_spec, make_tool_use, MockCheckpointStore, MockTool};
        use tau_ports::CheckpointStore as _;

        let spec = make_tool_spec("echo".into(), "echo tool".into(), Value::Null);
        let tool_arc: Arc<dyn DynTool> = Arc::new(MockTool::new("echo", spec));
        let (tools, validators, tool_specs_list) = make_tool_entry("echo", tool_arc);

        let llm: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlm::multi_turn(vec![
            vec![
                Ok(CompletionChunk::ToolUse(make_tool_use(
                    "call_1".into(),
                    "echo".into(),
                    Value::Null,
                ))),
                Ok(CompletionChunk::Finish {
                    stop_reason: PortsStopReason::ToolUse,
                    usage: Some(PortsTokenUsage::new(10, 5)),
                }),
            ],
            vec![
                Ok(CompletionChunk::Text {
                    delta: "done".into(),
                }),
                Ok(CompletionChunk::Finish {
                    stop_reason: PortsStopReason::EndTurn,
                    usage: Some(PortsTokenUsage::new(3, 2)),
                }),
            ],
        ]));

        let store = Arc::new(MockCheckpointStore::new());
        let opts = RunOptions {
            checkpoint_store: Some(store.clone()),
            run_id: Some("run-xyz".into()),
            ..test_run_options()
        };

        let stream = run_streaming_inner(
            llm,
            agent_def(),
            manifest_with_no_capabilities(),
            vec![],
            user_msg("hi"),
            opts,
            tools,
            validators,
            vec![],
            tool_specs_list,
            vec![],
            vec![],
        );
        let events = strip_gate_events(collect_events(Box::pin(stream)).await);
        assert!(
            matches!(events.last(), Some(RunEvent::RunCompleted { .. })),
            "expected RunCompleted; got {events:#?}"
        );

        // One checkpoint per turn, in order (turn 1 via the tool-loop site,
        // turn 2 via the end-turn site).
        let run_id = String::from("run-xyz");
        assert_eq!(store.persisted_turns(&run_id), alloc::vec![1, 2]);

        // The turn-2 checkpoint carries the full accumulated history
        // (user + assistant(tool_use) + tool_result + assistant(text)).
        let latest = store
            .load_latest(&run_id)
            .expect("load ok")
            .expect("a checkpoint exists");
        assert_eq!(latest.turn, 2);
        assert!(
            latest.history.len() >= 3,
            "history should accumulate across turns; got {}",
            latest.history.len()
        );
        // Token accounting accumulated across both turns (10+3 / 5+2).
        assert_eq!(latest.input_tokens, 13);
        assert_eq!(latest.output_tokens, 7);
    }

    /// Resume re-enters at the next turn after the last committed checkpoint
    /// WITHOUT re-billing completed turns. Proof: the resumed run is handed a
    /// `ScriptedLlm` with EXACTLY ONE turn of chunks. Had resume restarted at
    /// turn 1, the loop would need a second LLM call and exhaust the script;
    /// completing with a single call proves turns 1–2 were not re-run.
    #[tokio::test]
    async fn durable_resume_re_enters_at_next_turn_without_rebilling() {
        use tau_ports::fixtures::MockCheckpointStore;
        use tau_ports::CheckpointStore as _;

        // A checkpoint as it would stand after turn 2 crashed mid-turn-3.
        let checkpoint = tau_ports::TurnCheckpoint {
            run_id: "run-resumed".into(),
            turn: 2,
            history: alloc::vec![user_msg("original task")],
            input_tokens: 13,
            output_tokens: 7,
            pending_tool_uses: vec![],
        };

        // EXACTLY ONE turn available — the resumed turn 3.
        let llm: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlm::new(vec![
            Ok(CompletionChunk::Text {
                delta: "final".into(),
            }),
            Ok(CompletionChunk::Finish {
                stop_reason: PortsStopReason::EndTurn,
                usage: Some(PortsTokenUsage::new(5, 3)),
            }),
        ]));

        let store = Arc::new(MockCheckpointStore::new());
        let opts = RunOptions {
            checkpoint_store: Some(store.clone()),
            run_id: Some("run-resumed".into()),
            resume_from: Some(checkpoint),
            ..test_run_options()
        };

        let stream = run_streaming_inner(
            llm,
            agent_def(),
            manifest_with_no_capabilities(),
            vec![],
            user_msg("ignored-on-resume"),
            opts,
            HashMap::new(),
            HashMap::new(),
            vec![],
            vec![],
            vec![],
            vec![],
        );
        let events = strip_gate_events(collect_events(Box::pin(stream)).await);

        // Completed via a single LLM turn → turns 1–2 were not re-billed.
        assert!(
            matches!(events.last(), Some(RunEvent::RunCompleted { .. })),
            "expected RunCompleted via a single resumed turn; got {events:#?}"
        );
        // The completed turn is turn 3 (continued from checkpoint turn 2).
        let last_turn = events.iter().rev().find_map(|e| match e {
            RunEvent::TurnCompleted { turn, .. } => Some(*turn),
            _ => None,
        });
        assert_eq!(
            last_turn,
            Some(3),
            "resume must continue at turn 3, not restart at turn 1"
        );

        // The resumed turn committed its own checkpoint at turn 3, with token
        // accounting continued from the checkpoint (13+5 / 7+3).
        let run_id = String::from("run-resumed");
        assert_eq!(store.persisted_turns(&run_id), alloc::vec![3]);
        let latest = store.load_latest(&run_id).unwrap().unwrap();
        assert_eq!(latest.input_tokens, 18);
        assert_eq!(latest.output_tokens, 10);
    }

    /// A durable PerToolCall agent writes a mid-turn checkpoint after each
    /// tool call, carrying the remaining (not-yet-run) tools. After all tools
    /// are dispatched and the turn-boundary write finalizes the checkpoint, the
    /// final stored snapshot must have empty `pending_tool_uses`.
    ///
    /// MockCheckpointStore keys by (run_id, turn) and overwrites on each
    /// `persist`, so we cannot inspect the intermediate snapshot directly. The
    /// authoritative DoD for the carried-pending behavior is Task 7's resume
    /// test. Here we assert: run completes + final checkpoint is clean.
    #[tokio::test]
    async fn per_tool_call_checkpoints_carry_remaining_tools() {
        use tau_ports::fixtures::{make_tool_spec, make_tool_use, MockCheckpointStore, MockTool};
        use tau_ports::CheckpointStore as _;

        // One turn: two tool uses (a, b), then an end-turn turn.
        let spec_a = make_tool_spec("tool-a".into(), "a".into(), Value::Null);
        let spec_b = make_tool_spec("tool-b".into(), "b".into(), Value::Null);
        let a: Arc<dyn DynTool> = Arc::new(MockTool::new("tool-a", spec_a));
        let b: Arc<dyn DynTool> = Arc::new(MockTool::new("tool-b", spec_b));
        let (mut tools, mut validators, mut specs) = make_tool_entry("tool-a", a);
        let (tb, vb, sb) = make_tool_entry("tool-b", b);
        tools.extend(tb);
        validators.extend(vb);
        specs.extend(sb);

        let llm: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlm::multi_turn(vec![
            vec![
                Ok(CompletionChunk::ToolUse(make_tool_use(
                    "a".into(),
                    "tool-a".into(),
                    Value::Null,
                ))),
                Ok(CompletionChunk::ToolUse(make_tool_use(
                    "b".into(),
                    "tool-b".into(),
                    Value::Null,
                ))),
                Ok(CompletionChunk::Finish {
                    stop_reason: PortsStopReason::ToolUse,
                    usage: Some(PortsTokenUsage::new(10, 5)),
                }),
            ],
            vec![
                Ok(CompletionChunk::Text {
                    delta: "done".into(),
                }),
                Ok(CompletionChunk::Finish {
                    stop_reason: PortsStopReason::EndTurn,
                    usage: Some(PortsTokenUsage::new(3, 2)),
                }),
            ],
        ]));

        let store = Arc::new(MockCheckpointStore::new());
        let opts = RunOptions {
            checkpoint_store: Some(store.clone()),
            run_id: Some("run-ptc".into()),
            durable_granularity: Some(tau_ir::durable::CheckpointGranularity::PerToolCall),
            ..test_run_options()
        };

        let stream = run_streaming_inner(
            llm,
            agent_def(),
            manifest_with_no_capabilities(),
            vec![],
            user_msg("hi"),
            opts,
            tools,
            validators,
            vec![],
            specs,
            vec![],
            vec![],
        );
        let events = strip_gate_events(collect_events(Box::pin(stream)).await);
        assert!(
            matches!(events.last(), Some(RunEvent::RunCompleted { .. })),
            "got {events:#?}"
        );

        // After tool-a's mid-turn checkpoint, turn-1 carried [tool-b].
        // The turn-1 boundary write then finalized it with empty pending,
        // so load_latest (turn 2) carries no pending.
        let run_id = String::from("run-ptc");
        let latest = store.load_latest(&run_id).unwrap().unwrap();
        assert!(
            latest.pending_tool_uses.is_empty(),
            "final checkpoint must be clean; got {:?}",
            latest.pending_tool_uses
        );
        // Both turn 1 and turn 2 have committed checkpoints.
        assert_eq!(
            store.persisted_turns(&run_id),
            alloc::vec![1, 2],
            "expected turns [1, 2] to be persisted"
        );

        // Core per-tool-call invariant: the per-tool checkpoint written after
        // tool-a completed must carry tool-b in pending_tool_uses.  This
        // assertion fails if the per-tool checkpoint site is removed/disabled.
        let persists = store.all_persists();
        let mid = persists
            .iter()
            .find(|c| c.turn == 1 && !c.pending_tool_uses.is_empty())
            .expect(
                "expected a mid-turn per-tool checkpoint for turn 1 with non-empty \
                 pending_tool_uses — the PerToolCall checkpoint site may be missing",
            );
        assert_eq!(
            mid.pending_tool_uses.len(),
            1,
            "after tool-a, exactly tool-b should remain; got {:?}",
            mid.pending_tool_uses
        );
        assert_eq!(
            mid.pending_tool_uses[0].name, "tool-b",
            "the remaining pending tool must be tool-b"
        );
    }

    // ---- Task 7 helpers ----

    /// Build an Agent→Tool ToolCall message (as the stream.rs dispatch arm does
    /// for a normal tool-call turn). Used to seed checkpoint history.
    fn agent_tool_call_msg(tool_name: &str, args: Value) -> Message {
        Message::new(
            Address::Agent(AgentInstanceId::new()),
            Address::Tool(tool_name.to_string()),
            MessagePayload::ToolCall { args },
        )
    }

    /// Build a Tool→Agent ToolResult message (as the stream.rs dispatch arm does
    /// after a successful tool invocation). Used to seed checkpoint history.
    fn tool_result_msg(tool_name: &str, body: Value) -> Message {
        Message::new(
            Address::Tool(tool_name.to_string()),
            Address::Agent(AgentInstanceId::new()),
            MessagePayload::ToolResult { body },
        )
    }

    /// DoD: a turn issued two tools; the process crashed after tool-a's
    /// mid-turn checkpoint. Resume re-dispatches ONLY tool-b (tool-a is not
    /// re-invoked) and completes — with a single subsequent LLM call.
    #[tokio::test]
    async fn per_tool_call_resume_redispatches_only_pending_tool() {
        use tau_ports::fixtures::{make_tool_spec, make_tool_use, MockCheckpointStore, MockTool};

        // tool-a already completed (must NOT run again); tool-b is pending.
        let spec_a = make_tool_spec("tool-a".into(), "a".into(), Value::Null);
        let spec_b = make_tool_spec("tool-b".into(), "b".into(), Value::Null);
        let a = Arc::new(MockTool::new("tool-a", spec_a));
        let b = Arc::new(MockTool::new("tool-b", spec_b));
        let a_dyn: Arc<dyn DynTool> = a.clone();
        let b_dyn: Arc<dyn DynTool> = b.clone();
        let (mut tools, mut validators, mut specs) = make_tool_entry("tool-a", a_dyn);
        let (tb, vb, sb) = make_tool_entry("tool-b", b_dyn);
        tools.extend(tb);
        validators.extend(vb);
        specs.extend(sb);

        // Mid-turn checkpoint: turn 1 in progress, tool-a done, tool-b pending.
        let checkpoint = tau_ports::TurnCheckpoint {
            run_id: "run-mid".into(),
            turn: 1,
            history: alloc::vec![
                user_msg("do both"),
                // tool-a's ToolCall + ToolResult already in history:
                agent_tool_call_msg("tool-a", Value::Null),
                tool_result_msg("tool-a", Value::Null),
            ],
            input_tokens: 10,
            output_tokens: 5,
            pending_tool_uses: alloc::vec![
                make_tool_use("b".into(), "tool-b".into(), Value::Null,)
            ],
        };

        // EXACTLY ONE turn available — the post-completion turn 2.
        let llm: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlm::new(vec![
            Ok(CompletionChunk::Text {
                delta: "final".into(),
            }),
            Ok(CompletionChunk::Finish {
                stop_reason: PortsStopReason::EndTurn,
                usage: Some(PortsTokenUsage::new(4, 2)),
            }),
        ]));

        let store = Arc::new(MockCheckpointStore::new());
        let opts = RunOptions {
            checkpoint_store: Some(store.clone()),
            run_id: Some("run-mid".into()),
            resume_from: Some(checkpoint),
            durable_granularity: Some(tau_ir::durable::CheckpointGranularity::PerToolCall),
            ..test_run_options()
        };

        let stream = run_streaming_inner(
            llm,
            agent_def(),
            manifest_with_no_capabilities(),
            vec![],
            user_msg("ignored-on-resume"),
            opts,
            tools,
            validators,
            vec![],
            specs,
            vec![],
            vec![],
        );
        let events = strip_gate_events(collect_events(Box::pin(stream)).await);

        // tool-b dispatched exactly once; tool-a never re-invoked.
        assert_eq!(b.invocations().len(), 1, "tool-b must run once on resume");
        assert_eq!(a.invocations().len(), 0, "tool-a must NOT be re-invoked");

        // ToolCallStarted(tool-b) precedes ToolCallCompleted(tool-b) (invariant).
        let started = events
            .iter()
            .position(|e| matches!(e, RunEvent::ToolCallStarted { name, .. } if name == "tool-b"));
        let completed = events.iter().position(
            |e| matches!(e, RunEvent::ToolCallCompleted { name, .. } if name == "tool-b"),
        );
        assert!(
            started.is_some() && completed.is_some() && started < completed,
            "ToolCallStarted(tool-b) must precede ToolCallCompleted(tool-b); events: {events:#?}"
        );

        // The partial turn finished as turn 1 (no double-count), then turn 2 ran.
        let turns: Vec<u32> = events
            .iter()
            .filter_map(|e| match e {
                RunEvent::TurnCompleted { turn, .. } => Some(*turn),
                _ => None,
            })
            .collect();
        assert_eq!(
            turns,
            vec![1, 2],
            "finish turn 1 mid-turn, then turn 2; got {turns:?}"
        );
        assert!(
            matches!(events.last(), Some(RunEvent::RunCompleted { .. })),
            "stream must end with RunCompleted; got {events:#?}"
        );
    }
}
