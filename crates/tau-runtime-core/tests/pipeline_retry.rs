//! Integration tests for `run_pipeline`'s rewind-to-gate retry loop
//! (Task 20 — retry + rationale-feedback injection).
//!
//! Pipeline: `[agent:writer, check:report]` where `report` is a
//! Deliverable whose locus is `steps.writer.output` and whose judge is the
//! built-in judge. A single shared sequencing backend serves both the
//! writer-agent turns and the judge turns, in this interleaved order:
//!
//!   (1) writer attempt-1   -> BAD text
//!   (2) judge              -> {"met":false,...}
//!   (3) writer attempt-2   -> GOOD text   (because the check rewound to the gate)
//!   (4) judge              -> {"met":true,...}
//!
//! `deliverable_retry_converges` proves the run completes, attempt-2's
//! GOOD text lands in the store, exactly one `check.retry` event fires with
//! `next_attempt = 2`, and the attempt-2 writer request carries the
//! rejection feedback. `deliverable_retry_exhausts_and_aborts` proves a
//! `max_attempts = 1` policy with an always-failing judge aborts with
//! `CheckFailed { attempt: 1, .. }` and emits NO `check.retry`.

use std::boxed::Box;
use std::collections::BTreeMap;
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
use tau_ir::check::{Check, CheckVerify, JudgeRef, Locus, OnFail, RetryPolicy};
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

// ──────────────────────────────────────────────────────────────────────────
// SeqLlmBackend: returns scripted responses by invocation index.
// ──────────────────────────────────────────────────────────────────────────

/// LLM backend that returns `responses[call_idx]` on each invocation,
/// advancing `call_idx` and recording every request. The last response is
/// reused once the script is exhausted (so an always-failing judge can keep
/// answering across attempts without a longer script).
struct SeqLlmBackend {
    responses: Vec<CompletionResponse>,
    state: Mutex<SeqState>,
}

struct SeqState {
    next: usize,
    requests: Vec<CompletionRequest>,
}

impl SeqLlmBackend {
    fn new(responses: Vec<CompletionResponse>) -> Self {
        Self {
            responses,
            state: Mutex::new(SeqState {
                next: 0,
                requests: Vec::new(),
            }),
        }
    }

    /// Recorded requests in invocation order.
    fn requests(&self) -> Vec<CompletionRequest> {
        self.state
            .lock()
            .expect("seq mutex poisoned")
            .requests
            .clone()
    }

    /// Record the request and return the scripted response for this call.
    fn take(&self, req: CompletionRequest) -> CompletionResponse {
        let mut st = self.state.lock().expect("seq mutex poisoned");
        let idx = st.next;
        st.next += 1;
        st.requests.push(req);
        let pick = idx.min(self.responses.len().saturating_sub(1));
        self.responses[pick].clone()
    }
}

fn response(text: &str) -> CompletionResponse {
    serde_json::from_value(serde_json::json!({
        "text": text,
        "tool_uses": [],
        "stop_reason": "EndTurn",
        "usage": null,
    }))
    .expect("canned CompletionResponse deserializes")
}

impl LlmBackend for SeqLlmBackend {
    fn name(&self) -> &str {
        "seq-llm"
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        Ok(self.take(req))
    }

    async fn stream(
        &self,
        req: CompletionRequest,
    ) -> Result<tau_ports::CompletionStream, LlmError> {
        Ok(tau_ports::batch_to_stream(self.take(req)))
    }
}

/// Extract the concatenated text of every `User` message in a request.
fn user_texts(req: &CompletionRequest) -> String {
    req.messages
        .iter()
        .filter_map(|m| match m {
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
        .collect::<Vec<_>>()
        .join("\n")
}

// ──────────────────────────────────────────────────────────────────────────
// Dispatcher: SeqLlmBackend + InMemoryArtifactReader (no deterministic reg
// needed — the check is a Deliverable, judged by the LLM).
// ──────────────────────────────────────────────────────────────────────────

struct RetryDispatcher {
    backend: Arc<SeqLlmBackend>,
    reader: Arc<InMemoryArtifactReader>,
}

impl ToolDispatcher for RetryDispatcher {
    fn invoke<'a>(
        &'a self,
        _tool_id: &'a tau_ir::ToolId,
        _args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            Err(RuntimeError::Internal {
                message: "RetryDispatcher::invoke should never be called (no tools)".into(),
            })
        })
    }

    fn llm_backend_for(&self, _backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        Ok(self.backend.clone())
    }

    fn deterministic_registry(&self) -> Option<Arc<dyn DeterministicRegistry>> {
        None
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
        model_ref: tau_ir::ModelRef {
            backend: "seq-llm".into(),
            model_id: "seq-model".into(),
        },
        tool_refs: Vec::new(),
        context: None,
        budget: AgentBudget::default(),
        produces: Vec::new(),
    }
}

/// Build `[agent:writer, check:report]`. `report` is a Deliverable on
/// `steps.writer.output`, builtin judge, with the given retry policy.
fn build_module(retry: RetryPolicy) -> IrModule {
    let mut agents = BTreeMap::new();
    agents.insert(AgentId("writer".into()), writer_agent());

    let mut checks = BTreeMap::new();
    checks.insert(
        CheckId("report".into()),
        Check {
            id: CheckId("report".into()),
            verify: CheckVerify::Deliverable {
                locus: Locus::Output(PipelineStepId("writer".into())),
                must_satisfy: "must cite at least two sources".into(),
                judge: JudgeRef::Default {
                    model_ref: tau_ir::ModelRef {
                        backend: "seq-llm".into(),
                        model_id: "seq-model".into(),
                    },
                },
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
                id: PipelineStepId("report".into()),
                run: StepRun::Check(CheckId("report".into())),
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

// ──────────────────────────────────────────────────────────────────────────
// Trace-event capture (mirrors pipeline_executor.rs's harness, extended to
// also record the `next_attempt` field on check.retry events).
// ──────────────────────────────────────────────────────────────────────────

#[derive(Default, Clone)]
struct CapturedEvents(Arc<Mutex<Vec<String>>>);

impl<S: Subscriber> Layer<S> for CapturedEvents {
    fn on_new_span(&self, _attrs: &Attributes<'_>, _id: &Id, _ctx: Context<'_, S>) {}

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = RetryVisitor::default();
        event.record(&mut visitor);
        let name = visitor
            .name
            .unwrap_or_else(|| event.metadata().name().to_string());
        // For check.retry, append next_attempt so tests can assert on it.
        let label = match visitor.next_attempt {
            Some(n) => format!("{name}:next_attempt={n}"),
            None => name,
        };
        self.0
            .lock()
            .expect("captured-events mutex poisoned")
            .push(format!("event:{label}"));
    }
}

#[derive(Default)]
struct RetryVisitor {
    name: Option<String>,
    next_attempt: Option<u64>,
}

impl Visit for RetryVisitor {
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "next_attempt" {
            self.next_attempt = Some(value);
        }
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        if field.name() == "next_attempt" {
            self.next_attempt = Some(value as u64);
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "name" {
            self.name = Some(value.to_string());
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        match field.name() {
            "name" => {
                let raw = format!("{value:?}");
                self.name = Some(raw.trim_matches('"').to_string());
            }
            "next_attempt" => {
                if let Ok(n) = format!("{value:?}").trim().parse::<u64>() {
                    self.next_attempt = Some(n);
                }
            }
            _ => {}
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn deliverable_retry_converges() {
    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    // Scripted 4-call sequence.
    let backend = Arc::new(SeqLlmBackend::new(vec![
        response("only one source"), // (1) writer attempt-1 BAD
        response(r#"{"met":false,"rationale":"only 1 source; need >=2"}"#), // (2) judge fail
        response("two solid sources [a] [b]"), // (3) writer attempt-2 GOOD
        response(r#"{"met":true,"rationale":"ok"}"#), // (4) judge pass
    ]));

    let dispatcher = Arc::new(RetryDispatcher {
        backend: backend.clone(),
        reader: Arc::new(InMemoryArtifactReader::new()),
    });

    let module = build_module(RetryPolicy {
        on_fail: OnFail::Retry,
        max_attempts: 2,
        gate: PipelineStepId("writer".into()),
    });

    let store = run_pipeline(Arc::new(module), "draft a report".to_string(), dispatcher)
        .await
        .expect("retry should converge and the pipeline should complete");

    // Attempt-2's GOOD text is what ends up in the store.
    assert_eq!(
        store.get("writer").and_then(Value::as_str),
        Some("two solid sources [a] [b]"),
        "the converged (attempt-2) writer output must be recorded"
    );

    // Exactly one check.retry event, with next_attempt = 2.
    let events = captured.0.lock().expect("poisoned").clone();
    let retries: Vec<_> = events
        .iter()
        .filter(|e| e.starts_with("event:check.retry"))
        .collect();
    assert_eq!(
        retries.len(),
        1,
        "expected exactly one check.retry event; got: {events:?}"
    );
    assert_eq!(
        retries[0], "event:check.retry:next_attempt=2",
        "check.retry must carry next_attempt = 2; got: {retries:?}"
    );

    // The writer's attempt-2 request (3rd backend invocation) must carry the
    // rejection feedback as a prior turn — proving feedback injection.
    let requests = backend.requests();
    assert!(
        requests.len() >= 3,
        "expected at least 3 backend invocations; got {}",
        requests.len()
    );
    let attempt2_writer = &requests[2];
    assert!(
        user_texts(attempt2_writer).contains("Previous attempt rejected:"),
        "attempt-2 writer request must contain the rejection feedback; got: {:?}",
        user_texts(attempt2_writer)
    );
    assert!(
        user_texts(attempt2_writer).contains("only 1 source; need >=2"),
        "attempt-2 feedback must carry the judge's rationale; got: {:?}",
        user_texts(attempt2_writer)
    );
}

#[tokio::test]
async fn deliverable_retry_exhausts_and_aborts() {
    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    // Always-failing judge; writer text is irrelevant. max_attempts = 1 so
    // the very first failure is terminal (no retry).
    let backend = Arc::new(SeqLlmBackend::new(vec![
        response("bad draft"), // (1) writer attempt-1
        response(r#"{"met":false,"rationale":"never good enough"}"#), // (2) judge fail (reused)
    ]));

    let dispatcher = Arc::new(RetryDispatcher {
        backend: backend.clone(),
        reader: Arc::new(InMemoryArtifactReader::new()),
    });

    let module = build_module(RetryPolicy {
        on_fail: OnFail::Retry,
        max_attempts: 1,
        gate: PipelineStepId("writer".into()),
    });

    let err = run_pipeline(Arc::new(module), "draft a report".to_string(), dispatcher)
        .await
        .expect_err("max_attempts=1 with a failing judge must abort");

    match err {
        RuntimeError::CheckFailed {
            id, attempt, kind, ..
        } => {
            assert_eq!(id, "report");
            assert_eq!(kind, "deliverable");
            assert_eq!(attempt, 1, "must abort on attempt 1 when max_attempts = 1");
        }
        other => panic!("expected RuntimeError::CheckFailed, got: {other:?}"),
    }

    // No check.retry event must have fired.
    let events = captured.0.lock().expect("poisoned").clone();
    assert!(
        !events.iter().any(|e| e.starts_with("event:check.retry")),
        "no check.retry event may fire when attempts are exhausted; got: {events:?}"
    );
}

/// Characterization test: a producer agent with `budget.max_turns = Some(0)` exhausts its
/// budget before the check is ever evaluated. The retry loop is irrelevant because
/// `run_agent` returns `RunOutcome::Failed { OutOfResources }` and the `StepRun::Agent`
/// arm converts that immediately to `RuntimeError::Internal` — the pipeline aborts on the
/// very first writer step, well before any check or retry logic runs.
///
/// WHY this is expected to be GREEN immediately (given Task 20's code): budget enforcement
/// lives inside `run_agent` (via `RunOptions.max_turns`), which is called by the
/// `StepRun::Agent` arm BEFORE the pipeline ever reaches `StepRun::Check`. The retry
/// loop in `run_pipeline` therefore never sees the budget failure — it surfaces as a plain
/// `Err(RuntimeError::Internal)` from the writer step, not from any check evaluation.
/// `max_attempts = 3` is never consulted because the check step is never reached.
#[tokio::test]
async fn budget_cap_is_authoritative_below_max_attempts() {
    // The backend script is irrelevant: with max_turns = Some(0) the agent
    // loop will immediately return Failed without making any LLM calls.
    let backend = Arc::new(SeqLlmBackend::new(vec![
        response("this text will never be seen"), // placeholder; never consumed
    ]));

    let dispatcher = Arc::new(RetryDispatcher {
        backend: backend.clone(),
        reader: Arc::new(InMemoryArtifactReader::new()),
    });

    // Build a retry-enabled pipeline (max_attempts = 3, gate = writer) but
    // give the writer a budget of max_turns = 0.
    let mut module = build_module(RetryPolicy {
        on_fail: OnFail::Retry,
        max_attempts: 3,
        gate: PipelineStepId("writer".into()),
    });

    // Override the writer agent's budget: max_turns = Some(0) forces the
    // agent loop to fail immediately with OutOfResources before producing output.
    let writer_id = AgentId("writer".into());
    if let Some(agent) = module.workflow.agents.get_mut(&writer_id) {
        agent.budget = AgentBudget {
            max_turns: Some(0),
            max_tokens: None,
        };
    } else {
        panic!("writer agent not found in module");
    }

    let err = run_pipeline(Arc::new(module), "draft a report".to_string(), dispatcher)
        .await
        .expect_err("budget-exhausted agent must abort the run");

    // The StepRun::Agent arm converts RunOutcome::Failed to RuntimeError::Internal
    // with a message mentioning the step id and agent id.
    match &err {
        RuntimeError::Internal { message } => {
            assert!(
                message.contains("writer"),
                "error message must mention the failing agent/step id; got: {message:?}"
            );
            assert!(
                message.contains("failed"),
                "error message must contain 'failed'; got: {message:?}"
            );
        }
        other => panic!("expected RuntimeError::Internal (budget abort), got: {other:?}"),
    }

    // The backend was never invoked (budget = 0 turns → no LLM call at all).
    let requests = backend.requests();
    assert_eq!(
        requests.len(),
        0,
        "no LLM call should have been made; backend received {} requests",
        requests.len()
    );
}
