//! Integration test for `tau run --bundle` driving a `[[pipeline.steps]]`
//! pipeline carried in a v2 IR bundle.
//!
//! The bundle run path ([`crate::cmd::ir_dispatcher::run_via_ir`]) used to
//! ignore a bundled pipeline entirely: it ran the alphabetically-first
//! agent once via `run_ir`. This test proves the wiring fix — when the
//! verified bundle's IR carries `workflow.pipeline`, the engine sequences
//! the whole pipeline via `run_pipeline` and renders the LAST step's
//! output.
//!
//! Harness mirrors `run_pipeline.rs` (the cwd-path counterpart) but adds
//! the bits `tau build` needs: a `[project] version`, agent prompts, and a
//! schema-v6 lockfile (a schema-v4 echo fixture cannot feed `tau build`).
//! The same lockfile content is written to BOTH `tau.lock` (read by
//! `tau build`) and `tau-lock.toml` (`scope.lockfile_path()`, read by the
//! run path's plugin loader).
//!
//! Distinguishing the pipeline path from the single-agent path: the
//! single-agent renderer (`render_outcome`) emits `total_turns` +
//! `token_usage` in `--json` mode, whereas the pipeline renderer
//! (`render_pipeline_result`) emits only `outcome` + `final_message`. The
//! test asserts the completed-outcome JSON line has the final-step output
//! AND lacks `total_turns` — a signal only the pipeline branch produces.

#![allow(clippy::needless_raw_string_hashes)]

mod common;

use assert_cmd::Command as AssertCmd;

/// Author a 2-agent project + a buildable, runnable schema-v6 fixture with
/// a 2-step `gather → writer` pipeline. Returns the project tempdir.
fn setup_pipeline_bundle_project() -> tempfile::TempDir {
    let (echo_llm, _echo_tool) = common::echo_plugins::ensure_echo_plugins_built();
    let dir = tempfile::tempdir().expect("tempdir for pipeline-bundle project");
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

    // Per-package manifest for echo-llm. `tau build` enumerates each
    // lockfile package and requires its on-disk tree at
    // `.tau/packages/<name>/<version>/`; the run path reads this manifest
    // during package resolution.
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

    // Schema-v6 lockfile recording the echo-llm plugin binary path. v6 is
    // shape-identical to v4 here (the v6-only `synthesized_from` field is
    // optional and omitted); the bump matters only because `tau build`
    // loads the lockfile and a too-old schema would not carry the fields
    // it migrates. The SAME content is written to both filenames below.
    let now = "2026-04-28T00:00:00Z";
    let zero_sha = "0".repeat(40);
    let llm_path = echo_llm
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    let lockfile = format!(
        r#"schema_version = 6
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
    // `tau build` reads `tau.lock`; `scope.lockfile_path()` (plugin loader,
    // package resolution) reads `tau-lock.toml`. Keep both consistent.
    std::fs::write(root.join("tau.lock"), &lockfile).unwrap();
    std::fs::write(root.join("tau-lock.toml"), &lockfile).unwrap();

    // Project tau.toml: a [project] header with version (tau build needs
    // it), two agents, and a 2-step pipeline whose second step threads the
    // first step's output via `${steps.gather.output}`. The entry agent
    // (`gather`, alphabetically first) supplies the shared echo-llm
    // `canned_text`; echo ignores the threaded input but the template is
    // exercised at the engine layer.
    let project_toml = r#"[project]
name = "pipeline-bundle-demo"
version = "0.1.0"

[models]
default = { backend = "echo-llm", model = "claude-haiku-4-5" }

[agents.gather]
display_name = "Gatherer"
package      = "echo-llm@^0.1"
model        = "default"

[agents.gather.prompt]
system = "gather"

[agents.gather.config]
canned_text = "PIPELINE-FINAL-OUTPUT"

[agents.writer]
display_name = "Writer"
package      = "echo-llm@^0.1"
model        = "default"

[agents.writer.prompt]
system = "write"

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

/// Invoke the real `tau build` binary in `project`, returning the bundle
/// path it prints to stdout.
fn build_bundle(project: &std::path::Path, tau_home: &std::path::Path) -> std::path::PathBuf {
    let out = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["build"])
        .current_dir(project)
        .env("TAU_HOME", tau_home)
        .assert()
        .success()
        .get_output()
        .clone();
    let path = String::from_utf8(out.stdout).unwrap().trim().to_string();
    std::path::PathBuf::from(path)
}

#[test]
fn run_bundle_drives_pipeline_and_renders_final_step_output() {
    let dir = setup_pipeline_bundle_project();
    let tau_home = dir.path().join("global");
    std::fs::create_dir_all(&tau_home).unwrap();

    let bundle = build_bundle(dir.path(), &tau_home);

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        // Entry agent id is the first pipeline step (`gather`); the bundle
        // run path spawns its echo-llm backend, shared by both steps.
        .args([
            "run",
            "--bundle",
            bundle.to_str().unwrap(),
            "gather",
            "seed input",
            "--json",
        ])
        .current_dir(dir.path())
        .env("TAU_HOME", &tau_home)
        // Force the mock sandbox so the toy plugin spawns natively (see
        // run_pipeline.rs / setup_echo_project for the rationale).
        .env("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "expected bundle pipeline run to succeed; stderr={}\nstdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let outcome_line = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|v| v.get("outcome").is_some())
        .unwrap_or_else(|| panic!("--json should include an outcome JSON line; stdout: {stdout}"));

    // The final step (`writer`) reuses the entry agent's single echo-llm
    // backend, so its rendered output is the entry agent's canned text.
    assert_eq!(outcome_line["outcome"], "completed");
    assert_eq!(
        outcome_line["final_message"], "PIPELINE-FINAL-OUTPUT",
        "final_message should be the LAST pipeline step's output; outcome line: {outcome_line}"
    );

    // The pipeline renderer (`render_pipeline_result`) emits ONLY
    // `outcome` + `final_message`. The single-agent `render_outcome` would
    // also carry `total_turns` + `token_usage`. Its absence proves the
    // bundle run took the pipeline branch, not the single-agent `run_ir`
    // path.
    assert!(
        outcome_line.get("total_turns").is_none(),
        "pipeline outcome must NOT carry total_turns (that is the \
         single-agent renderer's shape); outcome line: {outcome_line}"
    );
}
