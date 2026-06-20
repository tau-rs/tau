//! Integration test for `tau dev` driving a `[[pipeline.steps]]` pipeline.
//!
//! When the dev project lowers to an `IrModule` whose `workflow.pipeline`
//! is `Some`, the dev REPL's one-shot turn (`tau dev -p`) sequences the
//! whole pipeline via `tau_runtime_core::interpreter::pipeline::run_pipeline`
//! and renders the LAST step's output — mirroring the `tau run` pipeline
//! path. When no pipeline is declared, the single-entry agent path runs
//! unchanged (covered by the other `cmd_dev_*` tests).
//!
//! Harness: reuses the pre-built `echo-llm` scripted-LLM plugin. A single
//! `echo-llm` backend instance (built from the ENTRY agent `gather`) is
//! shared by every pipeline agent step, and its `script` config is indexed
//! by an internal turn counter — so the FIRST agent step (`gather`) emits
//! `script[0]` and the SECOND (`writer`) emits `script[1]`.
//!
//! The assertion keys on the second step's distinct output
//! (`STEP-TWO-FINAL`): the single-agent fallback path would only ever make
//! the first backend call (`gather` → `script[0]` = `STEP-ONE`) and could
//! NOT produce `script[1]`. Seeing `STEP-TWO-FINAL` on stdout therefore
//! proves (a) the pipeline branch was taken, (b) the engine sequenced both
//! steps, (c) the LAST step's output was rendered.

use assert_cmd::Command as AssertCmd;

mod common;

/// Author a 2-agent project + lockfile pointing at the pre-built echo-llm
/// binary, with a 2-step pipeline. Mirrors `run_pipeline.rs`'s fixture but
/// configures the entry agent with a multi-turn `script` so the two steps
/// emit *different* outputs.
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

    // Project tau.toml: two agents + a 2-step pipeline. The ENTRY agent
    // (`gather`, alphabetically first → the dev default) supplies the shared
    // echo-llm `script`. Each agent step does exactly one LLM call
    // (`max_turns = 1`), so `gather` consumes `script[0]` and `writer`
    // consumes `script[1]`. The second step threads `gather`'s output via
    // the template (echo ignores it, but the engine substitutes it).
    let project_toml = r#"[project]
name = "dev-pipeline-demo"

[models]
default = { backend = "echo-llm", model = "claude-haiku-4-5" }

[agents.gather]
display_name = "Gatherer"
package      = "echo-llm@^0.1"
model        = "default"
max_turns    = 1

[agents.gather.config]
script = ["STEP-ONE", "STEP-TWO-FINAL"]

[agents.writer]
display_name = "Writer"
package      = "echo-llm@^0.1"
model        = "default"
max_turns    = 1

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

/// Per-test `$TAU_HOME` pointing at a fresh tempdir. `Scope::resolve`
/// auto-materializes the default `config.toml` inside it; a unique dir per
/// test avoids the Windows parallel-write race on the global config.
fn fresh_tau_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("tempdir for TAU_HOME");
    std::fs::create_dir_all(home.path()).unwrap();
    home
}

#[test]
fn dev_drives_pipeline_and_renders_final_step_output() {
    let dir = setup_pipeline_project();
    let home = fresh_tau_home();

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        // `-p` routes through one-shot mode → `DevSession::run_turn`, which
        // takes the pipeline branch because the project lowered to a module
        // with `workflow.pipeline = Some`.
        .args(["dev", ".", "-p", "seed input"])
        .current_dir(dir.path())
        .env("TAU_HOME", home.path())
        // Force the mock sandbox so the toy plugin is spawned natively
        // (mirrors run_pipeline.rs): on a host where `docker --version`
        // probes Available, the Container adapter would otherwise win and
        // break the stdin handshake.
        .env("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1")
        .timeout(std::time::Duration::from_secs(30))
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "expected dev pipeline run to succeed; stderr={}\nstdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    // Only the SECOND step (`writer`) yields `script[1]`. The single-agent
    // fallback path makes a single backend call and would render `STEP-ONE`,
    // never `STEP-TWO-FINAL`.
    assert!(
        stdout.contains("STEP-TWO-FINAL"),
        "stdout should carry the pipeline's final-step output (script[1]); got: {stdout}"
    );
}

#[test]
fn dev_pipeline_json_emits_completed_outcome() {
    let dir = setup_pipeline_project();
    let home = fresh_tau_home();

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["--json", "dev", ".", "-p", "seed input"])
        .current_dir(dir.path())
        .env("TAU_HOME", home.path())
        .env("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1")
        .timeout(std::time::Duration::from_secs(30))
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "expected dev pipeline run to succeed; stderr={}\nstdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    // `--json` mode emits one JSON object per line. Find the line carrying
    // the completed pipeline outcome and assert it rendered the final step.
    let outcome_line = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|v| v.get("outcome").is_some())
        .unwrap_or_else(|| panic!("--json should include an outcome JSON line; stdout: {stdout}"));
    assert_eq!(outcome_line["outcome"], "completed");
    assert_eq!(outcome_line["final_message"], "STEP-TWO-FINAL");
}
