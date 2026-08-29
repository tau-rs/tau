//! Pins the enforcement contract for IR-authored tools at the kernel's
//! dispatch-site capability gate (issue #581).
//!
//! `DispatcherTool` (the interpreter's `Tool` wrapper) intentionally does
//! NOT override `Tool::capabilities()`: the dispatch-site gate compares a
//! tool's *required* caps against the running agent's *grant*, and the IR
//! carries no per-agent grant — `Agent` nodes declare tools, prompts and
//! budgets only, and the interpreter's synthesised manifest grants `[]`.
//! Forwarding the IR tool node's declared caps into `capabilities()` would
//! therefore policy-deny every capability-bearing tool on every native
//! bundle run. Declared caps on IR `Tool` nodes are enforced elsewhere:
//!
//! - build time: governance `[allow]` ceiling (over-reach refuses the
//!   build; ungoverned bundles are refused by `tau run --bundle`) and the
//!   D-3b capability-fit shape check vs the target profile;
//! - native runtime: the OS sandbox plans built from plugin manifests
//!   (landlock/seccomp/SBPL + egress proxy) around plugin processes;
//! - wasm runtime: the load-bearing WIT world (EPIC 3.2) plus the host
//!   `WasiCtx` preopens/egress allow-list (EPIC 3.3);
//! - subflows: `AttenuatedDispatcher` gates a child's tool calls against
//!   the invoking subflow tool's declared caps (the frame grant) — the one
//!   runtime check that IS driven by IR-declared caps.
//!
//! The complementary fact — the dispatch gate DOES deny when a directly
//! registered (embedder) tool overrides `capabilities()` beyond the grant —
//! is pinned by `tool_dispatch_capability_denial_yields_run_completed_failed`
//! in `stream.rs`. If this test ever starts failing because the gate began
//! seeing non-empty `required` for IR tools, revisit issue #581: either a
//! grant concept was added to the IR (fine — update this contract) or the
//! change is about to break every capable bundle run.
//!
//! Execution-trace TUI spec §13.2 layers observability on top of this
//! contract WITHOUT weakening it: `DispatcherTool` forwards the host's
//! meet-clamped `effective_capabilities()` so a clamped MCP tool renders an
//! amber `clamp:<to>` waterfall row, while `capabilities()` stays
//! un-overridden so `required` at the gate remains `[]`. The second test
//! below pins exactly that split.
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
use tau_ports::{CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError};

use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::run_ir_streaming;
use tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};
use tau_runtime_core::outcome::RunOutcome;
use tau_runtime_core::stream::RunEvent;

// --- scripted backend: calls `fetch` once, then ends the turn ---
struct Scripted {
    queue: Mutex<Vec<CompletionResponse>>,
}

/// Build a `CompletionResponse` from JSON — the codebase's sanctioned
/// escape hatch for `#[non_exhaustive]` `tau-ports` types (see
/// `tests/run_ir_streaming.rs`).
fn resp(json: serde_json::Value) -> CompletionResponse {
    serde_json::from_value(json).expect("CompletionResponse deserializes")
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

// --- recording dispatcher: records every tool it is actually asked to invoke ---
struct Recording {
    seen: Arc<Mutex<Vec<String>>>,
    backend: Arc<dyn DynLlmBackend>,
}

impl ToolDispatcher for Recording {
    fn invoke<'a>(
        &'a self,
        tool_id: &'a ToolId,
        _args: &'a Value,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<ToolInvocationResult, RuntimeError>>
                + Send
                + 'a,
        >,
    > {
        self.seen.lock().unwrap().push(tool_id.0.clone());
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

fn net_http_any() -> tau_domain::Capability {
    #[derive(serde::Deserialize)]
    struct W {
        cap: tau_domain::Capability,
    }
    toml::from_str::<W>("[cap]\nkind=\"net.http\"\nhosts=\"any\"\n")
        .unwrap()
        .cap
}

/// Single root agent whose one Native tool declares `net.http hosts=any` —
/// strictly beyond the interpreter's empty synthesised grant.
fn module_with_net_tool() -> (IrModule, AgentId) {
    let entry = AgentId("a".into());

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
                declared: vec![net_http_any()],
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

#[tokio::test]
async fn ir_declared_caps_do_not_gate_root_dispatch() {
    let (module, entry) = module_with_net_tool();

    let backend: Arc<dyn DynLlmBackend> = Arc::new(Scripted {
        queue: Mutex::new(vec![
            resp(serde_json::json!({
                "text":"","tool_uses":[{"id":"t1","name":"fetch","input":{}}],
                "stop_reason":"ToolUse","usage":null
            })),
            resp(serde_json::json!({
                "text":"done","tool_uses":[],"stop_reason":"EndTurn","usage":null
            })),
        ]),
    });
    let seen = Arc::new(Mutex::new(Vec::new()));
    let dispatcher = Arc::new(Recording {
        seen: seen.clone(),
        backend,
    });

    let stream = run_ir_streaming(Arc::new(module), &entry, dispatcher, Vec::new())
        .await
        .expect("stream builds");
    let events: Vec<RunEvent> = Box::pin(stream).collect().await;

    // The run completes: the dispatch-site gate sees required = [] for the
    // IR-wrapped tool (DispatcherTool has no capabilities() override), so no
    // PolicyDenied fires despite the declared net.http cap and empty grant.
    match events.last() {
        Some(RunEvent::RunCompleted {
            outcome: RunOutcome::Completed { .. },
        }) => {}
        other => panic!("expected RunCompleted/Completed, got {other:?}"),
    }
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["fetch"],
        "the net.http-declaring tool must reach the dispatcher un-gated"
    );
}

/// Spec §13.2 + §13.6: forwarding *effective* capabilities must not
/// re-activate the dispatch gate. The tool still reaches the dispatcher
/// un-gated (issue #581), and its ToolCall row carries `Clamp`.
#[tokio::test]
async fn forwarded_effective_caps_do_not_gate_but_do_label_the_row() {
    use tau_ports::TraceEvent;
    use tau_runtime_core::interpreter::tool_dispatch::TraceSinkConfig;
    use tau_runtime_core::orchestration::trace::TraceSubscriber;

    struct Collector(Mutex<Vec<TraceEvent>>);
    impl TraceSubscriber for Collector {
        fn emit(&self, event: TraceEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    /// `Recording`, plus a trace sink and a clamped authority for `fetch`.
    struct RecordingClamped {
        seen: Arc<Mutex<Vec<String>>>,
        backend: Arc<dyn DynLlmBackend>,
        collector: Arc<Collector>,
    }

    impl ToolDispatcher for RecordingClamped {
        fn invoke<'a>(
            &'a self,
            tool_id: &'a ToolId,
            _args: &'a Value,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<ToolInvocationResult, RuntimeError>>
                    + Send
                    + 'a,
            >,
        > {
            self.seen.lock().unwrap().push(tool_id.0.clone());
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
                run_id: "run-581-sibling".into(),
                subscribers: vec![Arc::clone(&self.collector) as Arc<dyn TraceSubscriber>],
            })
        }

        fn tool_effective_capabilities(
            &self,
            tool_id: &ToolId,
        ) -> Option<Vec<tau_domain::Capability>> {
            #[derive(serde::Deserialize)]
            struct W {
                cap: tau_domain::Capability,
            }
            (tool_id.0 == "fetch").then(|| {
                vec![
                    toml::from_str::<W>("[cap]\nkind=\"net.http\"\nhosts=[\"api.weather.com\"]\n")
                        .unwrap()
                        .cap,
                ]
            })
        }
    }

    let (module, entry) = module_with_net_tool();
    let backend: Arc<dyn DynLlmBackend> = Arc::new(Scripted {
        queue: Mutex::new(vec![
            resp(serde_json::json!({
                "text":"","tool_uses":[{"id":"t1","name":"fetch","input":{}}],
                "stop_reason":"ToolUse","usage":null
            })),
            resp(serde_json::json!({
                "text":"done","tool_uses":[],"stop_reason":"EndTurn","usage":null
            })),
        ]),
    });
    let seen = Arc::new(Mutex::new(Vec::new()));
    let collector = Arc::new(Collector(Mutex::new(Vec::new())));
    let dispatcher = Arc::new(RecordingClamped {
        seen: seen.clone(),
        backend,
        collector: collector.clone(),
    });

    let stream = run_ir_streaming(Arc::new(module), &entry, dispatcher, Vec::new())
        .await
        .expect("stream builds");
    let events: Vec<RunEvent> = Box::pin(stream).collect().await;

    // (1) The #581 contract still holds: no PolicyDenied, tool dispatched.
    match events.last() {
        Some(RunEvent::RunCompleted {
            outcome: RunOutcome::Completed { .. },
        }) => {}
        other => panic!("expected RunCompleted/Completed, got {other:?}"),
    }
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["fetch"],
        "forwarding effective caps must NOT gate the tool (#581)"
    );

    // (2) …and the row is labelled with the clamp.
    let verdict = collector
        .0
        .lock()
        .unwrap()
        .iter()
        .find_map(|e| match &e.kind {
            tau_ports::TraceEventKind::ToolCall { capability, .. } => Some(capability.clone()),
            _ => None,
        })
        .expect("a ToolCall trace event");
    assert_eq!(
        verdict,
        Some(tau_ports::CapabilityVerdict::Clamp {
            to: "api.weather.com".into()
        })
    );
}
