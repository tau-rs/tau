//! Conformance test harness for the tau workflow IR.
//!
//! For each fixture, asserts the IR interpreter produces the expected
//! side effects under `DevMode`, and asserts cross-mode equivalence
//! (DevMode vs BundleMode) per D-7a (multiset side-effect equivalence).
//!
//! All seven fixtures are live as of β.3 PR-6: `01_agent_native_tool`,
//! `02_agent_mcp_tool`, `03_agent_denied_capability`,
//! `04_subflow_spawn_child`, `05_deterministic_step`,
//! `06_multi_turn_history`, and `07_mcp_weather_cassette`.
//! No `DEFERRED_FIXTURES` slots remain.

use std::path::Path;

use tau_ir_conformance::{
    assert_conform, bundle_mode::BundleMode, dev_mode::DevMode, ExecutionMode,
};
use tau_runtime_core::outcome::RunOutcome;

/// Fixture directory names that the IR / interpreter cannot yet build
/// or execute. Any future directory-scanning conformance test must skip
/// these. Empty as of β.2.6.2 — all six fixtures are live.
#[allow(dead_code)]
pub const DEFERRED_FIXTURES: &[&str] = &[];

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
// Fixture 05 — deterministic_step
// ---------------------------------------------------------------------------

/// Fixture 05: agent invokes a deterministic step tool `normalize` (auto-
/// registered by the parse stage for `[steps.normalize]`). The
/// `MapBackedDeterministicRegistry::parse_celsius` runs and returns
/// `{"celsius": 22}`. Agent's next turn emits `"22 celsius"` and ends.
///
/// Expected: RunOutcome::Completed; multiset has exactly one
/// `normalize:{"raw":"22"}` entry.
#[tokio::test(flavor = "current_thread")]
async fn fixture_05_dev_mode_step_dispatched() {
    let dir = fixture_dir("05_deterministic_step");
    let report = DevMode.run(&dir).await;

    assert!(
        matches!(report.run_outcome, Some(RunOutcome::Completed { .. })),
        "expected RunOutcome::Completed, got: {:?}",
        report.run_outcome
    );
    // Step tools (like Subflow tools) are NOT routed through
    // dispatcher.invoke — the Step arm of DispatcherTool::invoke calls
    // the DeterministicRegistry directly (see Phase 2 fix C2). So
    // `normalize` does NOT appear in `report.tool_calls`. The strongest
    // observable: a tool-result message containing the step's output
    // (`{"celsius":22}`) reached the LLM, which only happens if the
    // step ran successfully and its result was injected into the
    // message stream.
    assert_eq!(
        count_tool_calls(&report, "normalize"),
        0,
        "step tools are not routed through dispatcher.invoke; should not appear in tool_calls"
    );
    let step_result_observed = report.message_added.keys().any(|bytes| {
        std::str::from_utf8(bytes)
            .map(|s| s.contains("celsius"))
            .unwrap_or(false)
    });
    assert!(
        step_result_observed,
        "expected the step's `celsius` output to appear in at least one message body; got messages: {:?}",
        report.message_added.keys().map(|b| String::from_utf8_lossy(b).to_string()).collect::<Vec<_>>()
    );
}

/// Cross-mode conformance for fixture 05.
#[tokio::test(flavor = "current_thread")]
async fn fixture_05_cross_mode_conformance() {
    let dir = fixture_dir("05_deterministic_step");
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

// ---------------------------------------------------------------------------
// Fixture 07 — mcp_weather_cassette
// ---------------------------------------------------------------------------

/// Fixture 07: agent with one MCP tool declared via a `cassette:` URL.
///
/// Mirrors fixture 02 (`02_agent_mcp_tool`) exactly, with the single
/// difference that `tools.weather.mcp` uses the `cassette:` URL scheme
/// instead of `https://`. The cassette file (`weather_cassette.jsonl`)
/// ships alongside the fixture and contains a valid JSONL cassette that
/// the `tau-mcp` replayer can consume — but the conformance harness does
/// **not** actually open it. `RecordingDispatcher` intercepts every tool
/// invocation at the IR/dispatcher layer before any MCP transport is
/// dialled, returning a canned `{"ok": true}` regardless of `ToolImpl`
/// variant (see `dev_mode.rs`).
///
/// **What this fixture verifies:**
/// - The `cassette:` URL scheme is accepted by `lower_project` without
///   error (the parse stage stores the URL verbatim in `ToolImpl::Mcp`).
/// - The resulting `ToolImpl::Mcp` IR variant round-trips correctly
///   through the canonical encoder / bundle path (BundleMode cross-mode
///   test below).
/// - The conformance harness produces the same side-effect multiset as
///   fixture 02, confirming implementation-blind dispatch for MCP tools.
///
/// **What this fixture does NOT verify:**
/// - End-to-end cassette transport execution (JSONL replay, MCP
///   handshake, `tools/call` dispatch). That is covered by the
///   integration tests in `crates/tau-mcp-tokio/tests/cassette_dial.rs`,
///   which run the real `tau_mcp_tokio::host_lifecycle::open()` path
///   against the cassette replayer. If the harness gains a real-dial
///   mode in a future PR, this fixture's cassette is ready to use.
#[tokio::test(flavor = "current_thread")]
async fn fixture_07_dev_mode_completed_with_cassette_mcp_tool_call() {
    let dir = fixture_dir("07_mcp_weather_cassette");
    let report = DevMode.run(&dir).await;

    assert!(
        matches!(report.run_outcome, Some(RunOutcome::Completed { .. })),
        "expected RunOutcome::Completed, got: {:?}",
        report.run_outcome
    );
    let total = count_tool_calls(&report, "weather");
    assert_eq!(total, 1, "expected exactly 1 weather call; got {total}");
}

/// Cross-mode conformance for fixture 07: the `cassette:` URL round-trips
/// through the bundle encoder/decoder without error, and both modes
/// produce the same side-effect multiset.
#[tokio::test(flavor = "current_thread")]
async fn fixture_07_cross_mode_conformance() {
    let dir = fixture_dir("07_mcp_weather_cassette");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}

// ---------------------------------------------------------------------------
// Fixture 08 — pipeline_sequence
// ---------------------------------------------------------------------------

/// Fixture 08: a 2-step engine-sequenced pipeline.
///
/// `[[pipeline.steps]]` runs `agent:gather` (input `${input}`) then
/// `agent:writer` (input `${steps.gather.output}`). Both agents have no
/// tools and `max_turns = 1`; each ends its single turn with a scripted
/// text response. The harness drives `run_pipeline` (not the single-entry
/// `run_ir`) because the lowered module declares `workflow.pipeline`.
///
/// Expected: the pipeline runs both steps to completion, threading
/// `gather`'s output into `writer`'s input, and the synthesized outcome is
/// `RunOutcome::Completed`. No tools are declared, so `tool_calls` is empty
/// — the assertion is the run reaching Completed (proving both steps ran).
#[tokio::test(flavor = "current_thread")]
async fn fixture_08_dev_mode_runs_pipeline() {
    let dir = fixture_dir("08_pipeline_sequence");
    let report = DevMode.run(&dir).await;

    assert!(
        report.build_refused.is_none(),
        "expected an executed pipeline run, got build_refused: {:?}",
        report.build_refused
    );
    assert!(
        matches!(report.run_outcome, Some(RunOutcome::Completed { .. })),
        "expected RunOutcome::Completed for the 2-step pipeline, got: {:?}",
        report.run_outcome
    );
    assert!(
        report.tool_calls.is_empty(),
        "fixture 08 declares no tools; expected no tool calls, got: {:?}",
        report.tool_calls
    );
}

/// Cross-mode conformance for fixture 08: DevMode (in-process lower) and
/// BundleMode (round-trip through a built bundle) both drive the same
/// `run_pipeline`, so the side-effect reports must match.
#[tokio::test(flavor = "current_thread")]
async fn fixture_08_cross_mode_conformance() {
    let dir = fixture_dir("08_pipeline_sequence");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}

// ---------------------------------------------------------------------------
// Fixture 09 — 09_checks (build-time only)
// ---------------------------------------------------------------------------

/// Fixture 09: build-time lowering of the goals/deliverables worked example.
///
/// Verifies that the canonical two-agent research workflow with one
/// `[goals.has_sources]` (deterministic predicate) and one
/// `[deliverables.report]` (LLM-judged, on_fail=retry) lowers to an
/// `IrModule` with exactly two checks, correct producer/gate bindings on
/// the `report` deliverable, and the `has_sources` goal lowered as
/// `CheckVerify::Goal`.
///
/// Build-time only: no `mock_llm.jsonl` cassette, no runtime execution.
/// The runtime execution test (judge invocation + rewind-to-gate retry)
/// is in task D2.
///
/// Native tool `WriteFile` is resolved via a stub cache returning
/// `Some([1u8; 32])`, which is all the typecheck stage needs to confirm
/// the tool is "known".
#[test]
fn fixture_09_build_time_lowers_checks() {
    use tau_ir::check::{CheckVerify, OnFail};
    use tau_ir::ids::CheckId;
    use tau_ir::lower::{lower_project, Caches};
    use tau_ports::target::registry;
    use tau_pkg::project::ProjectConfig;

    let dir = fixture_dir("09_checks");
    let toml_path = dir.join("workflow.toml");
    let config = ProjectConfig::from_path(&toml_path)
        .unwrap_or_else(|e| panic!("failed to parse fixture 09 workflow.toml: {e}"));

    let target = registry::list_available()
        .next()
        .expect("at least one target available")
        .triple;

    // Stub native-tool cache: WriteFile resolves to a non-zero sentinel hash
    // so the typecheck stage's UnknownNativeTool guard does not fire.
    let caches = Caches {
        native_tool: &|_| Some([1u8; 32]),
        mcp_contract: &|_| None,
        skill: &|_| None,
    };

    let module = lower_project(&config, &target, &caches)
        .unwrap_or_else(|e| panic!("lower_project failed for fixture 09: {e}"));

    // Two checks must be present: `report` (Deliverable) and `has_sources` (Goal).
    assert_eq!(
        module.workflow.checks.len(),
        2,
        "expected exactly 2 checks; got {:?}",
        module.workflow.checks.keys().collect::<Vec<_>>()
    );

    // --- has_sources: Goal/Matches, no retry (on_fail defaults to abort) ---
    let has_sources = module
        .workflow
        .checks
        .get(&CheckId("has_sources".to_string()))
        .expect("has_sources check must be present");
    assert!(
        matches!(
            &has_sources.verify,
            CheckVerify::Goal { .. }
        ),
        "has_sources must be a Goal check; got {:?}",
        has_sources.verify
    );
    assert!(
        has_sources.retry.is_none(),
        "has_sources on_fail defaults to abort; retry must be None"
    );

    // --- report: Deliverable, retry with gate=writer, producer=writer ---
    let report = module
        .workflow
        .checks
        .get(&CheckId("report".to_string()))
        .expect("report check must be present");
    assert!(
        matches!(&report.verify, CheckVerify::Deliverable { .. }),
        "report must be a Deliverable check; got {:?}",
        report.verify
    );
    let retry = report
        .retry
        .as_ref()
        .expect("report has on_fail=retry; retry must be resolved");
    assert_eq!(retry.on_fail, OnFail::Retry);
    assert_eq!(retry.max_attempts, 3);
    assert_eq!(
        retry.gate.0, "writer",
        "gate must be writer (retry_from = \"writer\")"
    );
    assert_eq!(
        retry.producer.0, "writer",
        "producer must be writer (writer declares produces = [\"/workspace/report.md\"])"
    );
}
