//! Host-shell extension trait providing the public agent-run entry
//! points on top of [`tau_runtime_core::Runtime`].
//!
//! Pre-β.1.3.5b, methods like `run`, `run_with_history`, `run_streaming`,
//! `run_streaming_with_history`, `invoke_tool`, and `spawn_root_agent`
//! were inherent methods on a tau-runtime newtype wrapping
//! `tau_runtime_core::Runtime`. The newtype is gone — those methods now
//! live on this extension trait, implemented for the kernel `Runtime`
//! directly. Host shells that wish to provide their own ergonomic
//! overlay can implement a similar trait without re-defining the kernel
//! type.

use std::path::PathBuf;
use std::sync::Arc;

use futures_core::Stream;
use tau_domain::{AgentDefinition, Capability, Message, PackageManifest};
use tau_ports::{RunBudget, RunSnapshot, ToolResult, ToolSpec};
use tracing::{debug, info, instrument, warn};

use crate::builder::{DynTool, Runtime};
use crate::capability::check_capabilities;
use crate::error::{CoreRuntimeError, RuntimeError};
use crate::options::RunOptions;
use crate::outcome::RunOutcome;
use crate::stream::RunEvent;

/// Build a `RunOptions` with clock and random filled in.
///
/// Phase β.1.4 will replace `MockClock` with a real `TokioClock`.
/// Until then, every production entry point that constructs `RunOptions`
/// internally (i.e. does not receive one from the caller) MUST go through
/// this helper so that `clock_ref` / `random_ref` in `stream.rs` don't panic.
fn run_options_with_defaults() -> RunOptions {
    use tau_ports::{DeterministicRandom, MockClock};
    // TODO(beta.1.4): replace MockClock with TokioClock once the tokio
    // shell's `drive` entry exists and can inject a real wall-clock.
    let mut opts = RunOptions::default();
    opts.clock = Some(Arc::new(MockClock::new()));
    opts.random = Some(Arc::new(DeterministicRandom::seeded(0)));
    opts
}

/// Host-shell extension trait carrying the agent-run entry points.
///
/// Implemented for [`tau_runtime_core::Runtime`]. Callers that import
/// `tau_runtime::Runtime` (which re-exports the core type) get these
/// methods via `use tau_runtime::RuntimeShellExt;` or by enabling the
/// prelude.
///
/// Methods are intentionally non-`Send` async (the kernel's plugin
/// futures are non-`Send` by design — see `tau_runtime_core::builder`'s
/// `BoxFuture` for the rationale).
pub trait RuntimeShellExt {
    /// Stream an agent run from a single initial message.
    ///
    /// Convenience wrapper over [`RuntimeShellExt::run_streaming_with_history`]
    /// that passes an empty history.
    #[allow(async_fn_in_trait)]
    async fn run_streaming(
        &self,
        agent_def: AgentDefinition,
        package_manifest: PackageManifest,
        initial_message: Message,
        options: RunOptions,
    ) -> Result<impl Stream<Item = RunEvent> + 'static, RuntimeError>;

    /// Stream an agent run, prepending conversation history before the
    /// initial message.
    #[allow(async_fn_in_trait)]
    async fn run_streaming_with_history(
        &self,
        agent_def: AgentDefinition,
        package_manifest: PackageManifest,
        history: Vec<Message>,
        initial_message: Message,
        options: RunOptions,
    ) -> Result<impl Stream<Item = RunEvent> + 'static, RuntimeError>;

    /// Run an agent through one multi-turn iteration with no prior
    /// conversation history. Thin wrapper around [`RuntimeShellExt::run_with_history`].
    #[allow(async_fn_in_trait)]
    async fn run(
        &self,
        agent_def: AgentDefinition,
        package_manifest: PackageManifest,
        initial_message: Message,
        options: RunOptions,
    ) -> Result<RunOutcome, RuntimeError>;

    /// Run an agent with a pre-existing conversation history. See `run.rs`
    /// module docs for the full loop semantics, error/failure dichotomy,
    /// and tracing vocabulary.
    #[allow(async_fn_in_trait)]
    async fn run_with_history(
        &self,
        agent_def: AgentDefinition,
        package_manifest: PackageManifest,
        history: Vec<Message>,
        initial_message: Message,
        options: RunOptions,
    ) -> Result<RunOutcome, RuntimeError>;

    /// Convenience: [`RuntimeShellExt::run`] with default options
    /// (clock + random injected).
    #[allow(async_fn_in_trait)]
    async fn run_default(
        &self,
        agent_def: AgentDefinition,
        package_manifest: PackageManifest,
        initial_message: Message,
    ) -> Result<RunOutcome, RuntimeError>;

    /// Invoke a single tool by name without engaging the LLM loop.
    #[allow(async_fn_in_trait)]
    async fn invoke_tool(
        &self,
        agent_def: &AgentDefinition,
        package_manifest: &PackageManifest,
        tool_name: &str,
        args: tau_domain::Value,
    ) -> Result<ToolResult, RuntimeError>;

    /// Multi-agent orchestrated run entry point (ROADMAP §9, v1).
    #[allow(async_fn_in_trait)]
    async fn spawn_root_agent(
        self: Arc<Self>,
        root_agent_def: AgentDefinition,
        root_manifest: PackageManifest,
        initial_message: Message,
        budget: RunBudget,
        scope_root: PathBuf,
    ) -> Result<RunSnapshot, RuntimeError>;
}

impl RuntimeShellExt for Runtime {
    async fn run_streaming(
        &self,
        agent_def: AgentDefinition,
        package_manifest: PackageManifest,
        initial_message: Message,
        options: RunOptions,
    ) -> Result<impl Stream<Item = RunEvent> + 'static, RuntimeError> {
        self.run_streaming_with_history(
            agent_def,
            package_manifest,
            Vec::new(),
            initial_message,
            options,
        )
        .await
    }

    async fn run_streaming_with_history(
        &self,
        agent_def: AgentDefinition,
        package_manifest: PackageManifest,
        history: Vec<Message>,
        initial_message: Message,
        options: RunOptions,
    ) -> Result<impl Stream<Item = RunEvent> + 'static, RuntimeError> {
        use tau_ports::DenyEntry;

        info!(name = "runtime.run_streaming_started");

        // Step 1: Determine the effective grant set.
        let (granted_for_kernel, granted_for_session, deny_entries) =
            if let Some(override_grant) = options.granted_capabilities_override.as_ref() {
                debug!(
                    name = "runtime.streaming_capability_set_loaded_from_override",
                    count = override_grant.len(),
                    reason = "agent.<kind>.spawn child run",
                );
                let kernel: Vec<Capability> = override_grant.clone();
                let session: Vec<Capability> = override_grant.clone();
                (kernel, session, Vec::<DenyEntry>::new())
            } else if let Some(resolver) = options.capability_resolver.as_ref() {
                let resolved = resolver
                    .resolve(package_manifest.capabilities())
                    .map_err(|e| {
                        warn!(
                            name = "runtime.streaming_capability_override_rejected",
                            agent_id = %agent_def.id,
                            package_id = %agent_def.package.name,
                            kind = %e.kind,
                            reason = %e.reason,
                        );
                        RuntimeError::Core(CoreRuntimeError::CapabilityOverrideExpands {
                            kind: e.kind,
                            reason: e.reason,
                        })
                    })?;
                debug!(
                    name = "runtime.streaming_capability_set_loaded",
                    count = resolved.for_kernel.len(),
                );
                (
                    resolved.for_kernel,
                    resolved.for_session,
                    resolved.deny_entries,
                )
            } else {
                // No resolver and no override: use the manifest capabilities
                // unchanged (no narrowing, no deny entries). Right default for
                // shells with no override system.
                let kernel: Vec<Capability> = package_manifest.capabilities().to_vec();
                let session = kernel.clone();
                debug!(
                    name = "runtime.streaming_capability_set_loaded_passthrough",
                    count = kernel.len(),
                );
                (kernel, session, Vec::<DenyEntry>::new())
            };
        let granted: &[Capability] = &granted_for_kernel;

        // Step 5: Resolve LLM backend.
        let backend = self
            .resolve_llm_backend(agent_def.id.as_str(), agent_def.llm_backend.as_str())?
            .clone();

        // Step 6: Build capability-filtered tool_specs.
        let mut tool_specs: Vec<ToolSpec> = Vec::with_capacity(self.tools().len());
        for (name, tool) in self.tools().iter() {
            let required = tool.capabilities();
            if check_capabilities(granted, required).is_none() {
                tool_specs.push(tool.schema());
            } else {
                debug!(
                    name = "runtime.streaming_tool_filtered",
                    tool_name = name.as_str(),
                    "tool filtered out: missing capability",
                );
            }
        }

        // Step 7: Snapshot tools registry (Arc-clones).
        let tools: std::collections::HashMap<String, Arc<dyn DynTool>> = self
            .tools()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        // Step 8: Snapshot tool_validators registry.
        let tool_validators: std::collections::HashMap<
            String,
            crate::tool_args::ToolArgsValidator,
        > = self.tool_validators().clone().into_iter().collect();

        // Step 9: Construct and return the stream.
        let stream = crate::stream::run_streaming_inner(
            backend,
            agent_def,
            package_manifest,
            history,
            initial_message,
            options,
            tools,
            tool_validators,
            granted_for_kernel,
            tool_specs,
            deny_entries,
            granted_for_session,
        );
        Ok(stream)
    }

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
    async fn run_with_history(
        &self,
        agent_def: AgentDefinition,
        package_manifest: PackageManifest,
        history: Vec<Message>,
        initial_message: Message,
        options: RunOptions,
    ) -> Result<RunOutcome, RuntimeError> {
        use futures_core::Stream as _;
        use tau_ports::{LlmError, ToolError};

        // Delegate all agent-loop logic to run_streaming_with_history.
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
                    return Err(match kind.as_str() {
                        "ToolNotRegistered" => {
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
                        "Tool" => {
                            let tool_err = match tool_error_variant.as_deref() {
                                Some("BadArgs") => ToolError::BadArgs { reason: detail },
                                Some("SessionDead") => ToolError::SessionDead { reason: detail },
                                Some("DeadlineExceeded") => ToolError::DeadlineExceeded,
                                Some("CapabilityDenied") => {
                                    ToolError::CapabilityDenied { capability: detail }
                                }
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

    async fn run(
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

    async fn run_default(
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

    async fn invoke_tool(
        &self,
        agent_def: &AgentDefinition,
        package_manifest: &PackageManifest,
        tool_name: &str,
        args: tau_domain::Value,
    ) -> Result<ToolResult, RuntimeError> {
        // Delegate to core impl; convert core RuntimeError → host RuntimeError
        // via the From impl.
        tau_runtime_core::Runtime::invoke_tool(
            self,
            agent_def,
            package_manifest,
            tool_name,
            args,
            None,
            None,
        )
        .await
        .map_err(RuntimeError::Core)
    }

    async fn spawn_root_agent(
        self: Arc<Self>,
        root_agent_def: AgentDefinition,
        root_manifest: PackageManifest,
        initial_message: Message,
        budget: RunBudget,
        scope_root: PathBuf,
    ) -> Result<RunSnapshot, RuntimeError> {
        use core::cell::RefCell;

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

        let state_arc = Arc::new(RefCell::new(state));

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
            let mut s = state_arc.borrow_mut();
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
            let s = state_arc.borrow();
            s.snapshot(now_end)
        };
        Ok(snapshot)
    }
}
