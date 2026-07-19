//! Cross-cutting matrix of (subcommand × scenario) → expected exit code.
//!
//! Per ADR-0006 / spec §3.5 the exit-code taxonomy is three buckets:
//!
//! - `0` (`ExitCode::Success`): operation completed.
//! - `1` (`ExitCode::AgentFailed`): `tau run` only — agent ran but
//!   reported `RunOutcome::Failed`.
//! - `2` (`ExitCode::Error`): kernel/CLI error (parse failure, missing
//!   project tau.toml, install failure, validation rejection, etc.).
//!
//! Existing per-command suites (`cmd_run.rs`, `cmd_chat.rs`, ...) cover
//! exit codes incidentally. This file makes the matrix explicit so the
//! contract regresses loudly if it ever shifts. Each test asserts the
//! bucket via `assert_cmd`'s `code(...)`.

mod common;

use assert_cmd::Command as AssertCmd;

// ---- install ----------------------------------------------------------------

#[test]
fn install_success_is_zero() {
    let (fixture, url, _bare) = common::setup_local_package_fixture("hello-tool", "0.1.0");
    let global_dir = fixture.path().join("scope-global");
    std::fs::create_dir_all(&global_dir).unwrap();

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["install", "--global", &url])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success()
        .code(0);
}

#[test]
fn install_bad_url_is_two() {
    let global_dir = tempfile::tempdir().unwrap();

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["install", "--global", "not-a-url"])
        .env("TAU_HOME", global_dir.path())
        .assert()
        .failure()
        .code(2);
}

// ---- list -------------------------------------------------------------------

#[test]
fn list_success_is_zero() {
    let global_dir = tempfile::tempdir().unwrap();

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["list", "--global"])
        .env("TAU_HOME", global_dir.path())
        .assert()
        .success()
        .code(0);
}

#[test]
fn list_dry_run_rejected_is_two() {
    let global_dir = tempfile::tempdir().unwrap();

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["list", "--dry-run"])
        .env("TAU_HOME", global_dir.path())
        .assert()
        .failure()
        .code(2);
}

// ---- run --------------------------------------------------------------------

#[test]
fn run_completed_is_zero() {
    let dir = common::setup_echo_project("echo", "canned_text = \"pong\"\n", &[]);

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["run", "echo", "ping", "--no-governance"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(0),
        "expected 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn run_plugin_crash_is_two() {
    // echo-llm with `crash_after_handshake = true` panics on the first
    // `llm.complete` RPC, which the host surfaces as a kernel error
    // mapped to exit code 2 (distinct from agent-level Failed = 1).
    let dir = common::setup_echo_project("echo", "crash_after_handshake = true\n", &[]);

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["run", "echo", "anything", "--no-governance"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected 2 (kernel error); stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn run_missing_project_is_two() {
    let dir = tempfile::tempdir().unwrap();

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["run", "reviewer", "hi"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .assert()
        .failure()
        .code(2);
}

#[test]
fn run_unknown_agent_is_two() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("tau.toml"),
        r#"packages = ["anthropic"]

[project]
name = "demo"

[models]
default = { backend = "anthropic", model = "claude-haiku-4-5" }

[agents.reviewer]
display_name = "Code Reviewer"
package      = "code-reviewer@^0.1"
model        = "default"
"#,
    )
    .unwrap();

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["run", "ghost", "hi"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .assert()
        .failure()
        .code(2);
}

// ---- init -------------------------------------------------------------------

#[test]
fn init_success_is_zero() {
    let dir = common::temp_project();

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success()
        .code(0);
}

#[test]
fn init_existing_without_force_is_two() {
    let dir = common::temp_project();

    // First init: succeeds.
    AssertCmd::cargo_bin("tau")
        .unwrap()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success();

    // Second init without --force: rejected.
    AssertCmd::cargo_bin("tau")
        .unwrap()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .failure()
        .code(2);
}

// ---- chat -------------------------------------------------------------------

#[test]
fn chat_dry_run_is_zero() {
    let dir = common::setup_echo_project("echo", "canned_text = \"unused\"\n", &[]);
    let global_dir = dir.path().join("global");

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["chat", "echo", "--dry-run"])
        .current_dir(dir.path())
        .env("TAU_HOME", &global_dir)
        .assert()
        .success()
        .code(0);
}

#[test]
fn chat_json_flag_is_two() {
    let dir = common::setup_echo_project("echo", "canned_text = \"unused\"\n", &[]);
    let global_dir = dir.path().join("global");

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["--json", "chat", "echo"])
        .current_dir(dir.path())
        .env("TAU_HOME", &global_dir)
        .write_stdin("")
        .assert()
        .failure()
        .code(2);
}
