//! Integration test for the engine-sequenced pipeline executor.
//!
//! Builds a two-step agent pipeline in-test and proves:
//! - step `a` resolves `${input}` against the run input,
//! - step `b` resolves `${steps.a.output}` against the prior step's
//!   output (the output-threading contract).
//!
//! The LLM backend is a bespoke "echo" backend that returns the last
//! user-message text as its final assistant text — so each agent step's
//! output equals the rendered template fed into it. The two agents carry
//! no tools, so the dispatcher's `invoke` is never reached; only its
//! `llm_backend()` hook matters.

use std::boxed::Box;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tracing::field::{Field, Visit};
use tracing::span::Attributes;
use tracing::{Event, Id, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::prelude::*;
use tracing_subscriber::Layer;

use tau_ir::budget::AgentBudget;
use tau_ir::capability::CapabilityTable;
use tau_ir::check::{Check, CheckVerify, JudgeRef, Locus, OnFail, Predicate, Retry};
use tau_ir::ids::{AgentId, CheckId, PipelineStepId, StepId};
use tau_ir::lower::{lower_project, Caches};
use tau_ir::module::{IrFormatVersion, IrModule, Workflow};
use tau_ir::node::{Agent, Deterministic};
use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};
use tau_ir::NativeFnRef;
use tau_pkg::project::ProjectConfig;
use tau_ports::{
    batch_to_stream, CompletionRequest, CompletionResponse, ContentBlock, LlmBackend, LlmError,
    LlmProviderMessage,
};

use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::deterministic::DeterministicRegistry;
use tau_runtime_core::interpreter::pipeline::run_pipeline;
use tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};
use tau_runtime_core::outcome::PipelineStatus;

/// LLM backend that echoes the last user-message text back as its final
/// assistant text (no tool calls -> the agent loop completes in one turn).
struct EchoBackend;

impl EchoBackend {
    /// Build the echo `CompletionResponse` for a request: the last User
    /// message's concatenated text, no tool calls.
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
        // (the codebase's sanctioned escape hatch) rather than a struct
        // literal, which is forbidden outside `tau-ports`.
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
        // The kernel agent loop drives `stream`, not `complete`; fan the
        // echo response out into the equivalent chunk stream.
        Ok(tau_ports::batch_to_stream(Self::echo_response(&req)))
    }
}

/// Dispatcher that hands the echo backend to the interpreter. No agent in
/// the test pipeline carries tools, so `invoke` is never called.
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

    fn llm_backend(&self) -> Arc<dyn DynLlmBackend> {
        self.backend.clone()
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Deterministic-step test infrastructure
// ──────────────────────────────────────────────────────────────────────────────

/// A `DeterministicRegistry` that upper-cases its string input.
///
/// Accepts `Value::String(s)` and returns `Value::String(s.to_uppercase())`.
struct UpcaseRegistry;

impl DeterministicRegistry for UpcaseRegistry {
    fn invoke(&self, fn_name: &str, args: &Value) -> Result<Value, RuntimeError> {
        match fn_name {
            "upcase" => {
                let s = args.as_str().ok_or_else(|| RuntimeError::Internal {
                    message: format!("upcase: expected a string Value, got {args:?}"),
                })?;
                Ok(Value::String(s.to_uppercase()))
            }
            other => Err(RuntimeError::Internal {
                message: format!("UpcaseRegistry: unknown fn {other:?}"),
            }),
        }
    }
}

/// Dispatcher variant that wires the `UpcaseRegistry` into the interpreter.
struct EchoWithRegistryDispatcher {
    backend: Arc<dyn DynLlmBackend>,
    registry: Arc<UpcaseRegistry>,
}

impl ToolDispatcher for EchoWithRegistryDispatcher {
    fn invoke<'a>(
        &'a self,
        _tool_id: &'a tau_ir::ToolId,
        _args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            Err(RuntimeError::Internal {
                message: "EchoWithRegistryDispatcher::invoke: no tools in this pipeline".into(),
            })
        })
    }

    fn llm_backend(&self) -> Arc<dyn DynLlmBackend> {
        self.backend.clone()
    }

    fn deterministic_registry(
        &self,
    ) -> Option<Arc<dyn tau_runtime_core::interpreter::deterministic::DeterministicRegistry>> {
        Some(self.registry.clone())
    }
}

/// Construct an `Agent` node with the given id, no tools, default budget.
fn agent(id: &str) -> Agent {
    Agent {
        id: AgentId(id.into()),
        prompt: String::new(),
        model: "echo-model".into(),
        tool_refs: Vec::new(),
        context: None,
        budget: AgentBudget::default(),
    }
}

/// Build the two-agent pipeline module under test.
fn build_module() -> IrModule {
    let mut agents = BTreeMap::new();
    agents.insert(AgentId("a".into()), agent("a"));
    agents.insert(AgentId("b".into()), agent("b"));

    let pipeline = Pipeline {
        steps: vec![
            PipelineStep {
                id: PipelineStepId("a".into()),
                run: StepRun::Agent(AgentId("a".into())),
                input: "${input}".into(),
            },
            PipelineStep {
                id: PipelineStepId("b".into()),
                run: StepRun::Agent(AgentId("b".into())),
                input: "prev=${steps.a.output}".into(),
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
async fn pipeline_threads_step_output_through_template() {
    let module = build_module();
    let backend: Arc<dyn DynLlmBackend> = Arc::new(EchoBackend);
    let dispatcher = Arc::new(EchoDispatcher { backend });

    let store = run_pipeline(Arc::new(module), "SEED".to_string(), dispatcher)
        .await
        .expect("pipeline runs to completion")
        .outputs;

    // Step `a` rendered `${input}` -> "SEED"; the echo backend returns it
    // as final assistant text.
    assert_eq!(
        store.get("a").and_then(Value::as_str),
        Some("SEED"),
        "step a output should reflect the run input"
    );

    // Step `b` rendered `prev=${steps.a.output}` -> "prev=SEED", proving
    // the OutputStore threading: b's input embedded a's recorded output.
    assert_eq!(
        store.get("b").and_then(Value::as_str),
        Some("prev=SEED"),
        "step b output should reflect ${{steps.a.output}} threading"
    );
}

#[tokio::test]
async fn pipeline_deterministic_step_upcase() {
    // Pipeline:
    //   step a: Agent — echoes ${input} = "hello"
    //   step b: Deterministic(upcase) — receives "${steps.a.output}" = "hello"
    //           rendered_to_args("hello") → Value::String("hello") (not valid JSON)
    //           upcase → Value::String("HELLO")

    let step_id = StepId("upcase-step".into());

    let mut agents = BTreeMap::new();
    agents.insert(AgentId("a".into()), agent("a"));

    let mut steps = BTreeMap::new();
    steps.insert(
        step_id.clone(),
        Deterministic {
            id: step_id.clone(),
            fn_ref: NativeFnRef {
                name: "upcase".into(),
            },
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
        },
    );

    let pipeline = Pipeline {
        steps: vec![
            PipelineStep {
                id: PipelineStepId("a".into()),
                run: StepRun::Agent(AgentId("a".into())),
                input: "${input}".into(),
            },
            PipelineStep {
                id: PipelineStepId("b".into()),
                run: StepRun::Deterministic(step_id),
                input: "${steps.a.output}".into(),
            },
        ],
    };

    let target = tau_ports::target::registry::list_available()
        .next()
        .expect("at least one available target")
        .triple;

    let module = Arc::new(IrModule {
        ir_format: IrFormatVersion::current(),
        tau_version: env!("CARGO_PKG_VERSION").into(),
        target,
        workflow: Workflow {
            agents,
            tools: BTreeMap::new(),
            steps,
            edges: Vec::new(),
            capability_table: CapabilityTable(BTreeMap::new()),
            pipeline: Some(pipeline),
            checks: BTreeMap::new(),
        },
        triggers: Vec::new(),
    });

    let backend: Arc<dyn DynLlmBackend> = Arc::new(EchoBackend);
    let dispatcher = Arc::new(EchoWithRegistryDispatcher {
        backend,
        registry: Arc::new(UpcaseRegistry),
    });

    let store = run_pipeline(module, "hello".to_string(), dispatcher)
        .await
        .expect("pipeline runs to completion")
        .outputs;

    // Agent step `a` echoes its rendered input "hello".
    assert_eq!(
        store.get("a").and_then(Value::as_str),
        Some("hello"),
        "step a should echo the run input"
    );

    // Deterministic step `b` upper-cases "hello" → "HELLO".
    assert_eq!(
        store.get("b").and_then(Value::as_str),
        Some("HELLO"),
        "step b (upcase) should upper-case step a's output"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Trace-event capture harness (mirrors tau-runtime-tokio's tracing_emission.rs)
// ──────────────────────────────────────────────────────────────────────────────

/// Layer that records spans as `"span:<name>"` and events as
/// `"event:<name-field>"`, matching the emit form used in the pipeline
/// executor (`info!(name = EV_PIPELINE_STEP_STARTED, …)`).
#[derive(Default, Clone)]
struct CapturedEvents(Arc<Mutex<Vec<String>>>);

impl<S: Subscriber> Layer<S> for CapturedEvents {
    fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, _ctx: Context<'_, S>) {
        self.0
            .lock()
            .expect("captured-events mutex poisoned")
            .push(format!("span:{}", attrs.metadata().name()));
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = NameVisitor::default();
        event.record(&mut visitor);
        let name = visitor
            .name
            .unwrap_or_else(|| event.metadata().name().to_string());
        // Include the `id` field so the test can assert per-step ids are
        // propagated on both pipeline.step_started and pipeline.step_completed
        // events (rather than only asserting counts).
        let label = match visitor.id {
            Some(id) => format!("{name}:{id}"),
            None => name,
        };
        self.0
            .lock()
            .expect("captured-events mutex poisoned")
            .push(format!("event:{label}"));
    }
}

/// Visitor that extracts the `name` and `id` field values from a tracing
/// event. Accepts both `record_str` and the debug-formatted `record_debug`
/// form. The `name` field is used as the event label; `id` is captured so
/// callers can assert per-step ids are propagated correctly.
#[derive(Default)]
struct NameVisitor {
    name: Option<String>,
    id: Option<String>,
}

impl Visit for NameVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "name" => self.name = Some(value.to_string()),
            "id" => self.id = Some(value.to_string()),
            _ => {}
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let raw = format!("{value:?}");
        let cleaned = raw.trim_matches('"').to_string();
        match field.name() {
            "name" => self.name = Some(cleaned),
            "id" => self.id = Some(cleaned),
            _ => {}
        }
    }
}

#[tokio::test]
async fn pipeline_emits_step_started_and_completed_events() {
    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    let module = build_module(); // two-step (a, b) agent pipeline
    let backend: Arc<dyn DynLlmBackend> = Arc::new(EchoBackend);
    let dispatcher = Arc::new(EchoDispatcher { backend });

    run_pipeline(Arc::new(module), "TRACE_INPUT".to_string(), dispatcher)
        .await
        .expect("pipeline runs to completion");

    let events = captured.0.lock().expect("poisoned").clone();

    // Assert spans opened for each step.
    assert!(
        events.contains(&"span:pipeline.step".to_string()),
        "expected 'span:pipeline.step' to be captured; got: {events:?}"
    );

    // Assert step_started for both step ids — label is "<ev_name>:<step_id>".
    let started_events: Vec<_> = events
        .iter()
        .filter(|e| e.starts_with("event:pipeline.step_started"))
        .collect();
    assert_eq!(
        started_events.len(),
        2,
        "expected two pipeline.step_started events (one per step); got: {events:?}"
    );
    assert!(
        started_events.contains(&&"event:pipeline.step_started:a".to_string()),
        "expected pipeline.step_started with id='a'; got: {started_events:?}"
    );
    assert!(
        started_events.contains(&&"event:pipeline.step_started:b".to_string()),
        "expected pipeline.step_started with id='b'; got: {started_events:?}"
    );

    // Assert step_completed for both step ids — label is "<ev_name>:<step_id>".
    let completed_events: Vec<_> = events
        .iter()
        .filter(|e| e.starts_with("event:pipeline.step_completed"))
        .collect();
    assert_eq!(
        completed_events.len(),
        2,
        "expected two pipeline.step_completed events (one per step); got: {events:?}"
    );
    assert!(
        completed_events.contains(&&"event:pipeline.step_completed:a".to_string()),
        "expected pipeline.step_completed with id='a'; got: {completed_events:?}"
    );
    assert!(
        completed_events.contains(&&"event:pipeline.step_completed:b".to_string()),
        "expected pipeline.step_completed with id='b'; got: {completed_events:?}"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Goal-check evaluation tests (C1.3)
// ──────────────────────────────────────────────────────────────────────────────

/// Build a module with:
///   step "writer" (Agent) — echoes the run input
///   step "chk"    (Check → check "has_sources") — Matches predicate on writer's output
///
/// `pattern` is the regex passed to Predicate::Matches.
fn build_goal_check_module(pattern: &str) -> IrModule {
    let mut agents = std::collections::BTreeMap::new();
    agents.insert(AgentId("writer".into()), agent("writer"));

    let check_id = CheckId("has_sources".into());
    let mut checks = std::collections::BTreeMap::new();
    checks.insert(
        check_id.clone(),
        Check {
            id: check_id.clone(),
            verify: CheckVerify::Goal {
                evaluates: Locus::Output(PipelineStepId("writer".into())),
                predicate: Predicate::Matches(pattern.to_string()),
            },
            retry: None,
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
                id: PipelineStepId("chk".into()),
                run: StepRun::Check(check_id),
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
        triggers: Vec::new(),
        workflow: Workflow {
            agents,
            tools: std::collections::BTreeMap::new(),
            steps: std::collections::BTreeMap::new(),
            edges: Vec::new(),
            capability_table: CapabilityTable(std::collections::BTreeMap::new()),
            pipeline: Some(pipeline),
            checks,
        },
    }
}

#[tokio::test]
async fn goal_check_passes_when_predicate_matches() {
    // The agent echoes "## Sources\nfoo" (injected as run input).
    // The check uses Matches("(?m)^## Sources") which matches -> Completed.
    let module = build_goal_check_module("(?m)^## Sources");
    let backend: Arc<dyn DynLlmBackend> = Arc::new(EchoBackend);
    let dispatcher = Arc::new(EchoDispatcher { backend });

    let outcome = run_pipeline(
        Arc::new(module),
        "## Sources\nfoo".to_string(),
        dispatcher,
    )
    .await
    .expect("pipeline runs without kernel error");

    assert_eq!(
        outcome.status,
        PipelineStatus::Completed,
        "predicate matched -> pipeline should complete; got: {:?}",
        outcome.status
    );
    // The check step stores Bool(true) for a passing check.
    assert_eq!(
        outcome.outputs.get("chk"),
        Some(&serde_json::Value::Bool(true)),
        "passing check step stores Bool(true)"
    );
}

#[tokio::test]
async fn goal_check_aborts_when_predicate_does_not_match() {
    // Same module, but the run input has no "## Sources" header.
    let module = build_goal_check_module("(?m)^## Sources");
    let backend: Arc<dyn DynLlmBackend> = Arc::new(EchoBackend);
    let dispatcher = Arc::new(EchoDispatcher { backend });

    let outcome = run_pipeline(
        Arc::new(module),
        "no header here".to_string(),
        dispatcher,
    )
    .await
    .expect("run_pipeline returns Ok even on check abort");

    assert!(
        matches!(&outcome.status, PipelineStatus::CheckAborted { check, .. } if check == "has_sources"),
        "predicate failed -> pipeline should abort with CheckAborted; got: {:?}",
        outcome.status
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Deliverable check + rewind-to-gate retry (C2.3)
// ──────────────────────────────────────────────────────────────────────────────

/// A scripted LLM backend that pops responses from a queue on each `stream()`
/// call. This lets us drive multi-turn pipelines where the writer agent and
/// the judge agent see DIFFERENT responses across attempts.
struct ScriptedLlmBackend {
    /// Remaining responses (popped FIFO).
    queue: Mutex<VecDeque<CompletionResponse>>,
    /// Count of stream() invocations, for assertion.
    call_count: Mutex<usize>,
    /// Text of every last-user-turn message received, for assertion.
    received_last_user_turns: Mutex<Vec<String>>,
}

impl ScriptedLlmBackend {
    fn new(responses: Vec<CompletionResponse>) -> Self {
        Self {
            queue: Mutex::new(VecDeque::from(responses)),
            call_count: Mutex::new(0),
            received_last_user_turns: Mutex::new(Vec::new()),
        }
    }

    fn call_count(&self) -> usize {
        *self.call_count.lock().expect("call_count mutex poisoned")
    }

    fn received_last_user_turns(&self) -> Vec<String> {
        self.received_last_user_turns
            .lock()
            .expect("received_last_user_turns mutex poisoned")
            .clone()
    }
}

impl LlmBackend for ScriptedLlmBackend {
    fn name(&self) -> &str {
        "scripted-llm"
    }

    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        // The kernel always calls stream(); this path is a fallback.
        Err(LlmError::Provider {
            message: "ScriptedLlmBackend: complete() should not be called; use stream()".into(),
        })
    }

    async fn stream(
        &self,
        req: CompletionRequest,
    ) -> Result<tau_ports::CompletionStream, LlmError> {
        // Record the last user turn text for later assertions.
        let last_user_text = req
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

        {
            let mut turns = self
                .received_last_user_turns
                .lock()
                .expect("received_last_user_turns mutex poisoned");
            turns.push(last_user_text);
        }

        *self.call_count.lock().expect("call_count mutex poisoned") += 1;

        let resp = self
            .queue
            .lock()
            .expect("ScriptedLlmBackend queue mutex poisoned")
            .pop_front()
            .expect("ScriptedLlmBackend: ran out of scripted responses");

        Ok(batch_to_stream(resp))
    }
}

/// Dispatcher that wires a `ScriptedLlmBackend` (shared Arc) into the pipeline.
struct ScriptedDispatcher {
    backend: Arc<ScriptedLlmBackend>,
}

impl ToolDispatcher for ScriptedDispatcher {
    fn invoke<'a>(
        &'a self,
        _tool_id: &'a tau_ir::ToolId,
        _args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            Err(RuntimeError::Internal {
                message: "ScriptedDispatcher::invoke should never be called (no tools)".into(),
            })
        })
    }

    fn llm_backend(&self) -> Arc<dyn DynLlmBackend> {
        self.backend.clone()
    }
}

/// Build a `CompletionResponse` carrying `text` as its assistant text.
fn scripted_response(text: &str) -> CompletionResponse {
    serde_json::from_value(serde_json::json!({
        "text": text,
        "tool_uses": [],
        "stop_reason": "EndTurn",
        "usage": null,
    }))
    .expect("canned CompletionResponse deserializes")
}

/// Build an `IrModule` with a deliverable-check pipeline:
///
/// ```text
/// [writer (Agent)] -> [verify (Check:Deliverable, judge=Builtin)]
/// ```
///
/// The writer echoes its input. The check evaluates `steps.writer.output`
/// via the LLM judge with `must_satisfy = "must have at least 2 sources"`.
///
/// `retry` configures how `verify` handles failure; pass `None` for abort-only.
fn build_deliverable_check_module(retry: Option<Retry>) -> IrModule {
    let mut agents = BTreeMap::new();
    agents.insert(AgentId("writer".into()), agent("writer"));

    let check_id = CheckId("verify".into());
    let mut checks = BTreeMap::new();
    checks.insert(
        check_id.clone(),
        Check {
            id: check_id.clone(),
            verify: CheckVerify::Deliverable {
                locus: Locus::Output(PipelineStepId("writer".into())),
                must_satisfy: "must have at least 2 sources".to_string(),
                judge: JudgeRef::Builtin { model: None },
            },
            retry,
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
                id: PipelineStepId("verify".into()),
                run: StepRun::Check(check_id),
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
        triggers: Vec::new(),
        workflow: Workflow {
            agents,
            tools: BTreeMap::new(),
            steps: BTreeMap::new(),
            edges: Vec::new(),
            capability_table: CapabilityTable(BTreeMap::new()),
            pipeline: Some(pipeline),
            checks,
        },
    }
}

/// The retry struct used in retry tests.
fn retry_config(max_attempts: u32) -> Retry {
    Retry {
        on_fail: OnFail::Retry,
        max_attempts,
        gate: PipelineStepId("writer".into()),
        producer: PipelineStepId("writer".into()),
    }
}

/// Deliverable check with a judge that fails on attempt 1, passes on attempt 2.
///
/// Asserts:
/// - `PipelineStatus::Completed`
/// - Backend received exactly 4 stream() calls (writer1 + judge-fail + writer2 + judge-pass)
/// - The 2nd writer invocation received the rejection rationale in its last user turn
///   ("previous attempt was rejected" + "only 1 source")
/// - The check.retry tracing event was emitted exactly once
#[tokio::test]
async fn deliverable_retry_then_pass() {
    // Stream call order:
    //   1. writer (attempt 1) -> "Draft with 1 source only"
    //   2. judge  (attempt 1) -> met=false, "only 1 source"  (causes rewind)
    //   3. writer (attempt 2) -> "Draft with 2 sources: A, B"
    //   4. judge  (attempt 2) -> met=true, "ok"              (passes)
    let responses = vec![
        scripted_response("Draft with 1 source only"),
        scripted_response(r#"{"met": false, "rationale": "only 1 source"}"#),
        scripted_response("Draft with 2 sources: A, B"),
        scripted_response(r#"{"met": true, "rationale": "ok"}"#),
    ];

    let backend = Arc::new(ScriptedLlmBackend::new(responses));
    let module = Arc::new(build_deliverable_check_module(Some(retry_config(3))));

    // Capture check.retry events.
    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    let dispatcher = Arc::new(ScriptedDispatcher { backend: backend.clone() });
    let outcome =
        run_pipeline(module, "write a report".to_string(), dispatcher).await.expect("no kernel error");

    // Pipeline must complete.
    assert_eq!(
        outcome.status,
        PipelineStatus::Completed,
        "judge passed on 2nd attempt => Completed; got: {:?}",
        outcome.status
    );

    // Exactly 4 stream() calls (writer × 2 + judge × 2).
    assert_eq!(
        backend.call_count(),
        4,
        "expected 4 stream() calls (writer×2 + judge×2); got {}",
        backend.call_count()
    );

    // The 2nd writer call (index 2, the third stream() call overall) must
    // have received the rejection rationale. The ScriptedDispatcher records
    // the last user turn of every call; call index 2 (0-based) is the 3rd.
    let turns = backend.received_last_user_turns();
    assert!(
        turns.len() >= 3,
        "expected at least 3 recorded last-user-turns; got {}: {turns:?}",
        turns.len()
    );
    // The 3rd call is the writer on retry (index 2). Its last user turn is
    // the retry-feedback message injected by the pipeline executor.
    let retry_turn = &turns[2];
    assert!(
        retry_turn.contains("previous attempt was rejected"),
        "retry writer turn should contain 'previous attempt was rejected'; got: {retry_turn:?}"
    );
    assert!(
        retry_turn.contains("only 1 source"),
        "retry writer turn should contain the judge rationale 'only 1 source'; got: {retry_turn:?}"
    );

    // Exactly one check.retry event was emitted.
    let events = captured.0.lock().expect("poisoned").clone();
    let retry_events: Vec<_> = events
        .iter()
        .filter(|e| e.contains("check.retry"))
        .collect();
    assert_eq!(
        retry_events.len(),
        1,
        "expected exactly 1 check.retry event; got: {retry_events:?}"
    );
}

/// Deliverable check whose judge always fails; max_attempts=2 exhausts all
/// retries and the pipeline ends with `CheckAborted`.
///
/// Asserts:
/// - `PipelineStatus::CheckAborted { check: "verify", .. }`
/// - Backend received exactly 4 stream() calls (writer1 + judge-fail1 + writer2 + judge-fail2)
/// - Two check.retry events (none — only 1 rewind then abort on 2nd failure)
///   Actually: 1 rewind (after attempt 1) then abort (attempt 2 >= max_attempts=2)
#[tokio::test]
async fn deliverable_retry_exhausts() {
    // Stream call order:
    //   1. writer (attempt 1) -> "Draft A"
    //   2. judge  (attempt 1) -> met=false, "missing sources"  => rewind (attempt 1 < 2)
    //   3. writer (attempt 2) -> "Draft B"
    //   4. judge  (attempt 2) -> met=false, "still missing"    => abort  (attempt 2 >= 2)
    let responses = vec![
        scripted_response("Draft A"),
        scripted_response(r#"{"met": false, "rationale": "missing sources"}"#),
        scripted_response("Draft B"),
        scripted_response(r#"{"met": false, "rationale": "still missing"}"#),
    ];

    let backend = Arc::new(ScriptedLlmBackend::new(responses));
    let module = Arc::new(build_deliverable_check_module(Some(retry_config(2))));

    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    let dispatcher = Arc::new(ScriptedDispatcher { backend: backend.clone() });
    let outcome =
        run_pipeline(module, "write a report".to_string(), dispatcher).await.expect("no kernel error");

    // Pipeline must abort after all attempts fail.
    assert!(
        matches!(&outcome.status, PipelineStatus::CheckAborted { check, rationale }
            if check == "verify" && rationale.contains("still missing")),
        "judge always fails => CheckAborted with last rationale; got: {:?}",
        outcome.status
    );

    // Exactly 4 stream() calls (writer×2 + judge×2).
    assert_eq!(
        backend.call_count(),
        4,
        "expected 4 stream() calls (writer×2 + judge×2); got {}",
        backend.call_count()
    );

    // Exactly one check.retry event (fired after attempt 1; attempt 2 aborts).
    let events = captured.0.lock().expect("poisoned").clone();
    let retry_events: Vec<_> = events
        .iter()
        .filter(|e| e.contains("check.retry"))
        .collect();
    assert_eq!(
        retry_events.len(),
        1,
        "expected exactly 1 check.retry event (after attempt 1); got: {retry_events:?}"
    );
}

/// Deliverable check that passes on the first attempt (no retry needed).
///
/// Ensures the happy path works without any rewind.
#[tokio::test]
async fn deliverable_check_passes_first_attempt() {
    // Stream call order:
    //   1. writer  -> "Draft with 2 sources: A, B"
    //   2. judge   -> met=true, "ok"
    let responses = vec![
        scripted_response("Draft with 2 sources: A, B"),
        scripted_response(r#"{"met": true, "rationale": "ok"}"#),
    ];

    let backend = Arc::new(ScriptedLlmBackend::new(responses));
    let module = Arc::new(build_deliverable_check_module(None));
    let dispatcher = Arc::new(ScriptedDispatcher { backend: backend.clone() });

    let outcome =
        run_pipeline(module, "write a report".to_string(), dispatcher).await.expect("no kernel error");

    assert_eq!(
        outcome.status,
        PipelineStatus::Completed,
        "judge passed first try => Completed; got: {:?}",
        outcome.status
    );
    assert_eq!(
        backend.call_count(),
        2,
        "expected 2 stream() calls (writer + judge); got {}",
        backend.call_count()
    );
    // check step stores Bool(true).
    assert_eq!(
        outcome.outputs.get("verify"),
        Some(&serde_json::Value::Bool(true)),
        "passing deliverable check stores Bool(true)"
    );
}

/// Deliverable check with abort-only (no retry struct): judge fails =>
/// `CheckAborted` immediately (no rewind).
#[tokio::test]
async fn deliverable_check_aborts_without_retry() {
    // Stream call order:
    //   1. writer  -> "Draft with 0 sources"
    //   2. judge   -> met=false, "no sources found"
    let responses = vec![
        scripted_response("Draft with 0 sources"),
        scripted_response(r#"{"met": false, "rationale": "no sources found"}"#),
    ];

    let backend = Arc::new(ScriptedLlmBackend::new(responses));
    let module = Arc::new(build_deliverable_check_module(None));
    let dispatcher = Arc::new(ScriptedDispatcher { backend: backend.clone() });

    let outcome =
        run_pipeline(module, "write a report".to_string(), dispatcher).await.expect("no kernel error");

    assert!(
        matches!(&outcome.status, PipelineStatus::CheckAborted { check, rationale }
            if check == "verify" && rationale.contains("no sources found")),
        "judge fails + no retry => CheckAborted; got: {:?}",
        outcome.status
    );
    assert_eq!(
        backend.call_count(),
        2,
        "expected 2 stream() calls (writer + judge); got {}",
        backend.call_count()
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// D2: End-to-end authoring→lowering→runtime test (judge-fail → retry → pass)
// ──────────────────────────────────────────────────────────────────────────────

/// End-to-end test: author a tau.toml worked example, lower it via
/// `lower_project` (the full parse→resolve→typecheck pipeline), then drive
/// `run_pipeline` with a scripted LLM backend that:
///
/// 1. gather   → "gathered some notes"
/// 2. writer#1 → "Draft report without sources"   (no ## Sources header)
/// 3. judge#1  → `{"met": false, "rationale": "only 1 source cited"}`
///               ↳ judge sees locus=Output(writer) → rewind to writer
/// 4. writer#2 → "Draft report\n## Sources\n- Source A\n- Source B"
///               ↳ now contains "## Sources" → has_sources goal will pass
/// 5. judge#2  → `{"met": true, "rationale": "two sources cited"}`
///
/// Asserts:
/// - `PipelineStatus::Completed` (both deliverable + goal checks pass)
/// - exactly 5 `stream()` calls (gather + writer×2 + judge×2)
/// - exactly 1 `check.retry` tracing event fired during the run
///
/// Uses **Output loci** (`output = "steps.writer.output"`) for BOTH
/// the deliverable and the goal — so no filesystem, no `WriteFile` tool,
/// and no `produces` declarations are needed. The fixture lowers cleanly
/// with the SHA-256-of-name native-tool cache that the conformance harness
/// uses (but here there are no native tools at all).
#[tokio::test]
async fn e2e_through_lowering_judge_fail_retry_pass() {
    // ── 1. Author the TOML worked example ──────────────────────────────────
    //
    // Two agents (gather + writer), no tools.
    // Deliverable: Output locus on writer's output, LLM-judged, retry allowed.
    // Goal: Output locus on writer's output, regex predicate (## Sources header).
    //
    // Pipeline:
    //   gather -> writer -> verify_report (Deliverable check) -> verify_sources (Goal check)
    let toml = r#"
[project]
name = "research"

[agents.gather]
display_name = "Gather"
package      = "research@^0.1"
llm_backend  = "mock-llm"
model        = "claude-haiku-4-5"
max_turns    = 1

[agents.gather.prompt]
system = "Research the question; collect sources."

[agents.writer]
display_name = "Writer"
package      = "research@^0.1"
llm_backend  = "mock-llm"
model        = "claude-haiku-4-5"
max_turns    = 1

[agents.writer.prompt]
system = "Write the report from the notes."

[deliverables.report]
output       = "steps.writer.output"
must_satisfy = "A coherent summary that cites at least two sources."
on_fail      = "retry"
max_attempts = 3
retry_from   = "writer"

[goals.has_sources]
evaluates = "steps.writer.output"
check     = "matches"
pattern   = "(?m)^## Sources"

[[pipeline.steps]]
id    = "gather"
run   = "agent:gather"
input = "${input}"

[[pipeline.steps]]
id    = "writer"
run   = "agent:writer"
input = "${steps.gather.output}"

[[pipeline.steps]]
id    = "verify_report"
run   = "check:report"
input = "${steps.writer.output}"

[[pipeline.steps]]
id    = "verify_sources"
run   = "check:has_sources"
input = "${steps.writer.output}"
"#;

    // ── 2. Lower via the full authoring → IR path ──────────────────────────
    //
    // Uses the SHA-256-of-name cache (same as the CLI and conformance harness).
    // No native tools declared → cache is never invoked, so mcp_contract/skill
    // stubs are fine.
    let config = ProjectConfig::parse_str(toml)
        .expect("worked-example TOML must parse as a valid ProjectConfig");

    let target = tau_ports::target::registry::list_available()
        .next()
        .expect("at least one target triple available")
        .triple;

    // No native tools in this fixture → the native_tool cache closure is
    // never invoked. Using a panic stub documents that intent and catches
    // any accidental native-tool lookup at test time.
    let module = {
        let caches = Caches {
            native_tool: &|name: &str| {
                panic!("e2e_through_lowering: unexpected native_tool lookup for {name:?}; fixture declares no native tools")
            },
            mcp_contract: &|_| None,
            skill: &|_| None,
        };
        lower_project(&config, &target, &caches)
    }
    .expect("lower_project must succeed for the Output-locus worked example");

    // Confirm the lowered module has exactly 2 checks (report + has_sources).
    assert_eq!(
        module.workflow.checks.len(),
        2,
        "lowered module must have exactly 2 checks; got: {:?}",
        module.workflow.checks.keys().collect::<Vec<_>>()
    );

    // ── 3. Script the LLM responses ────────────────────────────────────────
    //
    // Stream call order:
    //   0. gather   → "gathered some notes"
    //   1. writer#1 → "Draft report without sources"   (no ## Sources → judge fails)
    //   2. judge#1  → {"met": false, "rationale": "only 1 source cited"}  → rewind
    //   3. writer#2 → "Draft report\n## Sources\n- Source A\n- Source B"
    //   4. judge#2  → {"met": true, "rationale": "two sources cited"}     → pass
    //
    // The has_sources Goal check reads writer's output from the OutputStore AFTER
    // judge#2 passes; the store holds writer#2's output which contains "## Sources".
    let responses = vec![
        scripted_response("gathered some notes"),
        scripted_response("Draft report without sources"),
        scripted_response(r#"{"met": false, "rationale": "only 1 source cited"}"#),
        scripted_response("Draft report\n## Sources\n- Source A\n- Source B"),
        scripted_response(r#"{"met": true, "rationale": "two sources cited"}"#),
    ];

    let backend = Arc::new(ScriptedLlmBackend::new(responses));

    // ── 4. Capture trace events ─────────────────────────────────────────────
    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    let dispatcher = Arc::new(ScriptedDispatcher {
        backend: backend.clone(),
    });

    // ── 5. Run the pipeline ─────────────────────────────────────────────────
    let outcome = run_pipeline(Arc::new(module), "write a report".to_string(), dispatcher)
        .await
        .expect("run_pipeline must not return a kernel error for the worked example");

    // ── 6. Assert Completed ─────────────────────────────────────────────────
    assert_eq!(
        outcome.status,
        PipelineStatus::Completed,
        "judge passed on 2nd attempt + goal matched => Completed; got: {:?}",
        outcome.status
    );

    // ── 7. Assert exactly 5 LLM stream() calls ─────────────────────────────
    //  gather + writer#1 + judge#1 + writer#2 + judge#2
    assert_eq!(
        backend.call_count(),
        5,
        "expected 5 stream() calls (gather + writer×2 + judge×2); got {}",
        backend.call_count()
    );

    // ── 8. Assert exactly 1 check.retry event ──────────────────────────────
    let events = captured.0.lock().expect("captured-events mutex poisoned").clone();
    let retry_events: Vec<_> = events
        .iter()
        .filter(|e| e.contains("check.retry"))
        .collect();
    assert_eq!(
        retry_events.len(),
        1,
        "expected exactly 1 check.retry event (after judge#1 fails); got: {retry_events:?}"
    );
}

/// Deliverable check with empty artifact: the pipeline executor catches this
/// BEFORE calling the judge (existence floor) and aborts immediately.
/// No judge call at all.
#[tokio::test]
async fn deliverable_check_aborts_on_empty_artifact() {
    // The writer outputs an empty string (the echo backend echoes its rendered input).
    // We use EchoBackend here since the writer is the only call.
    let backend: Arc<dyn DynLlmBackend> = Arc::new(EchoBackend);
    let module = Arc::new(build_deliverable_check_module(None));
    let dispatcher = Arc::new(EchoDispatcher { backend });

    // Pass an empty string so the writer echoes "" which the check sees as empty.
    let outcome = run_pipeline(module, String::new(), dispatcher).await.expect("no kernel error");

    assert!(
        matches!(&outcome.status, PipelineStatus::CheckAborted { check, rationale }
            if check == "verify" && rationale.contains("empty")),
        "empty artifact => CheckAborted with 'empty' rationale; got: {:?}",
        outcome.status
    );
}
