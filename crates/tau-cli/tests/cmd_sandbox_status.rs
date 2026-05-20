//! Integration tests for `tau sandbox status`.

use assert_cmd::Command as AssertCmd;
use predicates::prelude::*;

mod common;

#[test]
fn status_prints_platform_and_adapter_table() {
    let dir = common::setup_echo_project("echo", "canned_text = \"r\"\n", &[]);
    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["sandbox", "status"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .assert()
        .success()
        .stdout(predicate::str::contains("platform:"))
        .stdout(predicate::str::contains("adapters detected:"))
        .stdout(predicate::str::contains("native:"))
        .stdout(predicate::str::contains("container:"))
        .stdout(predicate::str::contains("passthrough:"));
}

#[test]
fn status_reports_project_required_tier() {
    let dir = common::setup_echo_project("echo", "canned_text = \"r\"\n", &[]);
    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["sandbox", "status"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .assert()
        .success()
        .stdout(predicate::str::contains("required_tier:"));
}

#[test]
fn status_always_exits_zero() {
    // Even if no scope config exists or resolution fails, status must
    // exit 0; errors are rendered in the output.
    let dir = tau_ports::fixtures::scratch_dir("sandbox-status-no-scope");
    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["sandbox", "status"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .assert()
        .success();
}

#[test]
fn status_shows_resolution_outcome() {
    let dir = common::setup_echo_project("echo", "canned_text = \"r\"\n", &[]);
    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["sandbox", "status"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .env("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1")
        .assert()
        .success()
        .stdout(predicate::str::contains("resolution:"));
}
