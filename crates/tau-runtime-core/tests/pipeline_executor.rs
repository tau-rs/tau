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
use tau_ir::ids::{AgentId, PipelineStepId, StepId};
use tau_ir::module::{IrFormatVersion, IrModule, Workflow};
use tau_ir::node::{Agent, Deterministic};
use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};
use tau_ir::NativeFnRef;
use tau_ports::{
    CompletionRequest, CompletionResponse, ContentBlock, LlmBackend, LlmError, LlmProviderMessage,
};

use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::deterministic::DeterministicRegistry;
use tau_runtime_core::interpreter::pipeline::run_pipeline;
use tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};

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

    fn llm_backend_for(&self, _backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        Ok(self.backend.clone())
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

    fn llm_backend_for(&self, _backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        Ok(self.backend.clone())
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
        .expect("pipeline runs to completion");

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
        .expect("pipeline runs to completion");

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
