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
use tau_ports::fixtures::MockSuspensionStore;
use tau_ports::orchestration::SuspensionStore;
use tau_ports::{
    CompletionRequest, CompletionResponse, ContentBlock, LlmBackend, LlmError, LlmProviderMessage,
};

use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::deterministic::DeterministicRegistry;
use tau_runtime_core::interpreter::output_store::OutputStore;
use tau_runtime_core::interpreter::pipeline::{
    run_pipeline, run_pipeline_suspendable, PipelineOutcome, ResumeState, SuspendConfig,
};
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
        prompt: tau_ir::prompt::PromptSource::inline(""),
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

// -----------------------------------------------------------------------
// `StepRun::Parallel` — bounded cooperative fork-join (EPIC 4.2, Task 5).
//
// Module shape:
//   fanout = Parallel:
//              branch[0]: left  = Agent(echo) input "left-out"
//              branch[1]: right = Agent(echo) input "right-out"
//   after  = Agent(echo) input "${steps.left.output}|${steps.right.output}"
//
// Proves both branches' outputs land in the shared store (index-ordered
// merge) AND that a downstream step can read both.
// -----------------------------------------------------------------------

/// Build a `[parallel:fanout, agent:after]` module. `fanout` forks two
/// branches (`left`, `right`), each a single `Agent(echo)` step; `after`
/// reads both branches' outputs.
fn parallel_module() -> IrModule {
    let mut agents = BTreeMap::new();
    agents.insert(AgentId("left".into()), agent("left"));
    agents.insert(AgentId("right".into()), agent("right"));
    agents.insert(AgentId("after".into()), agent("after"));

    let pipeline = Pipeline {
        steps: vec![
            PipelineStep {
                id: PipelineStepId("fanout".into()),
                run: StepRun::Parallel {
                    branches: vec![
                        vec![PipelineStep {
                            id: PipelineStepId("left".into()),
                            run: StepRun::Agent(AgentId("left".into())),
                            input: "left-out".into(),
                        }],
                        vec![PipelineStep {
                            id: PipelineStepId("right".into()),
                            run: StepRun::Agent(AgentId("right".into())),
                            input: "right-out".into(),
                        }],
                    ],
                },
                input: String::new(),
            },
            PipelineStep {
                id: PipelineStepId("after".into()),
                run: StepRun::Agent(AgentId("after".into())),
                input: "${steps.left.output}|${steps.right.output}".into(),
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
            checks: BTreeMap::new(),
        },
        triggers: Vec::new(),
    }
}

#[tokio::test]
async fn parallel_runs_all_branches_and_merges_index_ordered() {
    let module = parallel_module();
    let store = run_pipeline(Arc::new(module), "x".to_string(), dispatcher())
        .await
        .expect("runs");
    assert_eq!(store.get("left").unwrap(), &serde_json::json!("left-out"));
    assert_eq!(store.get("right").unwrap(), &serde_json::json!("right-out"));
    assert_eq!(
        store.get("after").unwrap(),
        &serde_json::json!("left-out|right-out")
    );
}

// -----------------------------------------------------------------------
// `StepRun::Suspend` — loud abort (EPIC 4.2, Task 6). HITL checkpoint/resume
// lands in EPIC 4.3; 4.2 aborts loudly with a named error rather than
// silently skip.
//
// Also: Option-A end-to-end regression guard. A downstream top-level step
// must be able to read a nested `Branch` arm's step output by its BARE id
// (`${steps.inner.output}`) — the flat-global namespace is the whole point
// of Option A (see ADR-0058/0059 and Tasks 3-5).
// -----------------------------------------------------------------------

/// Build a `[suspend:pause]` module: a single `Suspend` step, id "pause",
/// `resume_signal` "go".
fn suspend_module() -> IrModule {
    let pipeline = Pipeline {
        steps: vec![PipelineStep {
            id: PipelineStepId("pause".into()),
            run: StepRun::Suspend {
                resume_signal: "go".into(),
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
            agents: BTreeMap::new(),
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

/// `run_pipeline` (the non-suspend wrapper) has no `SuspensionStore` wired,
/// so a `Suspend` step still surfaces as a named error rather than pausing —
/// only `run_pipeline_suspendable` can pause.
#[tokio::test]
async fn run_pipeline_errors_suspend_unsupported() {
    let module = suspend_module();
    let err = run_pipeline(Arc::new(module), "x".to_string(), dispatcher())
        .await
        .expect_err("suspend aborts on the non-suspend wrapper");
    match err {
        RuntimeError::SuspendUnsupported {
            step,
            resume_signal,
        } => {
            assert_eq!(step, "pause");
            assert_eq!(resume_signal, "go");
        }
        other => panic!("expected SuspendUnsupported, got {other:?}"),
    }
}

/// Build a `[agent:seed, suspend:pause]` module: `seed`'s output lands in
/// the store, then the run pauses at `pause` (id "pause", resume_signal
/// "go"). Proves a persisted suspension snapshot carries the pre-suspend
/// step's output.
fn seed_then_suspend_module() -> IrModule {
    let mut agents = BTreeMap::new();
    agents.insert(AgentId("seed".into()), agent("seed"));

    let pipeline = Pipeline {
        steps: vec![
            PipelineStep {
                id: PipelineStepId("seed".into()),
                run: StepRun::Agent(AgentId("seed".into())),
                input: "${input}".into(),
            },
            PipelineStep {
                id: PipelineStepId("pause".into()),
                run: StepRun::Suspend {
                    resume_signal: "go".into(),
                },
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
            checks: BTreeMap::new(),
        },
        triggers: Vec::new(),
    }
}

/// Suspending a pipeline persists a `PipelineSuspension` (with the
/// pre-suspend step's output in the snapshot) and returns
/// `PipelineOutcome::Suspended` rather than erroring.
#[tokio::test]
async fn suspend_persists_and_returns_suspended() {
    let module = Arc::new(seed_then_suspend_module());
    let store: Arc<dyn SuspensionStore> = Arc::new(MockSuspensionStore::new());
    let outcome = run_pipeline_suspendable(
        module.clone(),
        "x".to_string(),
        dispatcher(),
        SuspendConfig {
            run_id: "r1".into(),
            store: store.clone(),
        },
        None,
    )
    .await
    .expect("suspends cleanly");
    match outcome {
        PipelineOutcome::Suspended {
            run_id,
            resume_signal,
            step_id,
        } => {
            assert_eq!(run_id, "r1");
            assert_eq!(resume_signal, "go");
            assert_eq!(step_id, "pause");
        }
        other => panic!("expected Suspended, got {other:?}"),
    }
    // The seed step's output was persisted in the snapshot.
    let susp = store
        .load_suspension(&"r1".to_string())
        .unwrap()
        .expect("suspension was persisted");
    assert_eq!(susp.step_cursor, 1); // index of "pause"
    assert!(susp.outputs.contains_key("seed"));
}

/// Construct an `Agent` node whose `model_ref.backend` equals `id` itself —
/// `CountingDispatcher::llm_backend_for` counts calls keyed by its `backend`
/// argument, so this makes the per-agent call counter queryable by agent id.
fn counted_agent(id: &str) -> Agent {
    let mut a = agent(id);
    a.model_ref.backend = id.into();
    a
}

/// Dispatcher wiring the echo backend and counting `llm_backend_for`
/// invocations, keyed by the backend argument (which `counted_agent` sets
/// equal to the agent id). Proves the resume path does not re-run the
/// pre-suspend prefix: each agent step calls `llm_backend_for` exactly once
/// per pipeline run it actually executes in.
struct CountingDispatcher {
    backend: Arc<dyn DynLlmBackend>,
    calls: Mutex<BTreeMap<String, usize>>,
}

impl CountingDispatcher {
    /// Number of times `llm_backend_for` was called with `agent_id` as the
    /// backend argument (i.e. how many times that agent step ran).
    fn calls(&self, agent_id: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .get(agent_id)
            .copied()
            .unwrap_or(0)
    }
}

impl ToolDispatcher for CountingDispatcher {
    fn invoke<'a>(
        &'a self,
        _tool_id: &'a tau_ir::ToolId,
        _args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            Err(RuntimeError::Internal {
                message: "CountingDispatcher::invoke should never be called (no tools)".into(),
            })
        })
    }

    fn llm_backend_for(&self, backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        *self
            .calls
            .lock()
            .unwrap()
            .entry(backend.to_string())
            .or_insert(0) += 1;
        Ok(self.backend.clone())
    }
}

fn resume_counting_dispatcher() -> Arc<CountingDispatcher> {
    let backend: Arc<dyn DynLlmBackend> = Arc::new(EchoBackend);
    Arc::new(CountingDispatcher {
        backend,
        calls: Mutex::new(BTreeMap::new()),
    })
}

/// Build a `[agent:seed, suspend:pause, agent:tail]` module: `seed` runs,
/// the run pauses at `pause` (resume_signal "go"), and `tail` only runs
/// once resumed. Both agents' `model_ref.backend` equal their pipeline-step
/// id (via `counted_agent`) so `CountingDispatcher::calls` can query each
/// step's invocation count independently.
fn seed_suspend_tail_module() -> IrModule {
    let mut agents = BTreeMap::new();
    agents.insert(AgentId("seed".into()), counted_agent("seed"));
    agents.insert(AgentId("tail".into()), counted_agent("tail"));

    let pipeline = Pipeline {
        steps: vec![
            PipelineStep {
                id: PipelineStepId("seed".into()),
                run: StepRun::Agent(AgentId("seed".into())),
                input: "${input}".into(),
            },
            PipelineStep {
                id: PipelineStepId("pause".into()),
                run: StepRun::Suspend {
                    resume_signal: "go".into(),
                },
                input: String::new(),
            },
            PipelineStep {
                id: PipelineStepId("tail".into()),
                run: StepRun::Agent(AgentId("tail".into())),
                input: "tail-input".into(),
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
            checks: BTreeMap::new(),
        },
        triggers: Vec::new(),
    }
}

/// Resuming a suspended pipeline restores the persisted `OutputStore`
/// snapshot and continues at `step_cursor + 1` — the pre-suspend prefix
/// (`seed`) must NOT re-run, and the post-suspend tail (`tail`) must run
/// exactly once.
#[tokio::test]
async fn resume_continues_without_rerunning_prefix() {
    let module = Arc::new(seed_suspend_tail_module());
    let counting = resume_counting_dispatcher();
    let store: Arc<dyn SuspensionStore> = Arc::new(MockSuspensionStore::new());

    // Run 1: suspends after seed.
    let _ = run_pipeline_suspendable(
        module.clone(),
        "x".into(),
        counting.clone(),
        SuspendConfig {
            run_id: "r2".into(),
            store: store.clone(),
        },
        None,
    )
    .await
    .unwrap();
    let susp = store
        .load_suspension(&"r2".to_string())
        .unwrap()
        .expect("suspension was persisted");

    // Run 2: resume restores the store and continues at cursor+1.
    let outcome = run_pipeline_suspendable(
        module.clone(),
        "x".into(),
        counting.clone(),
        SuspendConfig {
            run_id: "r2".into(),
            store: store.clone(),
        },
        Some(ResumeState {
            store: OutputStore::restore(susp.outputs),
            start_at: susp.step_cursor + 1,
        }),
    )
    .await
    .unwrap();

    assert!(matches!(outcome, PipelineOutcome::Completed(_)));
    // seed ran exactly once (run 1 only); tail ran exactly once (run 2 only).
    assert_eq!(
        counting.calls("seed"),
        1,
        "prefix must NOT be re-run on resume"
    );
    assert_eq!(counting.calls("tail"), 1);
}

/// Build a `[agent:seed, branch:gate(inner), agent:tail]` module:
///   seed = Agent(echo) input "${input}"                    -> "GO"
///   gate = Branch on Output(seed) Equals "GO":
///            then: [ inner = Agent(echo) input "inner-out" ]
///            otherwise: []
///   tail = Agent(echo) input "${steps.inner.output}"
///
/// `tail` is a top-level pipeline step, not nested inside the branch, yet
/// it reads `inner`'s output by its bare id — proving the flat-global
/// namespace (Option A) resolves nested-block step outputs for downstream
/// top-level steps.
fn branch_then_read_module() -> IrModule {
    let mut agents = BTreeMap::new();
    agents.insert(AgentId("seed".into()), agent("seed"));
    agents.insert(AgentId("inner".into()), agent("inner"));
    agents.insert(AgentId("tail".into()), agent("tail"));

    let pipeline = Pipeline {
        steps: vec![
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
                        id: PipelineStepId("inner".into()),
                        run: StepRun::Agent(AgentId("inner".into())),
                        input: "inner-out".into(),
                    }],
                    otherwise: vec![],
                },
                input: String::new(),
            },
            PipelineStep {
                id: PipelineStepId("tail".into()),
                run: StepRun::Agent(AgentId("tail".into())),
                input: "${steps.inner.output}".into(),
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
            checks: BTreeMap::new(),
        },
        triggers: Vec::new(),
    }
}

#[tokio::test]
async fn downstream_reads_block_output_by_bare_id() {
    let module = branch_then_read_module();
    let store = run_pipeline(Arc::new(module), "GO".to_string(), dispatcher())
        .await
        .expect("runs");
    assert_eq!(store.get("tail").unwrap(), &serde_json::json!("inner-out"));
}
