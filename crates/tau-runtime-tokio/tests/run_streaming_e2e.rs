#![cfg(unix)]
//! End-to-end integration tests for `Runtime::run_streaming` and
//! `run_streaming_with_history`. Realizes the spec §7 testing tier
//! for Tier 2 priority 8.
//!
//! Mirrors the in-process FsReadPlugin DynTool adapter pattern from
//! `tool_plugin_e2e.rs` (priority 3) and `tool_args_validation_e2e.rs`
//! (priority 6).

mod common;

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use fs_read_plugin_lib::plugin::{FsReadPlugin, FsReadSession};
use tau_domain::{AgentStatus, FailureKind, Value};
use tau_plugin_sdk::Configure;
use tau_ports::{
    fixtures::{make_completion_response, make_token_usage, make_tool_use},
    CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError, SessionContext,
    StopReason, Tool, ToolError, ToolResult, ToolSpec,
};
use tau_runtime_tokio::{builder::DynTool, RunEvent, RunOutcome, Runtime};

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

/// Scripted LLM: emits a pre-configured sequence of `CompletionResponse`s,
/// one per call to `complete` / `stream`. Used by all streaming e2e tests.
struct ScriptedLlm {
    responses: Mutex<VecDeque<CompletionResponse>>,
}

impl ScriptedLlm {
    fn new(responses: Vec<CompletionResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
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

/// Collect all `RunEvent`s from a stream until it returns `None`.
/// Accepts both `RunCompleted` and `FatalError` as terminal markers.
async fn collect_events(
    stream: impl futures_core::Stream<Item = RunEvent> + 'static,
) -> Vec<RunEvent> {
    let mut stream = Box::pin(stream);
    let mut events = Vec::new();
    loop {
        let next = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await;
        match next {
            None => break,
            Some(event) => {
                let is_terminal = matches!(
                    &event,
                    RunEvent::RunCompleted { .. } | RunEvent::FatalError { .. }
                );
                events.push(event);
                if is_terminal {
                    break;
                }
            }
        }
    }
    events
}

/// Build a manifest with `fs.read` capability granting `/**` (broad
/// enough to cover any path we create in /tmp).
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

/// Round-trip a `serde_json::Value` into `tau_domain::Value`.
fn to_value(json: serde_json::Value) -> Value {
    let s = serde_json::to_string(&json).expect("serialize");
    serde_json::from_str(&s).expect("round-trip")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Test 1: Scripted LLM emits Text("Hi") + Text(" there") + Finish(EndTurn).
/// Expected event sequence: TextDelta("Hi") → TextDelta(" there") →
/// TurnCompleted → RunCompleted { Completed }.
#[tokio::test]
async fn text_only_run_streams_text_deltas() {
    let turn1 = make_completion_response(
        "Hi there".into(), // text-only response
        Vec::new(),
        StopReason::EndTurn,
        Some(make_token_usage(10, 5)),
    );
    let llm = ScriptedLlm::new(vec![turn1]);
    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .build()
        .expect("Runtime builds");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "test-llm");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("say hi");

    let stream = runtime
        .run_streaming(agent_def, manifest, initial, common::run_options())
        .await
        .expect("run_streaming should not fail at construction time");

    let events = collect_events(stream).await;

    // batch_to_stream emits the entire text as a single Text chunk, then
    // the Finish chunk. So we expect: TextDelta → TurnCompleted → RunCompleted.
    // The text may be one delta ("Hi there") or split — either way the last
    // non-terminal events before TurnCompleted are TextDelta(s).
    assert!(
        events.len() >= 3,
        "expected at least 3 events (TextDelta, TurnCompleted, RunCompleted); got: {events:#?}"
    );

    // First event is the typed RunStarted gate event (β.7.5); the text
    // deltas follow once inference begins.
    assert!(
        matches!(&events[0], RunEvent::RunStarted),
        "expected first event to be RunStarted, got {:?}",
        events[0]
    );

    // A TextDelta arrives before the turn completes.
    let first_text = events
        .iter()
        .position(|e| matches!(e, RunEvent::TextDelta { .. }));
    let first_turn_completed = events
        .iter()
        .position(|e| matches!(e, RunEvent::TurnCompleted { .. }));
    assert!(
        matches!((first_text, first_turn_completed), (Some(t), Some(tc)) if t < tc),
        "expected a TextDelta before TurnCompleted, got {events:#?}"
    );

    // Concatenated text should equal the original.
    let concatenated: String = events
        .iter()
        .filter_map(|e| {
            if let RunEvent::TextDelta { delta } = e {
                Some(delta.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        concatenated, "Hi there",
        "concatenated TextDelta events must equal the original text"
    );

    // Second-to-last event: TurnCompleted.
    let n = events.len();
    assert!(
        matches!(&events[n - 2], RunEvent::TurnCompleted { .. }),
        "expected second-to-last event to be TurnCompleted, got {:?}",
        events[n - 2]
    );

    // Last event: RunCompleted { Completed }.
    let RunEvent::RunCompleted { outcome } = &events[n - 1] else {
        panic!(
            "expected last event to be RunCompleted, got {:?}",
            events[n - 1]
        );
    };
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "expected RunOutcome::Completed, got {:?}",
        outcome
    );
}

/// Test 2: Two-turn scenario. Turn 1: LLM emits tool_use(fs-read, path) +
/// Finish(ToolUse). Turn 2: LLM emits Text("done") + Finish(EndTurn).
/// Pre-create /tmp/foo_streaming_e2e.txt with known content.
/// Assert: ToolCallStarted BEFORE any ToolCallCompleted, ToolCallCompleted
/// after dispatch, TurnCompleted after both, RunCompleted { Completed }.
#[tokio::test]
async fn tool_use_run_streams_tool_call_started_then_completed() {
    let tmpfile = tempfile::NamedTempFile::new().expect("tempfile");
    let path = tmpfile.path().to_str().unwrap().to_string();
    std::fs::write(tmpfile.path(), b"streaming e2e content\n").expect("write tmpfile");

    let parent = tmpfile
        .path()
        .parent()
        .expect("tempfile has parent")
        .to_str()
        .unwrap();
    let glob = format!("{parent}/**");
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

    let args = to_value(serde_json::json!({ "path": path }));
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

    let llm = ScriptedLlm::new(vec![turn1, turn2]);
    let tool: Arc<dyn DynTool> = Arc::new(InProcessFsRead::new());

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_dyn_tool(tool)
        .build()
        .expect("Runtime builds");

    let agent_def = common::agent_def("agent-2", "test-agent", "test-pkg@0.1.0", "test-llm");
    let initial = common::user_message("read the file");

    let stream = runtime
        .run_streaming(agent_def, manifest, initial, common::run_options())
        .await
        .expect("run_streaming construction");

    let events = collect_events(stream).await;

    // Find positions of ToolCallStarted, ToolCallCompleted, TurnCompleted, RunCompleted.
    let started_idx = events
        .iter()
        .position(|e| matches!(e, RunEvent::ToolCallStarted { .. }));
    let completed_idx = events
        .iter()
        .position(|e| matches!(e, RunEvent::ToolCallCompleted { .. }));
    let run_completed_idx = events
        .iter()
        .position(|e| matches!(e, RunEvent::RunCompleted { .. }));

    let started_idx = started_idx.expect("must have ToolCallStarted event");
    let completed_idx = completed_idx.expect("must have ToolCallCompleted event");
    let run_completed_idx = run_completed_idx.expect("must have RunCompleted event");

    // ToolCallStarted fires BEFORE ToolCallCompleted (display intent spec requirement).
    assert!(
        started_idx < completed_idx,
        "ToolCallStarted ({started_idx}) must precede ToolCallCompleted ({completed_idx})"
    );

    // ToolCallCompleted arrives before RunCompleted.
    assert!(
        completed_idx < run_completed_idx,
        "ToolCallCompleted ({completed_idx}) must precede RunCompleted ({run_completed_idx})"
    );

    // ToolCallCompleted must carry Ok result (successful dispatch).
    assert_matches!(
        &events[completed_idx],
        RunEvent::ToolCallCompleted { result, name, .. } => {
            assert_eq!(name, "fs-read");
            assert!(
                result.is_ok(),
                "expected ToolCallCompleted with Ok result; got {:?}",
                result
            );
        }
    );

    // Final event: RunCompleted { Completed }.
    assert_matches!(
        &events[run_completed_idx],
        RunEvent::RunCompleted { outcome } => {
            assert!(
                matches!(outcome, RunOutcome::Completed { .. }),
                "expected RunOutcome::Completed, got {:?}",
                outcome
            );
        }
    );
}

/// Test 3: LLM emits a tool_use with malformed args ({path: 42} — a number
/// instead of a string). The validator catches this.
/// Assert events: ToolCallStarted → ToolCallCompleted { result: Err(reason) }.
/// Verify `reason` contains MANDATORY-rule substrings: "You sent:",
/// "Expected (input_schema):", "Specific issue". Run terminates with
/// RunCompleted { Completed }.
#[tokio::test]
async fn schema_validation_failure_emits_tool_call_completed_with_err() {
    // Bad args: path is a number (should be string).
    let bad_args = to_value(serde_json::json!({ "path": 42 }));
    let tool_use = make_tool_use("call_bad".into(), "fs-read".into(), bad_args);
    let turn1 = make_completion_response(
        String::new(),
        vec![tool_use],
        StopReason::ToolUse,
        Some(make_token_usage(10, 5)),
    );
    // After validation error, LLM self-corrects with EndTurn.
    let turn2 = make_completion_response(
        "I corrected the args".into(),
        Vec::new(),
        StopReason::EndTurn,
        Some(make_token_usage(6, 2)),
    );

    let llm = ScriptedLlm::new(vec![turn1, turn2]);
    let tool: Arc<dyn DynTool> = Arc::new(InProcessFsRead::new());

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_dyn_tool(tool)
        .build()
        .expect("Runtime builds");

    let agent_def = common::agent_def("agent-3", "test-agent", "test-pkg@0.1.0", "test-llm");
    // Broad fs.read capability so the capability check passes;
    // only schema validation should gate here.
    let manifest = manifest_with_fs_read_global();
    let initial = common::user_message("read a file");

    let stream = runtime
        .run_streaming(agent_def, manifest, initial, common::run_options())
        .await
        .expect("run_streaming construction");

    let events = collect_events(stream).await;

    // Locate ToolCallStarted and ToolCallCompleted.
    let started_idx = events
        .iter()
        .position(|e| matches!(e, RunEvent::ToolCallStarted { .. }))
        .expect("must have ToolCallStarted");
    let completed_idx = events
        .iter()
        .position(|e| matches!(e, RunEvent::ToolCallCompleted { .. }))
        .expect("must have ToolCallCompleted");

    // ToolCallStarted fires before ToolCallCompleted.
    assert!(
        started_idx < completed_idx,
        "ToolCallStarted must precede ToolCallCompleted"
    );

    // ToolCallCompleted carries an Err(reason).
    assert_matches!(
        &events[completed_idx],
        RunEvent::ToolCallCompleted { result, .. } => {
            assert_matches!(
                result,
                Err(reason) => {
                    // MANDATORY-rule substrings from tool_args::validate_tool_args (priority 6).
                    assert!(
                        reason.contains("You sent:"),
                        "MANDATORY: reason must contain 'You sent:'; got: {reason}"
                    );
                    assert!(
                        reason.contains("Expected (input_schema):"),
                        "MANDATORY: reason must contain 'Expected (input_schema):'; got: {reason}"
                    );
                    assert!(
                        reason.contains("Specific issue"),
                        "MANDATORY: reason must contain 'Specific issue'; got: {reason}"
                    );
                }
            );
        }
    );

    // Run terminates with RunCompleted { Completed } (self-correction).
    let run_completed = events
        .iter()
        .find(|e| matches!(e, RunEvent::RunCompleted { .. }))
        .expect("must have RunCompleted");
    assert_matches!(
        run_completed,
        RunEvent::RunCompleted { outcome } => {
            assert!(
                matches!(outcome, RunOutcome::Completed { .. }),
                "expected RunOutcome::Completed after self-correction; got {:?}",
                outcome
            );
        }
    );
}

/// Test 4: Agent's package manifest has NO capabilities. LLM emits an
/// fs-read tool_use. Assert events: ToolCallStarted →
/// RunCompleted { Failed { PolicyDenied } }.
///
/// NOTE the documented terminal-failure exception: ToolCallStarted fires
/// (during chunk drain) but ToolCallCompleted does NOT (the dispatch loop
/// terminates with capability denial before the tool is invoked). This is
/// the documented pump invariant exception: "every Started is paired with
/// Completed OR followed by terminal RunCompleted{Failed}/FatalError."
#[tokio::test]
async fn capability_denial_terminates_run() {
    let args = to_value(serde_json::json!({ "path": "/tmp/some-file.txt" }));
    let tool_use = make_tool_use("call_denied".into(), "fs-read".into(), args);
    let turn1 = make_completion_response(
        String::new(),
        vec![tool_use],
        StopReason::ToolUse,
        Some(make_token_usage(10, 5)),
    );

    let llm = ScriptedLlm::new(vec![turn1]);
    let tool: Arc<dyn DynTool> = Arc::new(InProcessFsRead::new());

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_dyn_tool(tool)
        .build()
        .expect("Runtime builds");

    let agent_def = common::agent_def("agent-4", "test-agent", "test-pkg@0.1.0", "test-llm");
    // Manifest with NO capabilities → capability check must deny fs-read.
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("read a file");

    let stream = runtime
        .run_streaming(agent_def, manifest, initial, common::run_options())
        .await
        .expect("run_streaming construction");

    let events = collect_events(stream).await;

    // Must have ToolCallStarted (fires during chunk drain per spec Q3-A).
    let started_idx = events
        .iter()
        .position(|e| matches!(e, RunEvent::ToolCallStarted { .. }))
        .expect("must have ToolCallStarted");

    // Must NOT have ToolCallCompleted (documented terminal-failure exception).
    let has_completed = events
        .iter()
        .any(|e| matches!(e, RunEvent::ToolCallCompleted { .. }));
    assert!(
        !has_completed,
        "capability denial must NOT emit ToolCallCompleted (pump invariant terminal exception)"
    );

    // Terminal event: RunCompleted { Failed { PolicyDenied } }.
    let run_completed_idx = events
        .iter()
        .position(|e| matches!(e, RunEvent::RunCompleted { .. }))
        .expect("must have RunCompleted");

    // ToolCallStarted precedes RunCompleted.
    assert!(
        started_idx < run_completed_idx,
        "ToolCallStarted ({started_idx}) must precede RunCompleted ({run_completed_idx})"
    );

    assert_matches!(
        &events[run_completed_idx],
        RunEvent::RunCompleted {
            outcome: RunOutcome::Failed {
                status: AgentStatus::Failed { kind, .. },
                ..
            },
        } => {
            assert_eq!(
                *kind,
                FailureKind::PolicyDenied,
                "capability denial must yield PolicyDenied; got {:?}",
                kind
            );
        }
    );
}

/// Test 5: LLM emits a tool_use every turn. Configure
/// `RunOptions { max_turns: 2, .. }`. After 2 turns of tool
/// dispatching, the while-condition fails and the loop emits
/// `RunCompleted { Failed { OutOfResources } }`.
#[tokio::test]
async fn max_turns_reached_yields_run_completed_failed_out_of_resources() {
    let tmpfile = tempfile::NamedTempFile::new().expect("tempfile");
    let path = tmpfile.path().to_str().unwrap().to_string();

    let parent = tmpfile
        .path()
        .parent()
        .expect("tempfile has parent")
        .to_str()
        .unwrap();
    let glob = format!("{parent}/**");
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

    // Build two identical tool-use turns so the LLM always requests a tool call.
    let make_turn = |id: &str| {
        let args = to_value(serde_json::json!({ "path": path }));
        let tu = make_tool_use(id.into(), "fs-read".into(), args);
        make_completion_response(
            String::new(),
            vec![tu],
            StopReason::ToolUse,
            Some(make_token_usage(10, 5)),
        )
    };

    let llm = ScriptedLlm::new(vec![make_turn("call_1"), make_turn("call_2")]);
    let tool: Arc<dyn DynTool> = Arc::new(InProcessFsRead::new());

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_dyn_tool(tool)
        .build()
        .expect("Runtime builds");

    let agent_def = common::agent_def("agent-5", "test-agent", "test-pkg@0.1.0", "test-llm");
    let initial = common::user_message("read the file repeatedly");

    let mut options = common::run_options();
    options.max_turns = 2;

    let stream = runtime
        .run_streaming(agent_def, manifest, initial, options)
        .await
        .expect("run_streaming construction");

    let events = collect_events(stream).await;

    // Must have at least one ToolCallStarted + ToolCallCompleted pair
    // (the tool dispatches succeed; it's the loop limit that fires).
    let has_started = events
        .iter()
        .any(|e| matches!(e, RunEvent::ToolCallStarted { .. }));
    assert!(has_started, "must have at least one ToolCallStarted");

    let has_tool_completed = events
        .iter()
        .any(|e| matches!(e, RunEvent::ToolCallCompleted { result: Ok(_), .. }));
    assert!(
        has_tool_completed,
        "must have at least one ToolCallCompleted Ok"
    );

    // Terminal event: RunCompleted { Failed { OutOfResources } }.
    let run_completed = events
        .iter()
        .find(|e| matches!(e, RunEvent::RunCompleted { .. }))
        .expect("must have RunCompleted");

    assert_matches!(
        run_completed,
        RunEvent::RunCompleted {
            outcome: RunOutcome::Failed {
                status: AgentStatus::Failed { kind, .. },
                ..
            },
        } => {
            assert_eq!(
                *kind,
                FailureKind::OutOfResources,
                "max_turns reached must yield OutOfResources; got {:?}",
                kind
            );
        }
    );
}
