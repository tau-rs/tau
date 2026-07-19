//! Run-loop no_std tool-arg validation proof (EPIC 0 T9).
//!
//! Drives `run_ir_streaming` under
//! `--no-default-features --features wasm-interpreter,tool-validation,test-fixtures`
//! and asserts that the run loop RUNS the `tool-validation` feature — not
//! just compiles it.
//!
//! The fixture builds an IR agent whose only tool declares a strict JSON
//! Schema (`"x": string` required). The scripted LLM emits a `ToolUse`
//! with missing `"x"` on turn 1; the validation path in `stream.rs`
//! (guarded by `#[cfg(feature = "tool-validation")]`) catches the bad args,
//! writes a `ToolError` self-correction into the conversation, and yields
//! `RunEvent::ToolCallCompleted { result: Err(_) }` — the dispatcher's
//! `invoke` is never called. The LLM then emits text on turn 2 and the run
//! completes successfully.
//!
//! Mirrored from `tests/run_ir_streaming.rs` (EchoBackend/EchoDispatcher +
//! `build_module` pattern, lines 1–177) with a ScriptedBackend addition and
//! a strict-schema tool in the IR workflow.

// Gate the entire file: it only compiles/runs when both features are present.
// This ensures the CI step
//   `cargo nextest run -p tau-runtime-core \
//      --no-default-features \
//      --features wasm-interpreter,tool-validation,test-fixtures \
//      --test no_std_validation`
// exercises the real validation code path.
#![cfg(all(feature = "test-fixtures", feature = "tool-validation"))]

use std::boxed::Box;
use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use futures_util::StreamExt as _;
use serde_json::Value as JsonValue;

use tau_ir::budget::AgentBudget;
use tau_ir::capability::CapabilityTable;
use tau_ir::ids::{AgentId, ToolId};
use tau_ir::module::{IrFormatVersion, IrModule, Workflow};
use tau_ir::node::{Agent, Tool, ToolSpec as IrToolSpec};
use tau_ir::tool_impl::{Hash256, NativeFnRef, ToolImpl};
use tau_ports::{
    CompletionChunk, CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError,
    StopReason,
};

use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::run_ir_streaming;
use tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};
use tau_runtime_core::outcome::RunOutcome;
use tau_runtime_core::stream::RunEvent;

// ---------------------------------------------------------------------------
// Scripted LLM backend
// ---------------------------------------------------------------------------

/// LLM backend that replays a pre-scripted sequence of turns (chunk vectors).
/// On each call to `stream()` it pops and yields the next turn's chunks.
struct ScriptedBackend {
    turns: Mutex<VecDeque<Vec<Result<CompletionChunk, LlmError>>>>,
}

impl ScriptedBackend {
    fn new(turns: Vec<Vec<Result<CompletionChunk, LlmError>>>) -> Self {
        Self {
            turns: Mutex::new(turns.into_iter().collect()),
        }
    }
}

impl LlmBackend for ScriptedBackend {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        unimplemented!("ScriptedBackend streams only")
    }

    async fn stream(&self, _req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        let chunks = self
            .turns
            .lock()
            .expect("lock poisoned")
            .pop_front()
            .ok_or_else(|| LlmError::Internal {
                message: "ScriptedBackend: no more turns scripted".into(),
            })?;
        Ok(Box::pin(async_stream::stream! {
            for chunk in chunks {
                yield chunk;
            }
        }))
    }
}

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

/// Dispatcher that returns the scripted backend and panics on `invoke`
/// (validation must intercept the bad-args call before dispatch reaches here).
struct ScriptedDispatcher {
    backend: Arc<dyn DynLlmBackend>,
}

impl ToolDispatcher for ScriptedDispatcher {
    fn invoke<'a>(
        &'a self,
        tool_id: &'a tau_ir::ToolId,
        _args: &'a JsonValue,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        let msg = format!(
            "ScriptedDispatcher::invoke MUST NOT be called for tool {:?} \
             — tool-arg validation should have intercepted the bad-args call",
            tool_id.0
        );
        Box::pin(async move { Err(RuntimeError::Internal { message: msg }) })
    }

    fn llm_backend_for(&self, _backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        Ok(self.backend.clone())
    }
}

// ---------------------------------------------------------------------------
// IR fixture builder
// ---------------------------------------------------------------------------

/// JSON Schema for the strict-schema tool: requires `"x": string`.
///
/// `IrToolSpec.input_schema` is `serde_json::Value` (see tau-ir/src/node.rs).
fn strict_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": { "x": { "type": "string" } },
        "required": ["x"]
    })
}

/// Build an `IrModule` with a single agent that declares one tool with a
/// strict input schema.
fn fixture_module() -> (IrModule, AgentId) {
    let tool_id = ToolId("strict-tool".into());
    let entry = AgentId("a".into());

    // Zero content_hash for test purposes — the hash is not validated at
    // runtime dispatch (it participates in the IR bundle hash only).
    let zero_hash: Hash256 = [0u8; 32];
    let ir_tool = Tool {
        id: tool_id.clone(),
        impl_: ToolImpl::Native {
            fn_ref: NativeFnRef {
                name: "strict-tool".into(),
            },
            content_hash: zero_hash,
        },
        capabilities: tau_ir::capability::CapabilityRequirements::default(),
        spec: IrToolSpec {
            name: "strict-tool".into(),
            description: "Tool requiring x: string".into(),
            input_schema: strict_schema(),
        },
    };

    let agent_node = Agent {
        id: entry.clone(),
        prompt: String::new(),
        model_ref: tau_ir::ModelRef {
            backend: "scripted".into(),
            model_id: "scripted-model".into(),
        },
        tool_refs: vec![tool_id.clone()],
        context: None,
        budget: AgentBudget::default(),
        produces: Vec::new(),
        output_schema: None,
        durable: None,
    };

    let mut agents = BTreeMap::new();
    agents.insert(entry.clone(), agent_node);

    let mut tools = BTreeMap::new();
    tools.insert(tool_id, ir_tool);

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
            capability_table: CapabilityTable(BTreeMap::new()),
            pipeline: None,
            checks: BTreeMap::new(),
        },
        triggers: Vec::new(),
    };

    (module, entry)
}

/// Build a `tau_domain::Value` from a `serde_json::Value`.
fn to_domain(v: serde_json::Value) -> tau_domain::Value {
    serde_json::from_value(v).expect("domain Value round-trip")
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

/// Prove that `tool-validation` RUNS no_std:
///
/// - Turn 1: scripted LLM emits `ToolUse("strict-tool", {})` — args are
///   missing the required `"x"` field.  The `#[cfg(feature = "tool-validation")]`
///   block in `stream.rs` catches this, writes a `ToolError` self-correction
///   into the conversation, and yields
///   `RunEvent::ToolCallCompleted { result: Err(_) }`.
///   The dispatcher's `invoke` is NEVER called.
/// - Turn 2: scripted LLM sees the ToolError in its context and emits
///   `"self-corrected" + EndTurn` → the run completes successfully.
///
/// This is an integration proof: if `tool-validation` were absent or broken,
/// the dispatcher's `invoke` would be called (returning an error that kills
/// the run), or the bad-args turn would be silently skipped.
#[tokio::test]
async fn run_loop_no_std_validation_rejects_bad_args_and_self_corrects() {
    // Turn 1: ToolUse with missing required field "x".
    let bad_args = to_domain(serde_json::json!({}));
    let bad_tool_use =
        tau_ports::fixtures::make_tool_use("call_001".into(), "strict-tool".into(), bad_args);

    // Turn 2: text response after LLM sees the ToolError correction.
    let scripted = ScriptedBackend::new(vec![
        // Turn 1: ToolUse with bad args + ToolUse stop reason.
        vec![
            Ok(CompletionChunk::ToolUse(bad_tool_use)),
            Ok(CompletionChunk::Finish {
                stop_reason: StopReason::ToolUse,
                usage: None,
            }),
        ],
        // Turn 2: text + EndTurn (LLM self-corrected).
        vec![
            Ok(CompletionChunk::Text {
                delta: "self-corrected".into(),
            }),
            Ok(CompletionChunk::Finish {
                stop_reason: StopReason::EndTurn,
                usage: None,
            }),
        ],
    ]);

    let (module, entry) = fixture_module();
    let dispatcher = Arc::new(ScriptedDispatcher {
        backend: Arc::new(scripted),
    });

    let stream = run_ir_streaming(Arc::new(module), &entry, dispatcher, Vec::new())
        .await
        .expect("stream builds");
    let events: Vec<RunEvent> = Box::pin(stream).collect().await;

    // Locate the ToolCallCompleted event and verify it carries Err (BadArgs).
    let tool_completed = events
        .iter()
        .find(|e| matches!(e, RunEvent::ToolCallCompleted { .. }));
    assert!(
        tool_completed.is_some(),
        "expected ToolCallCompleted event (validation must fire); got: {events:#?}"
    );
    let RunEvent::ToolCallCompleted { name, result, .. } = tool_completed.unwrap() else {
        unreachable!()
    };
    assert_eq!(name, "strict-tool", "completed event should name the tool");
    assert!(
        result.is_err(),
        "ToolCallCompleted result must be Err (BadArgs self-correction); got: {result:?}"
    );
    // result: Result<ToolResult, String> — the Err variant holds a String reason.
    let reason = result.as_ref().unwrap_err();
    assert!(
        reason.contains("You sent:"),
        "BadArgs reason MUST contain 'You sent:'; got: {reason}"
    );
    assert!(
        reason.contains("Expected (input_schema):"),
        "BadArgs reason MUST contain 'Expected (input_schema):'; got: {reason}"
    );
    assert!(
        reason.contains("Specific issue"),
        "BadArgs reason MUST contain 'Specific issue'; got: {reason}"
    );

    // The run must end successfully (LLM self-corrected on turn 2).
    let last = events.last().expect("stream must have events");
    assert!(
        matches!(
            last,
            RunEvent::RunCompleted {
                outcome: RunOutcome::Completed { .. }
            }
        ),
        "stream must end with RunCompleted Completed; got: {last:?}"
    );
}
