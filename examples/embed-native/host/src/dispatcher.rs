//! The Variant-B product's `ToolDispatcher`: executes tools in-process
//! and supplies real host ports (clock, random, LLM backend).
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;
use tau_ir::ToolId;
use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};

use crate::llm::{ScriptedLlmBackend, Turn};
use crate::ports::{HostRandom, SystemClock};

pub struct HostDispatcher {
    backend: Arc<dyn DynLlmBackend>,
    clock: Arc<SystemClock>,
    random: Arc<HostRandom>,
}

impl HostDispatcher {
    pub fn new() -> Self {
        // The workflow's LLM "reasoning" is scripted: call `echo`, then
        // reply "done". A real product returns its provider adapter here.
        let backend: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlmBackend::new(vec![
            Turn::ToolCall {
                id: "call-1".into(),
                name: "echo".into(),
                input: serde_json::from_value(serde_json::json!({"text": "hello"}))
                    .expect("echo input is a valid value"),
            },
            Turn::Text("done".into()),
        ]));
        Self {
            backend,
            clock: Arc::new(SystemClock),
            random: Arc::new(HostRandom::new()),
        }
    }
}

impl Default for HostDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolDispatcher for HostDispatcher {
    fn invoke<'a>(
        &'a self,
        tool_id: &'a ToolId,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        let tool = tool_id.0.clone();
        let args = args.clone();
        Box::pin(async move {
            match tool.as_str() {
                "echo" => {
                    let text = args.get("text").and_then(Value::as_str).unwrap_or("");
                    Ok(ToolInvocationResult {
                        body: Some(serde_json::json!({ "echoed": text })),
                        error: None,
                    })
                }
                other => Err(RuntimeError::Internal {
                    message: format!("host does not implement tool '{other}'"),
                }),
            }
        })
    }

    fn llm_backend_for(&self, _backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        // Single-backend host: every agent resolves to the scripted backend.
        Ok(self.backend.clone())
    }

    fn clock(&self) -> Option<Arc<dyn tau_ports::Clock>> {
        Some(self.clock.clone())
    }

    fn random(&self) -> Option<Arc<dyn tau_ports::RandomSource>> {
        Some(self.random.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_ir::ToolId;

    #[tokio::test]
    async fn invoke_echo_returns_echoed_text() {
        let d = HostDispatcher::new();
        let args = serde_json::json!({"text": "hello"});
        let out = d.invoke(&ToolId("echo".into()), &args).await.unwrap();
        assert_eq!(out.body.unwrap(), serde_json::json!({"echoed": "hello"}));
        assert!(out.error.is_none());
    }

    #[tokio::test]
    async fn invoke_unknown_tool_errors() {
        let d = HostDispatcher::new();
        let args = serde_json::json!({});
        let res = d.invoke(&ToolId("nope".into()), &args).await;
        assert!(res.is_err());
    }
}
