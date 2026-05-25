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

use tau_domain::Value;
use tau_ports::fixtures::{make_completion_response, make_token_usage, MockLlmBackend, MockTool};
use tau_ports::StopReason;
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
        .run(agent_def, manifest, initial, RunOptions::default())
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
        "event:runtime.run_completed",
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
        .run(agent_def, manifest, initial, RunOptions::default())
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
