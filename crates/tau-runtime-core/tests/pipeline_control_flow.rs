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
use std::sync::{Arc, Mutex};

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
use tau_runtime_core::vocabulary::{FN_BUILTIN_EQUALS, FN_BUILTIN_MATCHES};

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

// -----------------------------------------------------------------------
// `StepRun::Loop` — bounded walk, hard exhaustion, feedback threading
// (EPIC 4.2, Task 4).
//
// Module shape:
//   refine = Loop max_iters:
//              body:  [ improve = Agent(counting) input "${input}" ]
//              until: Output(improve) Matches "APPROVED"
//
// `CountingBackend` returns `"APPROVED"` only on its `approves_on`-th
// invocation (else `"nope <n>"`), so the loop converges on that iteration.
// It also records every user-turn text it sees per invocation number, so
// the feedback-threading test can assert the 2nd invocation's prompt
// carried the 1st iteration's rejection rationale.
// -----------------------------------------------------------------------

/// LLM backend that approves (`"APPROVED"`) on exactly its `approves_on`-th
/// call and echoes `"nope <n>"` otherwise. Records the joined user-turn
/// texts of every invocation, keyed by 1-based call number, so tests can
/// inspect what a given iteration's agent actually saw.
struct CountingBackend {
    count: Mutex<usize>,
    approves_on: usize,
    turns: Mutex<BTreeMap<usize, Vec<String>>>,
}

impl CountingBackend {
    fn new(approves_on: usize) -> Self {
        Self {
            count: Mutex::new(0),
            approves_on,
            turns: Mutex::new(BTreeMap::new()),
        }
    }

    /// The joined text of each user turn the backend saw on its `call`-th
    /// (1-based) invocation, or empty if that invocation never happened.
    fn turns_for(&self, call: usize) -> Vec<String> {
        self.turns
            .lock()
            .unwrap()
            .get(&call)
            .cloned()
            .unwrap_or_default()
    }

    fn respond(&self, req: &CompletionRequest) -> CompletionResponse {
        let n = {
            let mut count = self.count.lock().unwrap();
            *count += 1;
            *count
        };

        let user_turns: Vec<String> = req
            .messages
            .iter()
            .filter_map(|m| match m {
                LlmProviderMessage::User { content } => Some(
                    content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text(t) => Some(t.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                _ => None,
            })
            .collect();
        self.turns.lock().unwrap().insert(n, user_turns);

        let text = if n == self.approves_on {
            "APPROVED".to_string()
        } else {
            format!("nope {n}")
        };

        serde_json::from_value(serde_json::json!({
            "text": text,
            "tool_uses": [],
            "stop_reason": "EndTurn",
            "usage": null,
        }))
        .expect("canned CompletionResponse deserializes")
    }
}

impl LlmBackend for CountingBackend {
    fn name(&self) -> &str {
        "counting-llm"
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        Ok(self.respond(&req))
    }

    async fn stream(
        &self,
        req: CompletionRequest,
    ) -> Result<tau_ports::CompletionStream, LlmError> {
        Ok(tau_ports::batch_to_stream(self.respond(&req)))
    }
}

/// Registry answering `FN_BUILTIN_MATCHES` from the `{present, content,
/// pattern}` args object — `met = present && content == Some(pattern)`.
/// A literal-equality match suffices for these tests' fixed strings
/// (`"APPROVED"`); a real `Matches` predicate would regex-match, but the
/// interpreter only cares that the registry answers the fn, not how.
struct MatchesRegistry;

impl DeterministicRegistry for MatchesRegistry {
    fn invoke(&self, fn_name: &str, args: &Value) -> Result<Value, RuntimeError> {
        if fn_name == FN_BUILTIN_MATCHES {
            let present = args["present"].as_bool().unwrap_or(false);
            let content = args["content"].as_str();
            let pattern = args["pattern"].as_str();
            Ok(json!(present && content.is_some() && content == pattern))
        } else {
            Err(RuntimeError::Internal {
                message: format!("MatchesRegistry: unknown fn {fn_name}"),
            })
        }
    }
}

/// Dispatcher wiring a `CountingBackend` and a `MatchesRegistry` into the
/// interpreter. Exposes `backend` so feedback-threading tests can inspect
/// recorded turns after the run completes.
struct LoopDispatcher {
    backend: Arc<CountingBackend>,
    registry: Arc<MatchesRegistry>,
}

impl ToolDispatcher for LoopDispatcher {
    fn invoke<'a>(
        &'a self,
        _tool_id: &'a tau_ir::ToolId,
        _args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            Err(RuntimeError::Internal {
                message: "LoopDispatcher::invoke should never be called (no tools)".into(),
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

fn counting_dispatcher(approves_on: usize) -> Arc<LoopDispatcher> {
    Arc::new(LoopDispatcher {
        backend: Arc::new(CountingBackend::new(approves_on)),
        registry: Arc::new(MatchesRegistry),
    })
}

/// Build a `[loop:refine]` module: `refine` loops `improve = Agent(improve)
/// input "${input}"` up to `max_iters` times, exiting when `Output(improve)
/// Matches "APPROVED"` holds.
fn loop_module(max_iters: u64, approves_on: usize) -> IrModule {
    let _ = approves_on; // encoded in the dispatcher's backend, not the module
    let mut agents = BTreeMap::new();
    agents.insert(AgentId("improve".into()), agent("improve"));

    let pipeline = Pipeline {
        steps: vec![PipelineStep {
            id: PipelineStepId("refine".into()),
            run: StepRun::Loop {
                body: vec![PipelineStep {
                    id: PipelineStepId("improve".into()),
                    run: StepRun::Agent(AgentId("improve".into())),
                    input: "${input}".into(),
                }],
                until: Condition {
                    evaluates: Locus::Output(PipelineStepId("improve".into())),
                    predicate: GoalPredicate::Matches("APPROVED".into()),
                },
                max_iters,
            },
            input: String::new(),
        }],
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
            checks: BTreeMap::new(),
        },
        triggers: Vec::new(),
    }
}

#[tokio::test]
async fn loop_converges_and_stores_last_body_output() {
    // body: improve = Agent(counting) input "${input}"; approves on call #2
    // until: Output(improve) Matches "APPROVED"; max_iters 3
    let module = loop_module(3, 2);
    let store = run_pipeline(
        Arc::new(module),
        "draft".to_string(),
        counting_dispatcher(2),
    )
    .await
    .expect("converges");
    assert_eq!(
        store.get("improve").unwrap(),
        &serde_json::json!("APPROVED")
    );
}

#[tokio::test]
async fn loop_exhausts_hard_errors() {
    // approves_on = 99 (never within max_iters 3)
    let module = loop_module(3, 99);
    let err = run_pipeline(
        Arc::new(module),
        "draft".to_string(),
        counting_dispatcher(99),
    )
    .await
    .expect_err("must exhaust");
    match err {
        RuntimeError::LoopExhausted { step, max_iters } => {
            assert_eq!(step, "refine");
            assert_eq!(max_iters, 3);
        }
        other => panic!("expected LoopExhausted, got {other:?}"),
    }
}

#[tokio::test]
async fn loop_threads_until_feedback_into_next_iteration() {
    let module = loop_module(3, 2);
    let dispatcher = counting_dispatcher(2);
    let store = run_pipeline(Arc::new(module), "draft".to_string(), dispatcher.clone())
        .await
        .expect("converges");
    assert_eq!(
        store.get("improve").unwrap(),
        &serde_json::json!("APPROVED")
    );

    // The 2nd invocation's prompt must carry the 1st iteration's rejection
    // rationale as a prior turn (the `until` verdict's rationale, injected
    // via `initial_feedback`).
    let seen = dispatcher.backend.turns_for(2);
    assert!(
        seen.iter()
            .any(|m| m.contains("Previous attempt rejected:")),
        "expected iteration 2's prompt to contain the rejection prefix, got: {seen:?}"
    );
}
