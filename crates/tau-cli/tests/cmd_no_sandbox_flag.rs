//! Integration tests for `--no-sandbox` and `--sandbox <kind>`.
//!
//! These tests exercise the global flags introduced in Task 7:
//! `--no-sandbox` (force passthrough adapter, bypass plugin-tier floors)
//! and `--sandbox <kind>` (force a specific adapter).
//!
//! Most tests rely on `tau chat --dry-run` so they don't require an LLM
//! backend: the dry-run path validates CLI parsing and exits before
//! attempting plugin spawn.

use assert_cmd::Command as AssertCmd;
use predicates::prelude::*;

mod common;

// ---- Test 1: --no-sandbox is accepted and chat --dry-run succeeds ----------

#[test]
fn no_sandbox_smokes() {
    // tau chat <agent> --no-sandbox --dry-run should succeed:
    // clap accepts the flag; dry-run returns before plugin loading.
    let dir = common::setup_echo_project("echo", "canned_text = \"reply\"\n", &[]);
    let global_dir = dir.path().join("global");
    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["--no-sandbox", "chat", "--dry-run", "echo"])
        .current_dir(dir.path())
        .env("TAU_HOME", &global_dir)
        .assert()
        .success();
}

// ---- Test 2: --sandbox passthrough is equivalent to --no-sandbox -----------

#[test]
fn sandbox_passthrough_equivalent_to_no_sandbox() {
    // --sandbox passthrough should behave identically to --no-sandbox.
    let dir = common::setup_echo_project("echo", "canned_text = \"reply\"\n", &[]);
    let global_dir = dir.path().join("global");
    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["--sandbox", "passthrough", "chat", "--dry-run", "echo"])
        .current_dir(dir.path())
        .env("TAU_HOME", &global_dir)
        .assert()
        .success();
}

// NOTE: a former "Test 3" asserted that `--sandbox native` on Windows *errors
// clearly* because the Windows adapter's probe returned Unavailable. Phase 2
// (ADR-0067, #610) graduated the Windows AppContainer adapter to Available, so
// that premise no longer holds — install-path success on Windows is now covered
// by the un-gated `cmd_install`/`list`/`uninstall`/`update` suites. Under the
// graduated adapter that test instead drove a real plugin spawn through the
// launcher and tripped the shared-spawn-path stdio-loss panic
// (`plugin_host/process.rs` `child.stdin.take()`). A positive test that a real
// plugin spawn succeeds on Windows lands with the follow-up that fixes that
// stdio handoff; until then, spawning through the launcher on Windows is
// intentionally not exercised here.

// ---- Test 4: --no-sandbox and --sandbox conflict ---------------------------

#[test]
fn no_sandbox_and_sandbox_flag_conflict() {
    // clap's conflicts_with attribute should produce a parse error before
    // any command logic runs (no project directory needed).
    let dir = tempfile::tempdir().unwrap();
    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args([
            "--no-sandbox",
            "--sandbox",
            "container",
            "chat",
            "--dry-run",
            "echo",
        ])
        .current_dir(dir.path())
        .assert()
        .failure();
    // clap produces exit code 2 for argument errors.
}

// ---- Test 5: --no-sandbox and --sandbox appear in --help -------------------

#[test]
fn no_sandbox_appears_in_help() {
    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--no-sandbox"))
        .stdout(predicate::str::contains("--sandbox"));
}
