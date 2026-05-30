//! End-to-end smoke test for tool-args schema validation (Tier 2 priority 6).
//!
//! Three scenarios exercise the full pipeline (LLM tool_use → kernel
//! validation → MessagePayload::ToolError in conversation → loop
//! continues OR completes):
//!
//! - **Bad args, missing required field:** LLM emits `{}` (no `path`).
//!   `fs-read`'s schema requires `path: string`. Expected: a
//!   `MessagePayload::ToolError { kind: "tool_args_validation", .. }`
//!   appears in the conversation. The run hits max_turns because the
//!   one-shot LLM keeps emitting the same bad call.
//!
//! - **Bad args, type mismatch:** LLM emits `{"path": 42}` (int instead
//!   of string). Same expected shape.
//!
//! - **Scripted self-correction:** turn 1 emits bad args, turn 2 emits
//!   good args. Expected: `RunOutcome::Completed`. The conversation
//!   contains BOTH the validation error and a follow-up tool result.
//!
//! Gated `#[cfg(unix)]`: matches the convention from `tool_plugin_e2e.rs`
//! (Windows tempfile paths break TOML embedding); the validator logic
//! itself is OS-agnostic and is exercised cross-platform via the
//! per-module unit tests in `crates/tau-runtime/src/tool_args.rs`.

#![cfg(unix)]

mod common;

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use fs_read_plugin_lib::plugin::{FsReadPlugin, FsReadSession};
use tau_domain::{MessagePayload, Value};
use tau_plugin_sdk::Configure;
use tau_ports::{
    fixtures::{make_completion_response, make_token_usage, make_tool_use},
    CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError, SessionContext,
    StopReason, Tool, ToolError, ToolResult, ToolSpec,
};
use tau_runtime::{builder::DynTool, RunOptions, RunOutcome, Runtime};

use assert_matches::assert_matches;

// ---------------------------------------------------------------------------
// InProcessFsRead: DynTool adapter bridging FsReadSession → ()
// (verbatim from tool_plugin_e2e.rs — see module-level rationale there)
// ---------------------------------------------------------------------------

struct InProcessFsRead {
    plugin: FsReadPlugin,
    session: tokio::sync::Mutex<Option<FsReadSession>>,
}

impl InProcessFsRead {
    fn new() -> Self {
        let plugin = FsReadPlugin::from_config(Default::default())
            .expect("FsReadPlugin::from_config must not fail with default config");
        Self {
            plugin,
            session: tokio::sync::Mutex::new(None),
        }
    }
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

impl DynTool for InProcessFsRead {
    fn name(&self) -> &str {
        Tool::name(&self.plugin)
    }

    fn schema(&self) -> ToolSpec {
        Tool::schema(&self.plugin)
    }

    fn capabilities(&self) -> &[tau_domain::Capability] {
        Tool::capabilities(&self.plugin)
    }

    fn init<'a>(&'a self, ctx: SessionContext) -> BoxFuture<'a, Result<(), ToolError>> {
        Box::pin(async move {
            let s = Tool::init(&self.plugin, ctx).await?;
            *self.session.lock().await = Some(s);
            Ok(())
        })
    }

    fn invoke<'a>(
        &'a self,
        _ctx: &'a SessionContext,
        _session: &'a mut (),
        args: Value,
    ) -> BoxFuture<'a, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let mut guard = self.session.lock().await;
            let s = guard
                .as_mut()
                .expect("InProcessFsRead: init must be called before invoke");
            Tool::invoke(&self.plugin, s, args).await
        })
    }

    fn teardown<'a>(&'a self, _session: ()) -> BoxFuture<'a, Result<(), ToolError>> {
        Box::pin(async move {
            let taken = self.session.lock().await.take();
            if let Some(s) = taken {
                Tool::teardown(&self.plugin, s).await?;
            }
            Ok(())
        })
    }
}

// ---------------------------------------------------------------------------
// LLM fixtures
// ---------------------------------------------------------------------------

/// Emits the SAME tool_use forever. Used by tests where the LLM keeps
/// retrying the same bad call — the loop should produce a validation
/// error in the conversation but not terminate.
struct RepeatLlm {
    args: Value,
}

impl LlmBackend for RepeatLlm {
    fn name(&self) -> &str {
        "test-llm"
    }

    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let tool_use = make_tool_use("call_1".into(), "fs-read".into(), self.args.clone());
        Ok(make_completion_response(
            String::new(),
            vec![tool_use],
            StopReason::ToolUse,
            Some(make_token_usage(10, 5)),
        ))
    }

    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        let resp = self.complete(req).await?;
        Ok(tau_ports::batch_to_stream(resp))
    }
}

/// Two-turn LLM: turn 1 emits a (configurable) tool_use, turn 2 emits a
/// different tool_use, turn 3 emits an EndTurn "done". Used by the
/// self-correction test: turn 1 is bad args, turn 2 is good args.
struct ScriptedLlm {
    responses: Mutex<VecDeque<CompletionResponse>>,
}

impl ScriptedLlm {
    fn new(turn1_args: Value, turn2_args: Value) -> Self {
        let tool_use_1 = make_tool_use("call_1".into(), "fs-read".into(), turn1_args);
        let turn1 = make_completion_response(
            String::new(),
            vec![tool_use_1],
            StopReason::ToolUse,
            Some(make_token_usage(10, 5)),
        );
        let tool_use_2 = make_tool_use("call_2".into(), "fs-read".into(), turn2_args);
        let turn2 = make_completion_response(
            String::new(),
            vec![tool_use_2],
            StopReason::ToolUse,
            Some(make_token_usage(10, 5)),
        );
        let turn3 = make_completion_response(
            "done".into(),
            Vec::new(),
            StopReason::EndTurn,
            Some(make_token_usage(8, 3)),
        );
        Self {
            responses: Mutex::new(vec![turn1, turn2, turn3].into()),
        }
    }
}

impl LlmBackend for ScriptedLlm {
    fn name(&self) -> &str {
        "test-llm"
    }

    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        self.responses
            .lock()
            .expect("responses mutex poisoned")
            .pop_front()
            .ok_or_else(|| LlmError::Internal {
                message: "ScriptedLlm: no more scripted responses".into(),
            })
    }

    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        let resp = self.complete(req).await?;
        Ok(tau_ports::batch_to_stream(resp))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a tau_domain::Value from a serde_json literal.
fn args(json: serde_json::Value) -> Value {
    let s = serde_json::to_string(&json).expect("args serialize");
    serde_json::from_str(&s).expect("args round-trip through tau_domain::Value")
}

/// Build a manifest with `fs.read` capability granting `/**` (broad enough
/// to never block on capability checks; we want validation to be the
/// only gate exercised in these tests).
fn manifest_with_fs_read_global() -> tau_domain::PackageManifest {
    common::manifest_from_toml(
        r#"
name = "test-pkg"
version = "0.1.0"
description = "test package"
authors = []
source = "https://example.com/test.git"
kind = "tool"
dependencies = []

[[capabilities]]
kind = "fs.read"
paths = ["/**"]
"#,
    )
}

/// Find the FIRST MessagePayload::ToolError with kind "tool_args_validation"
/// in the conversation. Returns None if absent.
fn find_validation_error(messages: &[tau_domain::Message]) -> Option<(&str, &str)> {
    messages.iter().find_map(|m| match &m.payload {
        MessagePayload::ToolError { kind, message, .. } if kind == "tool_args_validation" => {
            Some((kind.as_str(), message.as_str()))
        }
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Bad args, missing required field. LLM emits `{}` repeatedly. fs-read's
/// schema requires `path: string`. Validation should fail; conversation
/// should contain a tool_args_validation MessagePayload::ToolError. The
/// run hits max_turns because the one-shot LLM keeps emitting the same
/// bad call.
#[tokio::test]
async fn bad_args_missing_required_field_surfaces_in_conversation() {
    let llm = RepeatLlm {
        args: args(serde_json::json!({})),
    };
    let tool: Arc<dyn DynTool> = Arc::new(InProcessFsRead::new());

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_dyn_tool(tool)
        .build()
        .expect("Runtime builds");

    let agent_def = common::agent_def(
        "agent-validation",
        "test-agent",
        "test-pkg@0.1.0",
        "test-llm",
    );
    let manifest = manifest_with_fs_read_global();
    let initial = common::user_message("read the file");

    // Cap turns low so the test finishes quickly even though the LLM
    // never stops emitting bad args.
    let mut options = common::run_options();
    options.max_turns = 2;

    let outcome = runtime
        .run(agent_def, manifest, initial, options)
        .await
        .expect("agent-level failures flow through Ok(RunOutcome::Failed)");

    // Outcome can be Failed (max_turns reached) — that's expected since
    // our one-shot LLM never emits a successful call.
    let messages = match &outcome {
        RunOutcome::Completed { all_messages, .. } => all_messages.clone(),
        RunOutcome::Failed { all_messages, .. } => all_messages.clone(),
        _ => panic!("unexpected RunOutcome variant"),
    };

    let (kind, message) = find_validation_error(&messages)
        .expect("conversation must contain a tool_args_validation ToolError");
    assert_eq!(kind, "tool_args_validation");
    assert!(
        message.contains("You sent:"),
        "MANDATORY rule: message must contain 'You sent:'; got: {message}"
    );
    assert!(
        message.contains("Expected (input_schema):"),
        "MANDATORY rule: message must contain 'Expected (input_schema):'; got: {message}"
    );
    assert!(
        message.contains("Specific issue"),
        "MANDATORY rule: message must contain 'Specific issue'; got: {message}"
    );
}

/// Bad args, type mismatch. LLM emits `{"path": 42}` (int, schema wants
/// string). Same expected shape.
#[tokio::test]
async fn bad_args_type_mismatch_surfaces_in_conversation() {
    let llm = RepeatLlm {
        args: args(serde_json::json!({ "path": 42 })),
    };
    let tool: Arc<dyn DynTool> = Arc::new(InProcessFsRead::new());

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_dyn_tool(tool)
        .build()
        .expect("Runtime builds");

    let agent_def = common::agent_def(
        "agent-validation",
        "test-agent",
        "test-pkg@0.1.0",
        "test-llm",
    );
    let manifest = manifest_with_fs_read_global();
    let initial = common::user_message("read the file");

    let mut options = common::run_options();
    options.max_turns = 2;

    let outcome = runtime
        .run(agent_def, manifest, initial, options)
        .await
        .expect("agent-level failures flow through Ok(RunOutcome::Failed)");

    let messages = match &outcome {
        RunOutcome::Completed { all_messages, .. } => all_messages.clone(),
        RunOutcome::Failed { all_messages, .. } => all_messages.clone(),
        _ => panic!("unexpected RunOutcome variant"),
    };

    let (kind, message) = find_validation_error(&messages)
        .expect("conversation must contain a tool_args_validation ToolError");
    assert_eq!(kind, "tool_args_validation");
    assert!(
        message.contains("You sent:"),
        "MANDATORY rule: 'You sent:' missing; got: {message}"
    );
    assert!(
        message.contains("Expected (input_schema):"),
        "MANDATORY rule: 'Expected (input_schema):' missing; got: {message}"
    );
    assert!(
        message.contains("Specific issue"),
        "MANDATORY rule: 'Specific issue' missing; got: {message}"
    );
}

/// Scripted self-correction: turn 1 = bad args (path: int), turn 2 =
/// good args (path: nonexistent file string). Expected:
/// RunOutcome::Completed (the loop survived the validation error). The
/// conversation contains BOTH a tool_args_validation ToolError AND a
/// follow-up tool result/error from turn 2's invoke (the file-not-found
/// IO error from fs-read returns is_error=true → MessagePayload::ToolError
/// with kind="tool_runtime_error", but it's a NEW message after the
/// validation error).
#[tokio::test]
async fn scripted_llm_self_corrects_after_validation_error() {
    let bad_args = args(serde_json::json!({ "path": 42 }));
    // Turn 2 uses a path under /tmp (covered by manifest's /** grant).
    // The file doesn't exist → fs-read returns is_error=true ToolResult,
    // which is a successful tool call from the loop's perspective.
    let good_args = args(serde_json::json!({ "path": "/tmp/tau-test-nonexistent-file" }));
    let llm = ScriptedLlm::new(bad_args, good_args);
    let tool: Arc<dyn DynTool> = Arc::new(InProcessFsRead::new());

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_dyn_tool(tool)
        .build()
        .expect("Runtime builds");

    let agent_def = common::agent_def(
        "agent-validation",
        "test-agent",
        "test-pkg@0.1.0",
        "test-llm",
    );
    let manifest = manifest_with_fs_read_global();
    let initial = common::user_message("read the file");

    let outcome = runtime
        .run(agent_def, manifest, initial, common::run_options())
        .await
        .expect("scripted scenario completes cleanly");

    assert_matches!(
        outcome,
        RunOutcome::Completed { all_messages, .. } => {
            // The conversation MUST contain the validation error.
            let validation_error = find_validation_error(&all_messages);
            assert!(
                validation_error.is_some(),
                "conversation must contain a tool_args_validation ToolError"
            );

            // The conversation MUST contain a SECOND tool-output message after
            // the validation error — either ToolResult (file unexpectedly
            // existed) or ToolError with kind != "tool_args_validation"
            // (file-not-found surfaced through fs-read's invoke).
            let validation_idx = all_messages
                .iter()
                .position(|m| {
                    matches!(&m.payload, MessagePayload::ToolError { kind, .. } if kind == "tool_args_validation")
                })
                .expect("validation error must be present");
            let post_validation_count = all_messages[validation_idx + 1..]
                .iter()
                .filter(|m| {
                    matches!(
                        &m.payload,
                        MessagePayload::ToolResult { .. } | MessagePayload::ToolError { .. }
                    )
                })
                .count();
            assert!(
                post_validation_count >= 1,
                "expected at least one tool result/error AFTER the validation error \
                 (turn 2's invoke); got 0. Messages: {:#?}",
                all_messages
            );
        }
    );
}
