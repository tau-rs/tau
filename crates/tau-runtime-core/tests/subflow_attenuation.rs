//! Integration test: a subflow spawned with an empty cap_subset must have
//! its child's excluded tool call denied by `AttenuatedDispatcher` before it
//! ever reaches the inner (real) dispatcher.
//!
//! Mirrors the scripted-backend + recording-dispatcher pattern in
//! `tests/run_ir_streaming.rs` (canonical `CompletionResponse` JSON shape via
//! the serde escape hatch) and the `attenuate.rs` unit tests (module/tool
//! construction idioms for `#[non_exhaustive]` IR types).
#![cfg(feature = "test-fixtures")]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use tau_ir::budget::AgentBudget;
use tau_ir::capability::CapabilityRequirements;
use tau_ir::ids::{AgentId, ToolId};
use tau_ir::module::{IrFormatVersion, IrModule, Workflow};
use tau_ir::node::{Agent, Tool, ToolSpec};
use tau_ir::tool_impl::ToolImpl;
use tau_ports::{CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError};

use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::run_ir;
use tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};
use tau_runtime_core::outcome::RunOutcome;

// --- scripted backend: parent calls `notify`, worker calls `page`, then both end ---
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

// --- recording inner dispatcher: records every tool it is actually asked to invoke ---
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

fn tool(id: &str, impl_: ToolImpl, caps: Vec<tau_domain::Capability>) -> Tool {
    Tool {
        id: ToolId(id.into()),
        impl_,
        capabilities: CapabilityRequirements { declared: caps },
        spec: ToolSpec {
            name: id.into(),
            description: String::new(),
            input_schema: Value::Null,
        },
    }
}

fn agent(id: &str, tools: &[&str]) -> Agent {
    Agent {
        id: AgentId(id.into()),
        prompt: tau_ir::prompt::PromptSource::Inline(String::new()),
        model_ref: tau_ir::model_ref::ModelRef {
            backend: "mock-llm".into(),
            model_id: "m".into(),
        },
        tool_refs: tools.iter().map(|s| ToolId(s.to_string())).collect(),
        context: None,
        budget: AgentBudget {
            max_turns: Some(3),
            max_tokens: None,
        },
        produces: vec![],
        output_schema: None,
        durable: None,
    }
}

fn net_http() -> tau_domain::Capability {
    #[derive(serde::Deserialize)]
    struct W {
        cap: tau_domain::Capability,
    }
    toml::from_str::<W>("[cap]\nkind=\"net.http\"\nhosts=\"any\"\n")
        .unwrap()
        .cap
}

#[tokio::test]
async fn empty_cap_subset_denies_child_tool_call() {
    let mut agents = BTreeMap::new();
    agents.insert(AgentId("parent".into()), agent("parent", &["notify"]));
    agents.insert(AgentId("worker".into()), agent("worker", &["page"]));
    let mut tools = BTreeMap::new();
    // notify: subflow -> worker, EMPTY cap_subset.
    tools.insert(
        ToolId("notify".into()),
        tool(
            "notify",
            ToolImpl::Subflow {
                target: AgentId("worker".into()),
            },
            vec![],
        ),
    );
    // page: needs net.http.
    tools.insert(
        ToolId("page".into()),
        tool(
            "page",
            ToolImpl::Native {
                fn_ref: tau_ir::tool_impl::NativeFnRef {
                    name: "page".into(),
                },
                content_hash: [2u8; 32],
            },
            vec![net_http()],
        ),
    );

    // `IrModule` has no `Default` impl (unlike `Workflow`, which does), so
    // every field is supplied explicitly — same as `attenuate.rs`'s
    // `module_with_tool` and `tests/run_ir_streaming.rs`'s fixture builder.
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
            tools,
            ..Default::default()
        },
        triggers: Vec::new(),
    });

    let queue = vec![
        resp(
            serde_json::json!({"text":"","tool_uses":[{"id":"p1","name":"notify","input":{}}],"stop_reason":"ToolUse","usage":null}),
        ),
        resp(
            serde_json::json!({"text":"","tool_uses":[{"id":"w1","name":"page","input":{}}],"stop_reason":"ToolUse","usage":null}),
        ),
        resp(
            serde_json::json!({"text":"paged","tool_uses":[],"stop_reason":"EndTurn","usage":null}),
        ),
        resp(
            serde_json::json!({"text":"done","tool_uses":[],"stop_reason":"EndTurn","usage":null}),
        ),
    ];
    let backend: Arc<dyn DynLlmBackend> = Arc::new(Scripted {
        queue: Mutex::new(queue),
    });
    let seen = Arc::new(Mutex::new(Vec::new()));
    let dispatcher = Arc::new(Recording {
        seen: seen.clone(),
        backend,
    });

    let outcome = run_ir(module, &AgentId("parent".into()), dispatcher, Vec::new())
        .await
        .unwrap();

    // The child's `page` call was denied by the attenuation frame (empty
    // cap_subset), so the inner dispatcher was NEVER asked to invoke `page`.
    assert!(
        !seen.lock().unwrap().iter().any(|t| t == "page"),
        "page must be denied before reaching the dispatcher; saw {:?}",
        seen.lock().unwrap()
    );
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "soft-deny: run still completes; got {outcome:?}"
    );
}
