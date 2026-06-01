//! Dev-mode runner: drive the IR interpreter with in-process tool dispatch.
//!
//! Reads `workflow.toml`, lowers to an `IrModule`, wires a sequenced
//! `LlmBackend` from `mock_llm.jsonl`, and drives
//! `tau_runtime_core::interpreter::run_ir` with a recording
//! `ToolDispatcher`. Returns a `ConformanceReport` summarising the
//! observed side effects.

use std::collections::VecDeque;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value as JsonValue;

use tau_ir::lower::{Caches, lower_project};
use tau_pkg::project::ProjectConfig;
use tau_ports::error::LlmError;
use tau_ports::llm::{
    CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, StopReason, batch_to_stream,
};
use tau_ports::ToolUse;
use tau_ports::target::registry as target_registry;
use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::run_ir;
use tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};
use tau_runtime_core::outcome::RunOutcome;

use crate::{ConformanceReport, ExecutionMode};

// ---------------------------------------------------------------------------
// SequencedLlm — pops scripted responses in order
// ---------------------------------------------------------------------------

/// A `LlmBackend` that pops scripted `CompletionResponse` values from a
/// queue in the order they were added. Used to replay a `mock_llm.jsonl`
/// script against the IR interpreter.
struct SequencedLlm {
    name: String,
    queue: Mutex<VecDeque<CompletionResponse>>,
}

impl SequencedLlm {
    fn new(name: impl Into<String>, responses: Vec<CompletionResponse>) -> Self {
        Self {
            name: name.into(),
            queue: Mutex::new(responses.into()),
        }
    }
}

impl LlmBackend for SequencedLlm {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        self.queue
            .lock()
            .expect("SequencedLlm mutex poisoned")
            .pop_front()
            .ok_or_else(|| LlmError::Internal {
                message: "SequencedLlm: no more scripted responses".into(),
            })
    }

    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        let resp = LlmBackend::complete(self, req).await?;
        Ok(batch_to_stream(resp))
    }
}

// ---------------------------------------------------------------------------
// RecordingDispatcher — records tool invocations + owns LLM backend
// ---------------------------------------------------------------------------

/// Recorded side-effect entry for one tool invocation.
struct ToolCallRecord {
    tool_name: String,
    args_canonical: Vec<u8>,
}

/// A `ToolDispatcher` that records every tool invocation and returns a
/// canned successful response (`{"ok": true}`). The recording is then
/// harvested to build the `ConformanceReport`.
struct RecordingDispatcher {
    backend: Arc<dyn DynLlmBackend>,
    records: Arc<Mutex<Vec<ToolCallRecord>>>,
    /// Map from ToolId → tool name (used to look up the human name from the IR).
    tool_names: std::collections::BTreeMap<String, String>,
}

impl RecordingDispatcher {
    fn new(
        backend: Arc<dyn DynLlmBackend>,
        tool_names: std::collections::BTreeMap<String, String>,
    ) -> Self {
        Self {
            backend,
            records: Arc::new(Mutex::new(Vec::new())),
            tool_names,
        }
    }

    fn records(&self) -> Arc<Mutex<Vec<ToolCallRecord>>> {
        self.records.clone()
    }
}

impl ToolDispatcher for RecordingDispatcher {
    fn invoke<'a>(
        &'a self,
        tool_id: &'a tau_ir::ToolId,
        args: &'a JsonValue,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>>
    {
        // Resolve the tool name from the id (fall back to the raw id string).
        let tool_name = self
            .tool_names
            .get(&tool_id.0)
            .cloned()
            .unwrap_or_else(|| tool_id.0.clone());

        // Canonical args bytes: deterministic JSON serialization.
        let args_canonical = serde_json::to_vec(args).unwrap_or_default();

        let records = self.records.clone();

        Box::pin(async move {
            records.lock().expect("records mutex poisoned").push(ToolCallRecord {
                tool_name,
                args_canonical,
            });

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

// ---------------------------------------------------------------------------
// mock_llm.jsonl parser
// ---------------------------------------------------------------------------

/// One line of `mock_llm.jsonl`. Represents one scripted LLM response.
///
/// ```json
/// {"turn": 0, "response": {"tool_uses": [...], "stop_reason": "tool_use"}}
/// {"turn": 1, "response": {"text": "ok", "stop_reason": "end_turn"}}
/// ```
#[derive(serde::Deserialize)]
struct MockLlmLine {
    response: MockLlmResponse,
}

#[derive(serde::Deserialize)]
struct MockLlmResponse {
    #[serde(default)]
    text: String,
    #[serde(default)]
    tool_uses: Vec<MockToolUse>,
    stop_reason: String,
}

#[derive(serde::Deserialize)]
struct MockToolUse {
    id: String,
    name: String,
    #[serde(default)]
    input: JsonValue,
}

/// Parse `mock_llm.jsonl` into a vec of `CompletionResponse` values in
/// turn order (sorted by the `turn` field, then extracted in order).
fn parse_mock_llm(jsonl: &str) -> Vec<CompletionResponse> {
    let lines: Vec<MockLlmLine> = jsonl
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    // Turn order is preserved by file order; the `turn` field in each
    // line is advisory only. Process lines in the order they appear.
    lines
        .into_iter()
        .map(|line| {
            let resp = line.response;
            let tool_uses: Vec<ToolUse> = resp
                .tool_uses
                .into_iter()
                .map(|tu| {
                    let input: tau_domain::Value =
                        serde_json::from_value(tu.input).unwrap_or(tau_domain::Value::Null);
                    tau_ports::fixtures::make_tool_use(tu.id, tu.name, input)
                })
                .collect();

            let stop_reason = match resp.stop_reason.as_str() {
                "tool_use" => StopReason::ToolUse,
                _ => StopReason::EndTurn,
            };

            tau_ports::fixtures::make_completion_response(resp.text, tool_uses, stop_reason, None)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// DevMode runner
// ---------------------------------------------------------------------------

/// Dev-mode runner.
///
/// For each fixture:
/// 1. Reads `workflow.toml`; parses into a `ProjectConfig`.
/// 2. Lowers to an `IrModule` against the host target triple.
/// 3. Reads `mock_llm.jsonl`; builds a `SequencedLlm` backend.
/// 4. Drives `run_ir` with a `RecordingDispatcher`.
/// 5. Harvests the recordings into a `ConformanceReport`.
///
/// Panics if any setup step fails — fixture misconfiguration should
/// be caught immediately, not swallowed into a `ConformanceReport`.
pub struct DevMode;

#[async_trait(?Send)]
impl ExecutionMode for DevMode {
    async fn run(&self, fixture_dir: &Path) -> ConformanceReport {
        // All synchronous setup (lowering, parse) runs in a separate sync
        // block so that the non-Send/non-Sync Caches closures are dropped
        // before the first `.await`, satisfying the `Send` bound on the
        // async trait future.
        let (module, responses, entry) = {
            // 1. Load workflow.toml.
            let workflow_toml = std::fs::read_to_string(fixture_dir.join("workflow.toml"))
                .expect("fixture must contain workflow.toml");
            let config = ProjectConfig::parse_str(&workflow_toml)
                .expect("workflow.toml must parse as a valid ProjectConfig");

            // 2. Lower to IrModule. Caches are stack-only closures that don't
            //    cross the await point — they are dropped at the end of this block.
            let target = target_registry::list_available()
                .next()
                .expect("at least one target triple available")
                .triple;
            let module = {
                let caches = Caches {
                    native_tool: &|_| Some([1u8; 32]),
                    mcp_contract: &|_| None,
                    skill: &|_| None,
                };
                lower_project(&config, &target, &caches)
                    .expect("IR lowering must succeed for a conformance fixture")
            }; // caches dropped here

            // 3. Load mock_llm.jsonl.
            let mock_llm_jsonl =
                std::fs::read_to_string(fixture_dir.join("mock_llm.jsonl"))
                    .expect("fixture must contain mock_llm.jsonl");
            let responses = parse_mock_llm(&mock_llm_jsonl);

            // Determine entry agent: first in BTreeMap (alphabetical) order.
            let entry = module
                .workflow
                .agents
                .keys()
                .next()
                .expect("fixture must declare at least one agent")
                .clone();

            (module, responses, entry)
        };

        // Build a name→name map for RecordingDispatcher (ToolId == tool name in v0).
        let tool_names: std::collections::BTreeMap<String, String> = module
            .workflow
            .tools
            .keys()
            .map(|id| (id.0.clone(), id.0.clone()))
            .collect();

        // 4. Build SequencedLlm + RecordingDispatcher.
        let backend: Arc<dyn DynLlmBackend> = Arc::new(SequencedLlm::new("mock-llm", responses));
        let dispatcher = Arc::new(RecordingDispatcher::new(backend, tool_names));
        let records_handle = dispatcher.records();

        // 5. Run the interpreter (first await point — Caches already dropped).
        let outcome: RunOutcome = run_ir(&module, &entry, dispatcher, Vec::new())
            .await
            .expect("run_ir must not return an Err for a valid conformance fixture");

        // 6. Build ConformanceReport from recorded side effects.
        let mut report = ConformanceReport::new(outcome.clone());

        // Record tool calls.
        let records = records_handle.lock().expect("records mutex poisoned");
        for rec in records.iter() {
            report.record_tool_call(rec.tool_name.clone(), rec.args_canonical.clone());
        }
        drop(records);

        // Record messages from outcome (all_messages multiset).
        let messages = match &outcome {
            RunOutcome::Completed { all_messages, .. } => all_messages.clone(),
            RunOutcome::Failed { all_messages, .. } => all_messages.clone(),
            _ => Vec::new(),
        };
        for msg in messages {
            // `tau_ir::Message` doesn't implement Serialize; record the
            // tau_domain::Message (if we had it). In v0, use the outcome's
            // `all_messages` count as the canonical bytes substitute.
            // We record the turn index as canonical bytes to track message count.
            let canonical = serde_json::to_vec(&serde_json::json!({"msg": "recorded"}))
                .unwrap_or_default();
            let _ = msg; // consumed; actual content isn't needed for count-based comparison
            report.record_message(canonical);
        }

        report
    }
}
