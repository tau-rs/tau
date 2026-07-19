//! Conformance test harness for the tau workflow IR.
//!
//! For each fixture, asserts the IR interpreter produces the expected
//! side effects under `DevMode`, and asserts cross-mode equivalence
//! (DevMode vs BundleMode) per D-7a (multiset side-effect equivalence).
//!
//! All fixtures are live:
//! `01_agent_native_tool`, `02_agent_mcp_tool`, `03_agent_denied_capability`,
//! `04_subflow_spawn_child`, `05_deterministic_step`,
//! `06_multi_turn_history`, `07_mcp_weather_cassette`,
//! `08_pipeline_sequence`, `09_deliverables_happy`,
//! `10_deliverable_retry`, `11_deliverable_no_producer`,
//! `12_pipeline_reverse_alpha`, `13_context_pipeline`,
//! `14_agent_output_schema`, `15_models_multi` (per-agent / per-judge
//! model resolution), `16_durable_per_turn`,
//! `17_durable_per_tool_call` (per-tool-call checkpoint granularity),
//! `18_durable_intent` (EPIC 6.1 intent-form durable knob), and
//! `19_subflow_attenuation_denied` (runtime subflow cap_subset denial —
//! contrast fixture 04's proper-narrowing allow).
//! No `DEFERRED_FIXTURES` slots remain.

use std::path::Path;

use tau_ir_conformance::{
    assert_conform, bundle_mode::BundleMode, dev_mode::DevMode, ExecutionMode,
};
use tau_runtime_core::outcome::RunOutcome;

/// Fixture directory names that the IR / interpreter cannot yet build
/// or execute. Any future directory-scanning conformance test must skip
/// these. Empty — all fourteen fixtures are live.
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
/// This is the "proper narrowing" case for the subflow runtime attenuation
/// decorator (`AttenuatedDispatcher`, wired at the `ToolImpl::Subflow` spawn
/// in `agent_loop.rs`): `notify.capabilities = [{ kind = "net.http" }]`
/// widens the frame grant so `meet({net.http}, {net.http}) = {net.http}` ⊇
/// `page`'s declared requirement — the call is allowed through to the
/// (recording) dispatcher. Contrast fixture 19
/// (`19_subflow_attenuation_denied`), which uses an EMPTY cap_subset and so
/// denies the identical child call.
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
// Fixture 09 — deliverables happy path (goal pass + deliverable pass)
// ---------------------------------------------------------------------------

/// Fixture 09: a `gather → writer` pipeline with a passing goal and a
/// passing deliverable — the acceptance test for the deliverables-and-goals
/// feature's happy path.
///
/// The lowering auto-appends two `StepRun::Check` steps at the pipeline
/// tail (goals by id, then deliverables by id), so the executed sequence is
/// `gather → writer → check:report_present (goal) → check:report (deliverable)`.
///
/// - **Goal** `report_present` (`check = "non_empty"`, Output locus
///   `steps.writer.output`): the harness `DeterministicRegistry` answers
///   `FN_BUILTIN_NON_EMPTY` by inspecting the `present`/`content` args the
///   interpreter builds from the `OutputStore` — no filesystem reader needed.
/// - **Deliverable** `report` (Output locus, built-in judge): the existence
///   floor passes (writer produced output), then the built-in judge runs as
///   a one-turn agent whose scripted LLM reply is `{"met": true, ...}`.
///
/// LLM call order consumed from `mock_llm.jsonl`: gather (turn 0), writer
/// (turn 1), judge (turn 2). The goal check consumes no LLM call.
///
/// Expected: both checks pass, the pipeline reaches `RunOutcome::Completed`,
/// and no tools are declared so `tool_calls` is empty.
#[tokio::test(flavor = "current_thread")]
async fn fixture_09_dev_mode_goal_and_deliverable_pass() {
    let dir = fixture_dir("09_deliverables_happy");
    let report = DevMode.run(&dir).await;

    assert!(
        report.build_refused.is_none(),
        "expected an executed pipeline run, got build_refused: {:?}",
        report.build_refused
    );
    assert!(
        matches!(report.run_outcome, Some(RunOutcome::Completed { .. })),
        "expected RunOutcome::Completed (goal + deliverable both pass), got: {:?}",
        report.run_outcome
    );
    assert!(
        report.tool_calls.is_empty(),
        "fixture 09 declares no tools; expected no tool calls, got: {:?}",
        report.tool_calls
    );
}

/// Cross-mode conformance for fixture 09: DevMode and BundleMode both drive
/// the same `run_pipeline` (with the same check steps + scripted judge), so
/// the side-effect reports must match.
#[tokio::test(flavor = "current_thread")]
async fn fixture_09_cross_mode_conformance() {
    let dir = fixture_dir("09_deliverables_happy");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}

// ---------------------------------------------------------------------------
// Fixture 10 — deliverable retry converges
// ---------------------------------------------------------------------------

/// Fixture 10: a `gather → writer` pipeline with a retrying deliverable that
/// converges on the SECOND attempt — the acceptance test for the
/// rewind-to-gate retry loop.
///
/// The deliverable `report` has `on_fail = "retry"`, `max_attempts = 3`,
/// `retry_from = "writer"`. Lowering auto-appends one `StepRun::Check` for the
/// deliverable at the pipeline tail, so the executed sequence is
/// `gather → writer → check:report (deliverable)`.
///
/// **Convergence path.** The built-in judge rejects the writer's first draft
/// (`{"met": false}`), so `run_pipeline` rewinds the index to the gate step
/// `writer` and re-runs the forward slice — `gather` runs ONCE (it is before
/// the gate), `writer` re-runs with the injected "Previous attempt rejected:
/// …" feedback turn. The second draft is accepted (`{"met": true}`) and the
/// pipeline completes.
///
/// **Scripted LLM call sequence** (the `SequencedLlm` is call-ordered — a flat
/// queue popped in order, independent of which agent calls — so a rewound
/// `writer` pops the NEXT scripted line, NOT a re-keyed one):
///
/// 1. `gather` (attempt 1)             → `"gathered: 42"`
/// 2. `writer` (attempt 1)             → `"draft"` (BAD)
/// 3. deliverable judge (attempt 1)    → `{"met": false, "rationale": "needs more detail"}`
/// 4. `writer` (attempt 2, post-rewind)→ `"report: … (revised)"` (GOOD)
/// 5. deliverable judge (attempt 2)    → `{"met": true, "rationale": "ok"}`
///
/// **Convergence assertion.** `ConformanceReport` does not capture trace
/// events, so there is no `check.retry` to inspect directly. Instead, reaching
/// `RunOutcome::Completed` is itself proof that attempt 2 ran: a single
/// `{"met": false}` does NOT abort (`max_attempts = 3`), it rewinds — so the
/// ONLY way to reach Completed is for the judge to eventually return
/// `{"met": true}`, which is scripted as the 5th line and is reachable only
/// after the writer re-runs (line 4). If the rewind had not occurred, the run
/// would have stalled / consumed the script out of order. No tools are
/// declared, so `tool_calls` is empty.
#[tokio::test(flavor = "current_thread")]
async fn fixture_10_dev_mode_deliverable_retry_converges() {
    let dir = fixture_dir("10_deliverable_retry");
    let report = DevMode.run(&dir).await;

    assert!(
        report.build_refused.is_none(),
        "expected an executed pipeline run, got build_refused: {:?}",
        report.build_refused
    );
    assert!(
        matches!(report.run_outcome, Some(RunOutcome::Completed { .. })),
        "expected RunOutcome::Completed (deliverable converges on attempt 2), got: {:?}",
        report.run_outcome
    );
    assert!(
        report.tool_calls.is_empty(),
        "fixture 10 declares no tools; expected no tool calls, got: {:?}",
        report.tool_calls
    );
}

/// Cross-mode conformance for fixture 10: DevMode and BundleMode both drive
/// the same `run_pipeline` (with the same rewind-to-gate retry + scripted
/// judge), so the side-effect reports must match.
#[tokio::test(flavor = "current_thread")]
async fn fixture_10_cross_mode_conformance() {
    let dir = fixture_dir("10_deliverable_retry");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}

// ---------------------------------------------------------------------------
// Fixture 11 — deliverable_no_producer (build refused)
// ---------------------------------------------------------------------------

/// Fixture 11: build-time `DeliverableNoProducer` refusal.
///
/// `[deliverables.report]` declares `path = "/workspace/report.md"` but
/// neither the `gather` agent nor the `writer` agent carries a
/// `produces = ["/workspace/report.md"]` entry. The `tau-pkg` validator's
/// `validate_postconditions` detects the missing producer binding during
/// `ProjectConfig::parse_str` (before `lower_project` runs) and returns
/// `ProjectConfigError::DeliverableNoProducer`.
///
/// BOTH DevMode and BundleMode must surface this as a `build_refused`
/// report (D-3b refusal symmetry). DevMode captures the `parse_str` error
/// in its sync setup block and returns early; BundleMode captures it inside
/// `build_ir_payload`. Neither surface hits the interpreter.
///
/// Expected: `build_refused` is `Some(_)`, the diagnostic contains
/// `"no producer"`, and the multiset fields are empty.
#[tokio::test(flavor = "current_thread")]
async fn fixture_11_dev_mode_build_refused_no_producer() {
    let dir = fixture_dir("11_deliverable_no_producer");
    let report = DevMode.run(&dir).await;

    let refused = report
        .build_refused
        .as_ref()
        .expect("expected build_refused; got an executed-run report");
    assert!(
        refused.contains("no producer"),
        "diagnostic should mention 'no producer'; got: {refused}"
    );
    assert!(
        refused.contains("report"),
        "diagnostic should name the deliverable id 'report'; got: {refused}"
    );
    assert!(report.tool_calls.is_empty());
    assert!(report.message_added.is_empty());
}

/// Cross-mode conformance for fixture 11: both modes must refuse with the
/// same diagnostic string (D-3b refusal symmetry). The `DeliverableNoProducer`
/// error comes from `ProjectConfig::parse_str` in both paths, so the
/// `Display` output is identical.
#[tokio::test(flavor = "current_thread")]
async fn fixture_11_cross_mode_conformance() {
    let dir = fixture_dir("11_deliverable_no_producer");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}

// ---------------------------------------------------------------------------
// Fixture 12 — pipeline final-message picks the last EXECUTED step (#337)
// ---------------------------------------------------------------------------

/// Regression lock for `drive_pipeline`'s final-message synthesis.
///
/// Fixture 12's step ids are reverse-alphabetical relative to execution
/// order (`zzz_first` runs first, `aaa_last` runs last). The synthesized
/// `final_message` must carry the LAST-executed step's output
/// (`aaa_last` → "last-step-output"). The previous implementation keyed
/// off `OutputStore::template_map().into_values().next_back()` — the
/// alphabetically-last id — which on this fixture would harvest
/// `zzz_first`'s "first-step-output" instead. `assert_conform` never
/// compares `final_message`, so this is the only test that pins the
/// step-selection logic.
///
/// (Renumbered from 09 to 12 during the deliverables-and-goals merge, which
/// claimed 09/10/11 for its own fixtures.)
#[tokio::test(flavor = "current_thread")]
async fn fixture_12_pipeline_picks_last_executed_step() {
    use tau_domain::MessagePayload;

    let dir = fixture_dir("12_pipeline_reverse_alpha");
    let report = DevMode.run(&dir).await;

    let final_message = match report.run_outcome {
        Some(RunOutcome::Completed {
            ref final_message, ..
        }) => final_message.clone(),
        other => panic!("expected RunOutcome::Completed, got: {other:?}"),
    };

    match final_message.payload {
        MessagePayload::Text { content } => assert_eq!(
            content, "last-step-output",
            "final_message must come from the last-EXECUTED step (aaa_last), \
             not the alphabetically-last step id (zzz_first)"
        ),
        other => panic!("expected a Text payload, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Fixture 13 — context pipeline runs identically dev vs bundle (β.4)
// ---------------------------------------------------------------------------

/// Fixture 13: a single agent (`mono`) carrying a 3-step context pipeline,
/// driven through a single end_turn turn — the end-to-end acceptance test
/// for the β.4 context-manager TOML→IR→runtime path.
///
/// The agent declares `[agents.mono.context.pipeline]` with three
/// transformers (`trim_old` → `compact_tool_outputs` → `fit_budget`, the
/// last step is `fit_budget` as the typecheck rule requires) plus their
/// per-step config tables. Lowering parses + typechecks the pipeline,
/// embeds it in the IR `Agent`, and the agent loop applies the transformers
/// to the message history before each turn.
///
/// This fixture has no pipeline block and no tools, so it runs through the
/// single-entry `run_ir` path (like fixtures 01-07) with `mono` auto-selected
/// as the alphabetically-first (and only) agent. On this single-turn run the
/// history is short, so the transformers are effectively no-ops — the value
/// is proving the context path lowers, typechecks, and executes WITHOUT
/// diverging between DevMode (in-process lower) and BundleMode (round-trip
/// through a built bundle).
///
/// Expected: `RunOutcome::Completed`, no tool calls, and cross-mode
/// equivalence under `assert_conform`. If the context path had an
/// integration defect (panic in `build_context_pipeline`, a typecheck
/// rejection of this valid pipeline, or transformer application breaking the
/// loop), this fixture would surface it as a failed/refused run here.
#[tokio::test(flavor = "current_thread")]
async fn fixture_13_dev_mode_completed() {
    let dir = fixture_dir("13_context_pipeline");
    let report = DevMode.run(&dir).await;

    assert!(
        report.build_refused.is_none(),
        "expected an executed run, got build_refused: {:?}",
        report.build_refused
    );
    assert!(
        matches!(report.run_outcome, Some(RunOutcome::Completed { .. })),
        "expected RunOutcome::Completed, got: {:?}",
        report.run_outcome
    );
    assert!(
        report.tool_calls.is_empty(),
        "fixture 13 declares no tools; expected no tool calls, got: {:?}",
        report.tool_calls
    );
}

/// Cross-mode conformance for fixture 13: DevMode (in-process lower of the
/// context pipeline) and BundleMode (the lowered context pipeline round-trips
/// through the canonical bundle encoder/decoder) must drive the agent loop
/// identically, so the side-effect reports compare byte-for-byte.
#[tokio::test(flavor = "current_thread")]
async fn fixture_13_cross_mode_conformance() {
    let dir = fixture_dir("13_context_pipeline");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}

// ---------------------------------------------------------------------------
// Fixture 14 — agent output_schema is additive + byte-stable (v1.3.0)
// ---------------------------------------------------------------------------

/// Fixture 14: mirrors fixture 01 (agent + one native tool, two turns) with an
/// additional `output_schema` on the agent. The schema does not affect
/// execution — it is carried verbatim on the IR `Agent` node. This dev-mode
/// test proves the v1.2.0→v1.3.0 additive field lowers and runs to completion
/// with the same single `read_temp` tool call as the schema-less fixture 01.
/// (The canonical bundle encode/decode round-trip of the field is covered by
/// the cross-mode test below.)
#[tokio::test(flavor = "current_thread")]
async fn fixture_14_dev_mode_completed_with_output_schema() {
    let dir = fixture_dir("14_agent_output_schema");
    let report = DevMode.run(&dir).await;

    assert!(
        report.build_refused.is_none(),
        "expected an executed run, got build_refused: {:?}",
        report.build_refused
    );
    assert!(
        matches!(report.run_outcome, Some(RunOutcome::Completed { .. })),
        "expected RunOutcome::Completed, got: {:?}",
        report.run_outcome
    );
    let total = count_tool_calls(&report, "read_temp");
    assert_eq!(total, 1, "expected exactly 1 read_temp call; got {total}");
}

/// Cross-mode conformance for fixture 14: the agent's `output_schema` round-trips
/// through the bundle's canonical encoder/decoder (BundleMode asserts
/// `canonical_hash` equality internally), and both modes produce the same
/// side-effect multiset.
#[tokio::test(flavor = "current_thread")]
async fn fixture_14_cross_mode_conformance() {
    let dir = fixture_dir("14_agent_output_schema");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}

// ---------------------------------------------------------------------------
// Fixture 15 — models_multi (per-agent / per-judge model resolution)
// ---------------------------------------------------------------------------

/// Fixture 15 lowering: a `[models]` table with two aliases on one backend,
/// two agents on different models, one deliverable whose judge inherits its
/// producer's model and one whose explicit `judge_model` overrides it.
///
/// This is the load-bearing assertion for per-agent / per-judge model
/// resolution (spec D2/D5/Q4): the resolved `ModelRef`s are baked at lowering.
///   * `gather`  → `deep`  → `mock-deep`
///   * `writer`  → `fast`  → `mock-fast`
///   * deliverable `report`  judge inherits producer `writer` → `mock-fast`
///   * deliverable `summary` judge has explicit `judge_model = "fast"` even
///     though its producer `gather` is `deep` → `mock-fast` (override, not
///     producer-derived).
#[test]
fn fixture_15_lowers_distinct_models_and_judge_resolution() {
    use tau_ir::check::{CheckVerify, JudgeRef};
    use tau_ir::ids::{AgentId, CheckId};
    use tau_ir_lower::{lower_project, Caches};
    use tau_pkg::project::ProjectConfig;

    let toml = std::fs::read_to_string(fixture_dir("15_models_multi").join("workflow.toml"))
        .expect("read workflow.toml");
    let config = ProjectConfig::parse_str(&toml).expect("parse + validate fixture-15");

    let target = tau_ports::target::registry::list_available()
        .next()
        .expect("at least one target triple available")
        .triple;
    let caches = Caches {
        native_tool: &|name: &str| {
            let seed = name.as_bytes().first().copied().unwrap_or(1);
            Some([seed; 32])
        },
        mcp_contract: &|_| None,
        skill: &|_| None,
        prompt_file: &|_| Ok(Vec::new()),
    };
    let module = lower_project(&config, &target, &caches)
        .expect("lower fixture-15")
        .module;

    // Agents carry distinct resolved model ids.
    let gather = module
        .workflow
        .agents
        .get(&AgentId("gather".into()))
        .expect("gather agent");
    let writer = module
        .workflow
        .agents
        .get(&AgentId("writer".into()))
        .expect("writer agent");
    assert_eq!(gather.model_ref.backend, "mock-llm");
    assert_eq!(gather.model_ref.model_id, "mock-deep");
    assert_eq!(writer.model_ref.model_id, "mock-fast");

    // Helper: extract a deliverable check's Default-judge model id.
    let judge_model_id = |check_id: &str| -> String {
        let check = module
            .workflow
            .checks
            .get(&CheckId(check_id.into()))
            .unwrap_or_else(|| panic!("check {check_id} present"));
        match &check.verify {
            CheckVerify::Deliverable {
                judge: JudgeRef::Default { model_ref },
                ..
            } => model_ref.model_id.clone(),
            other => panic!("check {check_id}: expected Default judge, got {other:?}"),
        }
    };

    // `report` inherits its producer (writer → fast).
    assert_eq!(judge_model_id("report"), "mock-fast");
    // `summary` uses the explicit judge_model = "fast", overriding its
    // producer `gather` (which is `deep`).
    assert_eq!(judge_model_id("summary"), "mock-fast");
}

/// Fixture 15 dev-mode: the multi-model pipeline + two judged deliverables
/// run to completion (gather → writer → report-judge → summary-judge).
#[tokio::test(flavor = "current_thread")]
async fn fixture_15_dev_mode_completed() {
    let dir = fixture_dir("15_models_multi");
    let report = DevMode.run(&dir).await;
    assert!(
        matches!(report.run_outcome, Some(RunOutcome::Completed { .. })),
        "expected RunOutcome::Completed, got: {:?}",
        report.run_outcome
    );
}

/// Fixture 15 cross-mode conformance (DevMode vs BundleMode).
#[tokio::test(flavor = "current_thread")]
async fn fixture_15_cross_mode_conformance() {
    let dir = fixture_dir("15_models_multi");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}

// ---------------------------------------------------------------------------
// Fixture 16 — durable agent (ADR-0053) is additive + byte-stable
// ---------------------------------------------------------------------------

/// Fixture 16: mirrors fixture 01 (agent + one native tool, two turns) with
/// an additional `[agents.fan.durable]` block. The durable config is carried
/// verbatim on the IR `Agent`; it does not affect execution (the conformance
/// harness drives the interpreter with no `CheckpointStore`, so no checkpoint
/// is written). This dev-mode test proves the v2.1.0→v2.2.0 additive field
/// lowers and runs to completion with the same single `read_temp` tool call
/// as the durable-less fixture 01.
#[tokio::test(flavor = "current_thread")]
async fn fixture_16_dev_mode_completed_with_durable() {
    let dir = fixture_dir("16_durable_per_turn");
    let report = DevMode.run(&dir).await;

    assert!(
        report.build_refused.is_none(),
        "expected an executed run, got build_refused: {:?}",
        report.build_refused
    );
    assert!(
        matches!(report.run_outcome, Some(RunOutcome::Completed { .. })),
        "expected RunOutcome::Completed, got: {:?}",
        report.run_outcome
    );
    let total = count_tool_calls(&report, "read_temp");
    assert_eq!(total, 1, "expected exactly 1 read_temp call; got {total}");
}

/// Cross-mode conformance for fixture 16: the agent's `durable` block
/// round-trips through the bundle's canonical encoder/decoder (BundleMode
/// asserts `canonical_hash` equality internally), and both modes produce the
/// same side-effect multiset — durable is observationally inert.
#[tokio::test(flavor = "current_thread")]
async fn fixture_16_cross_mode_conformance() {
    let dir = fixture_dir("16_durable_per_turn");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}

// ---------------------------------------------------------------------------
// Fixture 17 — durable per_tool_call (ADR-0053) is additive + byte-stable
// ---------------------------------------------------------------------------

/// Fixture 17: one turn issuing two native tool calls (read_temp +
/// read_humidity) with a `per_tool_call` durable block. The durable config
/// is carried verbatim on the IR `Agent`; it does not affect execution (the
/// conformance harness drives the interpreter with no `CheckpointStore`, so
/// no checkpoint is written). This dev-mode test proves the `per_tool_call`
/// granularity variant lowers and runs to completion with both tool calls
/// recorded — the observable side-effect multiset is identical to a
/// durable-less twin.
#[tokio::test(flavor = "current_thread")]
async fn fixture_17_dev_mode_completed_with_per_tool_call() {
    let dir = fixture_dir("17_durable_per_tool_call");
    let report = DevMode.run(&dir).await;
    assert!(
        report.build_refused.is_none(),
        "got build_refused: {:?}",
        report.build_refused
    );
    assert!(
        matches!(report.run_outcome, Some(RunOutcome::Completed { .. })),
        "got: {:?}",
        report.run_outcome
    );
    assert_eq!(count_tool_calls(&report, "read_temp"), 1);
    assert_eq!(count_tool_calls(&report, "read_humidity"), 1);
}

/// Cross-mode conformance for fixture 17: the agent's `durable` block with
/// `per_tool_call` granularity round-trips through the bundle's canonical
/// encoder/decoder (BundleMode asserts `canonical_hash` equality internally),
/// and both modes produce the same side-effect multiset — durable is
/// observationally inert.
#[tokio::test(flavor = "current_thread")]
async fn fixture_17_cross_mode_conformance() {
    let dir = fixture_dir("17_durable_per_tool_call");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}

// ---------------------------------------------------------------------------
// Fixture 18 — durable intent form (EPIC 6.1) is additive + byte-stable
// ---------------------------------------------------------------------------

/// Fixture 18: mirrors fixture 16 (agent + one native tool, two turns) with
/// the intent form `durable = "survive-restarts"` instead of an explicit
/// `[agents.fan.durable]` sub-table. The intent is carried verbatim on the
/// IR `Agent` as `Durability::Intent(DurabilityIntent::SurviveRestarts)`; it
/// does not affect execution (the conformance harness drives the interpreter
/// with no `CheckpointStore`, so no checkpoint is written). This dev-mode
/// test proves the intent form lowers and runs to completion with the same
/// single `read_temp` tool call as fixtures 01 / 16.
#[tokio::test(flavor = "current_thread")]
async fn fixture_18_dev_mode_completed_with_durable_intent() {
    let dir = fixture_dir("18_durable_intent");
    let report = DevMode.run(&dir).await;

    assert!(
        report.build_refused.is_none(),
        "expected an executed run, got build_refused: {:?}",
        report.build_refused
    );
    assert!(
        matches!(report.run_outcome, Some(RunOutcome::Completed { .. })),
        "expected RunOutcome::Completed, got: {:?}",
        report.run_outcome
    );
    let total = count_tool_calls(&report, "read_temp");
    assert_eq!(total, 1, "expected exactly 1 read_temp call; got {total}");
}

/// Cross-mode conformance for fixture 18: the agent's `durable` intent block
/// round-trips through the bundle's canonical encoder/decoder (BundleMode
/// asserts `canonical_hash` equality internally), and both modes produce the
/// same side-effect multiset — intent-form durable is observationally inert.
#[tokio::test(flavor = "current_thread")]
async fn fixture_18_cross_mode_conformance() {
    let dir = fixture_dir("18_durable_intent");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}

// ---------------------------------------------------------------------------
// Fixture 19 — subflow_attenuation_denied (runtime cap_subset denial)
// ---------------------------------------------------------------------------

/// Fixture 19: mirrors fixture 04 (parent invokes subflow tool `notify`,
/// spawning `worker`, which calls MCP tool `page`) except `notify`'s
/// cap_subset is EMPTY (`capabilities = []`).
///
/// The `AttenuatedDispatcher` wired at the `ToolImpl::Subflow` spawn in
/// `agent_loop.rs` computes `meet(∅, {net.http}) = ∅`, which does not cover
/// `page`'s declared `net.http` requirement. The call is denied — soft: an
/// `is_error` tool result is returned to the worker (never reaching
/// `RecordingDispatcher::invoke`), and a `runtime.subflow.attenuation_denied`
/// tracing event fires — and the worker's scripted final turn still runs, so
/// the overall run reaches `RunOutcome::Completed`.
///
/// Expected: RunOutcome::Completed (soft-deny, not a hard failure); `page`
/// does NOT appear in `tool_calls` (denied before ever reaching the
/// dispatcher); `notify` does not appear either (subflow tools are never
/// routed through `dispatcher.invoke` — same as fixture 04).
#[tokio::test(flavor = "current_thread")]
async fn fixture_19_dev_mode_page_denied_by_attenuation() {
    let dir = fixture_dir("19_subflow_attenuation_denied");
    let report = DevMode.run(&dir).await;

    assert!(
        report.build_refused.is_none(),
        "expected an executed run, got build_refused: {:?}",
        report.build_refused
    );
    assert!(
        matches!(report.run_outcome, Some(RunOutcome::Completed { .. })),
        "soft-deny: run must still complete, got: {:?}",
        report.run_outcome
    );
    assert_eq!(
        count_tool_calls(&report, "page"),
        0,
        "page must be denied by the empty cap_subset before reaching dispatcher.invoke"
    );
    assert_eq!(
        count_tool_calls(&report, "notify"),
        0,
        "subflow tools are not routed through dispatcher.invoke; should not appear in tool_calls"
    );
}

/// Cross-mode conformance for fixture 19: dev-mode and bundle-mode must
/// agree that `page` is denied, proving the attenuation decorator — which
/// lives in shared `tau-runtime-core` — behaves identically regardless of
/// which surface (in-process interpreter vs built bundle) drives it.
#[tokio::test(flavor = "current_thread")]
async fn fixture_19_cross_mode_conformance() {
    let dir = fixture_dir("19_subflow_attenuation_denied");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}

// ---------------------------------------------------------------------------
// Bundle reentrancy — the runtime half of the "safe-to-retry reentrant
// artifact" contract (durable-execution Phase B)
// ---------------------------------------------------------------------------

/// Reentrancy reference test: a built tau bundle is a pure function of
/// `(bundle, input)` — invoking it twice with the same input yields an
/// identical observable outcome.
///
/// This is the runtime half of the contract documented in
/// `docs/how-to/run-tau-under-a-durable-orchestrator.md`: a host
/// orchestrator (Temporal / Inngest / Cloudflare Workflows) owns
/// *durability* (when / whether to re-run), and tau guarantees the bundle
/// is a *reentrant* unit that is safe to retry. "Safe to retry" means a
/// second invocation with the same input produces the same observable
/// side effects as the first — exactly what a retry policy assumes.
///
/// The *build* half of the contract — that a bundle's content address
/// (`bundle.sha256`) is a pure function of its source — is proven
/// separately by `tau_pkg::bundle::reproduce` (e.g.
/// `reproducible_when_tree_unchanged`) and `verify_self_hash`. Together
/// the two halves establish "pure function of `(bundle_hash, input)`".
///
/// Fixture 06 (`06_multi_turn_history`) is used because a multi-turn agent
/// loop is the closest fixture to the long-running unit the durability
/// story cares about: three consecutive tool-use turns then an end_turn.
/// Comparison goes through [`assert_conform`] — outcome compared by kind,
/// side effects compared as `(tool_name, args)` and message multisets —
/// because the raw `RunOutcome` embeds per-run message UUIDs and
/// timestamps that are nondeterministic *by design* and are not part of
/// the observable outcome a retry must reproduce.
#[tokio::test(flavor = "current_thread")]
async fn bundle_invocation_is_reentrant_multi_turn() {
    let dir = fixture_dir("06_multi_turn_history");

    // Two independent invocations of the same bundle with the same input.
    let first = BundleMode.run(&dir).await;
    let second = BundleMode.run(&dir).await;

    // Both invocations must actually execute (not build-refuse) and reach
    // a completed outcome — otherwise "identical outcome" could be
    // trivially satisfied by two identical failures.
    assert!(
        first.build_refused.is_none(),
        "first invocation unexpectedly build-refused: {:?}",
        first.build_refused
    );
    assert!(
        matches!(first.run_outcome, Some(RunOutcome::Completed { .. })),
        "first invocation must complete, got: {:?}",
        first.run_outcome
    );

    // The load-bearing assertion: a retry of the same bundle with the same
    // input is observationally identical to the original invocation.
    assert_conform(&first, &second);
}
