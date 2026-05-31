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

    /// Return the LLM backend this dispatcher is wired to.
    ///
    /// The interpreter calls this once per agent-node execution to build
    /// a `RuntimeBuilder` for the inner agent loop. Implementors own the
    /// backend handle (typically an `Arc`-clone of the caller's backend).
    fn llm_backend(&self) -> Arc<dyn DynLlmBackend>;
}
