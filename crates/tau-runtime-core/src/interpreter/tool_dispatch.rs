//! Tool-dispatch trait — the boundary the interpreter calls through to
//! invoke a tool by id.
//!
//! `tau dev` provides an in-process implementation that maps the tool id
//! to a Rust callback. The bundle's wasm component provides an
//! implementation that routes through the host's `AmbientOpsGate`
//! (WASI imports + `tau.caps` custom-section enforcement per D-3).
//! The interpreter is identical in both modes.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use core::future::Future;
use core::pin::Pin;

use serde_json::Value;

use tau_ir::ToolId;

use crate::builder::DynLlmBackend;
use crate::error::RuntimeError;

/// Durable-execution handles supplied by the host (ADR-0053).
///
/// Returned from [`ToolDispatcher::checkpointing`]. When present AND the
/// agent declares `durable`, [`super::agent_loop`]'s `prepare_agent_run`
/// wires `store` + `run_id` (and any `resume` checkpoint) into the agent
/// loop's `RunOptions`. Bundling the three into one struct keeps the trait
/// to a single additive method.
pub struct DurableHandles {
    /// Where turn checkpoints are written/read.
    pub store: Arc<dyn tau_ports::CheckpointStore>,
    /// Run id used to key checkpoints and as the `--resume` handle.
    pub run_id: String,
    /// When resuming, the latest checkpoint to rehydrate from; `None` for a
    /// fresh durable run.
    pub resume: Option<tau_ports::TurnCheckpoint>,
    /// Host-resolved checkpoint granularity (EPIC 6.1). The host resolves the
    /// agent's `Durability` for its target and passes the concrete value here,
    /// so the core never resolves intent itself.
    pub checkpoint: tau_ir::durable::CheckpointGranularity,
}

/// Result of one tool invocation.
pub struct ToolInvocationResult {
    /// Successful body (None if the tool errored — see `error`).
    pub body: Option<Value>,
    /// Tool-side error (None on success).
    pub error: Option<String>,
}

/// Trace sink supplied by the host shell for an IR-interpreter run
/// (execution-trace TUI spec §13.3).
///
/// `spawn_root_agent` builds a full `RunState` for multi-agent runs; the
/// interpreter has no equivalent, so the host passes the two ingredients
/// the kernel's `TraceEvent` emit sites actually need — a run id and the
/// subscribers to fan out to — and `prepare_agent_run` assembles a
/// synthetic, orchestration-inert `RunState` around them.
///
/// Both fields are `Send + Sync`, so this crosses the dispatcher's
/// `D: Send + Sync` bound (an `Arc<RefCell<RunState>>` could not).
pub struct TraceSinkConfig {
    /// Run id stamped on every emitted `TraceEvent`; also the
    /// `.tau/runs/<run_id>.jsonl` filename the host writes.
    pub run_id: tau_ports::RunId,
    /// Sinks to fan events out to (JSONL writer, live TUI channel, …).
    pub subscribers: alloc::vec::Vec<Arc<dyn crate::orchestration::trace::TraceSubscriber>>,
}

/// Boundary the interpreter calls through to invoke tools and obtain
/// the LLM backend used for agent-loop construction.
pub trait ToolDispatcher {
    /// Invoke the tool identified by `tool_id` with `args`.
    fn invoke<'a>(
        &'a self,
        tool_id: &'a ToolId,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>>;

    /// Resolve the backend an agent/judge needs, by backend package name.
    ///
    /// The interpreter calls this once per agent-node execution to build
    /// a `RuntimeBuilder` for the inner agent loop. `backend` is the
    /// package name of the LLM backend (e.g. `"anthropic"`), taken from
    /// the IR `Agent`'s `model_ref.backend` field. Implementors should
    /// look up the named backend from their registry and return it, or
    /// surface a [`RuntimeError`] if the name is not registered.
    fn llm_backend_for(&self, backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError>;

    /// Optional handle to a deterministic-step registry.
    ///
    /// The interpreter calls this when an agent invokes a tool whose IR
    /// `ToolImpl` is `Step { id }`. Returning `None` is allowed and means
    /// "this dispatcher does not support deterministic steps" — invoking
    /// a `Step` tool against a `None` registry surfaces as a
    /// [`RuntimeError::Internal`] with a clear diagnostic.
    ///
    /// Production paths (e.g. `tau run --bundle`) currently return
    /// `None`; the deterministic-registry surface ships first with the
    /// conformance test runner in `tau-ir-conformance` and graduates to
    /// production once a real native-fn registry is wired (β.7+).
    fn deterministic_registry(
        &self,
    ) -> Option<Arc<dyn super::deterministic::DeterministicRegistry>> {
        None
    }

    /// Optional host [`tau_ports::Clock`] for the inner agent loop.
    ///
    /// `run_agent` builds the inner agent loop's `RunOptions`; in a
    /// production (non-`test-fixtures`) build it has no way to mint a
    /// real wall-clock itself (the kernel is `no_std`). The host shell —
    /// which owns the executor and therefore a concrete `Clock` (e.g.
    /// `TokioClock`) — supplies it here. Returning `None` (the default)
    /// is only safe under the `test-fixtures` feature, where `run_agent`
    /// injects a `MockClock`; otherwise `run_agent` panics with a clear
    /// "host shell must supply a clock" diagnostic.
    fn clock(&self) -> Option<Arc<dyn tau_ports::Clock>> {
        None
    }

    /// Optional host [`tau_ports::RandomSource`] for the inner agent loop.
    ///
    /// See [`Self::clock`] — same host-injection contract, for the
    /// entropy source the agent loop uses to mint session ids / ULIDs.
    fn random(&self) -> Option<Arc<dyn tau_ports::RandomSource>> {
        None
    }

    /// Optional reader for produced artifacts (checks). Default: none.
    ///
    /// Returning `None` means "this dispatcher does not support artifact
    /// reading" — any check that needs to read a filesystem path will
    /// surface as a [`crate::error::RuntimeError::Internal`] with a
    /// clear diagnostic. The tokio host wires in a `std::fs`-backed
    /// reader; tests wire in [`super::artifact::InMemoryArtifactReader`].
    fn artifact_reader(&self) -> Option<Arc<dyn super::artifact::ArtifactReader>> {
        None
    }

    /// Optional content-addressed asset store (D6-B).
    ///
    /// Maps an asset hash (`"sha256:" + 64 hex`) to its [`tau_ir::asset::AssetBlob`].
    /// The interpreter calls this at agent-run assembly to resolve a
    /// [`tau_ir::prompt::PromptSource::Asset`] prompt reference to bytes.
    /// Returning `None` (the default) means "no asset store"; resolving an
    /// `Asset` prompt then surfaces a structured [`RuntimeError`]. Hosts that
    /// run bundles or `tau dev` supply the map (loaded from the bundle's
    /// `[[assets]]` store or the fresh lowering output).
    fn assets(
        &self,
    ) -> Option<Arc<alloc::collections::BTreeMap<alloc::string::String, tau_ir::asset::AssetBlob>>>
    {
        None
    }

    /// Optional registry of user-supplied native context nodes (β.4).
    ///
    /// The interpreter calls this when building the per-turn context
    /// pipeline for an agent whose IR config references a
    /// `ContextNodeKind::Custom` node. Returning `None` (the default)
    /// means "this dispatcher supplies no custom context nodes" — a
    /// config that references a custom node against a `None` registry
    /// surfaces as a [`crate::error::RuntimeError::Internal`] from
    /// [`crate::context::build_context_pipeline`]. Configs that use only
    /// builtins resolve regardless of the registry.
    fn context_transformer_registry(
        &self,
    ) -> Option<Arc<dyn crate::context::ContextTransformerRegistry>> {
        None
    }

    /// Optional durable-execution handles (ADR-0053).
    ///
    /// Returning `Some` enables turn-level checkpointing for agents that
    /// declare `durable` (the gating is in `prepare_agent_run`). The tokio
    /// host returns a `FileCheckpointStore` + the run id (and, on
    /// `tau run --resume`, the loaded checkpoint); dispatchers without a
    /// durable store return `None` (the default) and durable agents then run
    /// as ordinary non-durable agents.
    fn checkpointing(&self) -> Option<DurableHandles> {
        None
    }

    /// The meet-clamped authority a tool actually runs under, when narrower
    /// than its declared capabilities (execution-trace TUI spec §12/§13.2).
    ///
    /// Returning `None` (the default) means "not narrowed, or this
    /// dispatcher does not track authority". `tau-cli`'s
    /// `ForwardingDispatcher` answers from the `Arc<dyn DynTool>` it holds
    /// for each MCP-backed tool, whose effective set was computed at MCP
    /// open time by `setup_mcp_runtime`.
    ///
    /// This is **observability only**. The value is forwarded onto the
    /// interpreter's `DispatcherTool` wrapper via
    /// `Tool::effective_capabilities()` so the kernel can emit a
    /// `CapabilityVerdict::Clamp` on the call's `ToolCall` trace event.
    /// Declared capabilities are deliberately NOT forwarded — see issue
    /// #581 and `tests/ir_dispatch_gate_inert.rs`.
    fn tool_effective_capabilities(
        &self,
        tool_id: &ToolId,
    ) -> Option<alloc::vec::Vec<tau_domain::Capability>> {
        let _ = tool_id;
        None
    }

    /// Optional trace sink for this run (spec §13.3).
    ///
    /// Returning `Some` makes the kernel's `TraceEvent` emit sites live for
    /// an IR-interpreter run: `prepare_agent_run` builds a synthetic
    /// `RunState` from it and attaches it as `RunOptions::orchestration_state`.
    /// Returning `None` (the default) preserves today's behavior — no trace
    /// emission at all — which is what the wasm guest, `tau dev` and the
    /// conformance runner rely on.
    ///
    /// The synthetic state is orchestration-*inert*: it carries a default
    /// (unlimited) budget and no `orchestration_runtime`, so the budget
    /// watchdog no-ops and the virtual-tool intercept stays disabled
    /// (§13.4).
    fn trace_sink(&self) -> Option<TraceSinkConfig> {
        None
    }
}
