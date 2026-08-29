//! End-to-end test for tau-workflow.
//!
//! Builds a minimal two-step Workflow, runs it through the real Runner
//! backed by a `MockLlmBackend` fixture (no real subprocess or network),
//! and asserts JSONL persistence round-trips correctly.
//!
//! The `echo-llm` fixture is emulated via `MockLlmBackend` from
//! `tau_ports::fixtures`: it returns a canned "hello" text reply for
//! every completion request, mimicking the behaviour of the echo-llm
//! subprocess plugin without requiring a real binary.
//!
//! Runs on Tier 0 (`just test` → `--workspace --all-targets`): it needs
//! no host shape, so it carries no feature gate. See tau-rs/tau#716.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tau_observe::layers::workflow_run_log::WorkflowRunLogLayer;
use tau_ports::fixtures::{make_completion_response, make_token_usage, MockLlmBackend};
use tau_ports::StopReason;
use tau_workflow::{
    persistence::{replay, run_log_path, StepStatus},
    RunOpts, Runner, Workflow,
};
use tracing::subscriber::DefaultGuard;
use tracing_subscriber::layer::SubscriberExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a minimal `PackageManifest` with the given LLM backend name.
///
/// Mirrors `common::manifest_with_no_capabilities` from tau-runtime/tests but
/// without pulling in that crate's test helpers — we reproduce the minimal
/// toml here to avoid a cross-crate test-helper dependency.
fn manifest_no_caps() -> tau_domain::PackageManifest {
    let raw: tau_domain::UncheckedManifest = toml::from_str(
        r#"
            name        = "test-pkg"
            version     = "0.1.0"
            description = "test package"
            authors     = []
            source      = "https://example.com/test.git"
            kind        = "tool"
            dependencies = []
            capabilities = []
        "#,
    )
    .expect("test manifest TOML must parse");
    raw.validate()
        .expect("test manifest must satisfy validation")
}

/// Build a minimal `AgentDefinition` backed by the given LLM backend name.
fn agent_def(llm_backend_name: &str) -> tau_domain::AgentDefinition {
    use std::str::FromStr;
    tau_domain::AgentDefinition::new(
        tau_domain::AgentId::from_str("echo").expect("valid agent id"),
        "echo".to_string(),
        tau_domain::PackageId::new(
            tau_domain::PackageName::from_str("test-pkg").expect("valid"),
            tau_domain::Version::parse("0.1.0").expect("valid"),
        ),
        tau_domain::PackageName::from_str(llm_backend_name).expect("valid llm backend name"),
    )
}

/// Install a `WorkflowRunLogLayer` pointed at `path` as the
/// thread-local subscriber for the duration of the returned guard.
///
/// `RunLog::append` is a thin `tracing::event!` emitter, so the JSONL is
/// materialized by this layer rather than by a direct write. `tau
/// workflow run` attaches it via `tau_observe::install`; a test that
/// installs no subscriber writes nothing at all and the log file is
/// never created. Mirrors `install_run_log_layer` in
/// `src/persistence.rs`, including `set_default` (not
/// `set_global_default`) so concurrent tests don't fight over one
/// global subscriber.
fn install_run_log_layer(path: &Path) -> DefaultGuard {
    let layer = WorkflowRunLogLayer::new(path.to_path_buf());
    let subscriber = tracing_subscriber::registry().with(layer);
    tracing::subscriber::set_default(subscriber)
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

/// Run a two-step linear workflow end-to-end against a `MockLlmBackend`.
///
/// Step 1: agent "echo" receives "${input}" → returns "hello"
/// Step 2: agent "echo" receives "${steps.first.output}" (= "hello") → returns "hello"
///
/// After the run, replay the JSONL log and assert both records are Ok.
#[tokio::test]
async fn linear_workflow_runs_two_agent_steps_and_persists_jsonl() {
    // 1. Temp scope for the JSONL log.
    let scope = tempfile::tempdir().expect("scope tempdir");

    // 2. Build a Runtime backed by MockLlmBackend.
    //    The mock stores a single canned response and clones it on every
    //    call, so both workflow steps receive "hello" — mirroring the
    //    echo-llm subprocess fixture behaviour.
    let resp = make_completion_response(
        "hello".into(),
        Vec::new(),
        StopReason::EndTurn,
        Some(make_token_usage(5, 5)),
    );
    let llm = MockLlmBackend::new("mock-echo").with_response(resp);

    let runtime = Arc::new(
        tau_runtime_tokio::Runtime::builder()
            .with_llm_backend(llm)
            .build()
            .expect("build runtime"),
    );

    // 3. Build the agents map: "echo" → (AgentDefinition, PackageManifest).
    //    The backend name "mock-echo" matches the MockLlmBackend name above.
    let def = agent_def("mock-echo");
    let manifest = manifest_no_caps();
    let mut agents: BTreeMap<String, (tau_domain::AgentDefinition, tau_domain::PackageManifest)> =
        BTreeMap::new();
    agents.insert("echo".into(), (def, manifest));

    // 4. Parse the inline two-step workflow.
    let wf_src = r#"
[workflow]
description = "echo pipeline"

[[steps]]
id = "first"
kind = "agent.run"
agent = "echo"
input = "${input}"

[[steps]]
id = "second"
kind = "agent.run"
agent = "echo"
input = "${steps.first.output}"
"#;
    let wf = Workflow::from_str(wf_src, &PathBuf::from("workflows/echo.toml"))
        .expect("workflow must parse");

    // 5. Install the run-log layer *before* the run.
    //    The layer needs the destination path up front, but
    //    `outcome.log_path` is only known afterwards — so pin `run_id`
    //    instead of letting the runner generate a ULID, and derive the
    //    same path the runner will compute via `run_log_path`.
    let run_id = "test-run";
    let expected_log_path = run_log_path(scope.path(), &wf.name, run_id);
    let _log_guard = install_run_log_layer(&expected_log_path);

    // 6. Run.
    let runner = Runner::new(runtime, scope.path().to_path_buf());
    let outcome = runner
        .run(
            &wf,
            RunOpts {
                input: "hello".into(),
                run_id: Some(run_id.into()),
                completed: vec![],
                agents,
            },
        )
        .await
        .expect("run must succeed");

    assert!(
        outcome.success,
        "workflow run must succeed; last_output={:?}",
        outcome.last_output
    );

    // The mock echoes "hello" back; both steps' outputs should be "hello".
    assert_eq!(
        outcome.last_output, "hello",
        "last step output should be 'hello' (MockLlmBackend canned response)"
    );

    // 7. The runner must have written to exactly the path the layer was
    //    installed on — otherwise the replay below would be asserting
    //    against a file nothing produced.
    assert_eq!(
        outcome.log_path, expected_log_path,
        "runner log path must match the path derived from run_log_path"
    );

    // 8. Replay the JSONL log and assert 2 records with status Ok.
    let records = replay(&outcome.log_path)
        .await
        .expect("replay must succeed");

    assert_eq!(records.len(), 2, "expected 2 step records in the JSONL log");

    assert_eq!(records[0].status, StepStatus::Ok);
    assert_eq!(records[0].step_id, "first");
    assert_eq!(
        records[0].output, "hello",
        "first step output should be 'hello'"
    );

    assert_eq!(records[1].status, StepStatus::Ok);
    assert_eq!(records[1].step_id, "second");
    assert_eq!(
        records[1].output, "hello",
        "second step output should be 'hello'"
    );

    // 9. Verify log path is inside the scope.
    assert!(
        outcome.log_path.starts_with(scope.path()),
        "log path must be inside the temp scope"
    );
}
