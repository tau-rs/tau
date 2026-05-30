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

#[cfg(test)]
use std::collections::BTreeMap;

#[cfg(test)]
use tau_domain::AgentInstanceId;
use tau_domain::{AgentDefinition, Capability, Message, PackageManifest};
#[cfg(test)]
use tau_domain::{AgentStatus, FailureKind, MessagePayload, Value};
#[cfg(test)]
use tau_ports::{ContentBlock, LlmProviderMessage, ToolUse};
use tracing::instrument;

use crate::builder::Runtime;
use crate::capability_override::EffectiveCapability;
#[cfg(test)]
use crate::error::CapabilityDenial;
use crate::error::{CoreRuntimeError, RuntimeError};
use crate::options::RunOptions;
#[cfg(test)]
use crate::options::TokenUsage;
use crate::outcome::RunOutcome;

// Re-export pure helpers from core so that stream.rs can still use
// `crate::run::*` paths unchanged (Task 3.5 migration shim).
pub(crate) use tau_runtime_core::run::{
    agent_messages_to_provider_messages, build_policy_denied_outcome, content_to_value,
    flatten_content_to_string,
};
// value_to_preview_string is pub in core but not used from tau-runtime directly.
#[allow(unused_imports)]
pub(crate) use tau_runtime_core::run::value_to_preview_string;

/// Build a `RunOptions` with clock and random filled in.
///
/// Phase β.1.4 will replace `MockClock` with a real `TokioClock`.
/// Until then, every production entry point that constructs `RunOptions`
/// internally (i.e. does not receive one from the caller) MUST go through
/// this helper so that `clock_ref` / `random_ref` in `stream.rs` don't panic.
///
/// Tests that need deterministic time or deterministic entropy should
/// supply their own `MockClock` / `DeterministicRandom` via `RunOptions`
/// fields directly.
fn run_options_with_defaults() -> RunOptions {
    use std::sync::Arc;
    use tau_ports::{DeterministicRandom, MockClock};
    let mut opts = RunOptions::default();
    // TODO(beta.1.4): replace MockClock with TokioClock once the tokio
    // shell's `drive` entry exists and can inject a real wall-clock.
    opts.clock = Some(Arc::new(MockClock::new()));
    opts.random = Some(Arc::new(DeterministicRandom::seeded(0)));
    opts
}

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
                            RuntimeError::Core(CoreRuntimeError::ToolNotRegistered {
                                tool_name,
                                registered,
                            })
                        }
                        "Llm" => RuntimeError::Core(CoreRuntimeError::Llm(LlmError::Internal {
                            message: detail,
                        })),
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
                            RuntimeError::Core(CoreRuntimeError::Tool(tool_err))
                        }
                        _ => RuntimeError::Core(CoreRuntimeError::Internal { message: detail }),
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

    /// Convenience: [`Runtime::run`] with default options (clock + random injected).
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
            run_options_with_defaults(),
        )
        .await
    }

    /// Invoke a single tool by name without engaging the LLM loop.
    ///
    /// Backwards-compatible wrapper over the core [`tau_runtime_core::Runtime::invoke_tool`]
    /// that forwards `None` for `clock` and `random` (uses test-fixture defaults).
    /// Callers that need deterministic entropy or wall-clock control should
    /// use the core method directly and supply port implementations.
    ///
    /// # Errors
    ///
    /// See [`tau_runtime_core::Runtime::invoke_tool`].
    pub async fn invoke_tool(
        &self,
        agent_def: &AgentDefinition,
        package_manifest: &PackageManifest,
        tool_name: &str,
        args: tau_domain::Value,
    ) -> Result<tau_ports::ToolResult, RuntimeError> {
        // Delegate to core impl; convert core RuntimeError → host RuntimeError
        // via the From impl.
        self.0
            .invoke_tool(agent_def, package_manifest, tool_name, args, None, None)
            .await
            .map_err(RuntimeError::Core)
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

        // Subscribe a JSONL writer via the tokio mpsc subscriber.
        let log_path = crate::orchestration::persistence::run_log_path(&scope_root, &run_id);
        let subscriber = crate::orchestration::trace_mpsc::channel_with_writer(log_path);
        state.trace.add_subscriber(subscriber);

        let state_arc = Arc::new(Mutex::new(state));

        let opts = {
            let mut o = run_options_with_defaults();
            o.orchestration_state = Some(state_arc.clone());
            o.orchestration_runtime = Some(self.clone());
            o
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
// Internal helpers (named per the plan; some are re-exported from core)
// ---------------------------------------------------------------------------

// agent_messages_to_provider_messages, build_policy_denied_outcome,
// flatten_content_to_string, content_to_value, value_to_preview_string
// are re-exported at top of file from tau_runtime_core::run.

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

// ---------------------------------------------------------------------------
// Small format/utility helpers (test-only)
// ---------------------------------------------------------------------------

/// Append the assistant's response (text + tool_uses) to the history.
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
        history.push(Message::new(
            tau_domain::Address::Agent(*agent_id),
            tau_domain::Address::User,
            MessagePayload::Text {
                content: String::new(),
            },
        ));
    }
}

/// Truncate `s` to at most `n` characters (Unicode scalar values).
#[cfg(test)]
fn truncate_to_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// ---------------------------------------------------------------------------
// Unit tests
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
        let denial = CapabilityDenial::new(
            "agent-x",
            "pkg-y",
            "file_read",
            "fs.read",
            "Filesystem(Read { paths: [\"/etc/passwd\"] })",
        );
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
        let s = "éééééé"; // 6 chars, 12 bytes
        assert_eq!(truncate_to_chars(s, 3), "ééé");
        assert_eq!(truncate_to_chars(s, 100), "éééééé");
        assert_eq!(truncate_to_chars(s, 0), "");
    }

    #[test]
    fn capability_kind_str_for_filesystem_read() {
        use crate::capability::capability_kind_str;
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
        use crate::capability::capability_kind_str;
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
        use crate::capability::capability_kind_str;
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
        use crate::capability::capability_kind_str;
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
        let unchecked: tau_domain::UncheckedManifest = toml::from_str(toml_str).unwrap();
        let manifest = unchecked.validate().unwrap();

        let result = runtime
            .invoke_tool(&agent_def, &manifest, "echo", Value::Null)
            .await
            .expect("invoke_tool must succeed");

        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            tau_ports::ToolContent::Text { text } => assert_eq!(text, "pong"),
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
        let unchecked: tau_domain::UncheckedManifest = toml::from_str(toml_str).unwrap();
        let manifest = unchecked.validate().unwrap();

        let err = runtime
            .invoke_tool(&agent_def, &manifest, "no-such-tool", Value::Null)
            .await
            .expect_err("should return ToolNotRegistered");

        assert!(
            matches!(
                err,
                RuntimeError::Core(CoreRuntimeError::ToolNotRegistered { .. })
            ),
            "expected ToolNotRegistered, got {err:?}"
        );
    }
}
