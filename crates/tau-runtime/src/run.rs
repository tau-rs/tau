//! Agent multi-turn run loop. The kernel surface — see spec §3.7.
//!
//! [`Runtime::run`] receives an initial [`Message`], drives the LLM
//! backend and tool plugins through a turn loop bounded by
//! [`RunOptions::max_turns`], applies capability checks before each
//! tool call, and returns a [`RunOutcome`].
//!
//! # Error vs failure dichotomy (ADR-0006)
//!
//! - Plugin/dispatch failures (LLM error, tool error, missing backend)
//!   bubble up as `Err(RuntimeError)`. The agent terminates abnormally.
//! - Agent-level failures — capability denied, max turns reached —
//!   are reported as `Ok(RunOutcome::Failed { status, .. })` with
//!   `status = AgentStatus::Failed { kind, .. }`. The conversation
//!   history is preserved for inspection.
//!
//! # Tracing
//!
//! Per spec §3.9 the run loop emits a fixed vocabulary of events
//! (`runtime.run_started`, `runtime.turn_started`, `llm.request_built`,
//! …) under named spans (`runtime.agent_run`, `runtime.turn`,
//! `llm.complete`, `dispatch.tool`, `capability.check`,
//! `tool.session_open`, `tool.invoke`, `tool.session_close`).
//! Sensitive-data discipline: arguments and message content never
//! travel above DEBUG; full content is TRACE-only and otherwise
//! truncated to a 256-char preview.

use std::collections::BTreeMap;

#[cfg(test)]
use tau_domain::AgentInstanceId;
use tau_domain::{
    AgentDefinition, AgentStatus, Capability, FailureKind, Message, MessagePayload,
    PackageManifest, Value,
};
use tau_ports::{ContentBlock, LlmProviderMessage, ToolContent, ToolUse};
use tracing::instrument;

use crate::builder::Runtime;
use crate::capability_override::EffectiveCapability;
use crate::error::{CapabilityDenial, RuntimeError};
use crate::options::{RunOptions, TokenUsage};
use crate::outcome::RunOutcome;

impl Runtime {
    /// Run an agent with a pre-existing conversation history.
    ///
    /// Identical to [`Runtime::run`] except `history` is pre-loaded into
    /// the messages buffer for turn 1: the kernel projects `history`
    /// (plus the new `initial_message`) onto the
    /// [`tau_ports::CompletionRequest`]'s `messages` list before the first LLM call.
    /// Subsequent turns evolve the buffer normally — assistant text,
    /// tool calls, tool results — exactly as in
    /// [`Runtime::run`].
    ///
    /// This is the entry point used by `tau chat` (sub-project 5) to
    /// thread REPL conversation history across user turns. Per NG6
    /// ("no persistent agent memory in core"), the kernel itself does
    /// not retain history between calls — the CLI accumulates a
    /// `Vec<Message>` and passes it back in on each turn.
    ///
    /// The loop is otherwise identical to [`Runtime::run`]:
    ///
    /// 1. Build a [`tau_ports::CompletionRequest`] from `history + initial_message`
    ///    on turn 1; from the evolving buffer on later turns.
    /// 2. Call the LLM backend.
    /// 3. Append the assistant text to the history.
    /// 4. If no tool_uses were emitted, return
    ///    [`RunOutcome::Completed`].
    /// 5. Otherwise, for each tool_use: capability-check, dispatch
    ///    through the registered tool plugin, append the result to the
    ///    history, and resume from step 1.
    /// 6. After [`RunOptions::max_turns`] iterations without
    ///    completion, return [`RunOutcome::Failed`] with
    ///    [`FailureKind::OutOfResources`].
    ///
    /// `Ok(RunOutcome::Failed)` is returned in two cases: capability
    /// denial ([`FailureKind::PolicyDenied`]) and max turns reached
    /// ([`FailureKind::OutOfResources`]). Plugin and dispatch errors
    /// bubble up as `Err(RuntimeError)` per ADR-0006.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // RunOutcome and AgentDefinition are #[non_exhaustive]; doctests
    /// // can't construct them via struct-literal syntax. Example illustrative.
    /// use tau_runtime::{Runtime, RunOptions};
    ///
    /// let outcome = runtime
    ///     .run_with_history(agent_def, manifest, history, initial_message, RunOptions::default())
    ///     .await?;
    /// ```
    #[instrument(
        name = "runtime.agent_run",
        skip_all,
        fields(
            agent_id = %agent_def.id,
            display_name = %agent_def.display_name,
            package_id = %agent_def.package.name,
            llm_backend_name = %agent_def.llm_backend,
            max_turns = options.max_turns,
            history_len = history.len(),
        ),
    )]
    pub async fn run_with_history(
        &self,
        agent_def: AgentDefinition,
        package_manifest: PackageManifest,
        history: Vec<Message>,
        initial_message: Message,
        options: RunOptions,
    ) -> Result<RunOutcome, RuntimeError> {
        use crate::stream::RunEvent;
        use futures_core::Stream as _;
        use tau_ports::{LlmError, ToolError};

        // Delegate all agent-loop logic to run_streaming_with_history.
        // This function is now a thin stream-drainer: it consumes the
        // stream and returns the terminal RunCompleted.outcome.
        // The streaming pump (stream.rs::run_streaming_inner) is the
        // single source of truth for the agent loop.
        let stream = self
            .run_streaming_with_history(
                agent_def,
                package_manifest,
                history,
                initial_message,
                options,
            )
            .await?;
        let mut stream = Box::pin(stream);
        loop {
            let next = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await;
            match next {
                Some(RunEvent::RunCompleted { outcome }) => return Ok(outcome),
                Some(RunEvent::FatalError {
                    kind,
                    detail,
                    context_json,
                    tool_error_variant,
                }) => {
                    // ADR-0006 error/failure dichotomy: the streaming pump
                    // emits FatalError for plugin/dispatch failures that
                    // must propagate as Err(RuntimeError) in the batch path.
                    return Err(match kind.as_str() {
                        "ToolNotRegistered" => {
                            // Parse context_json to extract tool_name and
                            // registered list (emitted by make_tool_not_registered_error).
                            let (tool_name, registered) = context_json
                                .as_deref()
                                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                                .and_then(|v| {
                                    let tn = v["tool_name"].as_str()?.to_owned();
                                    let reg: Vec<String> = v["registered"]
                                        .as_array()?
                                        .iter()
                                        .filter_map(|x| x.as_str().map(String::from))
                                        .collect();
                                    Some((tn, reg))
                                })
                                .unwrap_or_else(|| (detail.clone(), vec![]));
                            RuntimeError::ToolNotRegistered {
                                tool_name,
                                registered,
                            }
                        }
                        "Llm" => RuntimeError::Llm(LlmError::Internal { message: detail }),
                        // Reconstruct the typed ToolError variant using
                        // `tool_error_variant` recorded by make_tool_fatal_error.
                        // This preserves the BadArgs/SessionDead/etc. variant
                        // through the FatalError round-trip (Approach A fix).
                        "Tool" => {
                            let tool_err = match tool_error_variant.as_deref() {
                                Some("BadArgs") => ToolError::BadArgs { reason: detail },
                                Some("SessionDead") => ToolError::SessionDead { reason: detail },
                                Some("DeadlineExceeded") => ToolError::DeadlineExceeded,
                                Some("CapabilityDenied") => {
                                    ToolError::CapabilityDenied { capability: detail }
                                }
                                // Llm/Storage/Internal and unknown future
                                // variants all map to Internal — the detail
                                // string carries the Display output.
                                _ => ToolError::Internal { message: detail },
                            };
                            RuntimeError::Tool(tool_err)
                        }
                        _ => RuntimeError::Internal { message: detail },
                    });
                }
                Some(_) => continue,
                None => unreachable!(
                    "run_streaming_inner must yield exactly one RunCompleted before stream end"
                ),
            }
        }
    }

    /// Run an agent through one solo-path multi-turn iteration with no
    /// prior conversation history.
    ///
    /// Thin wrapper around [`Runtime::run_with_history`] with
    /// `history = vec![]`. See `run_with_history` for the full loop
    /// semantics, error/failure dichotomy, and tracing vocabulary.
    /// Existing callers that don't need history threading should
    /// continue to use this entry point.
    pub async fn run(
        &self,
        agent_def: AgentDefinition,
        package_manifest: PackageManifest,
        initial_message: Message,
        options: RunOptions,
    ) -> Result<RunOutcome, RuntimeError> {
        self.run_with_history(
            agent_def,
            package_manifest,
            Vec::new(),
            initial_message,
            options,
        )
        .await
    }

    /// Convenience: [`Runtime::run`] with [`RunOptions::default`].
    pub async fn run_default(
        &self,
        agent_def: AgentDefinition,
        package_manifest: PackageManifest,
        initial_message: Message,
    ) -> Result<RunOutcome, RuntimeError> {
        self.run(
            agent_def,
            package_manifest,
            initial_message,
            RunOptions::default(),
        )
        .await
    }

    /// Invoke a single tool by name without engaging the LLM loop.
    ///
    /// Bypasses the multi-turn agent driver — useful for callers that
    /// want to compose tools directly (e.g., `tau-workflow`'s
    /// `tool.call` step kind). The tool's capability requirements are
    /// still checked against the `agent_def`'s package grant set, so
    /// the caller must pass the workflow's default-agent definition.
    ///
    /// Follows the same sequence as the run loop's tool-dispatch arm:
    /// `resolve_tool → capability check → init → invoke → teardown`.
    ///
    /// # Errors
    ///
    /// - [`RuntimeError::ToolNotRegistered`] — the tool name is unknown.
    /// - [`RuntimeError::Internal`] — the agent's package does not grant
    ///   a capability required by the tool (capability-denied path; the
    ///   run loop surfaces this as `Ok(RunOutcome::Failed)` instead, but
    ///   the direct-dispatch caller has no `RunOutcome` envelope).
    /// - [`RuntimeError::Tool`] — the tool's `init`, `invoke`, or
    ///   `teardown` returned a [`tau_ports::ToolError`].
    pub async fn invoke_tool(
        &self,
        agent_def: &AgentDefinition,
        package_manifest: &PackageManifest,
        tool_name: &str,
        args: tau_domain::Value,
    ) -> Result<tau_ports::ToolResult, RuntimeError> {
        use tau_domain::AgentInstanceId;
        use tau_ports::SessionContext;

        let tool = self.resolve_tool(tool_name)?.clone();

        // Capability check: mirror the run loop's structural check.
        // If the agent's package grants are insufficient, surface as
        // RuntimeError::Internal (no CapabilityDenied variant exists on
        // RuntimeError; the run loop returns Ok(RunOutcome::Failed) in
        // the same situation, but invoke_tool has no RunOutcome envelope).
        let granted: Vec<tau_domain::Capability> = package_manifest.capabilities().to_vec();
        let required: &[tau_domain::Capability] = tool.capabilities();
        if let Some(missing) = crate::capability::check_capabilities(&granted, required) {
            let denial = crate::error::CapabilityDenial {
                agent_id: agent_def.id.to_string(),
                package_id: agent_def.package.name.to_string(),
                tool_name: tool_name.to_owned(),
                required_kind: crate::run::capability_kind_str(missing),
                required_detail: format!("{missing:?}"),
            };
            return Err(RuntimeError::Internal {
                message: format!("capability denied: {denial}"),
            });
        }

        // Build a minimal SessionContext (no deadline, no deny entries).
        // invoke_tool is a direct-dispatch path — callers that need
        // capability narrowing or deny carve-outs should use run().
        let ctx = SessionContext::new(AgentInstanceId::new(), uuid::Uuid::new_v4(), None)
            .with_granted_capabilities(granted);

        tool.init(ctx.clone()).await?;
        let result = tool.invoke(&ctx, &mut (), args).await;
        // teardown best-effort: don't mask invoke's error if both fail.
        let _ = tool.teardown(()).await;
        Ok(result?)
    }

    /// Multi-agent orchestrated run entry point (ROADMAP §9, v1).
    ///
    /// Builds a shared [`crate::orchestration::run_state::RunState`] (TaskList,
    /// trace stream, budget counters), spawns a JSONL persister subscribed to
    /// the trace stream, then runs the root agent via [`Runtime::run_with_history`]
    /// with `orchestration_state` set in `RunOptions`. Virtual tool calls
    /// (`task.*` / `run.*`) inside the agent loop are intercepted at the
    /// kernel-dispatch boundary (see `crate::stream::run_streaming_inner`).
    ///
    /// On completion: marks the run terminal status (Completed iff agent
    /// finished AND no orphan tasks), emits a final
    /// `TraceEventKind::OrphanedTasksAtTermination` if orphans remain, and
    /// returns a read-only [`tau_ports::RunSnapshot`].
    ///
    /// As of v1.1 (ADR-0025), `agent.<kind>.spawn` recursively invokes
    /// [`Runtime::run_with_history`] for the child via the `Arc<Self>`
    /// recursion handle threaded through `RunOptions::orchestration_runtime`.
    /// The child run inherits the parent's `PackageManifest`, gets a
    /// fresh `AgentId`, and runs with the validated narrowed grant from
    /// [`crate::orchestration::AgentSpawnRequest`].
    pub async fn spawn_root_agent(
        self: std::sync::Arc<Self>,
        root_agent_def: tau_domain::AgentDefinition,
        root_manifest: tau_domain::PackageManifest,
        initial_message: tau_domain::Message,
        budget: tau_ports::RunBudget,
        scope_root: std::path::PathBuf,
    ) -> Result<tau_ports::RunSnapshot, RuntimeError> {
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let run_id = ulid::Ulid::new().to_string();
        let root_agent_id = root_agent_def.id.to_string();
        let now = chrono::Utc::now();

        let mut state = crate::orchestration::run_state::RunState::new(
            run_id.clone(),
            root_agent_id,
            budget,
            now,
        );

        // Subscribe a JSONL writer before wrapping state in Arc<Mutex<>>.
        let log_path = crate::orchestration::persistence::run_log_path(&scope_root, &run_id);
        let writer_rx = state.trace.subscribe();
        let _writer_handle = crate::orchestration::persistence::spawn_writer(log_path, writer_rx);

        let state_arc = Arc::new(Mutex::new(state));

        let opts = crate::RunOptions {
            orchestration_state: Some(state_arc.clone()),
            orchestration_runtime: Some(self.clone()),
            ..crate::RunOptions::default()
        };

        let outcome = self
            .run_with_history(
                root_agent_def,
                root_manifest,
                Vec::new(),
                initial_message,
                opts,
            )
            .await?;

        let now_end = chrono::Utc::now();
        {
            let mut s = state_arc.lock().await;
            s.ended_at = Some(now_end);
            let success = matches!(outcome, crate::RunOutcome::Completed { .. });
            let orphans_present = !s.task_list.all_terminal();
            s.status = if success && !orphans_present {
                tau_ports::RunStatus::Completed
            } else {
                tau_ports::RunStatus::Failed
            };
            if orphans_present {
                let orphan_ids: Vec<_> = s
                    .task_list
                    .all()
                    .into_iter()
                    .filter(|t| {
                        !matches!(
                            t.status,
                            tau_ports::TaskStatus::Done
                                | tau_ports::TaskStatus::Failed
                                | tau_ports::TaskStatus::Discarded
                        )
                    })
                    .map(|t| t.id)
                    .collect();
                s.trace.emit(tau_ports::TraceEvent {
                    id: ulid::Ulid::new().to_string(),
                    ts: now_end,
                    run_id: run_id.clone(),
                    agent_id: None,
                    kind: tau_ports::TraceEventKind::OrphanedTasksAtTermination {
                        task_ids: orphan_ids,
                    },
                });
            }
        }

        let snapshot = {
            let s = state_arc.lock().await;
            s.snapshot(now_end)
        };
        Ok(snapshot)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers (named per the plan; each has a unit test below)
// ---------------------------------------------------------------------------

/// Build the `RunOutcome::Failed { kind: PolicyDenied, .. }` returned
/// when [`check_capabilities`] rejects a tool invocation. Centralizes
/// the construction so the run loop's denial branch reads cleanly.
pub(crate) fn build_policy_denied_outcome(
    denial: CapabilityDenial,
    all_messages: Vec<Message>,
    total_turns: u32,
    token_usage: TokenUsage,
) -> RunOutcome {
    RunOutcome::Failed {
        status: AgentStatus::failed(FailureKind::PolicyDenied, Some(format!("{denial}"))),
        all_messages,
        total_turns,
        token_usage,
    }
}

// ---------------------------------------------------------------------------
// Translation helpers (also internal)
// ---------------------------------------------------------------------------

/// Project the agent's [`Message`] history onto the LLM-call shape.
///
/// Per `tau_ports::llm` module-level docs, `tau_domain::Message`
/// (universal envelope) and [`LlmProviderMessage`] (provider call
/// shape) are intentionally distinct. This function is the single
/// projection point in the kernel.
///
/// v0.1 mapping:
///
/// | Sender / payload                 | Provider role | Block(s)                          |
/// | -------------------------------- | ------------- | --------------------------------- |
/// | `User` / `Text`                  | `User`        | `Text`                            |
/// | `Agent` / `Text`                 | `Assistant`   | `Text`                            |
/// | `Agent` / `ToolCall`             | `Assistant`   | `ToolUse` (id derived from name)  |
/// | `Tool` / `ToolResult` or Error   | `ToolResult`  | `Text` (flattened body)           |
/// | other                            | (skipped)     | —                                 |
///
/// Lifecycle/Custom payloads are skipped — they are not part of the
/// agent↔LLM dialogue at v0.1.
pub(crate) fn agent_messages_to_provider_messages(history: &[Message]) -> Vec<LlmProviderMessage> {
    let mut out = Vec::with_capacity(history.len());
    for m in history {
        match (&m.sender, &m.payload) {
            (tau_domain::Address::User, MessagePayload::Text { content }) => {
                out.push(LlmProviderMessage::user(vec![ContentBlock::Text(
                    content.clone(),
                )]));
            }
            (tau_domain::Address::Agent(_), MessagePayload::Text { content }) => {
                out.push(LlmProviderMessage::assistant(vec![ContentBlock::Text(
                    content.clone(),
                )]));
            }
            (tau_domain::Address::Agent(_), MessagePayload::ToolCall { args }) => {
                // `tool_use.id` round-trips into the provider's
                // `tool_use_id`; v0.1 derives it from the message id so
                // a follow-up `ToolResult` can be paired by the backend.
                let tool_name = match &m.recipient {
                    tau_domain::Address::Tool(name) => name.clone(),
                    _ => String::new(),
                };
                out.push(LlmProviderMessage::assistant(vec![ContentBlock::ToolUse(
                    ToolUse::new(format!("toolu_{}", m.id), tool_name, args.clone()),
                )]));
            }
            (tau_domain::Address::Tool(_), MessagePayload::ToolResult { body }) => {
                out.push(LlmProviderMessage::tool_result(
                    format!("toolu_{}", m.id),
                    vec![ContentBlock::Text(value_to_preview_string(body))],
                    false,
                ));
            }
            (
                tau_domain::Address::Tool(_),
                MessagePayload::ToolError {
                    kind: _,
                    message,
                    details: _,
                },
            ) => {
                out.push(LlmProviderMessage::tool_result(
                    format!("toolu_{}", m.id),
                    vec![ContentBlock::Text(message.clone())],
                    true,
                ));
            }
            // Lifecycle / Custom / cross-talk patterns: not part of the
            // v0.1 agent↔LLM dialogue. Skip silently — the integration
            // tests assert the projected shape, and additive semantics
            // are non-breaking.
            _ => {}
        }
    }
    out
}

/// Append the assistant's response (text + tool_uses) to the history.
///
/// Mints one `Message` per logical assistant action:
///
/// - One [`MessagePayload::Text`] message (only if `text` is non-empty).
/// - The tool_uses themselves are NOT pushed here — they become
///   `MessagePayload::ToolCall` messages later in the dispatch loop,
///   one per `tool_use`. This keeps the history's cause-effect order
///   consistent: text before any tool call, then the tool call paired
///   immediately with its result.
///
/// Now only called from unit tests (the agent loop moved to stream.rs).
#[cfg(test)]
pub(crate) fn append_assistant_response(
    history: &mut Vec<Message>,
    text: &str,
    tool_uses: &[ToolUse],
    agent_id: &AgentInstanceId,
) {
    if !text.is_empty() {
        history.push(Message::new(
            tau_domain::Address::Agent(*agent_id),
            tau_domain::Address::User,
            MessagePayload::Text {
                content: text.to_owned(),
            },
        ));
    } else if tool_uses.is_empty() {
        // Defensive: the LLM returned neither text nor tool_uses.
        // Spec says this shouldn't happen, but we still need a
        // recognisable assistant turn in the history so callers'
        // `final_message` is the assistant's empty response (not the
        // initial user prompt). Push an empty-text message rather than
        // synthesising a richer placeholder.
        history.push(Message::new(
            tau_domain::Address::Agent(*agent_id),
            tau_domain::Address::User,
            MessagePayload::Text {
                content: String::new(),
            },
        ));
    }
}

// ---------------------------------------------------------------------------
// Small format/utility helpers (file-private)
// ---------------------------------------------------------------------------

/// Truncate `s` to at most `n` characters (Unicode scalar values),
/// preserving UTF-8 boundaries. Returns an owned `String`. Used for
/// DEBUG-level previews.
///
/// Now only called from unit tests (the agent loop moved to stream.rs).
#[cfg(test)]
fn truncate_to_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Flatten a tool's content blocks into a single human-readable string,
/// for use as the `message` field of [`MessagePayload::ToolError`].
pub(crate) fn flatten_content_to_string(blocks: &[ToolContent]) -> String {
    let mut out = String::new();
    for block in blocks {
        match block {
            ToolContent::Text { text } => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
            ToolContent::Json { data } => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&value_to_preview_string(data));
            }
            // `ToolContent` is `#[non_exhaustive]`; future variants
            // contribute nothing to the preview at v0.1.
            _ => {}
        }
    }
    out
}

/// Build a [`Value`] from a tool's content blocks. v0.1 rule:
///
/// - exactly one [`ToolContent::Json { data }`] → return `data` directly;
/// - everything else → wrap into an `Object { content: Array of strings/values }`.
pub(crate) fn content_to_value(blocks: &[ToolContent]) -> Value {
    if blocks.len() == 1 {
        if let ToolContent::Json { data } = &blocks[0] {
            return data.clone();
        }
    }
    let arr: Vec<Value> = blocks
        .iter()
        .map(|b| match b {
            ToolContent::Text { text } => Value::String(text.clone()),
            ToolContent::Json { data } => data.clone(),
            _ => Value::Null,
        })
        .collect();
    let mut obj = BTreeMap::new();
    obj.insert("content".to_string(), Value::Array(arr));
    Value::Object(obj)
}

/// Compact preview string for a [`Value`]. Used for `LlmProviderMessage`
/// projection of a tool result body (the LLM-call shape carries text,
/// not structured Value, in v0.1's content-block surface).
fn value_to_preview_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Bytes(b) => format!("<{} bytes>", b.len()),
        Value::Array(_) | Value::Object(_) => format!("{v:?}"),
        // `Value` is `#[non_exhaustive]`; future variants degrade to a
        // generic Debug-format string rather than mis-classifying.
        _ => format!("{v:?}"),
    }
}

/// Build the post-narrow `Capability` view that flows to plugins via
/// `SessionContext.granted_capabilities`. Capability inner variants are
/// `#[non_exhaustive]` and can't be constructed cross-crate; we serialize
/// the source, splice in the narrowed allow-list / max_bytes, and
/// deserialize back.
///
/// Failure-safe: any serialization failure falls back to `eff.source.clone()`.
/// The kernel's structural cap check still applies — narrowing is best-effort
/// at this layer; panicking on a security-enforcement path would be the
/// wrong failure mode.
pub(crate) fn narrowed_capability_for_session(eff: &EffectiveCapability) -> Capability {
    use serde_json::{json, Value as Jv};

    let source_json = match serde_json::to_value(&eff.source) {
        Ok(v) => v,
        Err(_) => return eff.source.clone(),
    };
    let mut obj = match source_json.as_object() {
        Some(m) => m.clone(),
        None => return eff.source.clone(),
    };
    if let Some(allow) = &eff.allow_override {
        // Replace the kind-appropriate field. For unknown kinds (e.g. Custom),
        // bail and return source unchanged — narrowing is unsupported.
        let field = match obj.get("kind").and_then(Jv::as_str) {
            Some("fs.read") | Some("fs.write") | Some("fs.exec") => "paths",
            Some("net.http") => "hosts",
            Some("process.spawn") => "commands",
            _ => return eff.source.clone(),
        };
        obj.insert(field.to_string(), json!(allow));
    }
    if let Some(mb) = eff.max_bytes_override {
        obj.insert("max_bytes".to_string(), json!(mb));
    }
    serde_json::from_value(Jv::Object(obj)).unwrap_or_else(|_| eff.source.clone())
}

/// Top-level capability kind string used in
/// [`CapabilityDenial::required_kind`] and the `capability.deny` event.
pub(crate) fn capability_kind_str(cap: &Capability) -> String {
    use tau_domain::{
        AgentCapability, FsCapability, NetCapability, ProcessCapability, SkillCapability,
    };
    match cap {
        Capability::Filesystem(FsCapability::Read { .. }) => "fs.read".into(),
        Capability::Filesystem(FsCapability::Write { .. }) => "fs.write".into(),
        Capability::Filesystem(FsCapability::Exec { .. }) => "fs.exec".into(),
        Capability::Network(NetCapability::Http { .. }) => "net.http".into(),
        Capability::Process(ProcessCapability::Spawn { .. }) => "process.spawn".into(),
        Capability::Agent(AgentCapability::Spawn { .. }) => "agent.spawn".into(),
        Capability::TaskList { .. } => "task_list".into(),
        Capability::Plan { .. } => "plan".into(),
        Capability::Skill(SkillCapability::Spawn { .. }) => "skill.spawn".into(),
        Capability::Custom { name, .. } => name.clone(),
        // `Capability` is `#[non_exhaustive]`; future variants degrade
        // to a generic tag — additive evolution must not silently
        // mis-classify them.
        _ => "unknown".into(),
    }
}

// ---------------------------------------------------------------------------
// Unit tests for the named helpers (per Task 10's plan).
//
// Integration tests for `Runtime::run` itself live in tests/ (Tasks
// 11-16); they exercise the entire run loop and tracing emission.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use tau_domain::{Address, AgentInstanceId, MessagePayload};

    fn user_text_message(content: &str) -> Message {
        Message::new(
            Address::User,
            Address::Agent(AgentInstanceId::new()),
            MessagePayload::Text {
                content: content.into(),
            },
        )
    }

    // -------------------- build_policy_denied_outcome --------------------

    #[test]
    fn build_policy_denied_outcome_carries_denial_in_status() {
        let denial = CapabilityDenial {
            agent_id: "agent-x".into(),
            package_id: "pkg-y".into(),
            tool_name: "file_read".into(),
            required_kind: "fs.read".into(),
            required_detail: "Filesystem(Read { paths: [\"/etc/passwd\"] })".into(),
        };
        let out = build_policy_denied_outcome(denial, vec![], 3, TokenUsage::default());

        let RunOutcome::Failed {
            status,
            total_turns,
            token_usage,
            all_messages,
        } = out.clone()
        else {
            panic!("expected Failed, got {out:?}");
        };
        let AgentStatus::Failed { kind, detail, .. } = status.clone() else {
            panic!("expected AgentStatus::Failed, got {status:?}")
        };
        assert_eq!(kind, FailureKind::PolicyDenied);
        let detail = detail.expect("detail must be set");
        assert!(detail.contains("agent-x"), "got: {detail}");
        assert!(detail.contains("file_read"), "got: {detail}");
        assert!(detail.contains("fs.read"), "got: {detail}");
        assert_eq!(total_turns, 3);
        assert_eq!(token_usage, TokenUsage::default());
        assert!(all_messages.is_empty());
    }

    // -------------------- helper unit tests (smoke) --------------------

    #[test]
    fn agent_messages_to_provider_messages_maps_user_text_to_user_role() {
        let history = vec![user_text_message("hi")];
        let provider = agent_messages_to_provider_messages(&history);
        assert_eq!(provider.len(), 1);
        match &provider[0] {
            LlmProviderMessage::User { content } => {
                assert_eq!(content.len(), 1);
                match &content[0] {
                    ContentBlock::Text(t) => assert_eq!(t, "hi"),
                    other => panic!("expected Text, got {other:?}"),
                }
            }
            other => panic!("expected User, got {other:?}"),
        }
    }

    #[test]
    fn agent_messages_to_provider_messages_skips_lifecycle() {
        let m = Message::new(
            Address::System,
            Address::User,
            MessagePayload::Lifecycle(AgentStatus::Ready),
        );
        let provider = agent_messages_to_provider_messages(&[m]);
        assert!(provider.is_empty());
    }

    #[test]
    fn append_assistant_response_appends_only_text_when_present() {
        let mut history: Vec<Message> = vec![];
        let agent_id = AgentInstanceId::new();
        append_assistant_response(&mut history, "out", &[], &agent_id);
        assert_eq!(history.len(), 1);
        match (&history[0].sender, &history[0].payload) {
            (Address::Agent(id), MessagePayload::Text { content }) => {
                assert_eq!(*id, agent_id);
                assert_eq!(content, "out");
            }
            other => panic!("expected Agent / Text, got {other:?}"),
        }
    }

    #[test]
    fn append_assistant_response_no_text_no_tool_uses_pushes_empty_assistant_message() {
        // Defensive: an empty completion still produces an assistant
        // turn so callers' `final_message` doesn't accidentally surface
        // the user's prompt as the response.
        let mut history: Vec<Message> = vec![];
        let agent_id = AgentInstanceId::new();
        append_assistant_response(&mut history, "", &[], &agent_id);
        assert_eq!(history.len(), 1);
        match (&history[0].sender, &history[0].payload) {
            (Address::Agent(id), MessagePayload::Text { content }) => {
                assert_eq!(*id, agent_id);
                assert!(content.is_empty());
            }
            other => panic!("expected Agent / empty Text, got {other:?}"),
        }
    }

    #[test]
    fn truncate_to_chars_respects_utf8_boundaries() {
        // 3 bytes per "é" character in UTF-8; naïve byte-slicing would
        // panic on a non-boundary cut.
        let s = "éééééé"; // 6 chars, 12 bytes
        assert_eq!(truncate_to_chars(s, 3), "ééé");
        assert_eq!(truncate_to_chars(s, 100), "éééééé");
        assert_eq!(truncate_to_chars(s, 0), "");
    }

    #[test]
    fn capability_kind_str_for_filesystem_read() {
        // Round-trip an `fs.read` capability through the manifest wire
        // form (variant-level `#[non_exhaustive]` blocks struct-literal
        // construction from outside `tau-domain`) and assert the
        // top-level kind projection matches the existing dot-namespaced
        // taxonomy used by `CapabilityDenial::required_kind` and the
        // `runtime.tool_filtered` event.
        #[derive(serde::Deserialize)]
        struct CapWrapper {
            cap: Capability,
        }
        let cap = toml::from_str::<CapWrapper>(
            r#"[cap]
kind = "fs.read"
paths = ["**"]
"#,
        )
        .expect("test fs.read capability TOML must parse")
        .cap;
        assert_eq!(capability_kind_str(&cap), "fs.read");
    }

    #[test]
    fn capability_kind_str_for_custom_variant() {
        // `Capability::Custom` is structural (no inner non_exhaustive
        // variant), so we can construct it directly from outside
        // tau-domain. The other top-level variants wrap variant-level
        // `#[non_exhaustive]` enums (`FsCapability::Read { .. }` etc.)
        // and aren't reachable here without TOML deserialization;
        // their `kind_str` projection is exercised by the integration
        // tests in Tasks 11-16, where `check_capabilities` provides
        // them naturally.
        let mut params = BTreeMap::new();
        params.insert("servers".into(), Value::Null);
        let cap = Capability::Custom {
            name: "mcp.tool.use".into(),
            params,
        };
        assert_eq!(capability_kind_str(&cap), "mcp.tool.use");
    }

    #[test]
    fn capability_kind_str_for_task_list() {
        // Round-trip a `task_list` capability through the manifest wire
        // form. Orchestration grants flow through `capability_kind_str`
        // in `CapabilityDenial::required_kind`; this test pins the
        // projection so future denial events tag the namespace
        // correctly rather than landing in the `"unknown"` fallback.
        #[derive(serde::Deserialize)]
        struct CapWrapper {
            cap: Capability,
        }
        let cap = toml::from_str::<CapWrapper>(
            r#"[cap]
kind = "task_list"
mode = "write"
"#,
        )
        .expect("test task_list capability TOML must parse")
        .cap;
        assert_eq!(capability_kind_str(&cap), "task_list");
    }

    #[test]
    fn capability_kind_str_for_plan() {
        #[derive(serde::Deserialize)]
        struct CapWrapper {
            cap: Capability,
        }
        let cap = toml::from_str::<CapWrapper>(
            r#"[cap]
kind = "plan"
mode = "read"
"#,
        )
        .expect("test plan capability TOML must parse")
        .cap;
        assert_eq!(capability_kind_str(&cap), "plan");
    }

    // -------------------- invoke_tool --------------------

    #[tokio::test]
    async fn invoke_tool_dispatches_to_registered_tool_and_returns_result() {
        use std::str::FromStr;
        use tau_domain::{AgentId, PackageId, PackageName, UncheckedManifest, Version};
        use tau_ports::fixtures::{make_tool_result, make_tool_spec, MockLlmBackend, MockTool};
        use tau_ports::ToolContent;

        let spec = make_tool_spec(
            "echo".to_string(),
            "echo tool".to_string(),
            Value::Object(Default::default()),
        );
        let canned_result = make_tool_result(
            vec![ToolContent::Text {
                text: "pong".to_string(),
            }],
            false,
        );
        let tool = MockTool::new("echo", spec).with_result(canned_result.clone());

        let runtime = crate::builder::Runtime::builder()
            .with_llm_backend(MockLlmBackend::new("gpt-4"))
            .with_tool(tool)
            .build()
            .expect("build runtime");

        let pkg = PackageId::new(
            PackageName::from_str("test-pkg").unwrap(),
            Version::parse("0.1.0").unwrap(),
        );
        let agent_def = AgentDefinition::new(
            AgentId::from_str("test-agent").unwrap(),
            "test".to_string(),
            pkg,
            PackageName::from_str("gpt-4").unwrap(),
        );

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
        let manifest = unchecked.validate().unwrap();

        let result = runtime
            .invoke_tool(&agent_def, &manifest, "echo", Value::Null)
            .await
            .expect("invoke_tool must succeed");

        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            ToolContent::Text { text } => assert_eq!(text, "pong"),
            other => panic!("expected Text content, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_tool_returns_err_for_unknown_tool() {
        use std::str::FromStr;
        use tau_domain::{AgentId, PackageId, PackageName, UncheckedManifest, Version};
        use tau_ports::fixtures::MockLlmBackend;

        let runtime = crate::builder::Runtime::builder()
            .with_llm_backend(MockLlmBackend::new("gpt-4"))
            .build()
            .expect("build runtime");

        let pkg = PackageId::new(
            PackageName::from_str("test-pkg").unwrap(),
            Version::parse("0.1.0").unwrap(),
        );
        let agent_def = AgentDefinition::new(
            AgentId::from_str("test-agent").unwrap(),
            "test".to_string(),
            pkg,
            PackageName::from_str("gpt-4").unwrap(),
        );
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
        let manifest = unchecked.validate().unwrap();

        let err = runtime
            .invoke_tool(&agent_def, &manifest, "no-such-tool", Value::Null)
            .await
            .expect_err("should return ToolNotRegistered");

        assert!(
            matches!(err, RuntimeError::ToolNotRegistered { .. }),
            "expected ToolNotRegistered, got {err:?}"
        );
    }
}
