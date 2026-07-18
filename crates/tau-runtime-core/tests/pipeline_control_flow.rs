//! Integration tests for `run_pipeline`'s `StepRun::Branch` dispatch
//! (EPIC 4.2, Task 3).
//!
//! Builds a pipeline:
//!   seed  = Agent(echo)  input "${input}"
//!   gate  = Branch on Output(seed) Equals "GO":
//!             then:  [ hit  = Agent(echo) input "then-ran" ]
//!             else:  [ miss = Agent(echo) input "else-ran" ]
//!   after = Agent(echo) input "${steps.hit.output}"   (then-test only)
//!
//! and proves:
//! - a holding condition runs `then` (its steps' outputs land in the
//!   shared store, downstream steps can read them) and skips `otherwise`,
//! - a failing condition runs `otherwise` and skips `then`.
//!
//! The trailing `after` step is only present in the "then" module: it reads
//! `${steps.hit.output}`, which is unresolved whenever the else arm ran
//! instead (template resolution hard-errors on unresolved references), so
//! the "otherwise" test uses a variant without it — see `branch_module`'s
//! doc comment.
//!
//! The LLM backend is the same "echo" backend `pipeline_executor.rs` uses:
//! it returns the last user-message text as its final assistant text, so
//! each agent step's output equals its rendered input. The `Branch`
//! condition uses `GoalPredicate::Equals`, which is evaluated through a
//! `DeterministicRegistry` answering `FN_BUILTIN_EQUALS` (mirrors
//! `pipeline_check.rs`'s `NonEmptyRegistry`/`CheckDispatcher` pattern).

use std::boxed::Box;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{json, Value};

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
use tau_runtime_core::interpreter::deterministic::DeterministicRegistry;
use tau_runtime_core::interpreter::pipeline::run_pipeline;
use tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};
use tau_runtime_core::vocabulary::FN_BUILTIN_EQUALS;

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

/// Registry answering `FN_BUILTIN_EQUALS` from the `{present, content,
/// equals}` args object — `met = present && content == equals`.
struct EqualsRegistry;

impl DeterministicRegistry for EqualsRegistry {
    fn invoke(&self, fn_name: &str, args: &Value) -> Result<Value, RuntimeError> {
        if fn_name == FN_BUILTIN_EQUALS {
            let present = args["present"].as_bool().unwrap_or(false);
            let content = args["content"].as_str();
            let expected = args["equals"].as_str();
            Ok(json!(present && content.is_some() && content == expected))
        } else {
            Err(RuntimeError::Internal {
                message: format!("EqualsRegistry: unknown fn {fn_name}"),
            })
        }
    }
}

/// Dispatcher wiring the echo backend and an `EqualsRegistry` into the
/// interpreter. No agent in the test pipeline carries tools.
struct BranchDispatcher {
    backend: Arc<dyn DynLlmBackend>,
    registry: Arc<EqualsRegistry>,
}

impl ToolDispatcher for BranchDispatcher {
    fn invoke<'a>(
        &'a self,
        _tool_id: &'a tau_ir::ToolId,
        _args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            Err(RuntimeError::Internal {
                message: "BranchDispatcher::invoke should never be called (no tools)".into(),
            })
        })
    }

    fn llm_backend_for(&self, _backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        Ok(self.backend.clone())
    }

    fn deterministic_registry(&self) -> Option<Arc<dyn DeterministicRegistry>> {
        Some(self.registry.clone())
    }
}

fn dispatcher() -> Arc<BranchDispatcher> {
    let backend: Arc<dyn DynLlmBackend> = Arc::new(EchoBackend);
    Arc::new(BranchDispatcher {
        backend,
        registry: Arc::new(EqualsRegistry),
    })
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

/// Build the `[agent:seed, branch:gate, (agent:after)]` module described in
/// the module doc comment above. The branch condition checks
/// `Output(seed) Equals "GO"`.
///
/// `with_downstream` appends a trailing `after` step reading
/// `${steps.hit.output}` — used by the "then" test to prove a branch's
/// chosen-arm output is readable downstream. It is omitted for the
/// "otherwise" test: `${steps.hit.output}` is unresolved whenever `hit`
/// didn't run, and `tau_ir::template::resolve` hard-errors on unresolved
/// references (by design — see `TemplateError::Unresolved`), so a module
/// exercising the else-arm must not reference the not-taken arm's output.
fn branch_module(with_downstream: bool) -> IrModule {
    let mut agents = BTreeMap::new();
    agents.insert(AgentId("seed".into()), agent("seed"));
    agents.insert(AgentId("hit".into()), agent("hit"));
    agents.insert(AgentId("miss".into()), agent("miss"));
    agents.insert(AgentId("after".into()), agent("after"));

    let mut steps = vec![
        PipelineStep {
            id: PipelineStepId("seed".into()),
            run: StepRun::Agent(AgentId("seed".into())),
            input: "${input}".into(),
        },
        PipelineStep {
            id: PipelineStepId("gate".into()),
            run: StepRun::Branch {
                on: Condition {
                    evaluates: Locus::Output(PipelineStepId("seed".into())),
                    predicate: GoalPredicate::Equals("GO".into()),
                },
                then: vec![PipelineStep {
                    id: PipelineStepId("hit".into()),
                    run: StepRun::Agent(AgentId("hit".into())),
                    input: "then-ran".into(),
                }],
                otherwise: vec![PipelineStep {
                    id: PipelineStepId("miss".into()),
                    run: StepRun::Agent(AgentId("miss".into())),
                    input: "else-ran".into(),
                }],
            },
            input: String::new(),
        },
    ];
    if with_downstream {
        steps.push(PipelineStep {
            id: PipelineStepId("after".into()),
            run: StepRun::Agent(AgentId("after".into())),
            input: "${steps.hit.output}".into(),
        });
    }
    let pipeline = Pipeline { steps };

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
            checks: BTreeMap::new(),
        },
        triggers: Vec::new(),
    }
}

// A Branch whose condition holds runs `then`; its output is readable downstream.
#[tokio::test]
async fn branch_takes_then_when_condition_holds() {
    let module = branch_module(true);
    let store = run_pipeline(Arc::new(module), "GO".to_string(), dispatcher())
        .await
        .expect("runs");
    assert_eq!(store.get("hit").unwrap(), &serde_json::json!("then-ran"));
    assert!(store.get("miss").is_none());
    assert_eq!(store.get("after").unwrap(), &serde_json::json!("then-ran"));
}

#[tokio::test]
async fn branch_takes_otherwise_when_condition_fails() {
    let module = branch_module(false);
    let store = run_pipeline(Arc::new(module), "STOP".to_string(), dispatcher())
        .await
        .expect("runs");
    assert_eq!(store.get("miss").unwrap(), &serde_json::json!("else-ran"));
    assert!(store.get("hit").is_none());
}
