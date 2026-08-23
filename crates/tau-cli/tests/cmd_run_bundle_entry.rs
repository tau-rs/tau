//! Integration tests for `tau run --bundle` entry-agent resolution (#623).
//!
//! The bundle run path ([`crate::cmd::ir_dispatcher::run_via_ir`]) used to
//! silently ignore the positional `<agent>` argument and pick the
//! alphabetically-first agent in the IR module as the entry (the β.2 v0
//! choice). Consequence: the plugin backend was configured from the wrong
//! agent's `[agents.<id>.config]`. These tests pin the fix:
//!
//! - the positional agent id IS the entry, even when it does not sort
//!   first among the module's agents (the echo-llm backend's canned text
//!   comes from the REQUESTED agent's config, proving which agent
//!   configured the plugin), and
//! - an agent id not present in the bundle's IR module is a hard error
//!   naming the available agents, not a silent fallback.
//!
//! Harness mirrors `cmd_run_bundle_pipeline.rs` (echo-llm scaffold,
//! schema-v6 lockfile written to BOTH `tau.lock` and `tau-lock.toml`) but
//! authors NO `[pipeline]`, so the run exercises the single-entry-agent
//! `run_ir` branch.

#![allow(clippy::needless_raw_string_hashes)]

mod common;

use assert_cmd::Command as AssertCmd;

/// Author a 2-agent, pipeline-less project whose agents carry DIFFERENT
/// echo-llm `canned_text` configs. `alpha` sorts first; `zulu` is the
/// agent the tests request. Returns the project tempdir.
fn setup_two_agent_bundle_project() -> tempfile::TempDir {
    let (echo_llm, _echo_tool) = common::echo_plugins::ensure_echo_plugins_built();
    let dir = tempfile::tempdir().expect("tempdir for entry-agent bundle project");
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

    // Per-package manifest for echo-llm (`tau build` requires the on-disk
    // tree of every lockfile package; the run path reads this manifest
    // during package resolution).
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

    // Schema-v6 lockfile recording the echo-llm plugin binary path. The
    // SAME content goes to both filenames: `tau build` reads `tau.lock`;
    // `scope.lockfile_path()` (plugin loader) reads `tau-lock.toml`.
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
    std::fs::write(root.join("tau.lock"), &lockfile).unwrap();
    std::fs::write(root.join("tau-lock.toml"), &lockfile).unwrap();

    // Two agents, NO pipeline. The canned texts differ so the run's final
    // message proves which agent's config reached the plugin backend.
    let project_toml = r#"[project]
name = "entry-agent-demo"
version = "0.1.0"

[models]
default = { backend = "echo-llm", model = "claude-haiku-4-5" }

[agents.alpha]
display_name = "Alpha"
package      = "echo-llm@^0.1"
model        = "default"

[agents.alpha.prompt]
system = "alpha"

[agents.alpha.config]
canned_text = "ALPHA-CANNED"

[agents.zulu]
display_name = "Zulu"
package      = "echo-llm@^0.1"
model        = "default"

[agents.zulu.prompt]
system = "zulu"

[agents.zulu.config]
canned_text = "ZULU-CANNED"
"#;
    std::fs::write(root.join("tau.toml"), project_toml).unwrap();

    dir
}

/// Invoke the real `tau build` binary in `project`, returning the bundle
/// path it prints to stdout.
fn build_bundle(project: &std::path::Path, tau_home: &std::path::Path) -> std::path::PathBuf {
    let out = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["build", "--allow-ungoverned"])
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
fn run_bundle_entry_is_the_positional_agent_not_first_sorted() {
    let dir = setup_two_agent_bundle_project();
    let tau_home = dir.path().join("global");
    std::fs::create_dir_all(&tau_home).unwrap();

    let bundle = build_bundle(dir.path(), &tau_home);

    // Request `zulu` — the agent that does NOT sort first. Before #623 the
    // bundle path ran `alpha` (first BTreeMap key) and rendered
    // "ALPHA-CANNED" regardless of this argument.
    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args([
            "run",
            "--allow-ungoverned",
            "--bundle",
            bundle.to_str().unwrap(),
            "zulu",
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
        "expected bundle run to succeed; stderr={}\nstdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let outcome_line = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|v| v.get("outcome").is_some())
        .unwrap_or_else(|| panic!("--json should include an outcome JSON line; stdout: {stdout}"));

    assert_eq!(outcome_line["outcome"], "completed");
    assert_eq!(
        outcome_line["final_message"], "ZULU-CANNED",
        "the REQUESTED agent's config must reach the plugin backend; \
         outcome line: {outcome_line}"
    );
}

#[test]
fn run_bundle_unknown_agent_is_a_hard_error() {
    let dir = setup_two_agent_bundle_project();
    let tau_home = dir.path().join("global");
    std::fs::create_dir_all(&tau_home).unwrap();

    let bundle = build_bundle(dir.path(), &tau_home);

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args([
            "run",
            "--allow-ungoverned",
            "--bundle",
            bundle.to_str().unwrap(),
            "ghost",
            "seed input",
        ])
        .current_dir(dir.path())
        .env("TAU_HOME", &tau_home)
        .env("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1")
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "an agent id absent from the bundle's IR module must be refused, \
         not silently replaced; stdout={}",
        String::from_utf8_lossy(&output.stdout),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("\"ghost\""),
        "error must name the unknown agent id; stderr={stderr}"
    );
    assert!(
        stderr.contains("alpha") && stderr.contains("zulu"),
        "error must list the bundle's available agents; stderr={stderr}"
    );
}
