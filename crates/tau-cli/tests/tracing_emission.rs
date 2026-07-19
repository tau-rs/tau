//! Cross-cutting: tracing structural events fire on a happy-path run.
//!
//! The runtime emits a curated set of tracing events through
//! `tracing::{info,debug,warn}!(name = "...")` (e.g.
//! `runtime.run_started`, `runtime.completed`,
//! `runtime.turn_started`). The CLI installs a `tracing-subscriber` fmt
//! layer that writes those events to stderr in the standard human
//! format.
//!
//! # Approach
//!
//! Subprocess + `RUST_LOG`. We could install a custom `Layer` in-process
//! and call `lib::run_main`, but the binary calls `tracing::install`
//! once at startup which `init()`s a global subscriber — running it
//! twice in the same test binary would panic. The subprocess path is
//! brittler against format drift but trivially reliable, and the event
//! names we look for are stable (defined in `tau-runtime/src/run.rs`).
//!
//! Default filter is `tau=info`, which already fires
//! `runtime.run_started` (info-level). For the higher-fidelity events
//! we set `RUST_LOG=tau=debug` explicitly so `runtime.turn_started`
//! and friends are flushed.
//!
//! These tests spawn the real `echo-llm` plugin process via
//! [`common::setup_echo_project`] (Task 21) so the runtime sees a
//! complete, non-mocked turn lifecycle.

mod common;

use assert_cmd::Command as AssertCmd;

#[test]
fn run_emits_run_started_event_at_info() {
    let dir = common::setup_echo_project("echo", "canned_text = \"ok\"\n", &[]);

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["run", "echo", "hi", "--no-governance"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        // Default filter is tau=info -> run_started (info!) shows up.
        .env_remove("RUST_LOG")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "expected success; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("runtime.run_started"),
        "expected runtime.run_started in stderr; got:\n{stderr}"
    );
}

#[test]
fn run_emits_turn_lifecycle_events_at_debug() {
    let dir = common::setup_echo_project("echo", "canned_text = \"ok\"\n", &[]);

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["run", "echo", "hi", "--no-governance"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .env("RUST_LOG", "tau=debug")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "expected success; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    // High-confidence assertions: these names are defined in
    // tau-runtime/src/run.rs and are part of the runtime's public
    // observability surface.
    //
    // NOTE: `runtime.turn_completed` only fires after a tool dispatch
    // step; the happy-path `EndTurn` short-circuits via
    // `runtime.loop_terminated` then `runtime.completed` (info!),
    // so we assert the events that DO fire on a no-tool happy path.
    assert!(
        stderr.contains("runtime.turn_started"),
        "expected runtime.turn_started; got:\n{stderr}"
    );
    assert!(
        stderr.contains("runtime.loop_terminated"),
        "expected runtime.loop_terminated; got:\n{stderr}"
    );
    assert!(
        stderr.contains("runtime.completed"),
        "expected runtime.completed; got:\n{stderr}"
    );
    // LLM-side events are also part of the surface.
    assert!(
        stderr.contains("llm.request_built"),
        "expected llm.request_built; got:\n{stderr}"
    );
    assert!(
        stderr.contains("llm.response_received"),
        "expected llm.response_received; got:\n{stderr}"
    );
}
