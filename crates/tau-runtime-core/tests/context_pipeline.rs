//! β.4 context-manager integration tests over the real agent loop.
//!
//! Two run-time proofs, both driving `run_ir` end-to-end through
//! `agent_loop::run_agent` (the single shared agent-loop entry that wires
//! the IR `[agents.*.context]` config into `RunOptions.context_pipeline`):
//!
//! - `no_context_block_emits_zero_context_step_events`: an agent with NO
//!   `context` block runs to completion AND emits zero
//!   `runtime.context_step_ran` events — the pre-β.4 behavior is unchanged.
//!
//! - `native_custom_context_node_runs_end_to_end`: an agent whose IR
//!   `context` pipeline contains a `Custom { source = "native" }` node
//!   (`keep_last_only`) followed by the builtin `fit_budget`. The custom
//!   node is resolved at run time via the dispatcher's
//!   `context_transformer_registry()` hook and actually executes — proven
//!   by a `runtime.context_step_ran { step = "keep_last_only" }` event,
//!   which only fires from inside the per-turn application in `stream.rs`.
//!
//! The LLM backend is the same "echo last user message" shape used by
//! `pipeline_executor.rs`; the trace-capture layer mirrors that file's
//! `CapturedEvents` harness, extended to record the `step` field so the
//! per-transformer events are distinguishable.

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

use tau_domain::{Address, Message, MessagePayload};
use tau_ir::budget::AgentBudget;
use tau_ir::capability::CapabilityTable;
use tau_ir::context::{ContextConfig, ContextNodeKind, ContextStep, DeterminismClass};
use tau_ir::ids::AgentId;
use tau_ir::module::{IrFormatVersion, IrModule, Workflow};
use tau_ir::node::Agent;
use tau_ports::{
    CompletionRequest, CompletionResponse, ContentBlock, LlmBackend, LlmError, LlmProviderMessage,
};

use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::context::{
    CapabilityNeed, ContextFuture, ContextTransformer, ContextTransformerRegistry, TransformCx,
};
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::run_ir;
use tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};
use tau_runtime_core::outcome::RunOutcome;

// ──────────────────────────────────────────────────────────────────────────────
// Echo LLM backend (single-turn completion) — same shape as pipeline_executor.rs
// ──────────────────────────────────────────────────────────────────────────────

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

// ──────────────────────────────────────────────────────────────────────────────
// Custom context transformer + registry (the public extension point)
// ──────────────────────────────────────────────────────────────────────────────

/// A user-supplied native context node that keeps only the last message in
/// the per-turn view. Pure: a function of the message list alone.
struct KeepLastOnly;

impl ContextTransformer for KeepLastOnly {
    fn name(&self) -> &str {
        "keep_last_only"
    }

    fn determinism(&self) -> DeterminismClass {
        DeterminismClass::Pure
    }

    fn required_capabilities(&self) -> &[CapabilityNeed] {
        &[]
    }

    fn transform<'a>(&'a self, _cx: &'a TransformCx<'a>, msgs: Vec<Message>) -> ContextFuture<'a> {
        Box::pin(async move {
            let kept: Vec<Message> = msgs.into_iter().last().into_iter().collect();
            Ok(kept)
        })
    }
}

/// Registry that resolves the one custom node the test pipeline names.
struct TestRegistry;

impl ContextTransformerRegistry for TestRegistry {
    fn resolve(&self, name: &str) -> Option<Arc<dyn ContextTransformer>> {
        if name == "keep_last_only" {
            Some(Arc::new(KeepLastOnly))
        } else {
            None
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Dispatchers
// ──────────────────────────────────────────────────────────────────────────────

/// Dispatcher with NO context registry — used by the backward-compat test.
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

/// Dispatcher that exposes a `ContextTransformerRegistry` — this is the
/// run-path wire the custom-node test exercises. `run_agent` calls
/// `context_transformer_registry()` and feeds it to `build_context_pipeline`.
struct EchoWithContextRegistryDispatcher {
    backend: Arc<dyn DynLlmBackend>,
    registry: Arc<TestRegistry>,
}

impl ToolDispatcher for EchoWithContextRegistryDispatcher {
    fn invoke<'a>(
        &'a self,
        _tool_id: &'a tau_ir::ToolId,
        _args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            Err(RuntimeError::Internal {
                message: "EchoWithContextRegistryDispatcher::invoke: no tools in this pipeline"
                    .into(),
            })
        })
    }

    fn llm_backend_for(&self, _backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        Ok(self.backend.clone())
    }

    fn context_transformer_registry(&self) -> Option<Arc<dyn ContextTransformerRegistry>> {
        Some(self.registry.clone())
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// IR module construction
// ──────────────────────────────────────────────────────────────────────────────

/// Construct a single-agent `IrModule` with the given optional context
/// config. No tools, no pipeline (the agent is the entry node, driven
/// directly via `run_ir`).
fn build_module(context: Option<ContextConfig>) -> IrModule {
    let agent = Agent {
        id: AgentId("solo".into()),
        prompt: "you are a helpful assistant".into(),
        model_ref: tau_ir::ModelRef {
            backend: "echo-llm".into(),
            model_id: "echo-model".into(),
        },
        tool_refs: Vec::new(),
        context,
        budget: AgentBudget::default(),
        produces: Vec::new(),
    };

    let mut agents = BTreeMap::new();
    agents.insert(AgentId("solo".into()), agent);

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
            pipeline: None,
            checks: BTreeMap::new(),
        },
        triggers: Vec::new(),
    }
}

/// A `User -> System` text message.
fn user_msg(text: &str) -> Message {
    Message::new(
        Address::User,
        Address::System,
        MessagePayload::Text {
            content: text.into(),
        },
    )
}

// ──────────────────────────────────────────────────────────────────────────────
// Trace-event capture harness (mirrors pipeline_executor.rs's CapturedEvents,
// extended to record the `step` field so per-transformer events are distinct)
// ──────────────────────────────────────────────────────────────────────────────

/// Records each event as `"event:<name>"`, suffixed with `:<step>` when a
/// `step` field is present (context_step_ran carries `step = <transformer>`).
#[derive(Default, Clone)]
struct CapturedEvents(Arc<Mutex<Vec<String>>>);

impl<S: Subscriber> Layer<S> for CapturedEvents {
    fn on_new_span(&self, _attrs: &Attributes<'_>, _id: &Id, _ctx: Context<'_, S>) {}

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = NameVisitor::default();
        event.record(&mut visitor);
        let name = visitor
            .name
            .unwrap_or_else(|| event.metadata().name().to_string());
        let label = match visitor.step {
            Some(step) => format!("{name}:{step}"),
            None => name,
        };
        self.0
            .lock()
            .expect("captured-events mutex poisoned")
            .push(format!("event:{label}"));
    }
}

/// Extracts the `name` and `step` field values from a tracing event.
#[derive(Default)]
struct NameVisitor {
    name: Option<String>,
    step: Option<String>,
}

impl Visit for NameVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "name" => self.name = Some(value.to_string()),
            "step" => self.step = Some(value.to_string()),
            _ => {}
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let raw = format!("{value:?}");
        let cleaned = raw.trim_matches('"').to_string();
        match field.name() {
            "name" => self.name = Some(cleaned),
            "step" => self.step = Some(cleaned),
            _ => {}
        }
    }
}

/// The vocabulary constant for the per-step event (kept in sync with
/// `tau_runtime_core::vocabulary::EV_CONTEXT_STEP_RAN`). Asserting against
/// the literal here also guards against an accidental rename of the event.
const EV_CONTEXT_STEP_RAN: &str = "runtime.context_step_ran";

// ──────────────────────────────────────────────────────────────────────────────
// Test (a): backward-compat — no context block ⇒ zero context_step_ran events
// ──────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn no_context_block_emits_zero_context_step_events() {
    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    // Agent with NO context config -> empty default pipeline.
    let module = build_module(None);
    let backend: Arc<dyn DynLlmBackend> = Arc::new(EchoBackend);
    let dispatcher = Arc::new(EchoDispatcher { backend });

    let initial = std::vec![user_msg("first"), user_msg("second"), user_msg("LIVE")];

    let outcome = run_ir(
        Arc::new(module),
        &AgentId("solo".into()),
        dispatcher,
        initial,
    )
    .await
    .expect("run completes");

    // The run reaches Completed (echoes the last user message "LIVE").
    let final_text = match &outcome {
        RunOutcome::Completed { final_message, .. } => match &final_message.payload {
            MessagePayload::Text { content } => content.clone(),
            other => panic!("expected Text final message, got {other:?}"),
        },
        other => panic!("expected Completed, got {other:?}"),
    };
    assert_eq!(
        final_text, "LIVE",
        "echo backend returns the last user message text"
    );

    // Backward-compat invariant: ZERO context_step_ran events.
    let events = captured.0.lock().expect("poisoned").clone();
    let context_events: Vec<_> = events
        .iter()
        .filter(|e| e.starts_with(&format!("event:{EV_CONTEXT_STEP_RAN}")))
        .collect();
    assert!(
        context_events.is_empty(),
        "agent with no context block must emit zero {EV_CONTEXT_STEP_RAN} events; got: {context_events:?}"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Test (b): native custom node runs end-to-end via the dispatcher registry
// ──────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn native_custom_context_node_runs_end_to_end() {
    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    // IR context pipeline: custom `keep_last_only` (native) then builtin
    // `fit_budget`. `fit_budget` is last to match the typecheck-enforced
    // shape (the run path is past typecheck, but we stay realistic).
    let mut cfg = ContextConfig::default();
    cfg.pipeline = std::vec![
        ContextStep {
            transformer: "keep_last_only".into(),
            determinism: DeterminismClass::Pure,
            kind: ContextNodeKind::Custom {
                source: "native".into(),
                package: "test-nodes@^0.1".into(),
            },
            config: Default::default(),
        },
        ContextStep {
            transformer: "fit_budget".into(),
            determinism: DeterminismClass::Pure,
            kind: ContextNodeKind::Builtin,
            config: {
                let mut m = BTreeMap::new();
                m.insert("max_tokens".to_string(), serde_json::json!(4000));
                m
            },
        },
    ];

    let module = build_module(Some(cfg));
    let backend: Arc<dyn DynLlmBackend> = Arc::new(EchoBackend);
    let dispatcher = Arc::new(EchoWithContextRegistryDispatcher {
        backend,
        registry: Arc::new(TestRegistry),
    });

    // Multi-message history so `keep_last_only` has something to drop.
    let initial = std::vec![
        user_msg("turn one history"),
        user_msg("turn two history"),
        user_msg("LIVE TURN"),
    ];

    let outcome = run_ir(
        Arc::new(module),
        &AgentId("solo".into()),
        dispatcher,
        initial,
    )
    .await
    .expect("run completes");

    // The run reaches Completed. With `keep_last_only` applied, the per-turn
    // view fed to the echo backend is just the last message ("LIVE TURN"),
    // so the echoed final text is "LIVE TURN".
    let final_text = match &outcome {
        RunOutcome::Completed { final_message, .. } => match &final_message.payload {
            MessagePayload::Text { content } => content.clone(),
            other => panic!("expected Text final message, got {other:?}"),
        },
        other => panic!("expected Completed, got {other:?}"),
    };
    assert_eq!(
        final_text, "LIVE TURN",
        "custom node + echo backend should reflect the kept last message"
    );

    // RUN-TIME proof: the custom node actually executed inside the per-turn
    // application. `runtime.context_step_ran { step = "keep_last_only" }`
    // is emitted only from `stream.rs`'s pipeline loop, never from
    // build/resolve — so its presence proves the dispatcher-registry wire
    // and the public extension point both fire at run time.
    let events = captured.0.lock().expect("poisoned").clone();
    assert!(
        events.contains(&format!("event:{EV_CONTEXT_STEP_RAN}:keep_last_only")),
        "expected a {EV_CONTEXT_STEP_RAN} event for the custom node 'keep_last_only'; got: {events:?}"
    );
    // The builtin `fit_budget` also ran (proves the mixed builtin+custom
    // pipeline resolves and applies in order).
    assert!(
        events.contains(&format!("event:{EV_CONTEXT_STEP_RAN}:fit_budget")),
        "expected a {EV_CONTEXT_STEP_RAN} event for the builtin 'fit_budget'; got: {events:?}"
    );
}
