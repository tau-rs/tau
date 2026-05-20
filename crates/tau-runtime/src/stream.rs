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

use std::collections::HashMap;
use std::sync::Arc;

use futures_core::Stream;
use tau_domain::{
    Address, AgentDefinition, AgentInstanceId, Capability, Message, MessagePayload,
    PackageManifest, Value,
};
use tau_observe::vocabulary::{
    EV_CAPABILITY_DENY, EV_LLM_REQUEST_BUILT, EV_LLM_RESPONSE_RECEIVED, EV_RUNTIME_LOOP_TERMINATED,
    EV_RUNTIME_RUN_STARTED, EV_RUNTIME_TURN_STARTED, EV_TOOL_INVOKE_FAILED,
    EV_TOOL_SESSION_CLOSE_FAILED, EV_TOOL_SESSION_OPEN_FAILED,
};
use tau_ports::{
    CompletionChunk, CompletionRequest, DenyEntry, LlmError, SessionContext, StopReason,
    TokenUsage, ToolError, ToolResult, ToolSpec,
};
use tracing::{debug, info, info_span, warn, Instrument as _};

use crate::builder::{DynLlmBackend, DynTool};
use crate::options::RunOptions;
use crate::outcome::RunOutcome;
use crate::tool_args::ToolArgsValidator;

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
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum RunEvent {
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
/// Constructed inputs are pre-validated by the caller in Task 6
/// (`Runtime::run_streaming`); here we trust them.
#[allow(dead_code)] // wired up by Task 6
#[allow(clippy::too_many_arguments)] // 12 params intentional: see Task 4 design doc
pub(crate) fn run_streaming_inner(
    backend: Arc<dyn DynLlmBackend>,
    agent_def: AgentDefinition,
    package_manifest: PackageManifest,
    history: Vec<Message>,
    initial_message: Message,
    options: RunOptions,
    tools: HashMap<String, Arc<dyn DynTool>>,
    tool_validators: HashMap<String, ToolArgsValidator>,
    granted_capabilities: Vec<Capability>,
    tool_specs: Vec<ToolSpec>,
    deny_entries: Vec<DenyEntry>,
    granted_for_session: Vec<Capability>,
) -> impl Stream<Item = RunEvent> + 'static {
    async_stream::stream! {
        let agent_instance_id = AgentInstanceId::new();
        let mut messages: Vec<Message> = Vec::with_capacity(history.len() + 1);
        messages.extend(history);
        messages.push(initial_message);
        let mut total_turns: u32 = 0;
        let mut aggregated_tokens = crate::options::TokenUsage::default();

        info!(name = EV_RUNTIME_RUN_STARTED);

        // max_turns guard: immediately report out-of-resources if 0.
        if options.max_turns == 0 {
            yield make_max_turns_outcome(messages, total_turns, aggregated_tokens, options.max_turns);
            return;
        }

        // Multi-turn loop: continues until LLM responds with no tool uses
        // OR max_turns is reached.
        while total_turns < options.max_turns {
            total_turns += 1;
            debug!(name = EV_RUNTIME_TURN_STARTED, turn = total_turns);

            let mut request = CompletionRequest::new(agent_def.llm_backend.as_str().into());
            request.system = agent_def.system_prompt.clone();
            request.messages = crate::run::agent_messages_to_provider_messages(&messages);
            request.tools = tool_specs.clone();
            debug!(
                name = EV_LLM_REQUEST_BUILT,
                messages = request.messages.len(),
                tools = request.tools.len(),
            );

            let llm_stream_result = async { backend.stream(request).await }
                .instrument(info_span!("llm.complete"))
                .await;
            let mut llm_stream = match llm_stream_result {
                Ok(s) => s,
                Err(llm_err) => {
                    warn!(name = "runtime.streaming_llm_open_failed");
                    yield make_llm_fatal_error(llm_err);
                    return;
                }
            };

            let mut accumulated_text = String::new();
            let mut turn_stop_reason: Option<StopReason> = None;
            let mut turn_usage: Option<TokenUsage> = None;
            let mut pending_tool_uses: Vec<tau_ports::ToolUse> = Vec::new();

            // Drain the LLM stream for this turn.
            // CompletionStream is Pin<Box<dyn Stream + Send>>; .as_mut() gives Pin<&mut S>.
            loop {
                let next = std::future::poll_fn(|cx| llm_stream.as_mut().poll_next(cx)).await;
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
                        warn!(name = "runtime.streaming_llm_chunk_err");
                        yield make_llm_fatal_error(llm_err);
                        return;
                    }
                    // CompletionChunk is #[non_exhaustive]; ignore unknown variants.
                    Some(Ok(_)) => {}
                }
            }

            debug!(
                name = EV_LLM_RESPONSE_RECEIVED,
                text_len = accumulated_text.len(),
                tool_uses = pending_tool_uses.len(),
                stop_reason = ?turn_stop_reason,
            );

            // Append assistant text to history if present.
            if !accumulated_text.is_empty() {
                let agent_addr = Address::Agent(agent_instance_id);
                messages.push(Message::new(
                    agent_addr,
                    Address::User,
                    MessagePayload::Text {
                        content: accumulated_text.clone(),
                    },
                ));
            }

            // Accumulate token usage.
            if let Some(usage) = turn_usage {
                aggregated_tokens.input_tokens = aggregated_tokens
                    .input_tokens
                    .saturating_add(u64::from(usage.input_tokens));
                aggregated_tokens.output_tokens = aggregated_tokens
                    .output_tokens
                    .saturating_add(u64::from(usage.output_tokens));
            }

            // No tool uses → end of run.
            if pending_tool_uses.is_empty() {
                debug!(name = EV_RUNTIME_LOOP_TERMINATED, reason = "end_turn");
                yield RunEvent::TurnCompleted {
                    stop_reason: turn_stop_reason.unwrap_or(StopReason::EndTurn),
                    usage: turn_usage,
                    turn: total_turns,
                };

                let final_message = messages
                    .last()
                    .cloned()
                    .expect("messages contains at least the initial user message");
                info!(
                    name = "runtime.run_completed",
                    total_turns,
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
            for tool_use in &pending_tool_uses {
                debug!(
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
                        // Capability check against agent's grant.
                        let required_cap = crate::orchestration::required_capability(
                            &tool_use.name,
                        );
                        let required_slice = std::slice::from_ref(&required_cap);
                        let missing = crate::capability::check_capabilities(
                            &granted_capabilities,
                            required_slice,
                        );
                        if let Some(cap) = missing {
                            let kind = crate::run::capability_kind_str(cap);
                            warn!(
                                name = EV_CAPABILITY_DENY,
                                tool_name = %tool_use.name,
                                missing_kind = %kind,
                            );
                            let denial = crate::error::CapabilityDenial {
                                agent_id: agent_def.id.to_string(),
                                package_id: agent_def.package.name.to_string(),
                                tool_name: tool_use.name.clone(),
                                required_kind: kind,
                                required_detail: format!("{cap:?}"),
                            };
                            let outcome = crate::run::build_policy_denied_outcome(
                                denial,
                                messages,
                                total_turns,
                                aggregated_tokens,
                            );
                            yield RunEvent::RunCompleted { outcome };
                            return;
                        }

                        // Append the tool-call message (parallels normal path).
                        let agent_addr = Address::Agent(agent_instance_id);
                        let tool_addr = Address::Tool(tool_use.name.clone());
                        messages.push(Message::new(
                            agent_addr.clone(),
                            tool_addr.clone(),
                            MessagePayload::ToolCall {
                                args: tool_use.input.clone(),
                            },
                        ));

                        // Build the ToolResult.
                        let is_agent_spawn = tool_use.name.starts_with("agent.")
                            && tool_use.name.ends_with(".spawn");
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
                        if is_skill_spawn {
                            // Resolve the tau-pkg Scope from cwd.
                            let scope_result = std::env::current_dir()
                                .ok()
                                .and_then(|cwd| tau_pkg::Scope::resolve(&cwd).ok());
                            let scope = match scope_result {
                                Some(s) => s,
                                None => {
                                    yield make_skill_spawn_error_tool_result(
                                        tool_use,
                                        "no scope available for skill resolution",
                                    );
                                    // Append error tool-result message so LLM
                                    // history is coherent.
                                    messages.push(Message::new(
                                        tool_addr.clone(),
                                        agent_addr.clone(),
                                        MessagePayload::ToolError {
                                            kind: "orchestration_virtual_tool_error"
                                                .into(),
                                            message: "skill spawn failed: no scope \
                                                      available for skill resolution"
                                                .into(),
                                            details: None,
                                        },
                                    ));
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
                                &scope,
                            ) {
                                Ok(r) => r,
                                Err(e) => {
                                    let err_msg = format!("{e}");
                                    yield make_skill_spawn_error_tool_result(
                                        tool_use,
                                        &err_msg,
                                    );
                                    messages.push(Message::new(
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
                                    ));
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
                                            let s = state_arc.lock().await;
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
                                            std::str::FromStr::from_str(&child_id_str)
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
                                            orchestration_state: Some(
                                                state_arc.clone(),
                                            ),
                                            orchestration_runtime: Some(
                                                child_runtime.clone(),
                                            ),
                                            granted_capabilities_override: Some(
                                                skill_req.grant.clone(),
                                            ),
                                            ..crate::RunOptions::default()
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
                                            let mut s = state_arc.lock().await;
                                            let run_id = s.run_id.clone();
                                            s.trace.emit(tau_ports::TraceEvent {
                                                id: ulid::Ulid::new().to_string(),
                                                ts: chrono::Utc::now(),
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
                                        // infinitely sized).
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
                            messages.push(Message::new(
                                tool_addr.clone(),
                                agent_addr.clone(),
                                skill_result_payload,
                            ));

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
                                                let s = state_arc.lock().await;
                                                s.record_agent_spawn();
                                            }

                                            // Build child agent id: parent id +
                                            // 8-char ulid suffix, lowercased.
                                            // Falls back to parent id on
                                            // construction failure (defensive;
                                            // shouldn't happen for compliant
                                            // AgentIds).
                                            let suffix = ulid::Ulid::new()
                                                .to_string()
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
                                            let child_id = std::str::FromStr::from_str(
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
                                                orchestration_state: Some(
                                                    state_arc.clone(),
                                                ),
                                                orchestration_runtime: Some(
                                                    child_runtime.clone(),
                                                ),
                                                granted_capabilities_override: Some(
                                                    req.grant.clone(),
                                                ),
                                                ..crate::RunOptions::default()
                                            };

                                            // Initial user message for the child.
                                            let child_msg = Message::new(
                                                Address::User,
                                                Address::Agent(AgentInstanceId::new()),
                                                MessagePayload::Text {
                                                    content: req.message,
                                                },
                                            );

                                            // Emit a Spawn trace event before
                                            // recursing, so the printer / log
                                            // can pick it up.
                                            {
                                                let mut s = state_arc.lock().await;
                                                let run_id = s.run_id.clone();
                                                s.trace.emit(
                                                    tau_ports::TraceEvent {
                                                        id: ulid::Ulid::new()
                                                            .to_string(),
                                                        ts: chrono::Utc::now(),
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
                                let mut state = state_arc.lock().await;
                                crate::orchestration::dispatch(
                                    &tool_use.name,
                                    args_json,
                                    &agent_id_str,
                                    &mut state,
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
                        messages.push(Message::new(
                            tool_addr,
                            agent_addr,
                            result_payload,
                        ));

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

                // ----- Capability check -------------------------------------
                let required: &[Capability] = tool.capabilities();
                let missing =
                    crate::capability::check_capabilities(&granted_capabilities, required);
                if let Some(cap) = missing {
                    let kind = crate::run::capability_kind_str(cap);
                    warn!(
                        name = EV_CAPABILITY_DENY,
                        tool_name = %tool_use.name,
                        missing_kind = %kind,
                    );
                    let denial = crate::error::CapabilityDenial {
                        agent_id: agent_def.id.to_string(),
                        package_id: agent_def.package.name.to_string(),
                        tool_name: tool_use.name.clone(),
                        required_kind: kind,
                        required_detail: format!("{cap:?}"),
                    };
                    let outcome = crate::run::build_policy_denied_outcome(
                        denial,
                        messages,
                        total_turns,
                        aggregated_tokens,
                    );
                    yield RunEvent::RunCompleted { outcome };
                    return;
                }

                // ----- Append the tool-call message -------------------------
                let agent_addr = Address::Agent(agent_instance_id);
                let tool_addr = Address::Tool(tool_use.name.clone());
                messages.push(Message::new(
                    agent_addr.clone(),
                    tool_addr.clone(),
                    MessagePayload::ToolCall {
                        args: tool_use.input.clone(),
                    },
                ));

                // ----- Open a session ---------------------------------------
                let ctx = SessionContext::new(agent_instance_id, uuid::Uuid::new_v4(), None)
                    .with_granted_capabilities(granted_for_session.clone())
                    .with_deny_entries(deny_entries.clone());
                if let Err(err) = tool.init(ctx.clone()).await {
                    warn!(
                        name = EV_TOOL_SESSION_OPEN_FAILED,
                        tool_name = %tool_use.name,
                    );
                    yield make_tool_fatal_error(err);
                    return;
                }

                // ----- Schema validation ------------------------------------
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
                        let _ = tool.teardown(()).await; // best-effort
                        warn!(
                            name = "tool.args_validation_failed",
                            tool_name = %tool_use.name,
                        );
                        messages.push(Message::new(
                            tool_addr.clone(),
                            agent_addr.clone(),
                            MessagePayload::ToolError {
                                kind: "tool_args_validation".into(),
                                message: reason.clone(),
                                details: None,
                            },
                        ));
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
                        let _ = tool.teardown(()).await;
                        yield make_tool_fatal_error(other);
                        return;
                    }
                    Ok(_) => {} // proceed to invoke
                }

                // ----- Invoke -----------------------------------------------
                let invoke_outcome = tool.invoke(&ctx, &mut (), tool_use.input.clone()).await;
                let tool_result: ToolResult = match invoke_outcome {
                    Ok(r) => r,
                    Err(err) => {
                        warn!(
                            name = EV_TOOL_INVOKE_FAILED,
                            tool_name = %tool_use.name,
                        );
                        let _ = tool.teardown(()).await; // best-effort
                        yield make_tool_fatal_error(err);
                        return;
                    }
                };

                // ----- Close the session ------------------------------------
                if let Err(err) = tool.teardown(()).await {
                    warn!(
                        name = EV_TOOL_SESSION_CLOSE_FAILED,
                        tool_name = %tool_use.name,
                    );
                    yield make_tool_fatal_error(err);
                    return;
                }

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
                messages.push(Message::new(
                    tool_addr,
                    agent_addr,
                    result_payload,
                ));

                yield RunEvent::ToolCallCompleted {
                    id: tool_use.id.clone(),
                    name: tool_use.name.clone(),
                    result: Ok(tool_result),
                };
            }
            // End of per-tool dispatch for this turn.

            yield RunEvent::TurnCompleted {
                stop_reason: turn_stop_reason.unwrap_or(StopReason::ToolUse),
                usage: turn_usage,
                turn: total_turns,
            };

            // Loop back for the next turn (LLM will see tool results).
        }

        // ----- max_turns reached -------------------------------------------
        warn!(
            name = "runtime.streaming_max_turns_reached",
            max_turns = options.max_turns
        );
        yield make_max_turns_outcome(messages, total_turns, aggregated_tokens, options.max_turns);
    }
}

// ---------------------------------------------------------------------------
// Outcome helpers
// ---------------------------------------------------------------------------

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

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use tau_ports::fixtures::{make_token_usage, make_tool_result};

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
            RunOptions::default(),
            HashMap::new(),
            HashMap::new(),
            vec![],
            vec![],
            vec![],
            vec![],
        );
        let events = collect_events(Box::pin(stream)).await;

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
            RunOptions::default(),
            HashMap::new(),
            HashMap::new(),
            vec![],
            vec![],
            vec![],
            vec![],
        );
        let events = collect_events(Box::pin(stream)).await;

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
            RunOptions::default(),
            HashMap::new(),
            HashMap::new(),
            vec![],
            vec![],
            vec![],
            vec![],
        );
        let events = collect_events(Box::pin(stream)).await;

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
            RunOptions::default(),
            tools,
            validators,
            vec![],
            tool_specs_list,
            vec![],
            vec![],
        );
        let events = collect_events(Box::pin(stream)).await;

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
            RunOptions::default(),
            tools,
            validators,
            vec![], // no granted capabilities → denial
            tool_specs_list,
            vec![],
            vec![],
        );
        let events = collect_events(Box::pin(stream)).await;

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
            RunOptions::default(),
            tools,
            validators,
            vec![],
            tool_specs_list,
            vec![],
            vec![],
        );
        let events = collect_events(Box::pin(stream)).await;

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
            RunOptions::default(),
            tools,
            validators,
            vec![],
            tool_specs_list,
            vec![],
            vec![],
        );
        let events = collect_events(Box::pin(stream)).await;

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
            RunOptions::default(),
            tools,
            validators,
            vec![],
            tool_specs_list,
            vec![],
            vec![],
        );
        let events = collect_events(Box::pin(stream)).await;

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
        let options = RunOptions {
            max_turns: 1,
            ..RunOptions::default()
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
        let events = collect_events(Box::pin(stream)).await;

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
            RunOptions::default(),
            tools,
            validators,
            vec![],
            tool_specs_list,
            vec![],
            vec![],
        );
        let events = collect_events(Box::pin(stream)).await;

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
            RunOptions::default(),
            HashMap::new(),
            HashMap::new(),
            vec![],
            vec![],
            vec![],
            vec![],
        );
        let events = collect_events(Box::pin(stream)).await;

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
}
