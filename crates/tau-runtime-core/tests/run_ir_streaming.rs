//! `run_ir_streaming` yields the same logical run as `run_ir`, but as an
//! uncollapsed RunEvent stream ending in exactly one RunCompleted.
//!
//! No reusable `common` test helper exists for a no-tools single-agent IR
//! fixture + dispatcher, so this file builds the smallest inline fixture,
//! mirroring the `EchoBackend` / `EchoDispatcher` + `build_module` pattern
//! in `tests/pipeline_executor.rs` (here: a single agent, no tools, no
//! pipeline — `run_ir_streaming` drives the single-entry agent loop).
#![cfg(feature = "test-fixtures")]

use std::boxed::Box;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures_util::StreamExt as _;
use serde_json::Value;

use tau_ir::budget::AgentBudget;
use tau_ir::capability::CapabilityTable;
use tau_ir::ids::AgentId;
use tau_ir::module::{IrFormatVersion, IrModule, Workflow};
use tau_ir::node::Agent;
use tau_ports::{
    CompletionRequest, CompletionResponse, ContentBlock, LlmBackend, LlmError, LlmProviderMessage,
};

use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::run_ir_streaming;
use tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};
use tau_runtime_core::stream::RunEvent;

/// LLM backend that echoes the last user-message text back as its final
/// assistant text (no tool calls -> the agent loop completes in one turn).
struct EchoBackend;

impl EchoBackend {
    fn echo_response(req: &CompletionRequest) -> CompletionResponse {
        let text = req
            .messages
            .iter()
            .rev()
            .find_map(|m| match m {
                LlmProviderMessage::User { content } => {
                    let joined: String = content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text(t) => Some(t.as_str()),
                            _ => None,
                        })
                        .collect();
                    Some(joined)
                }
                _ => None,
            })
            .unwrap_or_default();

        // `CompletionResponse` is `#[non_exhaustive]`; build it via serde
        // (the codebase's sanctioned escape hatch).
        serde_json::from_value(serde_json::json!({
            "text": text,
            "tool_uses": [],
            "stop_reason": "EndTurn",
            "usage": null,
        }))
        .expect("canned CompletionResponse deserializes")
    }
}

impl LlmBackend for EchoBackend {
    fn name(&self) -> &str {
        "echo-llm"
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        Ok(Self::echo_response(&req))
    }

    async fn stream(
        &self,
        req: CompletionRequest,
    ) -> Result<tau_ports::CompletionStream, LlmError> {
        Ok(tau_ports::batch_to_stream(Self::echo_response(&req)))
    }
}

/// Dispatcher that hands the echo backend to the interpreter. The fixture
/// agent carries no tools, so `invoke` is never called.
struct EchoDispatcher {
    backend: Arc<dyn DynLlmBackend>,
}

impl ToolDispatcher for EchoDispatcher {
    fn invoke<'a>(
        &'a self,
        _tool_id: &'a tau_ir::ToolId,
        _args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            Err(RuntimeError::Internal {
                message: "EchoDispatcher::invoke should never be called (no tools)".into(),
            })
        })
    }

    fn llm_backend_for(&self, _backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        Ok(self.backend.clone())
    }
}

/// Construct an `Agent` node with the given id, no tools, default budget.
fn agent(id: &str) -> Agent {
    Agent {
        id: AgentId(id.into()),
        prompt: String::new(),
        model_ref: tau_ir::ModelRef {
            backend: "echo-llm".into(),
            model_id: "echo-model".into(),
        },
        tool_refs: Vec::new(),
        context: None,
        budget: AgentBudget::default(),
        produces: Vec::new(),
    }
}

/// Build a single-agent, no-tools, no-pipeline `IrModule` plus its entry
/// agent id and a dispatcher backed by the echo LLM.
fn single_agent_no_tools_fixture() -> (IrModule, AgentId, EchoDispatcher) {
    let entry = AgentId("a".into());
    let mut agents = BTreeMap::new();
    agents.insert(entry.clone(), agent("a"));

    let target = tau_ports::target::registry::list_available()
        .next()
        .expect("at least one available target")
        .triple;

    let module = IrModule {
        ir_format: IrFormatVersion::current(),
        tau_version: env!("CARGO_PKG_VERSION").into(),
        target,
        workflow: Workflow {
            agents,
            tools: BTreeMap::new(),
            steps: BTreeMap::new(),
            edges: Vec::new(),
            capability_table: CapabilityTable(BTreeMap::new()),
            pipeline: None,
            checks: BTreeMap::new(),
        },
        triggers: Vec::new(),
    };

    let backend: Arc<dyn DynLlmBackend> = Arc::new(EchoBackend);
    let dispatcher = EchoDispatcher { backend };

    (module, entry, dispatcher)
}

#[tokio::test(flavor = "current_thread")]
async fn run_ir_streaming_yields_run_completed_last() {
    let (module, entry, dispatcher) = single_agent_no_tools_fixture();
    let stream = run_ir_streaming(Arc::new(module), &entry, Arc::new(dispatcher), Vec::new())
        .await
        .expect("stream builds");
    let events: Vec<RunEvent> = Box::pin(stream).collect().await;
    assert!(
        matches!(events.last(), Some(RunEvent::RunCompleted { .. })),
        "stream must end with RunCompleted; got {:?}",
        events.last()
    );
}
