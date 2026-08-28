//! Per-agent loop driver.
//!
//! Routes through the existing `Runtime::run_with_history` (kernel
//! agent loop) by constructing a `Runtime` configured with the agent
//! node's tools, prompt, model, and budget. The ToolDispatcher trait
//! call (task 4.3) is what each tool reaches when invoked — the
//! `Runtime`'s tool registry is wired with a thin wrapper that delegates
//! to the dispatcher.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::str::FromStr;

use tau_domain::{
    Address, AgentDefinition, AgentId as DomainAgentId, Message, MessagePayload, PackageId,
    PackageManifest, PackageName, UncheckedManifest, Version,
};
use tau_ir::{Agent, IrModule, ToolId, ToolImpl};
use tau_ports::{
    tool::{SessionContext, ToolContent, ToolResult},
    ToolError, ToolSpec,
};

use crate::builder::Runtime;
use crate::error::RuntimeError;
use crate::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};
use crate::options::RunOptions;
use crate::outcome::RunOutcome;

// ---------------------------------------------------------------------------
// Value conversion helpers
// ---------------------------------------------------------------------------

/// Convert a `tau_domain::Value` to `serde_json::Value` via serde round-trip.
///
/// `tau_domain::Value` and `serde_json::Value` are distinct types; serde
/// is the lowest-friction bridge between them.
fn domain_value_to_json(v: &tau_domain::Value) -> serde_json::Value {
    serde_json::to_value(v).unwrap_or(serde_json::Value::Null)
}

/// Convert a `serde_json::Value` to `tau_domain::Value` via serde round-trip.
fn json_to_domain_value(v: serde_json::Value) -> tau_domain::Value {
    serde_json::from_value(v).unwrap_or(tau_domain::Value::Null)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the body text of the last `Assistant`-authored message in a
/// `RunOutcome`'s `all_messages`. Returns an empty `String` when no
/// assistant message exists (e.g. immediate `Failed` outcomes).
///
/// Used by `DispatcherTool::invoke`'s `Subflow` arm to convert a child
/// agent's terminal state into a tool-result body for the parent.
///
/// # Important
///
/// Both `RunOutcome::Completed` and `RunOutcome::Failed` carry `all_messages`
/// and this function returns the same shaped `String` for both. Callers MUST
/// match on the outcome variant separately to decide whether to surface the
/// result as a success or error — this function alone does not distinguish
/// between them.
pub(crate) fn last_assistant_text(outcome: &RunOutcome) -> String {
    // Note: `RunOutcome` is `#[non_exhaustive]` but we're in the defining
    // crate, so the compiler sees all variants. No `_` arm needed here;
    // adding one would generate an `unreachable_patterns` warning. Future
    // variants must be handled by updating this match.
    let messages = match outcome {
        RunOutcome::Completed { all_messages, .. } => all_messages,
        RunOutcome::Failed { all_messages, .. } => all_messages,
    };
    for msg in messages.iter().rev() {
        if matches!(msg.sender, tau_domain::Address::Agent(_)) {
            if let tau_domain::MessagePayload::Text { content } = &msg.payload {
                return content.clone();
            }
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------
// Dispatcher-backed Tool wrapper
// ---------------------------------------------------------------------------

/// A `tau_ports::Tool` whose `invoke` branches on the IR `ToolImpl`:
///
/// - `Native` / `Mcp` → forwarded to the user's `ToolDispatcher`.
/// - `Subflow { target }` → recursively `run_ir` against the target
///   agent in the same module. The recursion is broken with `Box::pin`
///   to satisfy Rust's no-self-referential-async-fn rule.
/// - `Step { id }` → looked up in `module.workflow.steps` and called
///   against the dispatcher's `DeterministicRegistry`.
///
/// One wrapper instance is created per tool in `agent.tool_refs`. Each
/// instance holds an `Arc<D>` (the dispatcher) and the `ToolId` it should
/// forward invocations to.
struct DispatcherTool<D> {
    /// Tool name as seen by the LLM (from the IR `ToolSpec`).
    tool_name: String,
    /// `ToolId` forwarded to the dispatcher on each invoke (Native/Mcp).
    tool_id: ToolId,
    /// LLM-facing spec (constructed via serde to bypass `#[non_exhaustive]`).
    spec: ToolSpec,
    /// IR module, shared so subflow recursion + step lookup work.
    module: alloc::sync::Arc<tau_ir::IrModule>,
    /// Clone of the IR tool's `ToolImpl`. Cheap to clone (a few hashes
    /// or a `String`).
    tool_impl: tau_ir::ToolImpl,
    /// Shared dispatcher handle.
    dispatcher: Arc<D>,
}

// `Tool::capabilities()` is intentionally NOT overridden (issue #581): the
// dispatch-site gate compares a tool's required caps against the running
// agent's grant, and the IR has no per-agent grant — the interpreter's
// synthesised manifest grants `[]`, so forwarding the IR tool node's
// declared caps here would policy-deny every capability-bearing tool on
// every bundle run. IR-declared caps are enforced at build time (governance
// ceiling + D-3b capability-fit), at the host boundary (OS sandbox / wasm
// WIT world + WasiCtx), and at subflow frames (`AttenuatedDispatcher`).
// Contract pinned by `tests/ir_dispatch_gate_inert.rs`.
impl<D> tau_ports::tool::Tool for DispatcherTool<D>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    type Session = ();

    fn name(&self) -> &str {
        &self.tool_name
    }

    fn schema(&self) -> ToolSpec {
        self.spec.clone()
    }

    async fn init(&self, _ctx: SessionContext) -> Result<Self::Session, ToolError> {
        Ok(())
    }

    async fn invoke(
        &self,
        _session: &mut Self::Session,
        args: tau_domain::Value,
    ) -> Result<ToolResult, ToolError> {
        // Convert tau_domain::Value → serde_json::Value once, then branch
        // on the IR ToolImpl. The branches do NOT share extraction logic
        // because Subflow returns a domain-side `RunOutcome` and Step
        // returns a raw `serde_json::Value` from the registry — both end
        // up as ToolResult Text content but the source paths differ.
        let json_args = domain_value_to_json(&args);

        match &self.tool_impl {
            ToolImpl::Native { .. } | ToolImpl::Mcp { .. } => {
                // Existing path: forward to the user's ToolDispatcher.
                //
                // Extract text symmetrically with `ForwardingDispatcher`'s body-shape
                // lift in `crates/tau-cli/src/cmd/ir_dispatcher.rs`. The dispatcher
                // try-parses the joined tool text as JSON and falls back to
                // `Value::String(text)` on parse failure, so plain text reaches
                // `body` as `Value::String("hello")`. `Display` on
                // `serde_json::Value::String("hello")` emits the literal quoted
                // form `"hello"` — corrupting every plain-text result the LLM
                // sees. Special-case `Value::String` to extract the raw `String`
                // (no quoting); fall through to `Display` for structured shapes
                // (objects/arrays/numbers/bools/null) so they serialise as
                // compact JSON. This makes the inverse direction lossless:
                //   plain text  → Value::String  → raw text   (no extra quotes)
                //   JSON object → Value::Object  → compact JSON
                let result: ToolInvocationResult = self
                    .dispatcher
                    .invoke(&self.tool_id, &json_args)
                    .await
                    .map_err(|e| ToolError::Internal {
                        message: alloc::format!("dispatcher error: {e}"),
                    })?;

                if let Some(err_msg) = result.error {
                    return Ok(ToolResult::new(
                        alloc::vec![ToolContent::Text { text: err_msg }],
                        true,
                    ));
                }

                let text = match result.body {
                    Some(serde_json::Value::String(s)) => s,
                    Some(v) => alloc::format!("{v}"),
                    None => alloc::string::String::new(),
                };
                Ok(ToolResult::new(
                    alloc::vec![ToolContent::Text { text }],
                    false,
                ))
            }

            ToolImpl::Subflow { target } => {
                // Box::pin breaks the async self-reference (run_ir →
                // run_agent → DispatcherTool::invoke → run_ir). Without it
                // the async-fn future type would be infinitely recursive.
                let module = self.module.clone();
                let target = target.clone();

                // Attenuation frame: this subflow tool's declared capabilities
                // are the cap_subset the child (and its descendants) run
                // under. Absent tool entry (shouldn't happen post-typecheck)
                // falls back to an empty grant — deny-by-default.
                let cap_subset = module
                    .workflow
                    .tools
                    .get(&self.tool_id)
                    .map(|t| t.capabilities.clone())
                    .unwrap_or_default();
                // Kept as a concrete `Arc<AttenuatedDispatcher>` (not
                // widened to `Arc<dyn ToolDispatcher + Send + Sync>`) because
                // `run_ir<D>` requires `D: Sized`; the inner-dispatcher
                // coercion to the trait object happens at the `new(...)`
                // call site instead, via `self.dispatcher.clone()`'s target
                // type (`Arc<dyn ToolDispatcher + Send + Sync>`).
                let child_dispatcher =
                    Arc::new(crate::interpreter::attenuate::AttenuatedDispatcher::new(
                        cap_subset,
                        self.tool_id.clone(),
                        target.0.clone(),
                        module.clone(),
                        self.dispatcher.clone(), // Arc<D> → Arc<dyn ToolDispatcher + Send + Sync>
                    ));

                let outcome = alloc::boxed::Box::pin(crate::interpreter::run_ir(
                    module,
                    &target,
                    child_dispatcher,
                    Vec::new(),
                ))
                .await
                .map_err(|e| ToolError::Internal {
                    message: alloc::format!("subflow recursion error: {e}"),
                })?;

                // Propagate child-agent failures as `is_error: true` tool
                // results so the parent agent sees them and can react.
                // A silently-swallowed Failed outcome would look like a
                // successful empty-string result to the parent — wrong.
                match &outcome {
                    RunOutcome::Failed { .. } => {
                        // Child agent failed — propagate as an error tool result so
                        // the parent agent sees an `is_error: true` and can react.
                        // The child's last assistant text (if any) is preserved as
                        // diagnostic content.
                        let last_text = last_assistant_text(&outcome);
                        let body = if last_text.is_empty() {
                            alloc::format!(
                                "subflow agent {:?} failed with no further detail",
                                target
                            )
                        } else {
                            alloc::format!("subflow agent {:?} failed: {}", target, last_text)
                        };
                        return Ok(ToolResult::new(
                            alloc::vec![ToolContent::Text { text: body }],
                            true,
                        ));
                    }
                    RunOutcome::Completed { .. } => {}
                }

                // Convert RunOutcome → tool result body. v0 contract:
                // emit the last `Assistant`-sent text from `all_messages`
                // as the tool result body. Empty string if none.
                let text = last_assistant_text(&outcome);
                Ok(ToolResult::new(
                    alloc::vec![ToolContent::Text { text }],
                    false,
                ))
            }

            ToolImpl::Step { id } => {
                // Step accounting (e.g. conformance test recording) lives in the
                // DeterministicRegistry implementation, NOT in the dispatcher's
                // `invoke` path — calling dispatcher.invoke for accounting would
                // fire a real plugin lookup in production (ForwardingDispatcher) for
                // a tool that doesn't exist there. Recording belongs in the registry.
                //
                // TODO (Phase 4): to count step invocations in conformance tests,
                // make `MapBackedDeterministicRegistry` push records into a shared
                // `Arc<Mutex<Vec<ToolCallRecord>>>` and expose that handle via
                // `RecordingDispatcher::deterministic_registry()`.

                // Look up the step in the IR's workflow.steps table.
                let step =
                    self.module
                        .workflow
                        .steps
                        .get(id)
                        .ok_or_else(|| ToolError::Internal {
                            message: alloc::format!(
                                "step tool {:?} references unknown step {:?} \
                             (typecheck should have caught this — possible IR corruption)",
                                self.tool_id,
                                id,
                            ),
                        })?;

                let registry = self.dispatcher.deterministic_registry().ok_or_else(|| {
                    ToolError::Internal {
                        message: alloc::format!(
                            "agent invoked step tool {:?} but the dispatcher \
                             did not provide a DeterministicRegistry",
                            self.tool_id,
                        ),
                    }
                })?;

                let result = registry
                    .invoke(&step.fn_ref.name, &json_args)
                    .map_err(|e| ToolError::Internal {
                        message: alloc::format!(
                            "deterministic step {:?} (fn={:?}) failed: {e}",
                            self.tool_id,
                            step.fn_ref.name,
                        ),
                    })?;

                // Same body-shape extraction as the Native/Mcp arm so the
                // LLM sees identically-shaped tool results regardless of
                // ToolImpl variant.
                let text = match result {
                    serde_json::Value::String(s) => s,
                    v => alloc::format!("{v}"),
                };
                Ok(ToolResult::new(
                    alloc::vec![ToolContent::Text { text }],
                    false,
                ))
            }
        }
    }

    async fn teardown(&self, _session: Self::Session) -> Result<(), ToolError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ToolSpec constructor (bypasses #[non_exhaustive] via serde)
// ---------------------------------------------------------------------------

/// Construct a `ToolSpec` from its fields.
///
/// `ToolSpec` is `#[non_exhaustive]` and has no public constructor outside
/// `tau-ports`. We construct it by deserializing from a `serde_json::Value`
/// that carries the three fields — this is the canonical escape hatch for
/// `#[non_exhaustive]` types that have serde derives.
fn make_tool_spec(name: &str, description: &str, input_schema: &serde_json::Value) -> ToolSpec {
    // `ToolSpec.input_schema` is `tau_domain::Value` (not serde_json::Value).
    // We round-trip through JSON to get the domain value.
    let input_schema_domain = json_to_domain_value(input_schema.clone());
    let json = serde_json::json!({
        "name": name,
        "description": description,
        "input_schema": input_schema_domain,
    });
    serde_json::from_value(json).unwrap_or_else(|_| {
        // Absolute fallback: empty-schema spec so the tool still registers.
        let fallback = serde_json::json!({
            "name": name,
            "description": description,
            "input_schema": tau_domain::Value::Object(Default::default()),
        });
        serde_json::from_value(fallback).expect("fallback ToolSpec must deserialize")
    })
}

// ---------------------------------------------------------------------------
// Synthesise a minimal PackageManifest from thin air (via serde)
// ---------------------------------------------------------------------------

/// Build a stub `PackageManifest` for IR agents (no plugin, no deps,
/// no capabilities).
///
/// The manifest is needed as a passthrough container for the kernel's
/// capability-check plumbing; IR agents declare their capabilities via
/// the IR capability table, not via this manifest.
///
/// `UncheckedManifest` is `#[non_exhaustive]`, so we construct it via
/// serde JSON deserialization — the same escape hatch used for other
/// non-exhaustive domain types.
fn stub_manifest() -> PackageManifest {
    // `UncheckedManifest` is `#[non_exhaustive]` (no external struct-literal
    // construction allowed), so we go through its serde `Deserialize` impl.
    //
    // Root cause of the original wasm panic: `PackageSource::from_str` for
    // `https://…` URLs requires the `package-source` feature (which enables
    // `url::Url`). Without it, `source` deserialization returns `MalformedUrl`
    // and `.expect(…)` panics. Fix: use an scp-style address
    // (`example.com:example/ir.git`) — always parseable, no feature gate.
    let json = serde_json::json!({
        "name": "ir-agent",
        "version": "0.0.0",
        "description": "Synthesised manifest for IR interpreter agent",
        "authors": [],
        "source": "example.com:example/ir.git",
        "kind": "tool",
        "dependencies": [],
        "capabilities": [],
        "sandbox": {}
    });
    let unchecked: UncheckedManifest =
        serde_json::from_value(json).expect("stub manifest JSON must deserialize");
    unchecked
        .validate()
        .expect("stub manifest should always validate")
}

// ---------------------------------------------------------------------------
// run_agent — the main entry
// ---------------------------------------------------------------------------

/// Construct everything needed to drive one `Agent` node through the kernel
/// agent loop, returning the pieces instead of running them.
///
/// This is the single shared construction path for both the batch entry
/// ([`run_agent`], via `Runtime::run_with_history`) and the streaming entry
/// ([`run_agent_streaming`], via `Runtime::run_streaming_with_history`). The
/// two MUST NOT drift: any change to backend wiring, tool registration,
/// `AgentDefinition` synthesis, `RunOptions` (clock/random injection + the
/// β.4 context pipeline), or history splitting belongs here so both callers
/// inherit it identically.
///
/// Behaviour is identical to the former inline body of `run_agent`; this is a
/// pure extraction (steps 1–7), with the final `rt.run_with_history(...)`
/// call left to the caller.
#[allow(clippy::type_complexity)]
fn prepare_agent_run<D>(
    module: alloc::sync::Arc<IrModule>,
    agent: &Agent,
    dispatcher: Arc<D>,
    initial_messages: Vec<Message>,
    spawn_tools: Vec<crate::interpreter::dynamic::SpawnTool<D>>,
) -> Result<
    (
        Runtime,
        AgentDefinition,
        PackageManifest,
        Vec<Message>,
        Message,
        RunOptions,
    ),
    RuntimeError,
>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    // 1. Obtain the LLM backend from the dispatcher, keyed by the IR backend name.
    let backend = dispatcher.llm_backend_for(&agent.model_ref.backend)?;
    let backend_name = String::from(backend.name());

    // 2. Build the RuntimeBuilder with the backend.
    let mut builder = Runtime::builder().with_dyn_llm_backend(backend);

    // 3. Register each tool in agent.tool_refs as a dispatcher-delegating wrapper.
    for tool_id in &agent.tool_refs {
        let ir_tool = module
            .workflow
            .tools
            .get(tool_id)
            .ok_or_else(|| RuntimeError::Internal {
                message: alloc::format!(
                    "agent {:?} references tool {:?} not in workflow.tools",
                    agent.id.0,
                    tool_id.0,
                ),
            })?;

        let spec = make_tool_spec(
            &ir_tool.spec.name,
            &ir_tool.spec.description,
            &ir_tool.spec.input_schema,
        );

        builder = builder.with_tool(DispatcherTool {
            tool_name: ir_tool.spec.name.clone(),
            tool_id: tool_id.clone(),
            spec,
            module: module.clone(),
            tool_impl: ir_tool.impl_.clone(),
            dispatcher: dispatcher.clone(),
        });
    }

    // 3b. EPIC 4.5: register one `SpawnTool` per offered kind of a
    //     `StepRun::Dynamic` region's coordinator (empty for every ordinary
    //     agent — only `run_agent_with_spawn_tools` passes a non-empty Vec).
    for st in spawn_tools {
        builder = builder.with_tool(st);
    }

    // 4. Build the Runtime.
    let rt = builder.build().map_err(|e| RuntimeError::Internal {
        message: alloc::format!("failed to build Runtime for IR agent {:?}: {e}", agent.id.0),
    })?;

    // 5. Synthesise the AgentDefinition from the IR agent.
    //    PackageName allows [a-z0-9-] starting with a letter — same as AgentId.
    //    We fall back to "ir-agent" if the agent id doesn't conform.
    let pkg_name = PackageName::from_str(&agent.id.0)
        .unwrap_or_else(|_| PackageName::from_str("ir-agent").expect("ir-agent is always valid"));

    let llm_backend_pkg_name = PackageName::from_str(&backend_name)
        .unwrap_or_else(|_| PackageName::from_str("ir-agent").expect("ir-agent is always valid"));

    let domain_agent_id = DomainAgentId::from_str(&agent.id.0)
        .unwrap_or_else(|_| DomainAgentId::from_str("ir-agent").expect("ir-agent is always valid"));

    // Resolve the agent's system prompt (D6-B). `Inline` is the literal text.
    // `Asset` references the bundle's content-addressed asset store, supplied
    // by the host via `ToolDispatcher::assets()`; look up the blob by hash and
    // decode its bytes as UTF-8. A missing store or missing/!utf-8 blob is a
    // structured error, not a panic — never trust input shape (the host's
    // load-time closed-world check should make the missing cases unreachable).
    let system_prompt = match &agent.prompt {
        tau_ir::prompt::PromptSource::Inline(s) => s.clone(),
        tau_ir::prompt::PromptSource::Asset(a) => {
            let store = dispatcher.assets().ok_or_else(|| RuntimeError::Internal {
                message: alloc::format!(
                    "agent {:?}: prompt references asset {:?} but the host supplied no asset store",
                    agent.id.0,
                    a.asset,
                ),
            })?;
            let blob = store.get(&a.asset).ok_or_else(|| RuntimeError::Internal {
                message: alloc::format!(
                    "agent {:?}: prompt asset {:?} not found in the bundle asset store",
                    agent.id.0,
                    a.asset,
                ),
            })?;
            alloc::string::String::from_utf8(blob.bytes.clone()).map_err(|_| {
                RuntimeError::Internal {
                    message: alloc::format!(
                        "agent {:?}: prompt asset {:?} is not valid UTF-8",
                        agent.id.0,
                        a.asset,
                    ),
                }
            })?
        }
    };

    let agent_def = AgentDefinition::new(
        domain_agent_id,
        agent.id.0.clone(),
        PackageId::new(
            pkg_name,
            Version::parse("0.0.0").expect("0.0.0 is always valid"),
        ),
        llm_backend_pkg_name,
    )
    .with_system_prompt(system_prompt)
    .with_model(agent.model_ref.model_id.clone());

    let manifest = stub_manifest();

    // 6. Build RunOptions from the agent's budget.
    let mut run_options = RunOptions::default();
    if let Some(max_turns) = agent.budget.max_turns {
        run_options.max_turns = max_turns;
    }
    // Prefer the host shell's clock/random, supplied through the
    // dispatcher (e.g. tau-cli's `ForwardingDispatcher` returns
    // `TokioClock` / `OsRandom`). This is how a production
    // (non-`test-fixtures`) build of `run_ir` / `run_pipeline` obtains a
    // real wall-clock and entropy source — the kernel is `no_std` and
    // cannot mint them itself. Dispatchers that don't override these
    // return `None` (the trait default), in which case the test-fixtures
    // injection below applies, or the panic guard fires.
    if run_options.clock.is_none() {
        run_options.clock = dispatcher.clock();
    }
    if run_options.random.is_none() {
        run_options.random = dispatcher.random();
    }
    // Inject test-fixture clock/random when neither the host shell nor the
    // dispatcher provided them (e.g. tau-ir-conformance drives run_ir
    // directly without the tokio shell drive entry). The test-fixtures
    // feature is the sanctioned escape hatch; matching the
    // spawn_root_agent_inner pattern in run.rs.
    #[cfg(feature = "test-fixtures")]
    {
        if run_options.clock.is_none() {
            run_options.clock = Some(alloc::sync::Arc::new(tau_ports::MockClock::new()));
        }
        if run_options.random.is_none() {
            run_options.random = Some(alloc::sync::Arc::new(
                tau_ports::DeterministicRandom::seeded(0),
            ));
        }
    }
    #[cfg(not(feature = "test-fixtures"))]
    {
        if run_options.clock.is_none() {
            panic!("prepare_agent_run: clock must be supplied unless test-fixtures is enabled");
        }
        if run_options.random.is_none() {
            panic!("prepare_agent_run: random must be supplied unless test-fixtures is enabled");
        }
    }

    // 6b. β.4: build the per-turn context pipeline from the IR agent's
    //     config. `run_agent` is the single shared agent-loop entry for
    //     all three drive paths (run_ir, run_pipeline, check), so wiring
    //     here covers `tau dev`, `tau run --bundle`, pipeline steps, and
    //     deliverable checks. Custom nodes resolve via the dispatcher's
    //     optional registry; builtins resolve regardless. Agents without
    //     a `context` config keep the empty default pipeline.
    if let Some(ctx_cfg) = agent.context.as_ref() {
        let registry = dispatcher.context_transformer_registry();
        run_options.context_pipeline =
            crate::context::build_context_pipeline(ctx_cfg, registry.as_deref())?;
    }

    // 6c. ADR-0053: wire durable checkpoint/resume for agents that opt in.
    //     Gated on `agent.durable` so only the declaring agent checkpoints —
    //     a non-durable judge / subflow / pipeline-step agent sharing this
    //     code path is left untouched. The host dispatcher supplies the
    //     store + run id (and any resume checkpoint); a dispatcher without a
    //     store leaves the agent running as ordinary (non-durable).
    if agent.durable.is_some() {
        if let Some(handles) = dispatcher.checkpointing() {
            run_options.checkpoint_store = Some(handles.store);
            run_options.run_id = Some(handles.run_id);
            run_options.resume_from = handles.resume;
            run_options.durable_granularity = Some(handles.checkpoint);
        }
    }

    // 7. Split initial_messages into history + initial_message.
    //    The kernel requires exactly one initial_message; if the caller
    //    provided none, synthesise a placeholder so the run loop has
    //    something to send to the LLM.
    let (history, initial_message) = split_history(initial_messages).unwrap_or_else(|| {
        // Use the no_std-safe constructor so this path compiles without the
        // `std` feature (required for the wasm guest that calls run_ir_streaming
        // with `initial_messages = Vec::new()`).
        let clock = run_options.clock.as_ref().expect("clock checked above");
        let random = run_options.random.as_ref().expect("random checked above");
        let placeholder = Message::new_with(
            crate::ids::message_id(clock, random),
            crate::ids::now_utc(clock),
            Address::User,
            Address::System,
            MessagePayload::Text {
                content: String::new(),
            },
        );
        (Vec::new(), placeholder)
    });

    // 8. Hand the constructed pieces back to the caller, which drives the
    //    kernel agent loop (batch via `run_with_history`, streaming via
    //    `run_streaming_with_history`).
    Ok((
        rt,
        agent_def,
        manifest,
        history,
        initial_message,
        run_options,
    ))
}

/// Execute one `Agent` node end-to-end through the existing kernel agent loop.
///
/// Constructs a `Runtime` from the dispatcher's LLM backend + the agent's
/// tool refs (each wrapped as a thin dispatcher-delegating `Tool`), then
/// calls `run_with_history` with a synthesised `AgentDefinition` and
/// `PackageManifest`. Construction is shared with [`run_agent_streaming`] via
/// [`prepare_agent_run`] so the batch and streaming paths cannot drift.
pub async fn run_agent<D>(
    module: alloc::sync::Arc<IrModule>,
    agent: &Agent,
    dispatcher: Arc<D>,
    initial_messages: Vec<Message>,
) -> Result<RunOutcome, RuntimeError>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    let (rt, agent_def, manifest, history, initial_message, run_options) =
        prepare_agent_run(module, agent, dispatcher, initial_messages, alloc::vec![])?;
    rt.run_with_history(agent_def, manifest, history, initial_message, run_options)
        .await
}

/// Execute one `Agent` node with additional per-kind spawn tools registered
/// into its tool registry, alongside its ordinary `tool_refs`.
///
/// EPIC 4.5: the sole caller is `pipeline.rs`'s `StepRun::Dynamic` arm, which
/// runs the region's coordinator agent with one [`crate::interpreter::dynamic::SpawnTool`]
/// per offered kind. Construction is shared with [`run_agent`] via
/// [`prepare_agent_run`] so the two entries cannot drift on backend wiring,
/// `AgentDefinition` synthesis, or `RunOptions` injection — only the tool
/// registry differs.
pub(crate) async fn run_agent_with_spawn_tools<D>(
    module: alloc::sync::Arc<IrModule>,
    agent: &Agent,
    dispatcher: Arc<D>,
    initial_messages: Vec<Message>,
    spawn_tools: Vec<crate::interpreter::dynamic::SpawnTool<D>>,
) -> Result<RunOutcome, RuntimeError>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    let (rt, agent_def, manifest, history, initial_message, run_options) =
        prepare_agent_run(module, agent, dispatcher, initial_messages, spawn_tools)?;
    rt.run_with_history(agent_def, manifest, history, initial_message, run_options)
        .await
}

/// Execute one `Agent` node through the kernel agent loop, returning the
/// uncollapsed [`crate::stream::RunEvent`] stream instead of a collapsed
/// [`RunOutcome`].
///
/// Construction is shared with [`run_agent`] via [`prepare_agent_run`] so the
/// batch and streaming paths cannot drift. The conformance dev profile drives
/// this to observe the per-event run (TextDelta / ToolCall* / TurnCompleted /
/// RunCompleted) rather than the collapsed terminal outcome.
///
/// The returned stream is `'static`: `Runtime::run_streaming_with_history`
/// snapshots its tool registry and backend into Arc-clones that the stream
/// owns, so the `Runtime` built here may be dropped after this call returns.
pub async fn run_agent_streaming<D>(
    module: alloc::sync::Arc<IrModule>,
    agent: &Agent,
    dispatcher: Arc<D>,
    initial_messages: Vec<Message>,
) -> Result<impl futures_core::Stream<Item = crate::stream::RunEvent> + 'static, RuntimeError>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    let (rt, agent_def, manifest, history, initial_message, run_options) =
        prepare_agent_run(module, agent, dispatcher, initial_messages, alloc::vec![])?;
    rt.run_streaming_with_history(agent_def, manifest, history, initial_message, run_options)
        .await
}

/// Split a `Vec<Message>` into `(history, last)`, returning `None` when empty.
fn split_history(mut messages: Vec<Message>) -> Option<(Vec<Message>, Message)> {
    if messages.is_empty() {
        return None;
    }
    let last = messages.pop().expect("non-empty checked above");
    Some((messages, last))
}

#[cfg(test)]
mod tests {
    //! Body-shape extraction regressions for `DispatcherTool::invoke`.
    //!
    //! These tests cover the inverse direction of the `ForwardingDispatcher`
    //! body-shape lift (see `crates/tau-cli/src/cmd/ir_dispatcher.rs`):
    //! plain text must round-trip as raw text (no extra JSON quoting),
    //! and structured shapes must serialise as compact JSON.
    use super::*;
    use alloc::boxed::Box;
    use alloc::string::ToString;
    use alloc::sync::Arc;
    use core::future::Future;
    use core::pin::Pin;
    use serde_json::Value;
    use tau_ir::ToolId;
    use tau_ports::fixtures::MockLlmBackend;
    use tau_ports::tool::Tool;
    use tokio as _; // exercised via #[tokio::test]

    use crate::builder::DynLlmBackend;
    use crate::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};

    /// Minimal `ToolDispatcher` that returns a fixed `ToolInvocationResult`
    /// on every `invoke`. Used to drive `DispatcherTool::invoke` in
    /// isolation so we can assert how it extracts text from `body`.
    struct FixedDispatcher {
        backend: Arc<dyn DynLlmBackend>,
        body: Option<Value>,
        error: Option<alloc::string::String>,
    }

    impl ToolDispatcher for FixedDispatcher {
        fn invoke<'a>(
            &'a self,
            _tool_id: &'a ToolId,
            _args: &'a Value,
        ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>>
        {
            let body = self.body.clone();
            let error = self.error.clone();
            Box::pin(async move { Ok(ToolInvocationResult { body, error }) })
        }

        fn llm_backend_for(&self, _backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
            Ok(self.backend.clone())
        }
    }

    /// Build a `DispatcherTool` carrying the given dispatcher, ready for
    /// a direct `Tool::invoke` call. Spec name / ToolId are arbitrary —
    /// `FixedDispatcher` ignores them.
    fn make_dispatcher_tool<D>(dispatcher: Arc<D>) -> DispatcherTool<D>
    where
        D: ToolDispatcher + Send + Sync + 'static,
    {
        let spec = make_tool_spec(
            "fixed",
            "fixed-output tool for tests",
            &serde_json::json!({}),
        );
        // The existing test fixtures only exercise the Native/Mcp dispatch
        // path of `invoke`; a stub `ToolImpl::Native` keeps that branch
        // active. Subflow/Step dispatch is covered by tau-ir-conformance.
        let stub_impl = tau_ir::ToolImpl::Native {
            fn_ref: tau_ir::NativeFnRef {
                name: "stub".to_string(),
            },
            content_hash: [0u8; 32],
        };
        // Empty IrModule is fine — Native dispatch never reads it.
        let stub_module = alloc::sync::Arc::new(tau_ir::IrModule {
            ir_format: tau_ir::IrFormatVersion::current(),
            tau_version: env!("CARGO_PKG_VERSION").into(),
            target: tau_ports::target::registry::list_available()
                .next()
                .expect("at least one target")
                .triple,
            workflow: tau_ir::Workflow {
                agents: alloc::collections::BTreeMap::new(),
                tools: alloc::collections::BTreeMap::new(),
                steps: alloc::collections::BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: tau_ir::CapabilityTable(alloc::collections::BTreeMap::new()),
                pipeline: None,
                checks: alloc::collections::BTreeMap::new(),
            },
            triggers: alloc::vec::Vec::new(),
        });
        DispatcherTool {
            tool_name: "fixed".to_string(),
            tool_id: ToolId("fixed".to_string()),
            spec,
            module: stub_module,
            tool_impl: stub_impl,
            dispatcher,
        }
    }

    #[tokio::test]
    async fn dispatcher_tool_extracts_raw_string_for_value_string_body() {
        // Body shape: `Value::String("hello")` — what `ForwardingDispatcher`
        // produces when the plugin tool emits plain text and the
        // try-parse-as-JSON path falls back to `Value::String`.
        let backend: Arc<dyn DynLlmBackend> = Arc::new(MockLlmBackend::new("mock-llm"));
        let dispatcher = Arc::new(FixedDispatcher {
            backend,
            body: Some(Value::String("hello".to_string())),
            error: None,
        });
        let tool = make_dispatcher_tool(dispatcher);

        let res = tool
            .invoke(&mut (), tau_domain::Value::Null)
            .await
            .expect("invoke must succeed");

        // Exactly one Text block containing the raw string — NOT the
        // JSON-quoted `"hello"` that `Display` on `Value::String` would
        // produce.
        assert_eq!(res.content.len(), 1);
        match &res.content[0] {
            tau_ports::tool::ToolContent::Text { text } => {
                assert_eq!(text, "hello");
            }
            other => panic!("expected Text content, got {other:?}"),
        }
        assert!(!res.is_error);
    }

    #[tokio::test]
    async fn dispatcher_tool_extracts_compact_json_for_value_object_body() {
        // Body shape: `Value::Object({"temp": 22})` — what
        // `ForwardingDispatcher` produces when the plugin tool emits a
        // JSON-shaped text payload and the try-parse path lifts it.
        let backend: Arc<dyn DynLlmBackend> = Arc::new(MockLlmBackend::new("mock-llm"));
        let dispatcher = Arc::new(FixedDispatcher {
            backend,
            body: Some(serde_json::json!({"temp": 22})),
            error: None,
        });
        let tool = make_dispatcher_tool(dispatcher);

        let res = tool
            .invoke(&mut (), tau_domain::Value::Null)
            .await
            .expect("invoke must succeed");

        assert_eq!(res.content.len(), 1);
        match &res.content[0] {
            tau_ports::tool::ToolContent::Text { text } => {
                // `Display` on `Value::Object` emits compact JSON,
                // matching what the LLM expects for structured tool
                // output.
                assert_eq!(text, r#"{"temp":22}"#);
            }
            other => panic!("expected Text content, got {other:?}"),
        }
        assert!(!res.is_error);
    }

    #[tokio::test]
    async fn dispatcher_tool_empty_text_for_none_body() {
        // Body shape: `None` — well-defined success with no payload.
        let backend: Arc<dyn DynLlmBackend> = Arc::new(MockLlmBackend::new("mock-llm"));
        let dispatcher = Arc::new(FixedDispatcher {
            backend,
            body: None,
            error: None,
        });
        let tool = make_dispatcher_tool(dispatcher);

        let res = tool
            .invoke(&mut (), tau_domain::Value::Null)
            .await
            .expect("invoke must succeed");

        match &res.content[0] {
            tau_ports::tool::ToolContent::Text { text } => assert!(text.is_empty()),
            other => panic!("expected Text content, got {other:?}"),
        }
        assert!(!res.is_error);
    }

    /// TDD assertion for Task 11: the `CompletionRequest` the LLM backend
    /// receives must carry the IR `Agent.model_ref.model_id`, NOT the
    /// backend package name (`model_ref.backend`).
    ///
    /// Wiring path: `prepare_agent_run` calls
    /// `AgentDefinition::with_model(agent.model_ref.model_id.clone())` →
    /// `stream.rs` builds
    /// `CompletionRequest::new(agent_def.model.clone().into())`.
    #[cfg(feature = "test-fixtures")]
    #[tokio::test]
    async fn completion_request_carries_model_id_not_backend_name() {
        use tau_ports::fixtures::{make_completion_response, MockLlmBackend};
        use tau_ports::llm::StopReason;

        // MockLlmBackend named "my-backend"; the IR model_id is "claude-haiku-4-5".
        // After wiring, the CompletionRequest the backend sees must have
        // model == "claude-haiku-4-5", not "my-backend".
        let backend = alloc::sync::Arc::new(MockLlmBackend::new("my-backend").with_response(
            make_completion_response("done".to_string(), alloc::vec![], StopReason::EndTurn, None),
        ));

        struct SingleBackendDispatcher {
            backend: alloc::sync::Arc<MockLlmBackend>,
        }

        impl ToolDispatcher for SingleBackendDispatcher {
            fn invoke<'a>(
                &'a self,
                _tool_id: &'a ToolId,
                _args: &'a Value,
            ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>>
            {
                Box::pin(async move {
                    Ok(ToolInvocationResult {
                        body: None,
                        error: None,
                    })
                })
            }

            fn llm_backend_for(
                &self,
                _backend: &str,
            ) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
                Ok(self.backend.clone())
            }
        }

        let dispatcher = alloc::sync::Arc::new(SingleBackendDispatcher {
            backend: backend.clone(),
        });

        let module = alloc::sync::Arc::new(tau_ir::IrModule {
            ir_format: tau_ir::IrFormatVersion::current(),
            tau_version: env!("CARGO_PKG_VERSION").into(),
            target: tau_ports::target::registry::list_available()
                .next()
                .expect("at least one target")
                .triple,
            workflow: tau_ir::Workflow {
                agents: alloc::collections::BTreeMap::new(),
                tools: alloc::collections::BTreeMap::new(),
                steps: alloc::collections::BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: tau_ir::CapabilityTable(alloc::collections::BTreeMap::new()),
                pipeline: None,
                checks: alloc::collections::BTreeMap::new(),
            },
            triggers: alloc::vec::Vec::new(),
        });

        let agent = tau_ir::node::Agent {
            id: tau_ir::ids::AgentId("test-agent".into()),
            prompt: tau_ir::prompt::PromptSource::inline(""),
            model_ref: tau_ir::model_ref::ModelRef {
                backend: "my-backend".into(),
                model_id: "claude-haiku-4-5".into(),
            },
            tool_refs: alloc::vec::Vec::new(),
            context: None,
            budget: tau_ir::budget::AgentBudget {
                max_turns: Some(1),
                max_tokens: None,
            },
            produces: alloc::vec::Vec::new(),
            output_schema: None,
            durable: None,
        };

        let user_msg = tau_domain::Message::new(
            tau_domain::Address::User,
            tau_domain::Address::Agent(tau_domain::AgentInstanceId::new()),
            tau_domain::MessagePayload::Text {
                content: "hello".into(),
            },
        );
        run_agent(module, &agent, dispatcher, alloc::vec![user_msg])
            .await
            .expect("run_agent must succeed");

        let invocations = backend.invocations();
        assert_eq!(invocations.len(), 1, "expected exactly one LLM call");
        assert_eq!(
            invocations[0].model, "claude-haiku-4-5",
            "CompletionRequest.model must be the model_id, not the backend package name"
        );
    }

    #[tokio::test]
    async fn asset_prompt_resolves_from_dispatcher_asset_store() {
        use tau_ports::fixtures::{make_completion_response, MockLlmBackend};
        use tau_ports::llm::StopReason;

        // D6-B: an agent whose prompt is a `PromptSource::Asset` must run with
        // the asset store's bytes as its system prompt — this is the run-side
        // half of the `system_file` fix (the other half being lowering emitting
        // the asset). Prove the resolved system prompt is the blob CONTENT.
        const CONTENT: &str = "You are a careful assistant.";
        let hash = tau_ir::asset::asset_hash(CONTENT.as_bytes());

        let backend = alloc::sync::Arc::new(MockLlmBackend::new("my-backend").with_response(
            make_completion_response("done".to_string(), alloc::vec![], StopReason::EndTurn, None),
        ));

        struct AssetDispatcher {
            backend: alloc::sync::Arc<MockLlmBackend>,
            assets:
                Arc<alloc::collections::BTreeMap<alloc::string::String, tau_ir::asset::AssetBlob>>,
        }
        impl ToolDispatcher for AssetDispatcher {
            fn invoke<'a>(
                &'a self,
                _tool_id: &'a ToolId,
                _args: &'a Value,
            ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>>
            {
                Box::pin(async move {
                    Ok(ToolInvocationResult {
                        body: None,
                        error: None,
                    })
                })
            }
            fn llm_backend_for(
                &self,
                _backend: &str,
            ) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
                Ok(self.backend.clone())
            }
            fn assets(
                &self,
            ) -> Option<
                Arc<alloc::collections::BTreeMap<alloc::string::String, tau_ir::asset::AssetBlob>>,
            > {
                Some(self.assets.clone())
            }
        }

        let mut asset_map = alloc::collections::BTreeMap::new();
        asset_map.insert(
            hash.clone(),
            tau_ir::asset::AssetBlob::prompt(CONTENT.as_bytes().to_vec()),
        );
        let dispatcher = alloc::sync::Arc::new(AssetDispatcher {
            backend: backend.clone(),
            assets: Arc::new(asset_map),
        });

        let module = alloc::sync::Arc::new(tau_ir::IrModule {
            ir_format: tau_ir::IrFormatVersion::current(),
            tau_version: env!("CARGO_PKG_VERSION").into(),
            target: tau_ports::target::registry::list_available()
                .next()
                .expect("at least one target")
                .triple,
            workflow: tau_ir::Workflow {
                agents: alloc::collections::BTreeMap::new(),
                tools: alloc::collections::BTreeMap::new(),
                steps: alloc::collections::BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: tau_ir::CapabilityTable(alloc::collections::BTreeMap::new()),
                pipeline: None,
                checks: alloc::collections::BTreeMap::new(),
            },
            triggers: alloc::vec::Vec::new(),
        });

        let agent = tau_ir::node::Agent {
            id: tau_ir::ids::AgentId("test-agent".into()),
            prompt: tau_ir::prompt::PromptSource::asset(hash),
            model_ref: tau_ir::model_ref::ModelRef {
                backend: "my-backend".into(),
                model_id: "claude-haiku-4-5".into(),
            },
            tool_refs: alloc::vec::Vec::new(),
            context: None,
            budget: tau_ir::budget::AgentBudget {
                max_turns: Some(1),
                max_tokens: None,
            },
            produces: alloc::vec::Vec::new(),
            output_schema: None,
            durable: None,
        };

        let user_msg = tau_domain::Message::new(
            tau_domain::Address::User,
            tau_domain::Address::Agent(tau_domain::AgentInstanceId::new()),
            tau_domain::MessagePayload::Text {
                content: "hello".into(),
            },
        );
        run_agent(module, &agent, dispatcher, alloc::vec![user_msg])
            .await
            .expect("run_agent must succeed");

        let invocations = backend.invocations();
        assert_eq!(invocations.len(), 1, "expected exactly one LLM call");
        assert_eq!(
            invocations[0].system.as_deref(),
            Some(CONTENT),
            "an Asset prompt must resolve to the asset store's blob content"
        );
    }

    #[tokio::test]
    async fn dispatcher_tool_routes_error_field_to_is_error_true() {
        // Body shape: `error = Some("boom")` — the dispatcher signals a
        // tool-side error. `DispatcherTool` must surface it as an
        // `is_error: true` `ToolResult` with the error text.
        let backend: Arc<dyn DynLlmBackend> = Arc::new(MockLlmBackend::new("mock-llm"));
        let dispatcher = Arc::new(FixedDispatcher {
            backend,
            body: None,
            error: Some("boom".to_string()),
        });
        let tool = make_dispatcher_tool(dispatcher);

        let res = tool
            .invoke(&mut (), tau_domain::Value::Null)
            .await
            .expect("invoke must succeed");

        assert!(res.is_error);
        match &res.content[0] {
            tau_ports::tool::ToolContent::Text { text } => assert_eq!(text, "boom"),
            other => panic!("expected Text content, got {other:?}"),
        }
    }
}
