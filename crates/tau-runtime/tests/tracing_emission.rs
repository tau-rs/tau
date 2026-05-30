//! Integration test: capture tracing events emitted by `Runtime::run`
//! during a happy-path agent execution and assert that the structural
//! event/span vocabulary documented in spec §3.9 is present.
//!
//! The test does NOT enumerate every event: it samples key milestones
//! (run start, turn start, LLM span, LLM response, run completed). If
//! the kernel emits substantially fewer events than expected, the
//! tracing vocabulary is broken; if it emits more, the test stays
//! happily silent.

mod common;

use std::fmt;
use std::sync::{Arc, Mutex};

use std::sync::atomic::{AtomicUsize, Ordering};

use tau_domain::{Capability, Value};
use tau_ports::fixtures::{make_completion_response, make_token_usage, MockLlmBackend, MockTool};
use tau_ports::{SessionContext, StopReason, Tool, ToolError, ToolResult, ToolSpec};
use tau_runtime::{RunOptions, Runtime};
use tracing::field::{Field, Visit};
use tracing::span::Attributes;
use tracing::{Event, Id, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::prelude::*;
use tracing_subscriber::Layer;

/// Layer that records each span open and event with a stable label.
/// Spans are recorded as `"span:<metadata-name>"` (the run loop names
/// its spans via `#[instrument(name = "...")]` and `info_span!("...")`,
/// both of which write to the metadata `name()`).
///
/// Events are recorded as `"event:<value-of-name-field>"`. The run
/// loop's `info!(name = "runtime.run_started", …)` syntax does NOT
/// override the event metadata `name()` — that override is reserved
/// for `#[instrument]` and `*_span!` macros. Instead, `name` is just
/// a regular field on the event. We extract it via `Visit`.
#[derive(Default, Clone)]
struct CapturedEvents(Arc<Mutex<Vec<String>>>);

impl<S: Subscriber> Layer<S> for CapturedEvents {
    fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, _ctx: Context<'_, S>) {
        self.0
            .lock()
            .expect("captured-events mutex poisoned")
            .push(format!("span:{}", attrs.metadata().name()));
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = NameVisitor::default();
        event.record(&mut visitor);
        let label = visitor
            .0
            .unwrap_or_else(|| event.metadata().name().to_string());
        self.0
            .lock()
            .expect("captured-events mutex poisoned")
            .push(format!("event:{label}"));
    }
}

/// `Visit`or that extracts the `name` field from an event's record set.
/// The kernel writes `name` as a `&'static str` literal, but tracing
/// stores string-typed values via either `record_str` (when the
/// recorder reports `String` support) or `record_debug` (the universal
/// fallback). We accept both and strip the surrounding `"…"` from the
/// debug form.
#[derive(Default)]
struct NameVisitor(Option<String>);

impl Visit for NameVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "name" {
            self.0 = Some(value.to_string());
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "name" {
            // `Debug` for `&str` formats as `"…"` — strip the quotes
            // so the captured label matches what the test asserts.
            let raw = format!("{value:?}");
            let trimmed = raw.trim_matches('"').to_string();
            self.0 = Some(trimmed);
        }
    }
}

#[tokio::test]
async fn run_emits_structural_tracing_vocabulary() {
    let captured = CapturedEvents::default();
    // Hold the dispatch guard for the duration of the test so all
    // `tracing::*` macro calls in `Runtime::run` resolve to our layer.
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    // Same shape as Task 11's happy-path scenario.
    let resp = make_completion_response(
        "hello world".into(),
        Vec::new(),
        StopReason::EndTurn,
        Some(make_token_usage(5, 10)),
    );
    let llm = MockLlmBackend::new("gpt-4").with_response(resp);

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .build()
        .expect("build runtime");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("hi");

    runtime
        .run(agent_def, manifest, initial, common::run_options())
        .await
        .expect("run succeeded");

    let captured_vec = captured
        .0
        .lock()
        .expect("captured-events mutex poisoned")
        .clone();

    // Assert structural vocabulary milestones are present (substring
    // match — the captured strings are the verbatim labels above).
    let expected = [
        "span:runtime.agent_run",
        "event:runtime.run_started",
        "span:runtime.turn",
        "event:runtime.turn_started",
        "span:llm.complete",
        "event:llm.response_received",
        "event:runtime.completed",
    ];
    for e in &expected {
        assert!(
            captured_vec.iter().any(|c| c == e),
            "missing tracing milestone {e:?}; captured = {captured_vec:?}"
        );
    }

    // Sanity bound: a happy-path run emits well over 8 span/event
    // entries (run_started, capability_set_loaded, turn_started,
    // request_built, llm.complete span, response_received, stop_reason
    // trace, token_usage, loop_terminated, run_completed, …). If we
    // see fewer than 8, the kernel's vocabulary is broken.
    assert!(
        captured_vec.len() >= 8,
        "expected >= 8 captured entries on a happy-path run; got {}: {captured_vec:?}",
        captured_vec.len()
    );

    // ADR-0006 §3.9: `runtime.turn` span fires exactly once per
    // executed turn. The happy-path run here executes a single turn.
    let turn_spans = captured_vec
        .iter()
        .filter(|c| c.as_str() == "span:runtime.turn")
        .count();
    assert_eq!(
        turn_spans, 1,
        "expected exactly 1 runtime.turn span on a single-turn run; got {turn_spans}: {captured_vec:?}"
    );
}

#[tokio::test]
async fn runtime_turn_span_fires_once_per_turn() {
    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    // Turn 1: a tool_use that forces a second turn.
    // Turn 2: plain text, ends the loop.
    let llm = common::MockLlmBackend::new("gpt-4")
        .add_tool_call("echo", Value::String("hi".into()))
        .add_text("done");
    // Default `MockTool` returns Ok(empty) — sufficient for the
    // dispatch step to succeed and the loop to advance to turn 2.
    let echo_tool = MockTool::new("echo", common::empty_tool_spec("echo"));

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_tool(echo_tool)
        .build()
        .expect("build runtime");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("hi");

    runtime
        .run(agent_def, manifest, initial, common::run_options())
        .await
        .expect("run succeeded");

    let captured_vec = captured
        .0
        .lock()
        .expect("captured-events mutex poisoned")
        .clone();

    // Exactly one `runtime.turn` span per executed turn.
    let turn_spans = captured_vec
        .iter()
        .filter(|c| c.as_str() == "span:runtime.turn")
        .count();
    assert_eq!(
        turn_spans, 2,
        "expected exactly 2 runtime.turn spans across a 2-turn run; got {turn_spans}: {captured_vec:?}"
    );

    // And one `runtime.turn_started` event per turn — sanity check
    // against off-by-one in the span placement.
    let turn_starts = captured_vec
        .iter()
        .filter(|c| c.as_str() == "event:runtime.turn_started")
        .count();
    assert_eq!(
        turn_starts, 2,
        "expected exactly 2 runtime.turn_started events; got {turn_starts}: {captured_vec:?}"
    );
}

// ---------------------------------------------------------------------------
// Task 3: terminal-event vocabulary tests
// ---------------------------------------------------------------------------

/// Normal termination: a 1-turn run ending with `EndTurn` must emit
/// `runtime.completed`.
#[tokio::test]
async fn runtime_completed_event_fires_on_normal_terminate() {
    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    let resp = make_completion_response(
        "done".into(),
        Vec::new(),
        StopReason::EndTurn,
        Some(make_token_usage(1, 1)),
    );
    let llm = tau_ports::fixtures::MockLlmBackend::new("gpt-4").with_response(resp);

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .build()
        .expect("build runtime");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("hi");

    runtime
        .run(agent_def, manifest, initial, common::run_options())
        .await
        .expect("run succeeded");

    let captured_vec = captured
        .0
        .lock()
        .expect("captured-events mutex poisoned")
        .clone();

    assert!(
        captured_vec.iter().any(|c| c == "event:runtime.completed"),
        "missing event:runtime.completed; captured = {captured_vec:?}"
    );
}

/// Loop exhausted: a run with `max_turns = 2` against an LLM that
/// always returns a tool_use must emit `runtime.max_turns_reached`.
#[tokio::test]
async fn runtime_max_turns_event_fires_when_loop_exhausted() {
    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    // Two tool-call turns; on turn 3 the loop guard fires before
    // we'd need a third LLM call, so the script only needs 2 entries
    // — but add a third defensively in case the implementation
    // bumps the LLM one more time.
    let llm = common::MockLlmBackend::new("gpt-4")
        .add_tool_call("echo", Value::String("hi".into()))
        .add_tool_call("echo", Value::String("hi".into()))
        .add_tool_call("echo", Value::String("hi".into()));
    let echo_tool = MockTool::new("echo", common::empty_tool_spec("echo"));

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_tool(echo_tool)
        .build()
        .expect("build runtime");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("loop");

    let mut opts = common::run_options();
    opts.max_turns = 2;
    // `Ok(RunOutcome::Failed { kind: OutOfResources })` is the
    // documented contract when max_turns is hit.
    let _outcome = runtime
        .run(agent_def, manifest, initial, opts)
        .await
        .expect("max-turns flows through Ok(RunOutcome::Failed)");

    let captured_vec = captured
        .0
        .lock()
        .expect("captured-events mutex poisoned")
        .clone();

    assert!(
        captured_vec
            .iter()
            .any(|c| c == "event:runtime.max_turns_reached"),
        "missing event:runtime.max_turns_reached; captured = {captured_vec:?}"
    );
}

/// Capability-denied path: agent attempts a tool whose required
/// capability is NOT granted; the run loop returns `Ok(RunOutcome::Failed
/// { kind: PolicyDenied })` and must emit `runtime.failed` before
/// terminating.
#[tokio::test]
async fn runtime_failed_event_fires_on_status_failed() {
    /// Tool that declares a non-empty `capabilities()` so the kernel's
    /// capability check rejects the call.
    struct RestrictedTool {
        schema: ToolSpec,
        required_caps: Vec<Capability>,
        invoke_count: std::sync::Arc<AtomicUsize>,
    }

    impl Tool for RestrictedTool {
        type Session = ();

        fn name(&self) -> &str {
            &self.schema.name
        }

        fn schema(&self) -> ToolSpec {
            self.schema.clone()
        }

        fn capabilities(&self) -> &[Capability] {
            &self.required_caps
        }

        async fn init(&self, _ctx: SessionContext) -> Result<(), ToolError> {
            Ok(())
        }

        async fn invoke(&self, _: &mut (), _args: Value) -> Result<ToolResult, ToolError> {
            self.invoke_count.fetch_add(1, Ordering::SeqCst);
            Ok(tau_ports::fixtures::make_tool_result(Vec::new(), false))
        }

        async fn teardown(&self, _: ()) -> Result<(), ToolError> {
            Ok(())
        }
    }

    /// Build an `fs.read` capability via the canonical TOML deserialization
    /// path. Variant-level `#[non_exhaustive]` blocks struct-literal
    /// construction from outside `tau-domain`.
    fn fs_read_cap(paths: &[&str]) -> Capability {
        #[derive(serde::Deserialize)]
        struct Wrapper {
            cap: Capability,
        }
        let paths_toml = paths
            .iter()
            .map(|p| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let toml_body = format!(
            r#"[cap]
kind = "fs.read"
paths = [{paths_toml}]
"#
        );
        toml::from_str::<Wrapper>(&toml_body)
            .expect("test fs.read capability TOML must parse")
            .cap
    }

    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    // LLM emits a single tool_use targeting the restricted tool.
    let llm = common::MockLlmBackend::new("gpt-4").add_tool_call("restricted-reader", Value::Null);

    let invoke_count = std::sync::Arc::new(AtomicUsize::new(0));
    let restricted = RestrictedTool {
        schema: common::empty_tool_spec("restricted-reader"),
        required_caps: vec![fs_read_cap(&["/etc/passwd"])],
        invoke_count: invoke_count.clone(),
    };

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_tool(restricted)
        .build()
        .expect("build runtime");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("read /etc/passwd");

    let _outcome = runtime
        .run(agent_def, manifest, initial, common::run_options())
        .await
        .expect("capability denial flows through Ok(RunOutcome::Failed)");

    let captured_vec = captured
        .0
        .lock()
        .expect("captured-events mutex poisoned")
        .clone();

    assert!(
        captured_vec.iter().any(|c| c == "event:runtime.failed"),
        "missing event:runtime.failed; captured = {captured_vec:?}"
    );
    // Sanity: tool's invoke was never reached (cap check rejected).
    assert_eq!(invoke_count.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------
// Task 4: LLM-event vocabulary tests
// ---------------------------------------------------------------------------

/// Happy-path 1-turn run: the kernel must emit each of the four
/// pre-tool-dispatch LLM lifecycle events: request built, response
/// received, token usage (because the fixture carries `Some(usage)`),
/// and stop reason.
#[tokio::test]
async fn llm_request_and_response_events_fire() {
    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    let resp = make_completion_response(
        "hello".into(),
        Vec::new(),
        StopReason::EndTurn,
        Some(make_token_usage(7, 3)),
    );
    let llm = MockLlmBackend::new("gpt-4").with_response(resp);

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .build()
        .expect("build runtime");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("hi");

    runtime
        .run(agent_def, manifest, initial, common::run_options())
        .await
        .expect("run succeeded");

    let captured_vec = captured
        .0
        .lock()
        .expect("captured-events mutex poisoned")
        .clone();

    for expected in [
        "event:llm.request_built",
        "event:llm.response_received",
        "event:llm.token_usage",
        "event:llm.stop_reason",
    ] {
        assert!(
            captured_vec.iter().any(|c| c == expected),
            "missing {expected:?}; captured = {captured_vec:?}"
        );
    }
}

/// A single LLM response carrying TWO `ToolUse` blocks must produce
/// exactly two `llm.tool_use_emitted` events (one per block).
///
/// Uses [`common::MockLlmBackend::add_tool_calls`] to script a single
/// turn whose response contains multiple `ToolUse` blocks, followed by
/// a terminating text turn.
#[tokio::test]
async fn llm_tool_use_emitted_fires_per_tool_block() {
    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    // Turn 1: two tool-use blocks in one response (forces a 2nd turn).
    // Turn 2: plain text, terminates the loop.
    let llm = common::MockLlmBackend::new("gpt-4")
        .add_tool_calls(vec![
            ("echo", Value::String("a".into())),
            ("echo", Value::String("b".into())),
        ])
        .add_text("done");

    let echo_tool = MockTool::new("echo", common::empty_tool_spec("echo"));

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_tool(echo_tool)
        .build()
        .expect("build runtime");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("call two tools");

    runtime
        .run(agent_def, manifest, initial, common::run_options())
        .await
        .expect("run succeeded");

    let captured_vec = captured
        .0
        .lock()
        .expect("captured-events mutex poisoned")
        .clone();

    let tool_use_count = captured_vec
        .iter()
        .filter(|c| c.as_str() == "event:llm.tool_use_emitted")
        .count();
    assert_eq!(
        tool_use_count, 2,
        "expected 2 llm.tool_use_emitted events, got {tool_use_count}; captured = {captured_vec:?}"
    );
}

// ---------------------------------------------------------------------------
// Task 5: dispatch.tool span + dispatch.tool_resolved event
// ---------------------------------------------------------------------------

/// A single LLM response carrying one `ToolUse` block must produce
/// exactly one `dispatch.tool` span and one `dispatch.tool_resolved`
/// event (fired after the registry lookup resolves to a concrete plugin).
#[tokio::test]
async fn dispatch_tool_resolved_fires_for_each_tool_call() {
    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    // Turn 1: one tool_use (forces a 2nd turn).
    // Turn 2: plain text, terminates the loop.
    let llm = common::MockLlmBackend::new("gpt-4")
        .add_tool_call("echo", Value::String("hi".into()))
        .add_text("done");
    let echo_tool = MockTool::new("echo", common::empty_tool_spec("echo"));

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_tool(echo_tool)
        .build()
        .expect("build runtime");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("call echo");

    runtime
        .run(agent_def, manifest, initial, common::run_options())
        .await
        .expect("run succeeded");

    let captured_vec = captured
        .0
        .lock()
        .expect("captured-events mutex poisoned")
        .clone();

    let dispatch_spans = captured_vec
        .iter()
        .filter(|c| c.as_str() == "span:dispatch.tool")
        .count();
    assert_eq!(
        dispatch_spans, 1,
        "expected 1 dispatch.tool span, got {dispatch_spans}; captured = {captured_vec:?}"
    );

    let resolved_events = captured_vec
        .iter()
        .filter(|c| c.as_str() == "event:dispatch.tool_resolved")
        .count();
    assert_eq!(
        resolved_events, 1,
        "expected 1 dispatch.tool_resolved event, got {resolved_events}; captured = {captured_vec:?}"
    );
}

// ---------------------------------------------------------------------------
// Task 6: capability.check span + 4 capability events (allow path)
//         + capability.deny (kept under capability.check via Option A:
//         the new `check_capabilities_for_tool` wrapper owns all 5 events,
//         the inline deny emissions in stream.rs were removed).
// ---------------------------------------------------------------------------

/// Happy-path 2-turn run with a tool dispatch: the kernel must open
/// the `capability.check` span and emit the four "no-deny" capability
/// events (required_loaded, granted_loaded, satisfies_check, allow).
#[tokio::test]
async fn capability_check_events_fire_on_allow() {
    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    // Turn 1: one tool_use (forces a 2nd turn).
    // Turn 2: plain text, terminates the loop.
    // The echo tool declares no required capabilities, so the
    // satisfies-check trivially passes and we land on the allow branch.
    let llm = common::MockLlmBackend::new("gpt-4")
        .add_tool_call("echo", Value::String("hi".into()))
        .add_text("done");
    let echo_tool = MockTool::new("echo", common::empty_tool_spec("echo"));

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_tool(echo_tool)
        .build()
        .expect("build runtime");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("call echo");

    runtime
        .run(agent_def, manifest, initial, common::run_options())
        .await
        .expect("run succeeded");

    let captured_vec = captured
        .0
        .lock()
        .expect("captured-events mutex poisoned")
        .clone();

    for expected in [
        "span:capability.check",
        "event:capability.required_loaded",
        "event:capability.granted_loaded",
        "event:capability.satisfies_check",
        "event:capability.allow",
    ] {
        assert!(
            captured_vec.iter().any(|c| c == expected),
            "missing {expected:?}; captured = {captured_vec:?}"
        );
    }
}

/// Capability-denied path: the agent attempts a tool whose required
/// capability is NOT granted; the wrapper's deny branch must fire
/// `capability.deny` under the `capability.check` span.
///
/// Same restricted-tool pattern as `runtime_failed_event_fires_on_status_failed`
/// (Task 3) — copied locally so this test owns its own setup.
#[tokio::test]
async fn capability_deny_fires_when_check_fails() {
    struct RestrictedTool {
        schema: ToolSpec,
        required_caps: Vec<Capability>,
    }

    impl Tool for RestrictedTool {
        type Session = ();

        fn name(&self) -> &str {
            &self.schema.name
        }

        fn schema(&self) -> ToolSpec {
            self.schema.clone()
        }

        fn capabilities(&self) -> &[Capability] {
            &self.required_caps
        }

        async fn init(&self, _ctx: SessionContext) -> Result<(), ToolError> {
            Ok(())
        }

        async fn invoke(&self, _: &mut (), _args: Value) -> Result<ToolResult, ToolError> {
            Ok(tau_ports::fixtures::make_tool_result(Vec::new(), false))
        }

        async fn teardown(&self, _: ()) -> Result<(), ToolError> {
            Ok(())
        }
    }

    fn fs_read_cap(paths: &[&str]) -> Capability {
        #[derive(serde::Deserialize)]
        struct Wrapper {
            cap: Capability,
        }
        let paths_toml = paths
            .iter()
            .map(|p| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let toml_body = format!(
            r#"[cap]
kind = "fs.read"
paths = [{paths_toml}]
"#
        );
        toml::from_str::<Wrapper>(&toml_body)
            .expect("test fs.read capability TOML must parse")
            .cap
    }

    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    let llm = common::MockLlmBackend::new("gpt-4").add_tool_call("restricted-reader", Value::Null);

    let restricted = RestrictedTool {
        schema: common::empty_tool_spec("restricted-reader"),
        required_caps: vec![fs_read_cap(&["/etc/passwd"])],
    };

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_tool(restricted)
        .build()
        .expect("build runtime");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("read /etc/passwd");

    let _outcome = runtime
        .run(agent_def, manifest, initial, common::run_options())
        .await
        .expect("capability denial flows through Ok(RunOutcome::Failed)");

    let captured_vec = captured
        .0
        .lock()
        .expect("captured-events mutex poisoned")
        .clone();

    assert!(
        captured_vec.iter().any(|c| c == "event:capability.deny"),
        "missing event:capability.deny; captured = {captured_vec:?}"
    );
    // Sanity: the deny path also opens the `capability.check` span
    // (Option A — wrapper owns the span end-to-end).
    assert!(
        captured_vec.iter().any(|c| c == "span:capability.check"),
        "missing span:capability.check on deny path; captured = {captured_vec:?}"
    );
}

// ---------------------------------------------------------------------------
// Task 8: message.added event at every history push
// ---------------------------------------------------------------------------

/// Drive a 2-turn run (one tool call + a terminating text response) and
/// assert that `message.added` fires exactly once per `messages.push`
/// site in `stream.rs`. The expected push sites for this scenario are:
///
///   1. The initial user message pushed before the turn loop opens.
///   2. The assistant tool-call message (`agent_addr → tool_addr`,
///      `MessagePayload::ToolCall`) pushed after the cap check.
///   3. The tool-result message (`tool_addr → agent_addr`,
///      `MessagePayload::ToolResult`) pushed after `Tool::invoke`.
///   4. The final assistant text message ("done") pushed on turn 2 when
///      the accumulated text is non-empty.
///
/// `history` is empty, so the `messages.extend(history)` loop emits
/// zero events. Total expected = 4.
#[tokio::test]
async fn message_added_count_matches_pushed_messages() {
    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    // Turn 1: one tool_use (forces a 2nd turn).
    // Turn 2: plain text, terminates the loop.
    let llm = common::MockLlmBackend::new("gpt-4")
        .add_tool_call("echo", Value::String("hi".into()))
        .add_text("done");
    let echo_tool = MockTool::new("echo", common::empty_tool_spec("echo"));

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_tool(echo_tool)
        .build()
        .expect("build runtime");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("call echo");

    runtime
        .run(agent_def, manifest, initial, common::run_options())
        .await
        .expect("run succeeded");

    let captured_vec = captured
        .0
        .lock()
        .expect("captured-events mutex poisoned")
        .clone();

    let count = captured_vec
        .iter()
        .filter(|c| c.as_str() == "event:message.added")
        .count();
    assert_eq!(
        count, 4,
        "expected 4 message.added events (initial user + assistant tool-call \
         + tool-result + final assistant text), got {count}; captured = {captured_vec:?}"
    );
}

// ---------------------------------------------------------------------------
// Task 7: tool.session_open/invoke/session_close spans + tool.args_received
//         + tool.result_received events
// ---------------------------------------------------------------------------

/// Drive one full happy-path tool invocation. Assert that all three
/// tool-session spans open at least once around the dispatch call site
/// (these spans wrap the per-tool init/invoke/teardown for every
/// `DynTool`, in-process MockTool included).
///
/// The `tool.args_received` and `tool.result_received` events live
/// inside `IpcTool::invoke` (they expose rmp-encoded byte counts that
/// only have meaning for the IPC path). They're exercised separately
/// by the feature-gated IPC test below.
#[tokio::test]
async fn tool_session_spans_fire_for_full_lifecycle() {
    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    // Turn 1: one tool_use (forces a 2nd turn).
    // Turn 2: plain text, terminates the loop.
    let llm = common::MockLlmBackend::new("gpt-4")
        .add_tool_call("echo", Value::String("hi".into()))
        .add_text("done");
    let echo_tool = MockTool::new("echo", common::empty_tool_spec("echo"));

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_tool(echo_tool)
        .build()
        .expect("build runtime");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("call echo");

    runtime
        .run(agent_def, manifest, initial, common::run_options())
        .await
        .expect("run succeeded");

    let captured_vec = captured
        .0
        .lock()
        .expect("captured-events mutex poisoned")
        .clone();

    for expected in [
        "span:tool.session_open",
        "span:tool.invoke",
        "span:tool.session_close",
    ] {
        assert!(
            captured_vec.iter().any(|c| c == expected),
            "missing {expected:?}; captured = {captured_vec:?}"
        );
    }
}

/// IPC-path test: registers a real `IpcTool` (driven by a `FakeStdioPeer`
/// via two duplex streams), runs one turn, and asserts the
/// `tool.args_received` and `tool.result_received` events fire from
/// inside `IpcTool::invoke`.
///
/// Gated by `feature = "test-support"` (same as `plugin_host_ipc_llm.rs`)
/// because the `IpcTool` constructor + `PluginProcess::new_for_test`
/// live behind that flag.
#[cfg(feature = "test-support")]
#[tokio::test]
async fn tool_ipc_args_and_result_events_fire_for_invoke() {
    use std::sync::Arc;
    use std::time::Duration;

    use tau_plugin_protocol::test_support::FakeStdioPeer;
    use tau_plugin_protocol::{FramedReader, FramedWriter, FramerOptions};
    use tau_ports::fixtures::{make_tool_result, make_tool_spec};
    use tau_ports::ToolContent;
    use tau_runtime::plugin_host::__internals::{DynAsyncWriter, IpcTool, PluginProcess};

    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    // ----- Build a paired (PluginProcess, FakeStdioPeer) -------------
    // Mirrors the helper in tests/plugin_host_ipc_llm.rs.
    let (peer_read_half, sut_write_half) = tokio::io::duplex(64 * 1024);
    let (sut_read_half, peer_write_half) = tokio::io::duplex(64 * 1024);
    let mut peer = FakeStdioPeer {
        reader: FramedReader::new(peer_read_half, FramerOptions::default()),
        writer: FramedWriter::new(peer_write_half),
    };
    let sut_reader = FramedReader::new(sut_read_half, FramerOptions::default());
    let sut_writer: FramedWriter<DynAsyncWriter> =
        FramedWriter::new(Box::new(sut_write_half) as DynAsyncWriter);
    let process = PluginProcess::new_for_test(
        "echo".to_string(),
        sut_reader,
        sut_writer,
        Duration::from_secs(5),
    );

    let spec = make_tool_spec(
        "echo".into(),
        "echo".into(),
        Value::Object(Default::default()),
    );
    let ipc_tool: Arc<dyn tau_runtime::builder::DynTool> =
        Arc::new(IpcTool::new("echo".to_string(), spec, Vec::new(), process));

    // Turn 1: one tool_use (forces a 2nd turn).
    // Turn 2: plain text, terminates the loop.
    let llm = common::MockLlmBackend::new("gpt-4")
        .add_tool_call("echo", Value::String("hi".into()))
        .add_text("done");

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_dyn_tool(ipc_tool)
        .build()
        .expect("build runtime");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("call echo");

    // Drive runtime and peer concurrently — IpcTool blocks on the
    // peer's tool.call response.
    let run_fut = runtime.run(agent_def, manifest, initial, common::run_options());
    let peer_fut = async {
        let (msgid, _params) = peer.expect_request("tool.call").await;
        let result = make_tool_result(
            vec![ToolContent::Text {
                text: "echoed".into(),
            }],
            false,
        );
        peer.send_response(msgid, &result)
            .await
            .expect("peer send_response");
    };
    let (run_outcome, ()) = tokio::join!(run_fut, peer_fut);
    run_outcome.expect("run succeeded");

    let captured_vec = captured
        .0
        .lock()
        .expect("captured-events mutex poisoned")
        .clone();

    for expected in [
        "span:tool.session_open",
        "span:tool.invoke",
        "span:tool.session_close",
        "event:tool.args_received",
        "event:tool.result_received",
    ] {
        assert!(
            captured_vec.iter().any(|c| c == expected),
            "missing {expected:?}; captured = {captured_vec:?}"
        );
    }
}
