//! Per-agent loop driver.
//!
//! Routes through the existing `Runtime::run_with_history` (kernel
//! agent loop) by constructing a `Runtime` configured with the agent
//! node's tools, prompt, model, and budget. The ToolDispatcher trait
//! call (task 4.3) is what each tool reaches when invoked — the
//! `Runtime`'s tool registry is wired with a thin wrapper that delegates
//! to the dispatcher.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::str::FromStr;

use tau_domain::{
    Address, AgentDefinition, AgentId as DomainAgentId, Message, MessagePayload, PackageId,
    PackageManifest, PackageName, UncheckedManifest, Version,
};
use tau_ir::{Agent, IrModule, ToolId};
use tau_ports::{
    tool::{SessionContext, ToolContent, ToolResult},
    ToolError, ToolSpec,
};

use crate::builder::Runtime;
use crate::error::RuntimeError;
use crate::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};
use crate::options::RunOptions;
use crate::outcome::RunOutcome;

// ---------------------------------------------------------------------------
// Value conversion helpers
// ---------------------------------------------------------------------------

/// Convert a `tau_domain::Value` to `serde_json::Value` via serde round-trip.
///
/// `tau_domain::Value` and `serde_json::Value` are distinct types; serde
/// is the lowest-friction bridge between them.
fn domain_value_to_json(v: &tau_domain::Value) -> serde_json::Value {
    serde_json::to_value(v).unwrap_or(serde_json::Value::Null)
}

/// Convert a `serde_json::Value` to `tau_domain::Value` via serde round-trip.
fn json_to_domain_value(v: serde_json::Value) -> tau_domain::Value {
    serde_json::from_value(v).unwrap_or(tau_domain::Value::Null)
}

// ---------------------------------------------------------------------------
// Dispatcher-backed Tool wrapper
// ---------------------------------------------------------------------------

/// A `tau_ports::Tool` whose `invoke` delegates to a `ToolDispatcher`.
///
/// One wrapper instance is created per tool in `agent.tool_refs`. Each
/// instance holds an `Arc<D>` (the dispatcher) and the `ToolId` it should
/// forward invocations to.
struct DispatcherTool<D> {
    /// Tool name as seen by the LLM (from the IR `ToolSpec`).
    tool_name: String,
    /// `ToolId` forwarded to the dispatcher on each invoke.
    tool_id: ToolId,
    /// LLM-facing spec (constructed via serde to bypass `#[non_exhaustive]`).
    spec: ToolSpec,
    /// Shared dispatcher handle.
    dispatcher: Arc<D>,
}

impl<D> tau_ports::tool::Tool for DispatcherTool<D>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    type Session = ();

    fn name(&self) -> &str {
        &self.tool_name
    }

    fn schema(&self) -> ToolSpec {
        self.spec.clone()
    }

    async fn init(&self, _ctx: SessionContext) -> Result<Self::Session, ToolError> {
        Ok(())
    }

    async fn invoke(
        &self,
        _session: &mut Self::Session,
        args: tau_domain::Value,
    ) -> Result<ToolResult, ToolError> {
        // Convert tau_domain::Value → serde_json::Value for the dispatcher.
        let json_args = domain_value_to_json(&args);

        let result: ToolInvocationResult = self
            .dispatcher
            .invoke(&self.tool_id, &json_args)
            .await
            .map_err(|e| ToolError::Internal {
                message: alloc::format!("dispatcher error: {e}"),
            })?;

        if let Some(err_msg) = result.error {
            return Ok(ToolResult::new(
                alloc::vec![ToolContent::Text { text: err_msg }],
                true,
            ));
        }

        let text = result
            .body
            .map(|v| alloc::format!("{v}"))
            .unwrap_or_default();
        Ok(ToolResult::new(
            alloc::vec![ToolContent::Text { text }],
            false,
        ))
    }

    async fn teardown(&self, _session: Self::Session) -> Result<(), ToolError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ToolSpec constructor (bypasses #[non_exhaustive] via serde)
// ---------------------------------------------------------------------------

/// Construct a `ToolSpec` from its fields.
///
/// `ToolSpec` is `#[non_exhaustive]` and has no public constructor outside
/// `tau-ports`. We construct it by deserializing from a `serde_json::Value`
/// that carries the three fields — this is the canonical escape hatch for
/// `#[non_exhaustive]` types that have serde derives.
fn make_tool_spec(name: &str, description: &str, input_schema: &serde_json::Value) -> ToolSpec {
    // `ToolSpec.input_schema` is `tau_domain::Value` (not serde_json::Value).
    // We round-trip through JSON to get the domain value.
    let input_schema_domain = json_to_domain_value(input_schema.clone());
    let json = serde_json::json!({
        "name": name,
        "description": description,
        "input_schema": input_schema_domain,
    });
    serde_json::from_value(json).unwrap_or_else(|_| {
        // Absolute fallback: empty-schema spec so the tool still registers.
        let fallback = serde_json::json!({
            "name": name,
            "description": description,
            "input_schema": tau_domain::Value::Object(Default::default()),
        });
        serde_json::from_value(fallback).expect("fallback ToolSpec must deserialize")
    })
}

// ---------------------------------------------------------------------------
// Synthesise a minimal PackageManifest from thin air (via serde)
// ---------------------------------------------------------------------------

/// Build a stub `PackageManifest` for IR agents (no plugin, no deps,
/// no capabilities).
///
/// The manifest is needed as a passthrough container for the kernel's
/// capability-check plumbing; IR agents declare their capabilities via
/// the IR capability table, not via this manifest.
///
/// `UncheckedManifest` is `#[non_exhaustive]`, so we construct it via
/// serde JSON deserialization — the same escape hatch used for other
/// non-exhaustive domain types.
fn stub_manifest() -> PackageManifest {
    let json = serde_json::json!({
        "name": "ir-agent",
        "version": "0.0.0",
        "description": "Synthesised manifest for IR interpreter agent",
        "authors": [],
        "source": "https://example.com/ir.git",
        "kind": "tool",
        "dependencies": [],
        "capabilities": [],
        "sandbox": {}
    });
    let unchecked: UncheckedManifest =
        serde_json::from_value(json).expect("stub manifest JSON must deserialize");
    unchecked
        .validate()
        .expect("stub manifest should always validate")
}

// ---------------------------------------------------------------------------
// run_agent — the main entry
// ---------------------------------------------------------------------------

/// Execute one `Agent` node end-to-end through the existing kernel agent loop.
///
/// Constructs a `Runtime` from the dispatcher's LLM backend + the agent's
/// tool refs (each wrapped as a thin dispatcher-delegating `Tool`), then
/// calls `run_with_history` with a synthesised `AgentDefinition` and
/// `PackageManifest`.
pub async fn run_agent<D>(
    module: &IrModule,
    agent: &Agent,
    dispatcher: Arc<D>,
    initial_messages: Vec<Message>,
) -> Result<RunOutcome, RuntimeError>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    // 1. Obtain the LLM backend from the dispatcher.
    let backend = dispatcher.llm_backend();
    let backend_name = String::from(backend.name());

    // 2. Build the RuntimeBuilder with the backend.
    let mut builder = Runtime::builder().with_dyn_llm_backend(backend);

    // 3. Register each tool in agent.tool_refs as a dispatcher-delegating wrapper.
    for tool_id in &agent.tool_refs {
        let ir_tool = module
            .workflow
            .tools
            .get(tool_id)
            .ok_or_else(|| RuntimeError::Internal {
                message: alloc::format!(
                    "agent {:?} references tool {:?} not in workflow.tools",
                    agent.id.0,
                    tool_id.0,
                ),
            })?;

        let spec = make_tool_spec(
            &ir_tool.spec.name,
            &ir_tool.spec.description,
            &ir_tool.spec.input_schema,
        );

        builder = builder.with_tool(DispatcherTool {
            tool_name: ir_tool.spec.name.clone(),
            tool_id: tool_id.clone(),
            spec,
            dispatcher: dispatcher.clone(),
        });
    }

    // 4. Build the Runtime.
    let rt = builder.build().map_err(|e| RuntimeError::Internal {
        message: alloc::format!("failed to build Runtime for IR agent {:?}: {e}", agent.id.0),
    })?;

    // 5. Synthesise the AgentDefinition from the IR agent.
    //    PackageName allows [a-z0-9-] starting with a letter — same as AgentId.
    //    We fall back to "ir-agent" if the agent id doesn't conform.
    let pkg_name = PackageName::from_str(&agent.id.0)
        .unwrap_or_else(|_| PackageName::from_str("ir-agent").expect("ir-agent is always valid"));

    let llm_backend_pkg_name = PackageName::from_str(&backend_name)
        .unwrap_or_else(|_| PackageName::from_str("ir-agent").expect("ir-agent is always valid"));

    let domain_agent_id = DomainAgentId::from_str(&agent.id.0)
        .unwrap_or_else(|_| DomainAgentId::from_str("ir-agent").expect("ir-agent is always valid"));

    let agent_def = AgentDefinition::new(
        domain_agent_id,
        agent.id.0.clone(),
        PackageId::new(
            pkg_name,
            Version::parse("0.0.0").expect("0.0.0 is always valid"),
        ),
        llm_backend_pkg_name,
    )
    .with_system_prompt(agent.prompt.clone());

    let manifest = stub_manifest();

    // 6. Build RunOptions from the agent's budget.
    let mut run_options = RunOptions::default();
    if let Some(max_turns) = agent.budget.max_turns {
        run_options.max_turns = max_turns;
    }
    // Inject test-fixture clock/random when the host shell has not provided
    // them (e.g. tau-ir-conformance drives run_ir directly without the tokio
    // shell drive entry). Production callers supply real implementations via
    // their shell. The test-fixtures feature is the sanctioned escape hatch;
    // matching the spawn_root_agent_inner pattern in run.rs.
    #[cfg(feature = "test-fixtures")]
    {
        if run_options.clock.is_none() {
            run_options.clock = Some(alloc::sync::Arc::new(tau_ports::MockClock::new()));
        }
        if run_options.random.is_none() {
            run_options.random = Some(alloc::sync::Arc::new(
                tau_ports::DeterministicRandom::seeded(0),
            ));
        }
    }
    #[cfg(not(feature = "test-fixtures"))]
    {
        if run_options.clock.is_none() {
            panic!("run_agent: clock must be supplied unless test-fixtures is enabled");
        }
        if run_options.random.is_none() {
            panic!("run_agent: random must be supplied unless test-fixtures is enabled");
        }
    }

    // 7. Split initial_messages into history + initial_message.
    //    The kernel requires exactly one initial_message; if the caller
    //    provided none, synthesise a placeholder so the run loop has
    //    something to send to the LLM.
    let (history, initial_message) = split_history(initial_messages).unwrap_or_else(|| {
        let placeholder = Message::new(
            Address::User,
            Address::System,
            MessagePayload::Text {
                content: String::new(),
            },
        );
        (Vec::new(), placeholder)
    });

    // 8. Run through the kernel agent loop.
    rt.run_with_history(agent_def, manifest, history, initial_message, run_options)
        .await
}

/// Split a `Vec<Message>` into `(history, last)`, returning `None` when empty.
fn split_history(mut messages: Vec<Message>) -> Option<(Vec<Message>, Message)> {
    if messages.is_empty() {
        return None;
    }
    let last = messages.pop().expect("non-empty checked above");
    Some((messages, last))
}
