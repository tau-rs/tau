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

use tau_ir::lower::{lower_project, Caches};
use tau_ir::IrModule;
use tau_pkg::project::ProjectConfig;
use tau_ports::error::LlmError;
use tau_ports::llm::{
    batch_to_stream, CompletionRequest, CompletionResponse, CompletionStream, LlmBackend,
    StopReason,
};
use tau_ports::target::registry as target_registry;
use tau_ports::ToolUse;
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
///
/// Shared with `bundle_mode` so DevMode and BundleMode drive the IR
/// interpreter through the exact same in-process scripted backend.
pub(crate) struct SequencedLlm {
    name: String,
    queue: Mutex<VecDeque<CompletionResponse>>,
}

impl SequencedLlm {
    pub(crate) fn new(name: impl Into<String>, responses: Vec<CompletionResponse>) -> Self {
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
pub(crate) struct ToolCallRecord {
    pub(crate) tool_name: String,
    pub(crate) args_canonical: Vec<u8>,
}

/// A `ToolDispatcher` that records every tool invocation and returns a
/// canned successful response (`{"ok": true}`). The recording is then
/// harvested to build the `ConformanceReport`.
///
/// Shared with `bundle_mode` so DevMode and BundleMode produce
/// byte-identical recordings for the same fixture.
pub(crate) struct RecordingDispatcher {
    backend: Arc<dyn DynLlmBackend>,
    records: Arc<Mutex<Vec<ToolCallRecord>>>,
    /// Map from ToolId → tool name (used to look up the human name from the IR).
    tool_names: std::collections::BTreeMap<String, String>,
}

impl RecordingDispatcher {
    pub(crate) fn new(
        backend: Arc<dyn DynLlmBackend>,
        tool_names: std::collections::BTreeMap<String, String>,
    ) -> Self {
        Self {
            backend,
            records: Arc::new(Mutex::new(Vec::new())),
            tool_names,
        }
    }

    pub(crate) fn records(&self) -> Arc<Mutex<Vec<ToolCallRecord>>> {
        self.records.clone()
    }
}

impl ToolDispatcher for RecordingDispatcher {
    fn invoke<'a>(
        &'a self,
        tool_id: &'a tau_ir::ToolId,
        args: &'a JsonValue,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
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
            records
                .lock()
                .expect("records mutex poisoned")
                .push(ToolCallRecord {
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
///
/// Shared with `bundle_mode` so both modes consume the same script bytes
/// and produce the same scripted-response sequence.
pub(crate) fn parse_mock_llm(jsonl: &str) -> Vec<CompletionResponse> {
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
        // async trait future. Returns Ok(...) on successful lowering or
        // Err(refusal_string) when the build was refused at lowering —
        // BundleMode mirrors this exactly so the two modes report
        // build-refused fixtures symmetrically (see ConformanceReport).
        let lowered: Result<(IrModule, Vec<CompletionResponse>, String), String> = {
            // 1. Load workflow.toml.
            let workflow_toml = std::fs::read_to_string(fixture_dir.join("workflow.toml"))
                .expect("fixture must contain workflow.toml");
            let config = ProjectConfig::parse_str(&workflow_toml)
                .expect("workflow.toml must parse as a valid ProjectConfig");

            // 2. Lower to IrModule. Caches are stack-only closures that don't
            //    cross the await point — they are dropped at the end of this block.
            //    Uses the SHA-256-of-name native-tool cache symmetric with
            //    `tau-cli::cmd::build::lower_ir` so DevMode and BundleMode
            //    hash identical IR bytes for the same source workflow.
            let target = target_registry::list_available()
                .next()
                .expect("at least one target triple available")
                .triple;
            let module_result = {
                let caches = Caches {
                    native_tool: &|name: &str| Some(crate::sha256_name(name)),
                    mcp_contract: &|_| None,
                    skill: &|_| None,
                };
                lower_project(&config, &target, &caches)
            }; // caches dropped here

            match module_result {
                Ok(module) => {
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
                    Ok((module, responses, entry.0))
                }
                Err(e) => Err(format!("{e}")),
            }
        };

        match lowered {
            Ok((module, responses, entry_id)) => {
                let entry = tau_ir::AgentId(entry_id);
                drive_module(&module, &entry, responses).await
            }
            Err(refusal) => ConformanceReport::build_refused(refusal),
        }
    }
}

/// Drive the IR interpreter for a (decoded) module with a scripted LLM
/// backend, recording side effects into a `ConformanceReport`.
///
/// Shared by DevMode (in-process lowering) and BundleMode (round-tripped
/// through a built bundle). Both modes feed in identical
/// `(module, entry, responses)` inputs for the same fixture, so the
/// emitted reports compare byte-for-byte under `assert_conform`.
pub(crate) async fn drive_module(
    module: &IrModule,
    entry: &tau_ir::AgentId,
    responses: Vec<CompletionResponse>,
) -> ConformanceReport {
    // Build a name→name map for RecordingDispatcher (ToolId == tool name in v0).
    let tool_names: std::collections::BTreeMap<String, String> = module
        .workflow
        .tools
        .keys()
        .map(|id| (id.0.clone(), id.0.clone()))
        .collect();

    // Build SequencedLlm + RecordingDispatcher.
    let backend: Arc<dyn DynLlmBackend> = Arc::new(SequencedLlm::new("mock-llm", responses));
    let dispatcher = Arc::new(RecordingDispatcher::new(backend, tool_names));
    let records_handle = dispatcher.records();

    // Run the interpreter.
    let outcome: RunOutcome = run_ir(std::sync::Arc::new(module.clone()), entry, dispatcher, Vec::new())
        .await
        .expect("run_ir must not return an Err for a valid conformance fixture");

    // Build ConformanceReport from recorded side effects.
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
    for msg in &messages {
        // Canonicalize the message body before recording so two runs
        // with different per-run UUIDs / timestamps produce identical
        // multiset keys. See `canonical_message_bytes` for the exact
        // shape — payload + address discriminants only.
        let canonical = crate::canonical_message_bytes(msg);
        report.record_message(canonical);
    }

    report
}
