//! Integration test for `tau run` pausing at a top-level `Suspend` step and
//! resuming with `--resume <RUN_ID> --signal <NAME>` (EPIC 4.3 Task 6).
//!
//! Harness: reuses the `echo-llm` scripted-LLM plugin fixture the same way
//! `run_pipeline.rs` does (see [`common::echo_plugins::ensure_echo_plugins_built`]),
//! so the whole run is offline — no real LLM backend is contacted. The
//! authored pipeline is:
//!
//!   [[pipeline.steps]] id = "seed"   run = "agent:seed"          (produces output)
//!   [[pipeline.steps]] id = "pause"  run = "suspend:approved"    (produces NO output)
//!   [[pipeline.steps]] id = "finish" run = "agent:seed"          (produces output)
//!
//! `seed` and `finish` both invoke the SAME `seed` agent (one echo-llm
//! backend instance, shared like `run_pipeline.rs`'s `gather`/`writer`
//! steps) so no second plugin needs to be spawned. `pause` is a top-level
//! `Suspend` — the only place a `Suspend` step is allowed (EPIC 4.3
//! typecheck) — so the run pauses there rather than running `finish`.
//!
//! `finish` is deliberately placed AFTER `pause`, not `pause` itself, as
//! the pipeline's last non-check step: a `Suspend` step stores no output
//! (like `Branch`/`Loop`/`Parallel`), so if `pause` were the last step,
//! `render_pipeline_result` would hit `store.get("pause") -> None` and
//! error out on the post-resume `Completed` render.

mod common;

use assert_cmd::Command as AssertCmd;

/// Author a single-agent project (`seed`) plus a lockfile pointing at the
/// pre-built echo-llm binary, whose pipeline pauses at a top-level
/// `Suspend` step and resumes into one more agent step. Mirrors
/// `run_pipeline.rs::setup_pipeline_project`'s lockfile/manifest authoring.
fn setup_suspend_project() -> tempfile::TempDir {
    let (echo_llm, _echo_tool) = common::echo_plugins::ensure_echo_plugins_built();
    let dir = tempfile::tempdir().expect("tempdir for suspend project");
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

    // Project tau.toml: one agent + a 3-step pipeline that pauses at a
    // top-level Suspend step (id "pause", signal "approved") between two
    // agent steps that both reuse the entry agent's echo-llm backend.
    let project_toml = r#"[project]
name = "suspend-demo"

[models]
default = { backend = "echo-llm", model = "claude-haiku-4-5" }

[agents.seed]
display_name = "Seed"
package      = "echo-llm@^0.1"
model        = "default"

[agents.seed.config]
canned_text = "SEED-OUTPUT"

[pipeline]

[[pipeline.steps]]
id = "seed"
run = "agent:seed"
input = "${input}"

[[pipeline.steps]]
id = "pause"
run = "suspend:approved"

[[pipeline.steps]]
id = "finish"
run = "agent:seed"
input = "${steps.seed.output}"
"#;
    std::fs::write(root.join("tau.toml"), project_toml).unwrap();

    dir
}

/// Run `tau run --json` against `dir`'s project, with the given extra args
/// appended after the (agent id, prompt) pair. Returns the full process
/// `Output` so callers can assert on both the status code and stdout.
fn run_tau(dir: &tempfile::TempDir, prompt: &str, extra_args: &[&str]) -> std::process::Output {
    let mut args = vec!["run", "--allow-ungoverned", "seed", prompt, "--json"];
    args.extend_from_slice(extra_args);
    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(args)
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        // Force the mock sandbox so the toy plugin is spawned natively.
        .env("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1")
        .output()
        .unwrap()
}

/// Find the JSON line on stdout carrying the `"outcome"` field (--json mode
/// emits one JSON object per line: resolve events, then the outcome line).
fn outcome_line(stdout: &str) -> serde_json::Value {
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|v| v.get("outcome").is_some())
        .unwrap_or_else(|| panic!("--json should include an outcome JSON line; stdout: {stdout}"))
}

#[test]
fn run_suspends_at_top_level_suspend_step_and_persists_snapshot() {
    let dir = setup_suspend_project();

    let output = run_tau(&dir, "seed input", &[]);

    assert_eq!(
        output.status.code(),
        Some(3),
        "a suspended pipeline run must exit 3 (ExitCode::Suspended); \
         stderr={}\nstdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let line = outcome_line(&stdout);
    assert_eq!(line["outcome"], "suspended");
    assert_eq!(line["resume_signal"], "approved");
    assert_eq!(line["step_id"], "pause");
    let run_id = line["run_id"]
        .as_str()
        .expect("run_id must be a string")
        .to_string();
    assert!(!run_id.is_empty(), "run_id must be non-empty; line: {line}");

    let suspend_json = dir
        .path()
        .join(".tau")
        .join("runs")
        .join(&run_id)
        .join("suspend.json");
    assert!(
        suspend_json.exists(),
        "expected suspend.json at {suspend_json:?} after a suspended run"
    );
}

#[test]
fn run_resume_with_matching_signal_completes_the_pipeline() {
    let dir = setup_suspend_project();

    let suspend_output = run_tau(&dir, "seed input", &[]);
    assert_eq!(suspend_output.status.code(), Some(3));
    let suspend_stdout = String::from_utf8(suspend_output.stdout).unwrap();
    let run_id = outcome_line(&suspend_stdout)["run_id"]
        .as_str()
        .expect("run_id must be a string")
        .to_string();

    let resume_output = run_tau(
        &dir,
        "resume input",
        &["--resume", &run_id, "--signal", "approved"],
    );

    assert!(
        resume_output.status.success(),
        "resuming with the matching signal must exit 0; stderr={}\nstdout={}",
        String::from_utf8_lossy(&resume_output.stderr),
        String::from_utf8_lossy(&resume_output.stdout),
    );

    let resume_stdout = String::from_utf8(resume_output.stdout).unwrap();
    let line = outcome_line(&resume_stdout);
    assert_eq!(line["outcome"], "completed");
    // `finish` re-invokes the same echo-llm backend, which ignores its
    // prompt and always emits the entry agent's canned text.
    assert_eq!(line["final_message"], "SEED-OUTPUT");
}

#[test]
fn run_resume_with_mismatched_signal_fails() {
    let dir = setup_suspend_project();

    let suspend_output = run_tau(&dir, "seed input", &[]);
    assert_eq!(suspend_output.status.code(), Some(3));
    let suspend_stdout = String::from_utf8(suspend_output.stdout).unwrap();
    let run_id = outcome_line(&suspend_stdout)["run_id"]
        .as_str()
        .expect("run_id must be a string")
        .to_string();

    let resume_output = run_tau(
        &dir,
        "resume input",
        &["--resume", &run_id, "--signal", "wrong"],
    );

    assert!(
        !resume_output.status.success(),
        "resuming with a mismatched signal must fail (non-zero exit); stdout={}",
        String::from_utf8_lossy(&resume_output.stdout),
    );
    let stderr = String::from_utf8_lossy(&resume_output.stderr);
    assert!(
        stderr.contains("signal") && stderr.contains("wrong"),
        "stderr should explain the signal mismatch; got: {stderr}"
    );
}
