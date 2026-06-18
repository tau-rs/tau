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

/// Result of one tool invocation.
pub struct ToolInvocationResult {
    /// Successful body (None if the tool errored — see `error`).
    pub body: Option<Value>,
    /// Tool-side error (None on success).
    pub error: Option<String>,
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
    fn llm_backend_for(
        &self,
        backend: &str,
    ) -> Result<Arc<dyn DynLlmBackend>, RuntimeError>;

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
}
