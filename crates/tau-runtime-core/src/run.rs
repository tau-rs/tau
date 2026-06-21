//! Agent-loop methods on [`Runtime`] and pure helper functions.
//!
//! This module contains the methods that can run in any async executor
//! (no tokio specifics), plus the pure free-function helpers used by
//! the streaming pump (`stream.rs` in the host shell).
//!
//! # What lives here vs. the host shell
//!
//! - [`Runtime::invoke_tool`] — single-tool direct dispatch; no LLM loop.
//! - [`build_policy_denied_outcome`], [`agent_messages_to_provider_messages`],
//!   [`flatten_content_to_string`], [`content_to_value`] — pure helpers used
//!   by the streaming pump.
//! - `narrowed_capability_for_session` — stays in `tau-runtime-tokio::run`
//!   until `capability_override` migrates to core (it consumes a
//!   `tau-pkg::EffectiveCapability`, which tau-pkg owns).
//!
//! - `run`, `run_with_history`, `run_default`, `spawn_root_agent` —
//!   **stay in the host shell** until `stream.rs` and the orchestration
//!   submodules migrate to core (Tasks 3.6/3.7).
//!
//! # Error routing
//!
//! Methods return `crate::error::RuntimeError` (the core error). Host-shell
//! callers whose return type is the shell-level `RuntimeError` get automatic
//! `?` conversion via `#[from] CoreRuntimeError`.

extern crate alloc;

use alloc::borrow::ToOwned;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use tau_domain::{
    AgentInstanceId, AgentStatus, Capability, FailureKind, Message, MessagePayload,
    PackageManifest, Value,
};
use tau_ports::{ContentBlock, LlmProviderMessage, ToolContent, ToolUse};
use tracing::{debug, instrument};

use crate::builder::Runtime;
use crate::capability::capability_kind_str;
use crate::error::{CapabilityDenial, RuntimeError};
use crate::options::TokenUsage;
use crate::outcome::RunOutcome;

/// Build a `RunOptions` with clock and random filled in from the
/// test-fixture defaults. Used by [`Runtime::run_default`] until the
/// tokio shell's `drive` entry can inject a real wall-clock /
/// OS-random source.
#[cfg(feature = "test-fixtures")]
fn run_options_with_defaults() -> crate::options::RunOptions {
    use tau_ports::{DeterministicRandom, MockClock};
    crate::options::RunOptions {
        clock: Some(Arc::new(MockClock::new())),
        random: Some(Arc::new(DeterministicRandom::seeded(0))),
        ..Default::default()
    }
}

impl Runtime {
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
    /// The `clock` and `random` parameters supply entropy for minting the
    /// tool session ID. Pass `None` to fall back to deterministic
    /// test-fixture defaults (acceptable in tests; production callers
    /// must supply real implementations via their shell's `drive` entry
    /// point).
    ///
    /// # Errors
    ///
    /// - [`RuntimeError::ToolNotRegistered`] — the tool name is unknown.
    /// - [`RuntimeError::Internal`] — the agent's package does not grant
    ///   a capability required by the tool.
    /// - [`RuntimeError::Tool`] — the tool's `init`, `invoke`, or
    ///   `teardown` returned a [`tau_ports::ToolError`].
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Runtime is #[non_exhaustive]; construct via builder.
    /// let result = runtime
    ///     .invoke_tool(&agent_def, &manifest, "echo", Value::Null, None, None)
    ///     .await?;
    /// ```
    #[instrument(
        name = "dispatch.tool",
        skip_all,
        fields(tool_name = %tool_name),
    )]
    pub async fn invoke_tool_with(
        &self,
        agent_def: &tau_domain::AgentDefinition,
        package_manifest: &PackageManifest,
        tool_name: &str,
        args: tau_domain::Value,
        clock: Option<Arc<dyn tau_ports::Clock>>,
        random: Option<Arc<dyn tau_ports::RandomSource>>,
    ) -> Result<tau_ports::ToolResult, RuntimeError> {
        use tau_ports::SessionContext;

        let tool = self.resolve_tool(tool_name)?.clone();
        debug!(
            name = "dispatch.tool_resolved",
            tool_name = %tool_name,
            plugin_id = %tool.name(),
        );

        // Capability check: mirror the run loop's structural check.
        let granted: Vec<Capability> = package_manifest.capabilities().to_vec();
        let required: &[Capability] = tool.capabilities();
        if let Some(missing) = crate::capability::check_capabilities(&granted, required) {
            let denial = CapabilityDenial::new(
                agent_def.id.to_string(),
                agent_def.package.name.to_string(),
                tool_name.to_owned(),
                capability_kind_str(missing),
                alloc::format!("{missing:?}"),
            );
            return Err(RuntimeError::Internal {
                message: alloc::format!("capability denied: {denial}"),
            });
        }

        // Mint a session UUID from the random source. When the caller
        // passes `None` (backwards-compat / test path), fall back to
        // the test-fixture DeterministicRandom if available, otherwise
        // produce a nil UUID. Production callers must supply a real
        // RandomSource via their shell's `drive` entry point.
        // Resolve the entropy source once, then mint both the session UUID
        // and the agent-instance id from the `Clock`/`RandomSource` ports so
        // ids are reproducible under deterministic ports (the no_std kernel
        // never reaches `AgentInstanceId::new`, which is std-only). When the
        // caller passes `None`, fall back to the test-fixture
        // DeterministicRandom if available, otherwise a nil/zero id.
        let instance_millis = clock.as_ref().map(|c| c.now().max(0) as u64).unwrap_or(0);
        let (session_uuid, instance_id) = match random {
            Some(ref r) => {
                let mut rb = [0u8; 10];
                r.fill(&mut rb);
                (
                    crate::ids::uuid_v4(r),
                    AgentInstanceId::from_parts(instance_millis, rb),
                )
            }
            None => {
                #[cfg(any(test, feature = "test-fixtures"))]
                {
                    let r: Arc<dyn tau_ports::RandomSource> =
                        Arc::new(tau_ports::DeterministicRandom::seeded(0));
                    let mut rb = [0u8; 10];
                    r.fill(&mut rb);
                    (
                        crate::ids::uuid_v4(&r),
                        AgentInstanceId::from_parts(instance_millis, rb),
                    )
                }
                #[cfg(not(any(test, feature = "test-fixtures")))]
                {
                    (
                        uuid::Uuid::nil(),
                        AgentInstanceId::from_parts(instance_millis, [0u8; 10]),
                    )
                }
            }
        };

        // Build a minimal SessionContext (no deny entries). `deadline` is a
        // `process`-feature-gated field (no_std hosts have no `SystemTime`),
        // so the constructor arity differs by feature.
        #[cfg(feature = "process")]
        let ctx =
            SessionContext::new(instance_id, session_uuid, None).with_granted_capabilities(granted);
        #[cfg(not(feature = "process"))]
        let ctx = SessionContext::new(instance_id, session_uuid).with_granted_capabilities(granted);

        tool.init(ctx.clone()).await.map_err(RuntimeError::from)?;
        let result = tool.invoke(&ctx, &mut (), args).await;
        // teardown best-effort: don't mask invoke's error if both fail.
        let _ = tool.teardown(()).await;
        result.map_err(RuntimeError::from)
    }

    /// Invoke a single tool by name, using deterministic test-fixture
    /// defaults for clock/random. Thin 4-arg wrapper over
    /// [`Self::invoke_tool_with`].
    pub async fn invoke_tool(
        &self,
        agent_def: &tau_domain::AgentDefinition,
        package_manifest: &PackageManifest,
        tool_name: &str,
        args: tau_domain::Value,
    ) -> Result<tau_ports::ToolResult, RuntimeError> {
        self.invoke_tool_with(agent_def, package_manifest, tool_name, args, None, None)
            .await
    }
}

// ---------------------------------------------------------------------------
// Agent-loop entry points (run / run_with_history / run_default /
// run_streaming / run_streaming_with_history / spawn_root_agent_inner)
//
// These methods used to live on a `RuntimeShellExt` extension trait in
// `tau-runtime`. Pre-β.1.3.5c they moved to inherent methods on the
// kernel `Runtime` so host shells without a newtype overlay (and
// embassy/wasm shells without tokio) can drive a run identically.
//
// The methods are intentionally non-`Send` async (the kernel's plugin
// futures are non-`Send` by design — see `builder::BoxFuture`).
// ---------------------------------------------------------------------------

#[cfg(feature = "wasm-interpreter")]
impl Runtime {
    /// Stream an agent run from a single initial message.
    ///
    /// Convenience wrapper over [`Self::run_streaming_with_history`] that
    /// passes an empty history.
    pub async fn run_streaming(
        &self,
        agent_def: tau_domain::AgentDefinition,
        package_manifest: PackageManifest,
        initial_message: Message,
        options: crate::options::RunOptions,
    ) -> Result<impl futures_core::Stream<Item = crate::stream::RunEvent> + 'static, RuntimeError>
    {
        self.run_streaming_with_history(
            agent_def,
            package_manifest,
            Vec::new(),
            initial_message,
            options,
        )
        .await
    }

    /// Stream an agent run, prepending conversation history before the
    /// initial message.
    pub async fn run_streaming_with_history(
        &self,
        agent_def: tau_domain::AgentDefinition,
        package_manifest: PackageManifest,
        history: Vec<Message>,
        initial_message: Message,
        options: crate::options::RunOptions,
    ) -> Result<impl futures_core::Stream<Item = crate::stream::RunEvent> + 'static, RuntimeError>
    {
        use hashbrown::HashMap as HbHashMap;
        use tau_ports::DenyEntry;

        tracing::info!(name = "runtime.run_streaming_started");

        // Step 1: Determine the effective grant set.
        let (granted_for_kernel, granted_for_session, deny_entries) =
            if let Some(override_grant) = options.granted_capabilities_override.as_ref() {
                tracing::debug!(
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
                        tracing::warn!(
                            name = "runtime.streaming_capability_override_rejected",
                            agent_id = %agent_def.id,
                            package_id = %agent_def.package.name,
                            kind = %e.kind,
                            reason = %e.reason,
                        );
                        RuntimeError::CapabilityOverrideExpands {
                            kind: e.kind,
                            reason: e.reason,
                        }
                    })?;
                tracing::debug!(
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
                tracing::debug!(
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
        let mut tool_specs: Vec<tau_ports::ToolSpec> = Vec::with_capacity(self.tools().len());
        for (name, tool) in self.tools().iter() {
            let required = tool.capabilities();
            if crate::capability::check_capabilities(granted, required).is_none() {
                tool_specs.push(tool.schema());
            } else {
                tracing::debug!(
                    name = "runtime.streaming_tool_filtered",
                    tool_name = name.as_str(),
                    "tool filtered out: missing capability",
                );
            }
        }

        // Step 7: Snapshot tools registry (Arc-clones).
        let tools: HbHashMap<alloc::string::String, Arc<dyn crate::builder::DynTool>> = self
            .tools()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        // Step 8: Snapshot tool_validators registry.
        // When `tool-validation` is enabled the validators are pre-compiled
        // at Runtime::build time and stored in the registry; without
        // `tool-validation` (e.g. the wasm guest) no schema validation runs.
        #[cfg(feature = "tool-validation")]
        let tool_validators: HbHashMap<
            alloc::string::String,
            crate::tool_args::ToolArgsValidator,
        > = self.tool_validators().clone().into_iter().collect();
        #[cfg(not(feature = "tool-validation"))]
        let tool_validators: HbHashMap<alloc::string::String, ()> = HbHashMap::default();

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

    /// Run an agent through one multi-turn iteration with no prior
    /// conversation history. Thin wrapper around
    /// [`Self::run_with_history`].
    pub async fn run(
        &self,
        agent_def: tau_domain::AgentDefinition,
        package_manifest: PackageManifest,
        initial_message: Message,
        options: crate::options::RunOptions,
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

    /// Run an agent with a pre-existing conversation history. See `run.rs`
    /// module docs for the full loop semantics, error/failure dichotomy,
    /// and tracing vocabulary.
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
        agent_def: tau_domain::AgentDefinition,
        package_manifest: PackageManifest,
        history: Vec<Message>,
        initial_message: Message,
        options: crate::options::RunOptions,
    ) -> Result<RunOutcome, RuntimeError> {
        use crate::stream::RunEvent;
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
        let mut stream = alloc::boxed::Box::pin(stream);
        loop {
            let next = core::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await;
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
                                    let reg: Vec<alloc::string::String> = v["registered"]
                                        .as_array()?
                                        .iter()
                                        .filter_map(|x| x.as_str().map(alloc::string::String::from))
                                        .collect();
                                    Some((tn, reg))
                                })
                                .unwrap_or_else(|| (detail.clone(), alloc::vec![]));
                            RuntimeError::ToolNotRegistered {
                                tool_name,
                                registered,
                            }
                        }
                        "Llm" => RuntimeError::Llm(LlmError::Internal { message: detail }),
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
                            RuntimeError::Tool(tool_err)
                        }
                        "ContextPipeline" => RuntimeError::ContextPipeline { detail },
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

    /// Convenience: [`Self::run`] with default options (clock + random
    /// injected from test fixtures). Requires the `test-fixtures` feature;
    /// production callers should construct `RunOptions` themselves via
    /// their shell's `drive` entry point.
    #[cfg(feature = "test-fixtures")]
    pub async fn run_default(
        &self,
        agent_def: tau_domain::AgentDefinition,
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
}

#[cfg(feature = "tool-validation")]
impl Runtime {
    /// Multi-agent orchestrated run entry point (ROADMAP §9, v1).
    ///
    /// The kernel-resident half of the spawn_root_agent flow. Host shells
    /// pass pre-attached trace subscribers (e.g. a tokio JSONL writer)
    /// in via `subscribers`; embassy/wasm shells with no persistence pass
    /// `Vec::new()`. The full `spawn_root_agent_with_scope` wrapper in
    /// `tau-runtime` builds the JSONL subscriber from the scope_root and
    /// delegates here.
    #[allow(clippy::too_many_arguments)]
    pub async fn spawn_root_agent_inner(
        self: Arc<Self>,
        root_agent_def: tau_domain::AgentDefinition,
        root_manifest: PackageManifest,
        initial_message: Message,
        budget: tau_ports::RunBudget,
        subscribers: Vec<Arc<dyn crate::orchestration::TraceSubscriber>>,
        clock: Option<Arc<dyn tau_ports::Clock>>,
        random: Option<Arc<dyn tau_ports::RandomSource>>,
        // Optional pre-minted run-id. When `Some`, the inner uses it
        // verbatim — host shells supply this so the JSONL log path and
        // the run-id inside trace events agree.
        run_id_override: Option<alloc::string::String>,
        // Project-scope root as a String. Host shells convert their PathBuf
        // via `path.to_string_lossy().into_owned()`. Used by the streaming
        // pump's skill-spawn intercept to resolve installed skills.
        scope_root: Option<alloc::string::String>,
        // Skill resolver for `skill.<name>.spawn` dispatch. Host shells
        // build a `TauPkgSkillResolver` from their scope; guest shells
        // pass `None` (or a `NoSkillResolver`). Carried into `RunOptions`.
        skill_resolver: Option<Arc<dyn tau_ports::SkillResolver>>,
    ) -> Result<tau_ports::RunSnapshot, RuntimeError> {
        use core::cell::RefCell;

        // Default clock/random for test paths if caller passes None.
        let clock = clock.unwrap_or_else(|| {
            #[cfg(feature = "test-fixtures")]
            {
                Arc::new(tau_ports::MockClock::new()) as Arc<dyn tau_ports::Clock>
            }
            #[cfg(not(feature = "test-fixtures"))]
            {
                panic!(
                    "spawn_root_agent_inner: clock must be supplied unless test-fixtures \
                     is enabled"
                );
            }
        });
        let random = random.unwrap_or_else(|| {
            #[cfg(feature = "test-fixtures")]
            {
                Arc::new(tau_ports::DeterministicRandom::seeded(0))
                    as Arc<dyn tau_ports::RandomSource>
            }
            #[cfg(not(feature = "test-fixtures"))]
            {
                panic!(
                    "spawn_root_agent_inner: random must be supplied unless test-fixtures \
                     is enabled"
                );
            }
        });

        let run_id = run_id_override.unwrap_or_else(|| crate::ids::ulid(&clock, &random));
        let root_agent_id = root_agent_def.id.to_string();
        let now = crate::ids::now_utc(&clock);

        let mut state = crate::orchestration::run_state::RunState::new(
            run_id.clone(),
            root_agent_id,
            budget,
            now,
        );

        // Attach pre-built subscribers (JSONL writer, etc).
        for sub in subscribers {
            state.trace.add_subscriber(sub);
        }

        // Arc<RefCell<RunState>>: kernel futures are non-Send by design
        // (see builder::BoxFuture alias), so RunState is single-task and
        // RefCell is the honest representation. Arc is kept (not Rc) so the
        // refcount machinery is uniform with the rest of the kernel surface.
        #[allow(clippy::arc_with_non_send_sync)]
        let state_arc = Arc::new(RefCell::new(state));

        let opts = crate::options::RunOptions {
            orchestration_state: Some(state_arc.clone()),
            orchestration_runtime: Some(self.clone()),
            clock: Some(clock.clone()),
            random: Some(random.clone()),
            scope_root,
            skill_resolver,
            ..Default::default()
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

        let now_end = crate::ids::now_utc(&clock);
        {
            let mut s = state_arc.borrow_mut();
            s.ended_at = Some(now_end);
            let success = matches!(outcome, RunOutcome::Completed { .. });
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
                    id: crate::ids::ulid(&clock, &random),
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

// ---------------------------------------------------------------------------
// Internal helpers (pure free functions used by the streaming pump)
// ---------------------------------------------------------------------------

/// Build the `RunOutcome::Failed { kind: PolicyDenied, .. }` returned
/// when [`crate::capability::check_capabilities`] rejects a tool invocation.
/// Centralizes the construction so the run loop's denial branch reads cleanly.
pub fn build_policy_denied_outcome(
    denial: CapabilityDenial,
    all_messages: Vec<Message>,
    total_turns: u32,
    token_usage: TokenUsage,
) -> RunOutcome {
    RunOutcome::Failed {
        status: AgentStatus::failed(FailureKind::PolicyDenied, Some(alloc::format!("{denial}"))),
        all_messages,
        total_turns,
        token_usage,
    }
}

/// Project the agent's [`Message`] history onto the LLM-call shape.
///
/// Per `tau_ports::llm` module-level docs, `tau_domain::Message`
/// (universal envelope) and [`LlmProviderMessage`] (provider call
/// shape) are intentionally distinct. This function is the single
/// projection point in the kernel.
pub fn agent_messages_to_provider_messages(history: &[Message]) -> Vec<LlmProviderMessage> {
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
                let tool_name = match &m.recipient {
                    tau_domain::Address::Tool(name) => name.clone(),
                    _ => String::new(),
                };
                out.push(LlmProviderMessage::assistant(vec![ContentBlock::ToolUse(
                    ToolUse::new(alloc::format!("toolu_{}", m.id), tool_name, args.clone()),
                )]));
            }
            (tau_domain::Address::Tool(_), MessagePayload::ToolResult { body }) => {
                out.push(LlmProviderMessage::tool_result(
                    alloc::format!("toolu_{}", m.id),
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
                    alloc::format!("toolu_{}", m.id),
                    vec![ContentBlock::Text(message.clone())],
                    true,
                ));
            }
            _ => {}
        }
    }
    out
}

/// Flatten a tool's content blocks into a single human-readable string.
pub fn flatten_content_to_string(blocks: &[ToolContent]) -> String {
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
            _ => {}
        }
    }
    out
}

/// Build a [`Value`] from a tool's content blocks.
pub fn content_to_value(blocks: &[ToolContent]) -> Value {
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

/// Compact preview string for a [`Value`].
pub fn value_to_preview_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Bytes(b) => alloc::format!("<{} bytes>", b.len()),
        Value::Array(_) | Value::Object(_) => alloc::format!("{v:?}"),
        _ => alloc::format!("{v:?}"),
    }
}

// narrowed_capability_for_session uses tau-pkg::EffectiveCapability (a
// tau-pkg type), so it stays in tau-runtime until capability_override
// migrates to core in Task 3.7.
