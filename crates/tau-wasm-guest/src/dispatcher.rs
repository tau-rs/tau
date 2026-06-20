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
        Self { backend, clock, random }
    }
}

impl ToolDispatcher for GuestDispatcher {
    fn invoke<'a>(
        &'a self,
        tool_id: &'a ToolId,
        _args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>>
    {
        let name = tool_id.0.clone();
        Box::pin(async move {
            Err(RuntimeError::Internal {
                message: format!(
                    "tool `{name}` invoked but the wasm guest wires no tools (E2)"
                ),
            })
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
}
