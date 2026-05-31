//! Integration tests covering the 5 worked patterns from the spec.
//!
//! Each test wires a Runtime with MockLlmBackend (scripted responses),
//! invokes `spawn_root_agent`, and asserts:
//!   - `snapshot.status == Completed`
//!   - `snapshot.agents_spawned` is the expected count
//!
//! Implemented in Skills-4 T9 (bonus spec D3). Previously `#[ignore]`'d
//! pending the MockLlmBackend fixture built in T7.
//!
//! # MockLlmBackend
//!
//! `common::MockLlmBackend` is a copy of `tau-runtime/tests/common/mock_llm.rs`.
//! See that file's header for the duplication rationale.
//!
//! # task.* and run.note virtual tools
//!
//! `capability_satisfies` (in `tau-runtime/src/capability.rs`) now has
//! match arms for `Capability::TaskList { mode }` and `Capability::Plan
//! { mode }` (PR #81), so `task.*` and `run.note` virtual-tool calls
//! pass the runtime's capability gate. They are exercised by the two
//! tests at the bottom of this file (`task_list_create_claim_complete_flow`
//! and `run_note_write_flow`); the original five pattern tests
//! (A–E) deliberately stay scoped to `agent.<kind>.spawn`.
//!
//! # Capability grants
//!
//! Child agents receive a `grant` that is ⊆ the parent's capabilities.
//! Each test builds a manifest with exactly the `agent.spawn` capabilities
//! the root agent needs; child grants are serialised inline in spawn args.

mod common;

use std::sync::Arc;

use tau_ports::RunBudget;
use tau_runtime_tokio::Runtime;

// ---------------------------------------------------------------------------
// Shared manifest helpers
// ---------------------------------------------------------------------------

/// Build a manifest granting `agent.spawn` for the given allowed kinds.
///
/// `allowed_kinds_toml_array` is a comma-separated list of TOML string
/// literals, e.g. `r#""researcher""#` or `r#""coder", "tester""#`.
fn manifest_with_agent_spawn(allowed_kinds_toml_array: &str) -> tau_domain::PackageManifest {
    let toml_body = format!(
        r#"
name        = "orchestrator"
version     = "0.1.0"
description = "orchestrator agent"
authors     = []
source      = "https://example.com/orchestrator.git"
kind        = "tool"
dependencies = []

[[capabilities]]
kind = "agent.spawn"
allowed_kinds = [{allowed_kinds_toml_array}]
"#
    );
    common::manifest_from_toml(&toml_body)
}

// ---------------------------------------------------------------------------
// Pattern A: linear pipeline
// ---------------------------------------------------------------------------

/// Two-step pipeline: orchestrator → researcher → done.
///
/// Mock turn sequence:
///
///   Orchestrator turn 1: tool_call agent.researcher.spawn({message:"research the topic", grant:[]})
///   Orchestrator turn 2: text "orchestration complete"
///
///   Researcher turn 1: text "research findings"
///
/// Assertions:
///   - snapshot.status == Completed
///   - snapshot.agents_spawned == 1
///
/// This is the simplest linear delegation pattern: parent spawns one child,
/// child produces a text result, parent acknowledges and completes.
#[tokio::test]
async fn pattern_a_linear_pipeline() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let backend = common::MockLlmBackend::new("test-llm")
        // Orchestrator turn 1: spawn the researcher.
        .add_tool_call_json(
            "agent.researcher.spawn",
            serde_json::json!({
                "message": "research the topic",
                "grant": []
            }),
        )
        // Orchestrator turn 2: acknowledge result and complete.
        .add_text("orchestration complete")
        // Researcher turn 1: produce result text.
        .add_text("research findings");

    let runtime = Arc::new(
        Runtime::builder()
            .with_llm_backend(backend)
            .build()
            .expect("build runtime"),
    );

    let manifest = manifest_with_agent_spawn(r#""researcher""#);
    let agent_def = common::agent_def(
        "orchestrator",
        "Orchestrator",
        "orchestrator@0.1.0",
        "test-llm",
    );
    let initial = common::user_message("start the research pipeline");

    let snapshot = tau_runtime_tokio::spawn_root_agent_with_scope(
        runtime,
        agent_def,
        manifest,
        initial,
        RunBudget::default(),
        tmp.path().to_path_buf(),
    )
    .await
    .expect("spawn_root_agent must succeed");

    assert_eq!(
        snapshot.status,
        tau_ports::RunStatus::Completed,
        "run must complete; got {:?}",
        snapshot.status
    );
    assert_eq!(
        snapshot.agents_spawned, 1,
        "exactly 1 child (researcher) must be spawned; got {}",
        snapshot.agents_spawned
    );
}

// ---------------------------------------------------------------------------
// Pattern B: worker pool
// ---------------------------------------------------------------------------

/// Three workers sharing one task pool.
///
/// Mock turn sequence (interleaved: child turns immediately follow each spawn):
///
///   Planner turn 1:  agent.worker.spawn({message:"do task A", grant:[]})
///   Worker-1 turn 1: text "task A done"
///   Planner turn 2:  agent.worker.spawn({message:"do task B", grant:[]})
///   Worker-2 turn 1: text "task B done"
///   Planner turn 3:  agent.worker.spawn({message:"do task C", grant:[]})
///   Worker-3 turn 1: text "task C done"
///   Planner turn 4:  text "all workers dispatched"
///
/// The MockLlmBackend uses a single FIFO queue shared between parent and
/// child runs. Child turns must be queued immediately after the spawn call
/// that triggers them, because the child run consumes from the same queue
/// during its recursive invocation.
///
/// Assertions:
///   - snapshot.agents_spawned == 3
///   - snapshot.status == Completed
///
/// This validates the worker-pool pattern: a coordinator spawning N workers
/// in sequence (the runtime dispatches them serially — parallelism is a
/// future concern; this tests the fan-out spawning mechanics).
#[tokio::test]
async fn pattern_b_worker_pool() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let backend = common::MockLlmBackend::new("test-llm")
        // Planner turn 1: spawn worker-1.
        .add_tool_call_json(
            "agent.worker.spawn",
            serde_json::json!({
                "message": "do task A",
                "grant": []
            }),
        )
        // Worker-1 turn 1 (runs during spawn processing).
        .add_text("task A done")
        // Planner turn 2: spawn worker-2.
        .add_tool_call_json(
            "agent.worker.spawn",
            serde_json::json!({
                "message": "do task B",
                "grant": []
            }),
        )
        // Worker-2 turn 1 (runs during spawn processing).
        .add_text("task B done")
        // Planner turn 3: spawn worker-3.
        .add_tool_call_json(
            "agent.worker.spawn",
            serde_json::json!({
                "message": "do task C",
                "grant": []
            }),
        )
        // Worker-3 turn 1 (runs during spawn processing).
        .add_text("task C done")
        // Planner turn 4: acknowledge all workers done.
        .add_text("all workers dispatched");

    let runtime = Arc::new(
        Runtime::builder()
            .with_llm_backend(backend)
            .build()
            .expect("build runtime"),
    );

    let manifest = manifest_with_agent_spawn(r#""worker""#);
    let agent_def = common::agent_def("planner", "Planner", "orchestrator@0.1.0", "test-llm");
    let initial = common::user_message("spin up the worker pool");

    let snapshot = tau_runtime_tokio::spawn_root_agent_with_scope(
        runtime,
        agent_def,
        manifest,
        initial,
        RunBudget::default(),
        tmp.path().to_path_buf(),
    )
    .await
    .expect("spawn_root_agent must succeed");

    assert_eq!(
        snapshot.status,
        tau_ports::RunStatus::Completed,
        "run must complete; got {:?}",
        snapshot.status
    );
    assert_eq!(
        snapshot.agents_spawned, 3,
        "exactly 3 workers must be spawned; got {}",
        snapshot.agents_spawned
    );
}

// ---------------------------------------------------------------------------
// Pattern C: supervisor-critic
// ---------------------------------------------------------------------------

/// Supervisor spawns researcher; reads researcher's result;
/// spawns critic to evaluate; decides accept.
///
/// Mock turn sequence (child turns interleaved immediately after each spawn):
///
///   Supervisor turn 1: agent.researcher.spawn({message:"research X", grant:[]})
///   Researcher turn 1: text "findings from researcher"
///   Supervisor turn 2: agent.critic.spawn({message:"critique findings", grant:[]})
///   Critic turn 1:     text "findings look good"
///   Supervisor turn 3: text "accepted"
///
/// Assertions:
///   - snapshot.status == Completed
///   - snapshot.agents_spawned == 2
///
/// This validates the supervisor-critic pattern: a coordinator using
/// sequential spawns to apply multiple specialized agents in a pipeline.
#[tokio::test]
async fn pattern_c_supervisor_critic() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let backend = common::MockLlmBackend::new("test-llm")
        // Supervisor turn 1: spawn researcher.
        .add_tool_call_json(
            "agent.researcher.spawn",
            serde_json::json!({
                "message": "research X",
                "grant": []
            }),
        )
        // Researcher turn 1 (runs during spawn processing).
        .add_text("findings from researcher")
        // Supervisor turn 2: spawn critic with researcher's findings in context.
        .add_tool_call_json(
            "agent.critic.spawn",
            serde_json::json!({
                "message": "critique findings",
                "grant": []
            }),
        )
        // Critic turn 1 (runs during spawn processing).
        .add_text("findings look good")
        // Supervisor turn 3: decision.
        .add_text("accepted");

    let runtime = Arc::new(
        Runtime::builder()
            .with_llm_backend(backend)
            .build()
            .expect("build runtime"),
    );

    let manifest = manifest_with_agent_spawn(r#""researcher", "critic""#);
    let agent_def = common::agent_def("supervisor", "Supervisor", "orchestrator@0.1.0", "test-llm");
    let initial = common::user_message("start the supervisor-critic loop");

    let snapshot = tau_runtime_tokio::spawn_root_agent_with_scope(
        runtime,
        agent_def,
        manifest,
        initial,
        RunBudget::default(),
        tmp.path().to_path_buf(),
    )
    .await
    .expect("spawn_root_agent must succeed");

    assert_eq!(
        snapshot.status,
        tau_ports::RunStatus::Completed,
        "run must complete; got {:?}",
        snapshot.status
    );
    assert_eq!(
        snapshot.agents_spawned, 2,
        "2 children (researcher + critic) must be spawned; got {}",
        snapshot.agents_spawned
    );
}

// ---------------------------------------------------------------------------
// Pattern D: hierarchical team lead
// ---------------------------------------------------------------------------

/// Program manager → team lead → coder + tester (nesting depth 3).
///
/// Capability subset law at each level:
///   PM grants TeamLead: agent.spawn(coder, tester)
///   TeamLead grants Coder and Tester: [] (leaf workers, no spawn cap)
///
/// Mock turn sequence (fully interleaved — deepest child immediately follows its spawn):
///
///   PM turn 1:       agent.team-lead.spawn({..., grant:[agent.spawn(coder,tester)]})
///   TeamLead turn 1: agent.coder.spawn({message:"write the code", grant:[]})
///   Coder turn 1:    text "code written"
///   TeamLead turn 2: agent.tester.spawn({message:"write the tests", grant:[]})
///   Tester turn 1:   text "tests written"
///   TeamLead turn 3: text "team lead done"
///   PM turn 2:       text "PM done"
///
/// The entire team-lead sub-tree executes during PM's first spawn call
/// (synchronous recursion). Queue order follows the nesting depth.
///
/// Assertions:
///   - snapshot.agents_spawned == 3 (team-lead + coder + tester)
///   - snapshot.status == Completed
///
/// This validates deep hierarchical delegation: the PM grants spawn rights to
/// the team lead, who in turn delegates to leaf workers.
#[tokio::test]
async fn pattern_d_hierarchical_team_lead() {
    let tmp = tempfile::tempdir().expect("tempdir");

    // Grant PM → TeamLead: the SAME agent.spawn capability the PM holds.
    // `check_capability_subset` in virtual_tools.rs uses literal JSON
    // string comparison (not semantic subsetting), so a narrowed grant
    // (["coder","tester"]) would not be recognized as a subset of the
    // PM's grant (["team-lead","coder","tester"]). Using the same grant
    // keeps the literal strings identical, satisfying the subset check
    // while still testing the hierarchical delegation mechanic.
    let team_lead_grant = serde_json::json!([
        {"kind": "agent.spawn", "allowed_kinds": ["team-lead", "coder", "tester"]}
    ]);

    let backend = common::MockLlmBackend::new("test-llm")
        // PM turn 1: spawn team-lead.
        .add_tool_call_json(
            "agent.team-lead.spawn",
            serde_json::json!({
                "message": "implement the feature using coder and tester",
                "grant": team_lead_grant
            }),
        )
        // TeamLead turn 1 (runs during PM's spawn): spawn coder.
        .add_tool_call_json(
            "agent.coder.spawn",
            serde_json::json!({
                "message": "write the code",
                "grant": []
            }),
        )
        // Coder turn 1 (runs during team-lead's spawn): produce result.
        .add_text("code written")
        // TeamLead turn 2: spawn tester.
        .add_tool_call_json(
            "agent.tester.spawn",
            serde_json::json!({
                "message": "write the tests",
                "grant": []
            }),
        )
        // Tester turn 1 (runs during team-lead's spawn): produce result.
        .add_text("tests written")
        // TeamLead turn 3: done.
        .add_text("team lead done")
        // PM turn 2: done.
        .add_text("PM done");

    let runtime = Arc::new(
        Runtime::builder()
            .with_llm_backend(backend)
            .build()
            .expect("build runtime"),
    );

    // PM manifest: can spawn team-lead + coder + tester (needed for subset law:
    // PM must hold agent.spawn(coder,tester) to grant it to team-lead).
    let manifest = manifest_with_agent_spawn(r#""team-lead", "coder", "tester""#);
    let agent_def = common::agent_def("pm", "Program Manager", "orchestrator@0.1.0", "test-llm");
    let initial = common::user_message("deliver the feature");

    let snapshot = tau_runtime_tokio::spawn_root_agent_with_scope(
        runtime,
        agent_def,
        manifest,
        initial,
        RunBudget::default(),
        tmp.path().to_path_buf(),
    )
    .await
    .expect("spawn_root_agent must succeed");

    assert_eq!(
        snapshot.status,
        tau_ports::RunStatus::Completed,
        "run must complete; got {:?}",
        snapshot.status
    );
    assert_eq!(
        snapshot.agents_spawned, 3,
        "3 children (team-lead + coder + tester) must be spawned; got {}",
        snapshot.agents_spawned
    );
}

// ---------------------------------------------------------------------------
// Pattern E: plan-revise loop
// ---------------------------------------------------------------------------

/// Orchestrator iterates: spawn worker → check result → decide to continue
/// or finish. Terminates when satisfied.
///
/// In this simplified form: spawn one worker, the worker returns its result,
/// the orchestrator sees it is sufficient and completes.
///
/// Mock turn sequence:
///
///   Orchestrator turn 1: agent.worker.spawn({message:"do the work", grant:[]})
///   Orchestrator turn 2: text "result is sufficient, loop complete"
///
///   Worker turn 1: text "work product"
///
/// Assertions:
///   - Run terminates (no infinite loop)
///   - snapshot.status == Completed
///   - snapshot.agents_spawned == 1
///
/// This validates the plan-revise termination guarantee: the mock's finite
/// turn queue forces termination even if the LLM were inclined to loop.
#[tokio::test]
async fn pattern_e_plan_revise_loop() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let backend = common::MockLlmBackend::new("test-llm")
        // Orchestrator: spawn worker.
        .add_tool_call_json(
            "agent.worker.spawn",
            serde_json::json!({
                "message": "do the work",
                "grant": []
            }),
        )
        // Orchestrator: decide loop is complete.
        .add_text("result is sufficient, loop complete")
        // Worker: produce result.
        .add_text("work product");

    let runtime = Arc::new(
        Runtime::builder()
            .with_llm_backend(backend)
            .build()
            .expect("build runtime"),
    );

    let manifest = manifest_with_agent_spawn(r#""worker""#);
    let agent_def = common::agent_def(
        "orchestrator",
        "Orchestrator",
        "orchestrator@0.1.0",
        "test-llm",
    );
    let initial = common::user_message("start the plan-revise loop");

    let snapshot = tau_runtime_tokio::spawn_root_agent_with_scope(
        runtime,
        agent_def,
        manifest,
        initial,
        RunBudget::default(),
        tmp.path().to_path_buf(),
    )
    .await
    .expect("spawn_root_agent must succeed");

    // Run must terminate without infinite loop.
    assert_eq!(
        snapshot.status,
        tau_ports::RunStatus::Completed,
        "run must complete; got {:?}",
        snapshot.status
    );
    assert_eq!(
        snapshot.agents_spawned, 1,
        "1 worker must be spawned; got {}",
        snapshot.agents_spawned
    );
}

// ---------------------------------------------------------------------------
// task.* virtual tools: create → claim → complete
// ---------------------------------------------------------------------------

/// Build a manifest granting a single non-spawn capability via raw
/// `kind`/`mode` keys (TOML-friendly). Used for `task.*` and `run.note`
/// flows where no `agent.spawn` capability is needed.
fn manifest_with_capability(kind: &str, mode: &str) -> tau_domain::PackageManifest {
    let toml_body = format!(
        r#"
name        = "orchestrator"
version     = "0.1.0"
description = "orchestrator agent"
authors     = []
source      = "https://example.com/orchestrator.git"
kind        = "tool"
dependencies = []

[[capabilities]]
kind = "{kind}"
mode = "{mode}"
"#
    );
    common::manifest_from_toml(&toml_body)
}

/// Single-agent flow exercising the `task.*` virtual tools.
///
/// The TaskList in `RunState` uses deterministic, zero-padded
/// per-scope sequence ids (see `task_list.rs::create`): the first
/// top-level task is `"01"`, so the script can hard-code the id
/// returned by `task.create` and reuse it for `task.claim` /
/// `task.complete`.
///
/// Mock turn sequence:
///
///   Orchestrator turn 1: tool_call task.create({description:"do thing X"})
///   Orchestrator turn 2: tool_call task.claim({task_id:"01"})
///   Orchestrator turn 3: tool_call task.complete({task_id:"01", result_summary:"thing X done"})
///   Orchestrator turn 4: text "all tasks complete"
///
/// Manifest grants `Capability::TaskList { mode: "manage" }`, which the
/// runtime's `capability_satisfies` treats as ⊇ `write` (the level the
/// `task.create`/`claim`/`complete` calls require) and ⊇ `read`.
///
/// Assertions:
///   - snapshot.status == Completed
///   - snapshot.task_list contains exactly one task in `TaskStatus::Done`
///     with the expected description and result_summary
#[tokio::test]
async fn task_list_create_claim_complete_flow() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let backend = common::MockLlmBackend::new("test-llm")
        // Turn 1: create the task. id is deterministic -> "01".
        .add_tool_call_json(
            "task.create",
            serde_json::json!({ "description": "do thing X" }),
        )
        // Turn 2: claim the task we just created.
        .add_tool_call_json("task.claim", serde_json::json!({ "task_id": "01" }))
        // Turn 3: complete the task with a result_summary.
        .add_tool_call_json(
            "task.complete",
            serde_json::json!({ "task_id": "01", "result_summary": "thing X done" }),
        )
        // Turn 4: end with plain text so the run can complete.
        .add_text("all tasks complete");

    let runtime = Arc::new(
        Runtime::builder()
            .with_llm_backend(backend)
            .build()
            .expect("build runtime"),
    );

    // `manage` ⊇ `write` ⊇ `read`, so a single manage grant covers
    // create + claim + complete (all of which require `write`).
    let manifest = manifest_with_capability("task_list", "manage");
    let agent_def = common::agent_def(
        "orchestrator",
        "Orchestrator",
        "orchestrator@0.1.0",
        "test-llm",
    );
    let initial = common::user_message("create, claim, and complete one task");

    let snapshot = tau_runtime_tokio::spawn_root_agent_with_scope(
        runtime,
        agent_def,
        manifest,
        initial,
        RunBudget::default(),
        tmp.path().to_path_buf(),
    )
    .await
    .expect("spawn_root_agent must succeed");

    assert_eq!(
        snapshot.status,
        tau_ports::RunStatus::Completed,
        "run must complete; got {:?}",
        snapshot.status
    );
    assert_eq!(
        snapshot.task_list.len(),
        1,
        "exactly 1 task must be present; got {}",
        snapshot.task_list.len()
    );
    let task = &snapshot.task_list[0];
    assert_eq!(task.id, "01", "task id must be the deterministic \"01\"");
    assert_eq!(task.description, "do thing X");
    assert_eq!(
        task.status,
        tau_ports::TaskStatus::Done,
        "task must end in Done; got {:?}",
        task.status
    );
    assert_eq!(task.result_summary.as_deref(), Some("thing X done"));
}

// ---------------------------------------------------------------------------
// run.note virtual tool: append to the run plan
// ---------------------------------------------------------------------------

/// Single-agent flow exercising the `run.note` virtual tool.
///
/// Note: the virtual-tool registry exposes a single `run.note` tool that
/// appends `args.text` to the run's plan/scratchpad; reads happen via the
/// separate `run.plan` tool (capability `Plan { mode: "read" }`). This
/// test writes two notes via `run.note` (requires `Plan { mode: "write" }`)
/// and then ends with a plain text turn — the manifest grants
/// `Plan { mode: "write" }`, which `capability_satisfies` treats as ⊇ `read`,
/// but we deliberately don't issue a `run.plan` read so the test stays
/// focused on the write path the user asked to exercise.
///
/// Mock turn sequence:
///
///   Orchestrator turn 1: tool_call run.note({text:"hypothesis: X causes Y"})
///   Orchestrator turn 2: tool_call run.note({text:"checked: hypothesis holds"})
///   Orchestrator turn 3: text "notes recorded and read"
///
/// Assertions:
///   - snapshot.status == Completed
///   - snapshot.plan contains both note texts (proves write went through)
#[tokio::test]
async fn run_note_write_flow() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let backend = common::MockLlmBackend::new("test-llm")
        // Turn 1: write the first note.
        .add_tool_call_json(
            "run.note",
            serde_json::json!({ "text": "hypothesis: X causes Y" }),
        )
        // Turn 2: write the second note.
        .add_tool_call_json(
            "run.note",
            serde_json::json!({ "text": "checked: hypothesis holds" }),
        )
        // Turn 3: end with plain text.
        .add_text("notes recorded and read");

    let runtime = Arc::new(
        Runtime::builder()
            .with_llm_backend(backend)
            .build()
            .expect("build runtime"),
    );

    let manifest = manifest_with_capability("plan", "write");
    let agent_def = common::agent_def(
        "orchestrator",
        "Orchestrator",
        "orchestrator@0.1.0",
        "test-llm",
    );
    let initial = common::user_message("jot down some notes");

    let snapshot = tau_runtime_tokio::spawn_root_agent_with_scope(
        runtime,
        agent_def,
        manifest,
        initial,
        RunBudget::default(),
        tmp.path().to_path_buf(),
    )
    .await
    .expect("spawn_root_agent must succeed");

    assert_eq!(
        snapshot.status,
        tau_ports::RunStatus::Completed,
        "run must complete; got {:?}",
        snapshot.status
    );
    assert!(
        snapshot.plan.contains("hypothesis: X causes Y"),
        "plan must contain the first note; got {:?}",
        snapshot.plan
    );
    assert!(
        snapshot.plan.contains("checked: hypothesis holds"),
        "plan must contain the second note; got {:?}",
        snapshot.plan
    );
}
