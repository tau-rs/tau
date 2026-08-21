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

// ---- Test 3: --sandbox native on Windows spawns through the real adapter ----
//
// macOS satisfies `RegistryKind::Native` via `tau-sandbox-darwin`
// (sandbox-exec); Linux via `tau-sandbox-native` (landlock + seccomp +
// namespaces); Windows via `tau-sandbox-windows` (AppContainer launcher),
// which #610 (Phase 2, ADR-0067) graduated from Unavailable to Available.
//
// This is the regression guard for the launcher/sandbox-exec stdio-loss bug:
// the Windows adapter's `wrap_spawn` rebuilds the command (`*cmd =
// Command::new(launcher)`), which used to discard the piped stdio the plugin
// host set beforehand, so `child.stdin.take().expect(...)` panicked at
// plugin_host/process.rs. The fix applies stdio *after* `wrap_spawn`, so the
// real forced-Native spawn path must no longer panic. The plugin cannot
// actually complete here (the AppContainer launcher isn't on PATH in CI), so
// the command still fails — but it must fail *gracefully*, never with the
// stdio `expect` panic.
#[cfg(target_os = "windows")]
#[test]
fn sandbox_native_on_windows_does_not_panic_on_stdio() {
    let dir = common::setup_echo_project("echo", "canned_text = \"reply\"\n", &[]);
    let global_dir = dir.path().join("global");
    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["--sandbox", "native", "chat", "echo"])
        .current_dir(dir.path())
        .env("TAU_HOME", &global_dir)
        // Unset the mock env var so the real forced-Native adapter (the
        // AppContainer launcher, which rebuilds the command) is exercised and
        // not bypassed by mock-sandbox injection.
        .env_remove("TAU_TESTING_ALLOW_MOCK_SANDBOX")
        .write_stdin("")
        .assert()
        // The stdio-loss bug manifested as a panic in the plugin-host spawn
        // path. Assert its exact signatures are absent regardless of how the
        // (necessarily failing) spawn resolves.
        .stderr(predicate::str::contains("stdin piped").not())
        .stderr(predicate::str::contains("panicked").not());
}

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
