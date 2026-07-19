//! Integration test for `tau run` driving a `[[pipeline.steps]]` pipeline.
//!
//! Plan 1 Task 11: when the project lowers to an `IrModule` whose
//! `workflow.pipeline` is `Some`, `tau run` sequences the whole pipeline
//! via `tau_runtime_core::interpreter::pipeline::run_pipeline` and renders
//! the LAST step's output as the run result — instead of running a single
//! entry agent.
//!
//! Harness: reuses the `echo-llm` scripted-LLM plugin from
//! [`common::setup_echo_project`]'s building blocks, but authors a project
//! with TWO agents (`gather`, `writer`) plus a 2-step pipeline whose second
//! step threads the first step's output via `${steps.gather.output}`.
//!
//! echo-llm ignores its prompt and replays a fixed `canned_text`, and the
//! cwd `tau run` path spawns ONE backend instance (configured from the
//! ENTRY agent's `[agents.<id>.config]`), shared by every pipeline agent
//! step. So both agent steps emit the entry agent's canned text, and the
//! pipeline's final-step output is that canned text. The assertion is that
//! the pipeline path executed end-to-end (exit 0) and rendered the final
//! step's output — which the single-agent path could not have produced for
//! a 2-step pipeline.

mod common;

use assert_cmd::Command as AssertCmd;

/// Author a 2-agent project + lockfile pointing at the pre-built echo-llm
/// binary, with a 2-step pipeline. Mirrors `common::setup_echo_project`'s
/// lockfile/manifest authoring but writes a custom tau.toml (the shared
/// helper only emits a single agent).
fn setup_pipeline_project() -> tempfile::TempDir {
    let (echo_llm, _echo_tool) = common::echo_plugins::ensure_echo_plugins_built();
    let dir = tempfile::tempdir().expect("tempdir for pipeline project");
    let root = dir.path();

    std::fs::create_dir_all(root.join(".tau")).unwrap();
    // Scope config.toml with required_tier = "none" (no strict/light
    // isolation needed for the toy plugin).
    std::fs::write(
        root.join(".tau").join("config.toml"),
        r#"schema_version = 2
kind = "project"
created_at = "2026-05-01T00:00:00Z"
created_by_tau_version = "0.0.0"

[sandbox]
required_tier = "none"
"#,
    )
    .unwrap();

    // Per-package manifest for echo-llm (read by build_agent_definition).
    let pkg_dir = root
        .join(".tau")
        .join("packages")
        .join("echo-llm")
        .join("0.1.0");
    std::fs::create_dir_all(&pkg_dir).unwrap();
    std::fs::write(
        pkg_dir.join("tau.toml"),
        r#"name = "echo-llm"
version = "0.1.0"
description = "echo plugin fixture"
authors = ["tester <test@example.com>"]
source = "https://example.com/echo-llm.git"
kind = "llm-backend"
dependencies = []
capabilities = []
"#,
    )
    .unwrap();

    // Lockfile recording the echo-llm plugin binary path.
    let now = "2026-04-28T00:00:00Z";
    let zero_sha = "0".repeat(40);
    let llm_path = echo_llm
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    let lockfile = format!(
        r#"schema_version = 4
generated_by_tau_version = "0.0.0"
generated_at = "{now}"

[[package]]
name = "echo-llm"
active_version = "0.1.0"
source = "https://example.com/echo-llm.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "{zero_sha}"
sha256 = ""
installed_at = "{now}"

[package.plugin]
binary_path = "{llm_path}"
built_at = "{now}"

[package.plugin.manifest]
provides = "llm_backend"
kind = "rust-cargo"
bin = "echo-llm"
"#
    );
    std::fs::write(root.join("tau-lock.toml"), lockfile).unwrap();

    // Project tau.toml: two agents + a 2-step pipeline. The entry agent
    // (`gather`) supplies the shared echo-llm `canned_text`; the second
    // step threads `gather`'s output into `writer` via the template (echo
    // ignores it, but the template is exercised at the engine layer).
    let project_toml = r#"[project]
name = "pipeline-demo"

[models]
default = { backend = "echo-llm", model = "claude-haiku-4-5" }

[agents.gather]
display_name = "Gatherer"
package      = "echo-llm@^0.1"
model        = "default"

[agents.gather.config]
canned_text = "PIPELINE-FINAL-OUTPUT"

[agents.writer]
display_name = "Writer"
package      = "echo-llm@^0.1"
model        = "default"

[agents.writer.config]
canned_text = "writer-canned"

[pipeline]

[[pipeline.steps]]
id = "gather"
run = "agent:gather"
input = "${input}"

[[pipeline.steps]]
id = "writer"
run = "agent:writer"
input = "${steps.gather.output}"
"#;
    std::fs::write(root.join("tau.toml"), project_toml).unwrap();

    dir
}

/// Same project layout as `setup_pipeline_project` but with a passing goal
/// appended to the tau.toml. The goal evaluates the `writer` step's output
/// with the `non_empty` predicate — which always passes because echo-llm
/// emits a non-empty canned text. The goal is lowered to a `StepRun::Check`
/// step appended after `writer`, making the pipeline look like:
///   [gather(agent), writer(agent), report_present(check)]
///
/// Before the fix, `try_run_pipeline` selected `report_present` (the last
/// step) as `last_step_id`, hit `store.get("report_present") → None`, and
/// returned an error. After the fix it skips the trailing check step and
/// renders `writer`'s output.
fn setup_pipeline_project_with_goal() -> tempfile::TempDir {
    let (echo_llm, _echo_tool) = common::echo_plugins::ensure_echo_plugins_built();
    let dir = tempfile::tempdir().expect("tempdir for pipeline+goal project");
    let root = dir.path();

    std::fs::create_dir_all(root.join(".tau")).unwrap();
    std::fs::write(
        root.join(".tau").join("config.toml"),
        r#"schema_version = 2
kind = "project"
created_at = "2026-05-01T00:00:00Z"
created_by_tau_version = "0.0.0"

[sandbox]
required_tier = "none"
"#,
    )
    .unwrap();

    let pkg_dir = root
        .join(".tau")
        .join("packages")
        .join("echo-llm")
        .join("0.1.0");
    std::fs::create_dir_all(&pkg_dir).unwrap();
    std::fs::write(
        pkg_dir.join("tau.toml"),
        r#"name = "echo-llm"
version = "0.1.0"
description = "echo plugin fixture"
authors = ["tester <test@example.com>"]
source = "https://example.com/echo-llm.git"
kind = "llm-backend"
dependencies = []
capabilities = []
"#,
    )
    .unwrap();

    let now = "2026-04-28T00:00:00Z";
    let zero_sha = "0".repeat(40);
    let llm_path = echo_llm
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    let lockfile = format!(
        r#"schema_version = 4
generated_by_tau_version = "0.0.0"
generated_at = "{now}"

[[package]]
name = "echo-llm"
active_version = "0.1.0"
source = "https://example.com/echo-llm.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "{zero_sha}"
sha256 = ""
installed_at = "{now}"

[package.plugin]
binary_path = "{llm_path}"
built_at = "{now}"

[package.plugin.manifest]
provides = "llm_backend"
kind = "rust-cargo"
bin = "echo-llm"
"#
    );
    std::fs::write(root.join("tau-lock.toml"), lockfile).unwrap();

    // Same two-step pipeline as setup_pipeline_project PLUS a [goals.*]
    // block. The goal is auto-appended by the lowerer as a StepRun::Check
    // step at the pipeline tail: [gather, writer, report_present(check)].
    // This is the shape that triggered the bug: last() returned
    // "report_present", store.get("report_present") → None → error.
    let project_toml = r#"[project]
name = "pipeline-goal-demo"

[models]
default = { backend = "echo-llm", model = "claude-haiku-4-5" }

[agents.gather]
display_name = "Gatherer"
package      = "echo-llm@^0.1"
model        = "default"

[agents.gather.config]
canned_text = "PIPELINE-FINAL-OUTPUT"

[agents.writer]
display_name = "Writer"
package      = "echo-llm@^0.1"
model        = "default"

[agents.writer.config]
canned_text = "PIPELINE-FINAL-OUTPUT"

[pipeline]

[[pipeline.steps]]
id = "gather"
run = "agent:gather"
input = "${input}"

[[pipeline.steps]]
id = "writer"
run = "agent:writer"
input = "${steps.gather.output}"

# Goal: check that the writer step produced non-empty output.
# Lowered to a StepRun::Check step appended AFTER `writer`.
# This is the step that used to be incorrectly selected as last_step_id.
[goals.report_present]
evaluates = "steps.writer.output"
check     = "non_empty"
"#;
    std::fs::write(root.join("tau.toml"), project_toml).unwrap();

    dir
}

/// Regression test for the check-tail bug: when a project declares a goal,
/// the lowerer appends a StepRun::Check step at the pipeline tail. Before
/// the fix, try_run_pipeline selected that check step as last_step_id,
/// called store.get("report_present") → None, and returned:
///   "pipeline completed but the final step recorded no output"
///
/// After the fix, the last NON-check step (writer) is selected, and the
/// run succeeds, rendering the writer's output.
#[test]
fn run_pipeline_with_trailing_goal_check_succeeds_and_renders_writer_output() {
    let dir = setup_pipeline_project_with_goal();

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["run", "gather", "seed input", "--no-governance"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .env("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "pipeline with trailing goal-check must succeed (exit 0); \
         stderr={}\nstdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("PIPELINE-FINAL-OUTPUT"),
        "stdout must contain the writer step's output (not a check-no-output error); \
         got: {stdout}"
    );
}

#[test]
fn run_drives_pipeline_and_renders_final_step_output() {
    let dir = setup_pipeline_project();

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        // The entry agent id is the first pipeline step (`gather`); the
        // cwd path spawns its echo-llm backend, shared by both steps.
        .args(["run", "gather", "seed input", "--no-governance"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        // Force the mock sandbox so the toy plugin is spawned natively
        // (mirrors `common::setup_echo_project`'s process-env knob): on a
        // host where `docker --version` probes Available, the Container
        // adapter would otherwise win and break the stdin handshake.
        .env("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "expected pipeline run to succeed; stderr={}\nstdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    // The final step (`writer`) reuses the entry agent's single echo-llm
    // backend, so its output is the entry agent's canned text. Seeing it
    // on stdout proves: (a) the pipeline branch was taken, (b) the engine
    // sequenced both steps, (c) the LAST step's output was rendered.
    assert!(
        stdout.contains("PIPELINE-FINAL-OUTPUT"),
        "stdout should carry the pipeline's final-step output; got: {stdout}"
    );
}

#[test]
fn run_pipeline_json_emits_completed_outcome() {
    let dir = setup_pipeline_project();

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["run", "gather", "seed input", "--json", "--no-governance"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .env("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "expected pipeline run to succeed; stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    // --json mode emits one JSON object per line (resolve events + the
    // pipeline outcome). Find the line carrying the completed outcome.
    let outcome_line = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|v| v.get("outcome").is_some())
        .unwrap_or_else(|| panic!("--json should include an outcome JSON line; stdout: {stdout}"));
    assert_eq!(outcome_line["outcome"], "completed");
    assert_eq!(outcome_line["final_message"], "PIPELINE-FINAL-OUTPUT");
}
