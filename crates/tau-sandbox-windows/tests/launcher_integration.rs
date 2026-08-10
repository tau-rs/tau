//! Windows-only: proves the launcher runs a target inside an AppContainer.
#![cfg(all(target_os = "windows", feature = "integration-tests"))]

use std::process::Command;

/// The launcher runs a benign target and forwards its exit code + stdout.
#[test]
fn launcher_runs_target_and_forwards_exit_and_stdout() {
    // Create a real AppContainer profile for the run (unique name).
    let profile = format!("tau-test-{}", std::process::id());
    tau_sandbox_windows::test_support::create_profile(&profile).expect("create profile");

    let out = Command::new(env!("CARGO_BIN_EXE_tau-appcontainer-launcher"))
        .args([
            "--profile",
            &profile,
            "--",
            "cmd",
            "/C",
            "echo hello & exit 7",
        ])
        .output()
        .expect("spawn launcher");

    tau_sandbox_windows::test_support::delete_profile(&profile).ok();

    assert!(
        String::from_utf8_lossy(&out.stdout).contains("hello"),
        "stdout: {:?}",
        out.stdout
    );
    assert_eq!(out.status.code(), Some(7), "exit code should propagate");
}
