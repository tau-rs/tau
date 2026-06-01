//! Conformance test harness for the tau workflow IR.
//!
//! One `DevMode` test per fixture asserts that the IR interpreter
//! produces the expected side effects (RunOutcome::Completed + at least
//! one tool_call recorded).
//!
//! The cross-mode (`DevMode` vs `BundleMode`) comparison is registered
//! but `#[ignore]`'d until β.2.6.1 unstubs the bundle-mode dispatch path.

use std::path::Path;

use tau_ir_conformance::{
    assert_conform, bundle_mode::BundleMode, dev_mode::DevMode, ExecutionMode,
};
use tau_runtime_core::outcome::RunOutcome;

fn fixture_dir(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

// ---------------------------------------------------------------------------
// Fixture 01 — agent_native_tool (DevMode, single-mode assertion)
// ---------------------------------------------------------------------------

/// Fixture 01: agent with one native tool, two turns.
///
/// Turn 0: LLM emits `read_temp` tool_use.
/// Turn 1: LLM emits text "ok" + end_turn.
///
/// Expected: RunOutcome::Completed + one `read_temp` tool call recorded.
///
/// Uses `current_thread` flavor because `run_ir` returns a non-`Send`
/// future (tau-runtime-core uses `RefCell<RunState>` internally).
#[tokio::test(flavor = "current_thread")]
async fn fixture_01_dev_mode_completed_with_tool_call() {
    let dir = fixture_dir("01_agent_native_tool");
    let report = DevMode.run(&dir).await;

    // Outcome must be Completed.
    assert!(
        matches!(report.run_outcome, RunOutcome::Completed { .. }),
        "expected RunOutcome::Completed, got: {:?}",
        report.run_outcome
    );

    // At least one tool call must have been recorded.
    assert!(
        !report.tool_calls.is_empty(),
        "expected at least one tool call to be recorded; got none"
    );

    // Specifically, read_temp must appear with count 1.
    let read_temp_key = (
        "read_temp".to_string(),
        serde_json::to_vec(&serde_json::json!({"ok": true})).unwrap_or_default(),
    );
    // The args passed are `{}` (empty object) not `{"ok": true}` — look by tool name.
    let total_read_temp: u32 = report
        .tool_calls
        .iter()
        .filter(|((name, _), _)| name == "read_temp")
        .map(|(_, count)| count)
        .sum();
    let _ = read_temp_key; // suppress unused warning

    assert_eq!(
        total_read_temp, 1,
        "expected exactly 1 read_temp call; got {total_read_temp}"
    );
}

// ---------------------------------------------------------------------------
// Cross-mode comparison (IGNORED — β.2.6.1 unblocks)
// ---------------------------------------------------------------------------

/// Cross-mode conformance: fixture 01 must produce the same side-effect
/// multiset under DevMode and BundleMode.
///
/// IGNORE-REASON: β.2.6.1 unblocks: bundle_mode dispatch is stubbed
/// (β.2.5 deferred ToolDispatcher wiring in `tau run --bundle` path).
/// Remove this `#[ignore]` once β.2.6.1 ships.
#[tokio::test(flavor = "current_thread")]
#[ignore = "β.2.6.1 unblocks: bundle_mode dispatch is stubbed (β.2.5 deferred ToolDispatcher wiring)"]
async fn fixture_01_cross_mode_conformance() {
    let dir = fixture_dir("01_agent_native_tool");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}
