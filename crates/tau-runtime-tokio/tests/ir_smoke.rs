//! Smoke test: drive a minimal IrModule through run_ir; expect
//! Completed with the mocked LLM's tool calls reflected in the
//! outcome.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use tau_ir::{
    Agent, AgentBudget, AgentId, CapabilityRequirements, CapabilityTable, IrFormatVersion,
    IrModule, NativeFnRef, Tool, ToolId, ToolImpl, Workflow,
};
use tau_ports::fixtures::MockLlmBackend;
use tau_ports::target::registry as target_registry;
use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::run_ir;
use tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};
use tau_runtime_core::outcome::RunOutcome;

struct StubDispatcher {
    backend: Arc<dyn DynLlmBackend>,
}

impl StubDispatcher {
    fn new() -> Self {
        Self {
            backend: Arc::new(MockLlmBackend::new("stub-llm")),
        }
    }
}

impl ToolDispatcher for StubDispatcher {
    fn invoke<'a>(
        &'a self,
        _tool_id: &'a ToolId,
        _args: &'a serde_json::Value,
    ) -> Pin<
        Box<
            dyn std::future::Future<Output = Result<ToolInvocationResult, RuntimeError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            Ok(ToolInvocationResult {
                body: Some(serde_json::json!({"ok": true})),
                error: None,
            })
        })
    }

    fn llm_backend(&self) -> Arc<dyn DynLlmBackend> {
        self.backend.clone()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn run_ir_minimal_module_completes() {
    let module = sample_module();
    let entry = AgentId("monitor".into());
    let outcome = run_ir(
        Arc::new(module),
        &entry,
        Arc::new(StubDispatcher::new()),
        Vec::new(),
    )
    .await
    .expect("run_ir returns outcome");
    match outcome {
        RunOutcome::Completed { total_turns, .. } => {
            // The MockLlmBackend returns one empty EndTurn response (no
            // tool_uses), so the kernel completes in exactly 1 turn.
            assert_eq!(total_turns, 1, "expected exactly 1 LLM turn");
        }
        other => panic!("expected Completed, got {:?}", other),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn run_ir_missing_entry_agent_returns_error() {
    let module = sample_module();
    let entry = AgentId("does-not-exist".into());
    let result = run_ir(
        Arc::new(module),
        &entry,
        Arc::new(StubDispatcher::new()),
        Vec::new(),
    )
    .await;
    assert!(
        matches!(result, Err(RuntimeError::AgentNotFound { .. })),
        "expected AgentNotFound, got {:?}",
        result
    );
}

fn sample_module() -> IrModule {
    let mut agents = BTreeMap::new();
    agents.insert(
        AgentId("monitor".into()),
        Agent {
            id: AgentId("monitor".into()),
            prompt: "You are a monitor agent.".into(),
            model: "claude-haiku-4-5".into(),
            tool_refs: vec![ToolId("read_temp".into())],
            context: None,
            budget: AgentBudget {
                max_turns: Some(1),
                max_tokens: None,
            },
        },
    );

    let mut tools = BTreeMap::new();
    tools.insert(
        ToolId("read_temp".into()),
        Tool {
            id: ToolId("read_temp".into()),
            impl_: ToolImpl::Native {
                fn_ref: NativeFnRef {
                    name: "ReadTemp".into(),
                },
                content_hash: [1u8; 32],
            },
            capabilities: CapabilityRequirements::default(),
            spec: tau_ir::node::ToolSpec {
                name: "read_temp".into(),
                description: "Read the current temperature".into(),
                input_schema: serde_json::json!({"type": "object", "properties": {}}),
            },
        },
    );

    IrModule {
        ir_format: IrFormatVersion::current(),
        tau_version: "0.0.0".into(),
        target: target_registry::list_available()
            .next()
            .expect("at least one target available")
            .triple,
        workflow: Workflow {
            agents,
            tools,
            steps: Default::default(),
            edges: Default::default(),
            capability_table: CapabilityTable(Default::default()),
            pipeline: None,
        },
    }
}
