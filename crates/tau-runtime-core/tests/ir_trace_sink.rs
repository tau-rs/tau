//! Spec §13.3 + §13.1 + §13.2: an IR-interpreter run with a trace sink
//! emits `ToolCall` trace events, and a meet-clamped tool's row carries
//! `CapabilityVerdict::Clamp` even though the IR gate sees `required = []`
//! (issue #581).
#![cfg(feature = "test-fixtures")]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use futures_util::StreamExt as _;
use serde_json::Value;

use tau_ir::budget::AgentBudget;
use tau_ir::capability::CapabilityRequirements;
use tau_ir::ids::{AgentId, ToolId};
use tau_ir::module::{IrFormatVersion, IrModule, Workflow};
use tau_ir::node::{Agent, Tool, ToolSpec};
use tau_ir::tool_impl::{NativeFnRef, ToolImpl};
use tau_ports::{
    CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError, TraceEvent,
};

use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::run_ir_streaming;
use tau_runtime_core::interpreter::tool_dispatch::{
    ToolDispatcher, ToolInvocationResult, TraceSinkConfig,
};
use tau_runtime_core::orchestration::trace::TraceSubscriber;
use tau_runtime_core::stream::RunEvent;

fn resp(json: serde_json::Value) -> CompletionResponse {
    serde_json::from_value(json).expect("CompletionResponse deserializes")
}

fn test_cap(toml_str: &str) -> tau_domain::Capability {
    #[derive(serde::Deserialize)]
    struct CapWrapper {
        cap: tau_domain::Capability,
    }
    toml::from_str::<CapWrapper>(toml_str).unwrap().cap
}

struct Scripted {
    queue: Mutex<Vec<CompletionResponse>>,
}

impl LlmBackend for Scripted {
    fn name(&self) -> &str {
        "mock-llm"
    }
    async fn complete(&self, _r: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        Ok(self.queue.lock().unwrap().remove(0))
    }
    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        Ok(tau_ports::batch_to_stream(
            LlmBackend::complete(self, req).await?,
        ))
    }
}

/// Collects every trace event the kernel emits.
struct Collector(Mutex<Vec<TraceEvent>>);

impl TraceSubscriber for Collector {
    fn emit(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

/// A dispatcher that supplies both a trace sink and a clamped authority.
struct SinkDispatcher {
    backend: Arc<dyn DynLlmBackend>,
    collector: Arc<Collector>,
    clamped_id: ToolId,
    effective: Vec<tau_domain::Capability>,
}

impl ToolDispatcher for SinkDispatcher {
    fn invoke<'a>(
        &'a self,
        _tool_id: &'a ToolId,
        _args: &'a Value,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<ToolInvocationResult, RuntimeError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async {
            Ok(ToolInvocationResult {
                body: Some(Value::String("ok".into())),
                error: None,
            })
        })
    }

    fn llm_backend_for(&self, _b: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        Ok(self.backend.clone())
    }

    fn trace_sink(&self) -> Option<TraceSinkConfig> {
        Some(TraceSinkConfig {
            run_id: "run-ir-clamp".into(),
            subscribers: vec![Arc::clone(&self.collector) as Arc<dyn TraceSubscriber>],
        })
    }

    fn tool_effective_capabilities(&self, tool_id: &ToolId) -> Option<Vec<tau_domain::Capability>> {
        (tool_id == &self.clamped_id).then(|| self.effective.clone())
    }
}

/// Single agent, single `net.http`-declaring tool (mirrors
/// `ir_dispatch_gate_inert.rs`'s fixture).
fn module_with_net_tool() -> (IrModule, AgentId) {
    let entry = AgentId("a".into());
    let net_any = test_cap("[cap]\nkind=\"net.http\"\nhosts=\"any\"\n");

    let mut tools = BTreeMap::new();
    tools.insert(
        ToolId("fetch".into()),
        Tool {
            id: ToolId("fetch".into()),
            impl_: ToolImpl::Native {
                fn_ref: NativeFnRef {
                    name: "fetch".into(),
                },
                content_hash: [1u8; 32],
            },
            capabilities: CapabilityRequirements {
                declared: vec![net_any],
            },
            spec: ToolSpec {
                name: "fetch".into(),
                description: String::new(),
                input_schema: Value::Null,
            },
        },
    );

    let mut agents = BTreeMap::new();
    agents.insert(
        entry.clone(),
        Agent {
            id: entry.clone(),
            prompt: tau_ir::prompt::PromptSource::Inline(String::new()),
            model_ref: tau_ir::model_ref::ModelRef {
                backend: "mock-llm".into(),
                model_id: "m".into(),
            },
            tool_refs: vec![ToolId("fetch".into())],
            context: None,
            budget: AgentBudget {
                max_turns: Some(3),
                max_tokens: None,
            },
            produces: vec![],
            output_schema: None,
            durable: None,
        },
    );

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
            tools,
            steps: BTreeMap::new(),
            edges: Vec::new(),
            capability_table: tau_ir::capability::CapabilityTable(BTreeMap::new()),
            pipeline: None,
            checks: BTreeMap::new(),
        },
        triggers: Vec::new(),
    };

    (module, entry)
}

fn scripted_backend() -> Arc<dyn DynLlmBackend> {
    Arc::new(Scripted {
        queue: Mutex::new(vec![
            resp(serde_json::json!({
                "text":"","tool_uses":[{"id":"t1","name":"fetch","input":{}}],
                "stop_reason":"ToolUse","usage":null
            })),
            resp(serde_json::json!({
                "text":"done","tool_uses":[],"stop_reason":"EndTurn","usage":null
            })),
        ]),
    })
}

#[tokio::test]
async fn ir_run_with_sink_emits_clamp_tool_call_row() {
    let (module, entry) = module_with_net_tool();
    let collector = Arc::new(Collector(Mutex::new(Vec::new())));
    let dispatcher = Arc::new(SinkDispatcher {
        backend: scripted_backend(),
        collector: collector.clone(),
        clamped_id: ToolId("fetch".into()),
        effective: vec![test_cap(
            "[cap]\nkind = \"net.http\"\nhosts = [\"api.weather.com\"]\n",
        )],
    });

    let stream = run_ir_streaming(Arc::new(module), &entry, dispatcher, Vec::new())
        .await
        .expect("stream builds");
    let _events: Vec<RunEvent> = Box::pin(stream).collect().await;

    let events = collector.0.lock().unwrap().clone();
    let tool_call = events
        .iter()
        .find_map(|e| match &e.kind {
            tau_ports::TraceEventKind::ToolCall {
                tool_name,
                capability,
                ..
            } => Some((tool_name.clone(), capability.clone())),
            _ => None,
        })
        .expect("an IR run with a trace sink must emit a ToolCall trace event");

    assert_eq!(tool_call.0, "fetch");
    assert_eq!(
        tool_call.1,
        Some(tau_ports::CapabilityVerdict::Clamp {
            to: "api.weather.com".into()
        }),
        "the meet-clamped IR tool must render an amber clamp row"
    );
    assert!(
        events.iter().all(|e| e.run_id == "run-ir-clamp"),
        "every event must carry the sink's run id"
    );
}

#[tokio::test]
async fn ir_run_without_sink_still_completes() {
    // Regression guard: dispatchers that don't override `trace_sink` (the
    // wasm guest, `tau dev`, conformance) must behave exactly as before.
    struct NoSink {
        backend: Arc<dyn DynLlmBackend>,
    }
    impl ToolDispatcher for NoSink {
        fn invoke<'a>(
            &'a self,
            _tool_id: &'a ToolId,
            _args: &'a Value,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<ToolInvocationResult, RuntimeError>>
                    + Send
                    + 'a,
            >,
        > {
            Box::pin(async {
                Ok(ToolInvocationResult {
                    body: Some(Value::String("ok".into())),
                    error: None,
                })
            })
        }
        fn llm_backend_for(&self, _b: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
            Ok(self.backend.clone())
        }
    }

    let (module, entry) = module_with_net_tool();
    let dispatcher = Arc::new(NoSink {
        backend: scripted_backend(),
    });

    let stream = run_ir_streaming(Arc::new(module), &entry, dispatcher, Vec::new())
        .await
        .expect("stream builds");
    let events: Vec<RunEvent> = Box::pin(stream).collect().await;

    // The run still completes; there is simply no trace sink to emit into.
    assert!(matches!(events.last(), Some(RunEvent::RunCompleted { .. })));
}
