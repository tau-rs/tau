//! Integration test for `run_pipeline`'s `StepRun::Check` dispatch
//! (Task 19 — abort path, no retry yet).
//!
//! Builds a pipeline `[agent:writer, check:g]` where `g` is a goal
//! (`NonEmpty` on `steps.writer.output`) and proves:
//! - a passing goal lets the pipeline complete (`Ok(store)` with the
//!   writer output present),
//! - a failing goal aborts with `RuntimeError::CheckFailed { id, .. }`.
//!
//! The dispatcher returns BOTH an `InMemoryArtifactReader` (so path loci
//! could be read) AND a `DeterministicRegistry` answering
//! `FN_BUILTIN_NON_EMPTY`. The writer agent runs on the same echo backend
//! the existing `pipeline_executor.rs` suite uses.

use std::boxed::Box;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{json, Value};

use tau_ir::budget::AgentBudget;
use tau_ir::capability::CapabilityTable;
use tau_ir::check::{Check, CheckVerify, GoalPredicate, Locus, OnFail, RetryPolicy};
use tau_ir::ids::{AgentId, CheckId, PipelineStepId};
use tau_ir::module::{IrFormatVersion, IrModule, Workflow};
use tau_ir::node::Agent;
use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};
use tau_ports::{
    CompletionRequest, CompletionResponse, ContentBlock, LlmBackend, LlmError, LlmProviderMessage,
};

use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::artifact::{ArtifactReader, InMemoryArtifactReader};
use tau_runtime_core::interpreter::deterministic::DeterministicRegistry;
use tau_runtime_core::interpreter::pipeline::run_pipeline;
use tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};
use tau_runtime_core::vocabulary::FN_BUILTIN_NON_EMPTY;

/// LLM backend that echoes the last user-message text back as its final
/// assistant text (mirrors `pipeline_executor.rs`'s `EchoBackend`).
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

/// Registry answering `FN_BUILTIN_NON_EMPTY` from the `{present, content}`
/// args object — `met = present && !content.is_empty()`.
struct NonEmptyRegistry;

impl DeterministicRegistry for NonEmptyRegistry {
    fn invoke(&self, fn_name: &str, args: &Value) -> Result<Value, RuntimeError> {
        if fn_name == FN_BUILTIN_NON_EMPTY {
            let present = args["present"].as_bool().unwrap_or(false);
            let content = args["content"].as_str().unwrap_or("");
            Ok(json!(present && !content.is_empty()))
        } else {
            Err(RuntimeError::Internal {
                message: format!("NonEmptyRegistry: unknown fn {fn_name}"),
            })
        }
    }
}

/// Dispatcher wiring the echo backend, a `NonEmptyRegistry`, and an
/// `InMemoryArtifactReader` into the interpreter.
struct CheckDispatcher {
    backend: Arc<dyn DynLlmBackend>,
    registry: Arc<NonEmptyRegistry>,
    reader: Arc<InMemoryArtifactReader>,
}

impl ToolDispatcher for CheckDispatcher {
    fn invoke<'a>(
        &'a self,
        _tool_id: &'a tau_ir::ToolId,
        _args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            Err(RuntimeError::Internal {
                message: "CheckDispatcher::invoke should never be called (no tools)".into(),
            })
        })
    }

    fn llm_backend(&self) -> Arc<dyn DynLlmBackend> {
        self.backend.clone()
    }

    fn deterministic_registry(&self) -> Option<Arc<dyn DeterministicRegistry>> {
        Some(self.registry.clone())
    }

    fn artifact_reader(&self) -> Option<Arc<dyn ArtifactReader>> {
        Some(self.reader.clone())
    }
}

/// Construct a `writer` agent with no tools, default budget.
fn writer_agent() -> Agent {
    Agent {
        id: AgentId("writer".into()),
        prompt: String::new(),
        model: "echo-model".into(),
        tool_refs: Vec::new(),
        context: None,
        budget: AgentBudget::default(),
        produces: Vec::new(),
        output_schema: None,
    }
}

/// Build the `[agent:writer, check:g]` module. The goal `g` evaluates the
/// given `locus` with `NonEmpty`. The writer step renders `input`, so
/// passing "SEED" produces a non-empty output; passing "" produces empty.
fn build_module() -> IrModule {
    let mut agents = BTreeMap::new();
    agents.insert(AgentId("writer".into()), writer_agent());

    let mut checks = BTreeMap::new();
    checks.insert(
        CheckId("g".into()),
        Check {
            id: CheckId("g".into()),
            verify: CheckVerify::Goal {
                evaluates: Locus::Output(PipelineStepId("writer".into())),
                predicate: GoalPredicate::NonEmpty,
            },
            retry: RetryPolicy {
                on_fail: OnFail::Abort,
                max_attempts: 1,
                gate: PipelineStepId("writer".into()),
            },
        },
    );

    let pipeline = Pipeline {
        steps: vec![
            PipelineStep {
                id: PipelineStepId("writer".into()),
                run: StepRun::Agent(AgentId("writer".into())),
                input: "${input}".into(),
            },
            PipelineStep {
                id: PipelineStepId("g".into()),
                run: StepRun::Check(CheckId("g".into())),
                input: String::new(),
            },
        ],
    };

    let target = tau_ports::target::registry::list_available()
        .next()
        .expect("at least one available target")
        .triple;

    IrModule {
        ir_format: IrFormatVersion::current(),
        tau_version: env!("CARGO_PKG_VERSION").into(),
        target,
        workflow: Workflow {
            agents,
            tools: BTreeMap::new(),
            steps: BTreeMap::new(),
            edges: Vec::new(),
            capability_table: CapabilityTable(BTreeMap::new()),
            pipeline: Some(pipeline),
            checks,
        },
        triggers: Vec::new(),
    }
}

fn dispatcher() -> Arc<CheckDispatcher> {
    let backend: Arc<dyn DynLlmBackend> = Arc::new(EchoBackend);
    Arc::new(CheckDispatcher {
        backend,
        registry: Arc::new(NonEmptyRegistry),
        reader: Arc::new(InMemoryArtifactReader::new()),
    })
}

#[tokio::test]
async fn check_step_passing_goal_completes() {
    let module = build_module();

    let store = run_pipeline(Arc::new(module), "SEED".to_string(), dispatcher())
        .await
        .expect("pipeline runs to completion through the passing check");

    // The writer step's output is recorded; the check stores nothing.
    assert_eq!(
        store.get("writer").and_then(Value::as_str),
        Some("SEED"),
        "writer output should be present after a passing check"
    );
    assert!(
        store.get("g").is_none(),
        "a check step must store no output"
    );
}

#[tokio::test]
async fn check_step_failing_goal_aborts() {
    let module = build_module();

    // Empty run input -> writer echoes "" -> NonEmpty goal fails.
    let err = run_pipeline(Arc::new(module), String::new(), dispatcher())
        .await
        .expect_err("a failing goal must abort the pipeline");

    match err {
        RuntimeError::CheckFailed { id, kind, .. } => {
            assert_eq!(id, "g");
            assert_eq!(kind, "goal");
        }
        other => panic!("expected RuntimeError::CheckFailed, got: {other:?}"),
    }
}
