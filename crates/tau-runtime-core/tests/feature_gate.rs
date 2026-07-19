//! Honesty test for the load-time IR feature-fit gate (PR2 Task 9).
//!
//! A module that walks an `IrFeature` the interpreter does not implement
//! (here: `StepRun::Branch`) must be rejected by `run_ir_streaming` AT
//! LOAD — before the entry-agent lookup, before the stream/pipeline is
//! built, and long before the mid-run `StepRun::Branch` arm in
//! `interpreter::pipeline` (which still returns `RuntimeError::Internal`
//! as defense-in-depth, but must now be unreachable from this entry
//! point). A plain single-agent module with no unsupported features must
//! still load and run to completion.
//!
//! Mirrors the `EchoBackend`/`EchoDispatcher` + inline `build_module`
//! pattern in `tests/run_ir_streaming.rs` (no shared `common` helper
//! exists yet for IR fixtures).
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
use tau_ir::check::{Condition, GoalPredicate, Locus};
use tau_ir::ids::{AgentId, PipelineStepId};
use tau_ir::module::{IrFormatVersion, IrModule, Workflow};
use tau_ir::node::Agent;
use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};
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

/// Dispatcher that hands the echo backend to the interpreter. Neither
/// fixture in this file carries tools, so `invoke` is never called.
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
        output_schema: None,
        durable: None,
    }
}

fn base_workflow(entry: &AgentId) -> (Workflow, EchoDispatcher) {
    let mut agents = BTreeMap::new();
    agents.insert(entry.clone(), agent(&entry.0));

    let workflow = Workflow {
        agents,
        tools: BTreeMap::new(),
        steps: BTreeMap::new(),
        edges: Vec::new(),
        capability_table: CapabilityTable(BTreeMap::new()),
        pipeline: None,
        checks: BTreeMap::new(),
    };

    let backend: Arc<dyn DynLlmBackend> = Arc::new(EchoBackend);
    let dispatcher = EchoDispatcher { backend };
    (workflow, dispatcher)
}

fn build_module(workflow: Workflow) -> IrModule {
    let target = tau_ports::target::registry::list_available()
        .next()
        .expect("at least one available target")
        .triple;

    IrModule {
        ir_format: IrFormatVersion::current(),
        tau_version: env!("CARGO_PKG_VERSION").into(),
        target,
        workflow,
        triggers: Vec::new(),
    }
}

/// A single-agent, no-tools, no-pipeline module: requires only
/// `IrFeature::Pipeline`-free features, all of which the interpreter
/// supports. Must load and run to completion.
#[tokio::test(flavor = "current_thread")]
async fn plain_single_agent_module_loads_and_runs() {
    let entry = AgentId("a".into());
    let (workflow, dispatcher) = base_workflow(&entry);
    let module = build_module(workflow);

    let stream = run_ir_streaming(Arc::new(module), &entry, Arc::new(dispatcher), Vec::new())
        .await
        .expect("a module with only supported features must load");
    let events: Vec<RunEvent> = Box::pin(stream).collect().await;
    assert!(
        matches!(events.last(), Some(RunEvent::RunCompleted { .. })),
        "stream must end with RunCompleted; got {:?}",
        events.last()
    );
}

/// A module whose pipeline contains a `StepRun::Branch` block requires
/// `IrFeature::Branch`, which the interpreter does not implement (yet).
/// `run_ir_streaming` must reject it with `UnsupportedFeature` before
/// ever reaching the mid-run `Internal` arm in `interpreter::pipeline`
/// (proving rejection happens AT LOAD, not mid-run).
#[tokio::test(flavor = "current_thread")]
async fn branch_module_rejected_at_load_not_mid_run() {
    let entry = AgentId("a".into());
    let (mut workflow, dispatcher) = base_workflow(&entry);

    let inner_step = PipelineStep {
        id: PipelineStepId("inner".into()),
        run: StepRun::Agent(entry.clone()),
        input: "${input}".into(),
    };
    let branch_step = PipelineStep {
        id: PipelineStepId("b".into()),
        run: StepRun::Branch {
            on: Condition {
                evaluates: Locus::Path("/flag".into()),
                predicate: GoalPredicate::Exists,
            },
            then: vec![inner_step],
            otherwise: vec![],
        },
        input: "${input}".into(),
    };
    workflow.pipeline = Some(Pipeline {
        steps: vec![branch_step],
    });

    let module = build_module(workflow);

    let result = run_ir_streaming(Arc::new(module), &entry, Arc::new(dispatcher), Vec::new()).await;

    match result {
        Err(RuntimeError::UnsupportedFeature { features }) => {
            assert!(
                features.iter().any(|f| f == "Branch"),
                "expected Branch among unsupported features, got {features:?}"
            );
        }
        Ok(_) => panic!("Branch module must be rejected at load, not accepted"),
        Err(other) => panic!("expected UnsupportedFeature, got a different error: {other}"),
    }
}
