//! Conformance test harness for the tau workflow IR.
//!
//! For each fixture, asserts the IR interpreter produces the expected
//! side effects under `DevMode`, and asserts cross-mode equivalence
//! (DevMode vs BundleMode) per D-7a (multiset side-effect equivalence).
//!
//! # Deferred fixtures
//!
//! One fixture slot ships empty (just a `README.md`) because the IR /
//! interpreter is not yet expansive enough to author it. It is listed
//! in [`DEFERRED_FIXTURES`] and skipped by any future directory-scanning
//! test. The fixture's README documents the exact IR / interpreter changes
//! needed to un-defer it:
//!
//! - `05_deterministic_step`: needs `tau-runtime-core`'s agent loop to
//!   dispatch `Deterministic` nodes via the `ToolDispatcher` surface.

use std::path::Path;

use tau_ir_conformance::{
    assert_conform, bundle_mode::BundleMode, dev_mode::DevMode, ExecutionMode,
};
use tau_runtime_core::outcome::RunOutcome;

/// Fixture directory names that the IR / interpreter cannot yet build
/// or execute. Any future directory-scanning conformance test must skip
/// these. See the module docs for the unblock requirements per fixture.
#[allow(dead_code)]
pub const DEFERRED_FIXTURES: &[&str] = &["05_deterministic_step"];

fn fixture_dir(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

/// Sum `tool_calls` entries for a given tool name (collapsing across
/// distinct `args_canonical` byte sequences).
fn count_tool_calls(report: &tau_ir_conformance::ConformanceReport, tool: &str) -> u32 {
    report
        .tool_calls
        .iter()
        .filter(|((name, _), _)| name == tool)
        .map(|(_, count)| *count)
        .sum()
}

// ---------------------------------------------------------------------------
// Fixture 01 — agent_native_tool
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

    assert!(
        matches!(report.run_outcome, Some(RunOutcome::Completed { .. })),
        "expected RunOutcome::Completed, got: {:?}",
        report.run_outcome
    );
    assert!(
        !report.tool_calls.is_empty(),
        "expected at least one tool call to be recorded; got none"
    );
    let total = count_tool_calls(&report, "read_temp");
    assert_eq!(total, 1, "expected exactly 1 read_temp call; got {total}");
}

/// Cross-mode conformance for fixture 01.
#[tokio::test(flavor = "current_thread")]
async fn fixture_01_cross_mode_conformance() {
    let dir = fixture_dir("01_agent_native_tool");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}

// ---------------------------------------------------------------------------
// Fixture 02 — agent_mcp_tool
// ---------------------------------------------------------------------------

/// Fixture 02: agent with one MCP tool, two turns.
///
/// Mirrors fixture 01 except `tools.weather.mcp = "..."` instead of
/// `native = "..."`. The `RecordingDispatcher` is implementation-blind
/// (it canned-responses every tool regardless of `ToolImpl`), so the
/// side-effect multiset is the same shape — only the embedded IR
/// `ToolImpl::Mcp` variant differs from fixture 01's `ToolImpl::Native`.
#[tokio::test(flavor = "current_thread")]
async fn fixture_02_dev_mode_completed_with_mcp_tool_call() {
    let dir = fixture_dir("02_agent_mcp_tool");
    let report = DevMode.run(&dir).await;

    assert!(
        matches!(report.run_outcome, Some(RunOutcome::Completed { .. })),
        "expected RunOutcome::Completed, got: {:?}",
        report.run_outcome
    );
    let total = count_tool_calls(&report, "weather");
    assert_eq!(total, 1, "expected exactly 1 weather call; got {total}");
}

/// Cross-mode conformance for fixture 02.
#[tokio::test(flavor = "current_thread")]
async fn fixture_02_cross_mode_conformance() {
    let dir = fixture_dir("02_agent_mcp_tool");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}

// ---------------------------------------------------------------------------
// Fixture 03 — agent_denied_capability (build-refused)
// ---------------------------------------------------------------------------

/// Fixture 03: build-time capability-fit refusal.
///
/// `tools.forbidden.capabilities = [{ kind = "agent.spawn" }]` is not
/// in the host target's `required_shapes`, so
/// `tau_ir::lower::capability_fit::check` refuses lowering with
/// `IrError::CapabilityFitFailed`. BOTH modes must surface a
/// `build_refused` report (D-3b refusal symmetry).
#[tokio::test(flavor = "current_thread")]
async fn fixture_03_dev_mode_build_refused() {
    let dir = fixture_dir("03_agent_denied_capability");
    let report = DevMode.run(&dir).await;
    let refused = report
        .build_refused
        .as_ref()
        .expect("expected build_refused; got an executed-run report");
    assert!(
        refused.to_lowercase().contains("capability")
            || refused.contains("AgentSpawn")
            || refused.to_lowercase().contains("agentspawn"),
        "diagnostic should name the capability-fit refusal; got: {refused}"
    );
    assert!(report.tool_calls.is_empty());
    assert!(report.message_added.is_empty());
}

/// Cross-mode conformance for fixture 03: both modes must refuse with
/// the same `IrError::Display` string.
#[tokio::test(flavor = "current_thread")]
async fn fixture_03_cross_mode_conformance() {
    let dir = fixture_dir("03_agent_denied_capability");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}

// ---------------------------------------------------------------------------
// Fixture 04 — subflow_spawn_child
// ---------------------------------------------------------------------------

/// Fixture 04: parent agent invokes a subflow tool that spawns the
/// `worker` child agent; child calls an MCP `page` tool then ends; parent
/// receives the child's final assistant text as the tool result and ends.
///
/// Subflow tools themselves are NOT routed through
/// `RecordingDispatcher::invoke` (the Subflow arm of
/// `DispatcherTool::invoke` goes through `Box::pin(run_ir(...))`
/// directly — see Phase 2 fix C2). So `notify` does not appear in
/// `report.tool_calls`. We assert subflow execution via the CHILD's
/// recorded tool calls: if `page` is recorded, the recursive `run_ir`
/// ran (since `page` only exists in the child agent).
///
/// Expected: RunOutcome::Completed; multiset has `page:{}` = 1
/// (the child's MCP call), proving the subflow recursion executed.
#[tokio::test(flavor = "current_thread")]
async fn fixture_04_dev_mode_subflow_dispatched() {
    let dir = fixture_dir("04_subflow_spawn_child");
    let report = DevMode.run(&dir).await;

    assert!(
        matches!(report.run_outcome, Some(RunOutcome::Completed { .. })),
        "expected RunOutcome::Completed, got: {:?}",
        report.run_outcome
    );
    assert_eq!(
        count_tool_calls(&report, "page"),
        1,
        "expected exactly 1 page (child MCP) call — proves subflow recursion ran"
    );
    assert_eq!(
        count_tool_calls(&report, "notify"),
        0,
        "subflow tools are not routed through dispatcher.invoke; should not appear in tool_calls"
    );
}

/// Cross-mode conformance for fixture 04.
#[tokio::test(flavor = "current_thread")]
async fn fixture_04_cross_mode_conformance() {
    let dir = fixture_dir("04_subflow_spawn_child");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}

// ---------------------------------------------------------------------------
// Fixture 06 — multi_turn_history
// ---------------------------------------------------------------------------

/// Fixture 06: three consecutive tool-use turns + one end_turn.
///
/// Asserts the multiset count for `read_temp:{}` is exactly 3.
#[tokio::test(flavor = "current_thread")]
async fn fixture_06_dev_mode_three_tool_calls() {
    let dir = fixture_dir("06_multi_turn_history");
    let report = DevMode.run(&dir).await;

    assert!(
        matches!(report.run_outcome, Some(RunOutcome::Completed { .. })),
        "expected RunOutcome::Completed, got: {:?}",
        report.run_outcome
    );
    let total = count_tool_calls(&report, "read_temp");
    assert_eq!(total, 3, "expected exactly 3 read_temp calls; got {total}");
}

/// Cross-mode conformance for fixture 06.
#[tokio::test(flavor = "current_thread")]
async fn fixture_06_cross_mode_conformance() {
    let dir = fixture_dir("06_multi_turn_history");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}
