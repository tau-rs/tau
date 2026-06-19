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

    fn llm_backend_for(&self, _backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        // Single-backend recording dispatcher: every agent/judge resolves to
        // the one mock backend regardless of the name baked into its model_ref.
        Ok(self.backend.clone())
    }

    fn deterministic_registry(
        &self,
    ) -> Option<
        std::sync::Arc<dyn tau_runtime_core::interpreter::deterministic::DeterministicRegistry>,
    > {
        Some(crate::fixture_deterministic_registry())
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
            // Parse + validate: project-config validation errors (e.g.
            // `DeliverableNoProducer`) surface here as `Err`, not at the
            // `lower_project` stage — capture them symmetrically with
            // BundleMode so build-refused fixtures compare correctly.
            let config = match ProjectConfig::parse_str(&workflow_toml) {
                Ok(c) => c,
                Err(e) => {
                    // Move the mock_llm.jsonl read out of scope (it is only
                    // consumed when the run actually executes), then early-
                    // exit with the refusal string — no lowering needed.
                    return ConformanceReport::build_refused(format!("{e}"));
                }
            };

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
                let module = Arc::new(module);
                // Pipeline workflows are engine-sequenced: there is no
                // single entry agent loop. Branch to the pipeline driver
                // when the lowered module declares one; otherwise keep the
                // single-entry `run_ir` path unchanged (fixtures 01-07).
                if module.workflow.pipeline.is_some() {
                    drive_pipeline(module, CONFORMANCE_PIPELINE_INPUT.to_string(), responses).await
                } else {
                    let entry = tau_ir::AgentId(entry_id);
                    drive_module(module, &entry, responses).await
                }
            }
            Err(refusal) => ConformanceReport::build_refused(refusal),
        }
    }
}

/// Fixed run input fed to a pipeline's first step (`${input}`). Held
/// constant so DevMode and BundleMode render the same step templates and
/// produce byte-identical reports under `assert_conform`.
pub(crate) const CONFORMANCE_PIPELINE_INPUT: &str = "conformance-input";

/// Drive the IR interpreter for a (decoded) module with a scripted LLM
/// backend, recording side effects into a `ConformanceReport`.
///
/// Shared by DevMode (in-process lowering) and BundleMode (round-tripped
/// through a built bundle). Both modes feed in identical
/// `(module, entry, responses)` inputs for the same fixture, so the
/// emitted reports compare byte-for-byte under `assert_conform`.
pub(crate) async fn drive_module(
    module: Arc<IrModule>,
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

    // Run the interpreter. The Arc is passed directly — no clone needed.
    let outcome: RunOutcome = run_ir(module, entry, dispatcher, Vec::new())
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

/// Drive an `IrModule`'s engine-sequenced pipeline with a scripted LLM
/// backend, recording side effects into a `ConformanceReport`.
///
/// Used instead of [`drive_module`] when the lowered module declares a
/// `workflow.pipeline` (the single-entry agent loop does not exist for
/// pipeline workflows — the engine sequences the steps). Shared by
/// DevMode (in-process lowering) and BundleMode (round-tripped through a
/// built bundle); both feed identical `(module, responses)` for the same
/// fixture so the emitted reports compare under `assert_conform`.
///
/// `run_pipeline` returns only the per-step output store (it discards the
/// internal per-agent `RunOutcome`s and message histories), so:
///
/// - **Tool calls** are still captured: each pipeline agent runs through
///   the SAME shared `RecordingDispatcher`, so any tool the agents invoke
///   is recorded exactly as in `drive_module`.
/// - **Messages** are NOT observable here — the pipeline executor does not
///   surface them — so `message_added` stays empty. Cross-mode
///   conformance still holds because BOTH modes drive the same
///   `run_pipeline` and observe the same (empty) message multiset.
/// - **Outcome** is synthesized as `RunOutcome::Completed` on `Ok`. A
///   pipeline-step failure surfaces as `Err(RuntimeError)` from
///   `run_pipeline`, which we map to `RunOutcome::Failed` so both modes
///   report a failed pipeline symmetrically.
pub(crate) async fn drive_pipeline(
    module: Arc<IrModule>,
    input: String,
    responses: Vec<CompletionResponse>,
) -> ConformanceReport {
    use tau_runtime_core::interpreter::pipeline::run_pipeline;

    // Build a name→name map for RecordingDispatcher (ToolId == tool name in v0).
    let tool_names: std::collections::BTreeMap<String, String> = module
        .workflow
        .tools
        .keys()
        .map(|id| (id.0.clone(), id.0.clone()))
        .collect();

    let backend: Arc<dyn DynLlmBackend> = Arc::new(SequencedLlm::new("mock-llm", responses));
    let dispatcher = Arc::new(RecordingDispatcher::new(backend, tool_names));
    let records_handle = dispatcher.records();

    // The id of the LAST pipeline step in execution order — its stored
    // output is the run's final result. Captured before `module` is moved
    // into `run_pipeline`. Mirrors production `render_pipeline_result`
    // (tau-cli::cmd::run), which keys off `pipeline.steps.last()` rather
    // than the alphabetically-last id surfaced by `template_map().into_values()`:
    // the BTreeMap projection only coincidentally yields the last-executed
    // step when step ids sort in execution order (fixture 08: `gather` <
    // `writer`). A reverse-alphabetical fixture would silently harvest the
    // wrong step.
    let last_step_id: Option<String> = module
        .workflow
        .pipeline
        .as_ref()
        .and_then(|p| p.steps.last())
        .map(|step| step.id.0.clone());

    match run_pipeline(module, input, dispatcher).await {
        Ok(store) => {
            // Synthesize a Completed outcome carrying the last step's
            // output text as the final message (the pipeline executor
            // does not return a RunOutcome of its own). The message
            // multiset stays empty — the pipeline executor surfaces no
            // message history — but tool calls recorded by the shared
            // dispatcher are harvested below.
            //
            // A `Value::String` renders as its inner text; any structured
            // value renders as compact JSON — symmetric with
            // `OutputStore::template_map` and production
            // `render_pipeline_result`.
            let final_text = last_step_id
                .as_deref()
                .and_then(|id| store.get(id))
                .map(|value| match value {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            let final_message = pipeline_final_message(&final_text);
            let outcome = RunOutcome::Completed {
                final_message,
                all_messages: Vec::new(),
                total_turns: 0,
                token_usage: Default::default(),
            };
            let mut report = ConformanceReport::new(outcome);
            let records = records_handle.lock().expect("records mutex poisoned");
            for rec in records.iter() {
                report.record_tool_call(rec.tool_name.clone(), rec.args_canonical.clone());
            }
            report
        }
        // A pipeline-step failure surfaces as Err here. Map it to a
        // Failed outcome so both modes report the failure symmetrically
        // (assert_conform compares the RunOutcome *discriminant*; the `e`
        // detail is not part of the compared key, matching how the
        // single-agent path discards per-run nondeterminism).
        Err(_e) => ConformanceReport::new(RunOutcome::Failed {
            status: tau_domain::AgentStatus::Stopped,
            all_messages: Vec::new(),
            total_turns: 0,
            token_usage: Default::default(),
        }),
    }
}

/// Build the synthetic `final_message` for a completed pipeline run from
/// the last step's output text. Uses the same `Message::new` /
/// `Address::User → Address::System` shape so the value is canonicalizable
/// by `canonical_message_bytes` if a caller ever records it; the pipeline
/// path leaves `message_added` empty, so this only populates the outcome's
/// `final_message` slot.
fn pipeline_final_message(text: &str) -> tau_domain::Message {
    use tau_domain::{Address, Message, MessagePayload};
    Message::new(
        Address::System,
        Address::User,
        MessagePayload::Text {
            content: text.to_string(),
        },
    )
}
