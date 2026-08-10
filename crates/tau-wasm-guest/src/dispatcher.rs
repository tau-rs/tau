//! In-guest `ToolDispatcher` for the E2 cassette-only scenario: no tools,
//! a single host-backed LLM backend, and host-backed clock/random for
//! determinism.

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::sync::Arc;
use core::future::Future;
use core::pin::Pin;

use serde_json::Value;

use tau_ir::ToolId;
use tau_ports::{Clock, RandomSource};
use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};

pub struct GuestDispatcher {
    backend: Arc<dyn DynLlmBackend>,
    clock: Arc<dyn Clock>,
    random: Arc<dyn RandomSource>,
}

impl GuestDispatcher {
    pub fn new(
        backend: Arc<dyn DynLlmBackend>,
        clock: Arc<dyn Clock>,
        random: Arc<dyn RandomSource>,
    ) -> Self {
        Self {
            backend,
            clock,
            random,
        }
    }
}

impl ToolDispatcher for GuestDispatcher {
    fn invoke<'a>(
        &'a self,
        tool_id: &'a ToolId,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        let name = tool_id.0.clone();
        let args_owned = args.clone();
        Box::pin(async move {
            match tau_native_tools::invoke(&name, &args_owned) {
                Some(body) => Ok(ToolInvocationResult {
                    body: Some(body),
                    error: None,
                }),
                None => Err(RuntimeError::Internal {
                    message: format!("tau-wasm-guest: unknown native tool `{name}`"),
                }),
            }
        })
    }

    fn llm_backend_for(&self, _backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        Ok(self.backend.clone())
    }

    fn clock(&self) -> Option<Arc<dyn Clock>> {
        Some(self.clock.clone())
    }

    fn random(&self) -> Option<Arc<dyn RandomSource>> {
        Some(self.random.clone())
    }

    fn egress_host_mediated(&self) -> bool {
        // EPIC 3.4: on wasm, network egress goes through `wasi:http`, gated
        // host-side by the embedder's `EgressPolicy` (EPIC 3.3, built from the
        // same allow-bounded caps). The in-guest per-tool net check is then
        // redundant and diverges from the host authority, so it is skipped —
        // the host is the sole net-egress gate.
        true
    }
}
