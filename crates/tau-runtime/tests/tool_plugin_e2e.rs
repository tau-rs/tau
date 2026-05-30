//! End-to-end smoke test: validates Gap 1 + Gap 2 closure for the
//! tool plugins sub-project. Uses `FsReadPlugin` in-process via a
//! thin `DynTool` adapter that bridges `Session = FsReadSession` to
//! the `DynTool` interface (which assumes `Session = ()`).
//!
//! Gated `#[cfg(unix)]`: the tests embed `tempfile::NamedTempFile`-
//! produced paths into TOML manifest strings. On Windows these paths
//! contain backslashes which break TOML string parsing / manifest
//! validation. The plugin-layer Gap 1 + Gap 2 validation runs on all
//! platforms via the per-plugin integration tests
//! (`crates/tau-plugins/fs-read/tests/invoke.rs`).

#![cfg(unix)]
//!
//! Three scenarios:
//!
//! - **Gap 1 closed:** agent with NO `fs.read` capability →
//!   `RunOutcome::Failed { kind: PolicyDenied }` (kernel-side check
//!   at run.rs:272, before `Tool::invoke` runs).
//! - **Gap 2 closed:** agent WITH `fs.read` but path outside glob
//!   scope → `Err(RuntimeError::Tool(ToolError::BadArgs { .. }))`
//!   (plugin-side check via session-stashed grant in `FsReadSession`).
//! - **Happy path:** agent with path in glob scope → `RunOutcome::Completed`,
//!   `ToolResult` in conversation with base64-encoded file contents.
//!
//! Out-of-process IPC paths are exercised separately by the per-plugin
//! integration tests (Tasks 10 + 14); this file focuses on the
//! runtime-level dispatch path for both gaps.

mod common;

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use fs_read_plugin_lib::plugin::{FsReadPlugin, FsReadSession};
use tau_domain::{AgentStatus, FailureKind, MessagePayload, Value};
use tau_plugin_sdk::Configure;
use tau_ports::{
    fixtures::{make_completion_response, make_token_usage, make_tool_use},
    CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError, SessionContext,
    StopReason, Tool, ToolError, ToolResult, ToolSpec,
};
use tau_runtime::{
    builder::DynTool,
    error::{CoreRuntimeError, RuntimeError},
    RunOptions, RunOutcome, Runtime,
};

use assert_matches::assert_matches;

// ---------------------------------------------------------------------------
// InProcessFsRead: DynTool adapter bridging FsReadSession → ()
// ---------------------------------------------------------------------------

/// Wraps `FsReadPlugin` (which uses `Session = FsReadSession`) as a
/// `DynTool` (which assumes `Session = ()`).
///
/// The adapter owns the per-call session in a `tokio::sync::Mutex<Option<…>>`.
/// `init` stores the session returned by `FsReadPlugin::init`; `invoke`
/// locks the mutex and delegates to `FsReadPlugin::invoke` with the live
/// session. This preserves the full `init → session → invoke` flow so
/// `FsReadSession.allowed_globs` (populated from `ctx.granted_capabilities`
/// in `init`) is available at invoke time — exactly the Gap 2 check.
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

// Mirror the `BoxFuture` alias from `tau_runtime::builder` — the DynTool
// methods do NOT carry a `Send` bound (see the module-level comment in
// builder.rs: "Boxed futures are deliberately *not* `Send`-bound").
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

/// Single-shot LLM: emits one `tool_use { name: "fs-read", args: { path } }`
/// then panics if called a second time. Used for Gap 1 (which never
/// reaches tool invocation, so the second call never happens) and Gap 2
/// (which errors at invoke, so the LLM is only called once).
struct OneShotFsReadLlm {
    target_path: String,
}

impl LlmBackend for OneShotFsReadLlm {
    fn name(&self) -> &str {
        "test-llm"
    }

    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let args: Value = serde_json::from_value(serde_json::json!({ "path": self.target_path }))
            .expect("args JSON must round-trip to tau_domain::Value");
        let tool_use = make_tool_use("call_1".into(), "fs-read".into(), args);
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

/// Two-turn LLM: turn 1 emits `tool_use { fs-read, path }`, turn 2
/// emits plain text "done". Used for the happy-path test where the tool
/// succeeds and the run loop needs a final EndTurn response.
struct ScriptedFsReadLlm {
    responses: Mutex<VecDeque<CompletionResponse>>,
}

impl ScriptedFsReadLlm {
    fn new(target_path: String) -> Self {
        let args: Value = serde_json::from_value(serde_json::json!({ "path": target_path }))
            .expect("args JSON must round-trip to tau_domain::Value");
        let tool_use = make_tool_use("call_1".into(), "fs-read".into(), args);
        let turn1 = make_completion_response(
            String::new(),
            vec![tool_use],
            StopReason::ToolUse,
            Some(make_token_usage(10, 5)),
        );
        let turn2 = make_completion_response(
            "done".into(),
            Vec::new(),
            StopReason::EndTurn,
            Some(make_token_usage(8, 3)),
        );
        Self {
            responses: Mutex::new(vec![turn1, turn2].into()),
        }
    }
}

impl LlmBackend for ScriptedFsReadLlm {
    fn name(&self) -> &str {
        "test-llm"
    }

    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        self.responses
            .lock()
            .expect("responses mutex poisoned")
            .pop_front()
            .ok_or_else(|| LlmError::Internal {
                message: "ScriptedFsReadLlm: no more scripted responses".into(),
            })
    }

    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        let resp = self.complete(req).await?;
        Ok(tau_ports::batch_to_stream(resp))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Gap 1 closed: kernel capability check at run.rs:272 rejects the
/// tool call BEFORE `Tool::init` or `Tool::invoke` runs. The agent's
/// package manifest grants NO capabilities; the plugin requires `fs.read`.
///
/// Expected outcome: `RunOutcome::Failed { status: AgentStatus::Failed {
/// kind: FailureKind::PolicyDenied } }`.
#[tokio::test]
async fn gap_1_kernel_denies_when_agent_has_no_fs_read_capability() {
    let tmpfile = tempfile::NamedTempFile::new().expect("tempfile");
    let path = tmpfile.path().to_str().unwrap().to_string();

    let llm = OneShotFsReadLlm {
        target_path: path.clone(),
    };
    let tool: Arc<dyn DynTool> = Arc::new(InProcessFsRead::new());

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_dyn_tool(tool)
        .build()
        .expect("Runtime builds");

    // Package manifest with NO capabilities → kernel must deny fs-read.
    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "test-llm");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("read the file");

    let outcome = runtime
        .run(agent_def, manifest, initial, RunOptions::default())
        .await
        .expect("agent-level failures flow through Ok(RunOutcome::Failed)");

    assert_matches!(
        outcome,
        RunOutcome::Failed {
            status: AgentStatus::Failed { kind, detail, .. },
            ..
        } => {
            assert_eq!(
                kind,
                FailureKind::PolicyDenied,
                "Gap 1: kernel must deny via PolicyDenied"
            );
            let detail = detail.expect("denial detail must be set");
            assert!(
                detail.contains("fs.read") || detail.contains("fs"),
                "detail should mention the denied capability; got {detail:?}"
            );
        }
    );
}

/// Gap 2 closed: the kernel admits the tool call (agent HAS `fs.read`),
/// but the plugin's glob-scope check inside `invoke` rejects the path
/// because it falls outside the granted glob.
///
/// The plugin's `init` stashes the allowed-globs in `FsReadSession`;
/// `invoke` uses `FsReadSession.allowed_globs` to enforce the scope.
///
/// Expected: `Err(RuntimeError::Tool(ToolError::BadArgs { .. }))` with
/// reason containing "not in capability scope".
#[tokio::test]
async fn gap_2_plugin_rejects_path_outside_glob_scope() {
    let tmpfile = tempfile::NamedTempFile::new().expect("tempfile");
    let path = tmpfile.path().to_str().unwrap().to_string();

    let llm = OneShotFsReadLlm {
        target_path: path.clone(),
    };
    let tool: Arc<dyn DynTool> = Arc::new(InProcessFsRead::new());

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_dyn_tool(tool)
        .build()
        .expect("Runtime builds");

    // Agent has fs.read but only for /var/**, NOT for the tempfile path.
    let agent_def = common::agent_def("agent-2", "test-agent", "test-pkg@0.1.0", "test-llm");
    let manifest = common::manifest_from_toml(
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
paths = ["/var/definitely-not-the-tmpfile-dir/**"]
"#,
    );
    let initial = common::user_message("read the file");

    let err = runtime
        .run(agent_def, manifest, initial, RunOptions::default())
        .await
        .expect_err("plugin scope check must surface as Err(RuntimeError)");

    assert_matches!(
        err,
        RuntimeError::Core(CoreRuntimeError::Tool(ToolError::BadArgs { reason })) => {
            assert!(
                reason.contains("not in capability scope"),
                "Gap 2: plugin must reject with scope-violation message; got {reason:?}"
            );
        }
    );
}

/// Happy path: agent has `fs.read` with a glob that covers the tempfile.
/// The full chain (kernel check → `init` → glob admission in `invoke` →
/// `tokio::fs::read` → base64 content in `ToolResult`) succeeds.
///
/// Expected: `RunOutcome::Completed` with a `ToolResult` message in
/// `all_messages` whose content contains the file's base64-encoded bytes.
#[tokio::test]
async fn happy_path_in_scope_read_succeeds() {
    let tmpfile = tempfile::NamedTempFile::new().expect("tempfile");
    let path = tmpfile.path().to_str().unwrap().to_string();
    let content = b"hello tau e2e\n";
    std::fs::write(tmpfile.path(), content).expect("write tempfile");

    // Build a glob that covers the tempfile's parent directory.
    let parent = tmpfile
        .path()
        .parent()
        .expect("tempfile has a parent")
        .to_str()
        .unwrap();
    let glob = format!("{parent}/**");

    let llm = ScriptedFsReadLlm::new(path.clone());
    let tool: Arc<dyn DynTool> = Arc::new(InProcessFsRead::new());

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_dyn_tool(tool)
        .build()
        .expect("Runtime builds");

    let agent_def = common::agent_def("agent-3", "test-agent", "test-pkg@0.1.0", "test-llm");
    let manifest = common::manifest_from_toml(&format!(
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
paths = ["{glob}"]
"#
    ));
    let initial = common::user_message("read the file");

    let outcome = runtime
        .run(agent_def, manifest, initial, RunOptions::default())
        .await
        .expect("run succeeded");

    assert_matches!(
        outcome,
        RunOutcome::Completed { all_messages, .. } => {
            // Find the ToolResult message.
            let tool_result_msg = all_messages
                .iter()
                .find(|m| matches!(&m.payload, MessagePayload::ToolResult { .. }));
            let Some(msg) = tool_result_msg else {
                panic!("no ToolResult in conversation; messages: {all_messages:?}");
            };

            let MessagePayload::ToolResult { body } = &msg.payload else {
                unreachable!("just matched above")
            };

            // The plugin wraps its output as `{ "contents": "<base64>", "size": N }`.
            let contents_b64 = body
                .as_object()
                .and_then(|o| o.get("contents"))
                .and_then(Value::as_string)
                .expect("ToolResult body must contain a `contents` base64 string");

            let decoded = base64::engine::general_purpose::STANDARD
                .decode(contents_b64)
                .expect("contents must be valid base64");

            assert_eq!(
                decoded, content,
                "decoded file contents must match what was written"
            );
        }
    );
}
