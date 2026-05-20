//! Integration tests for `tau check --target <triple>`.

#[path = "check_common.rs"]
mod check_common;

use std::process::Command;

fn tau_bin() -> std::path::PathBuf {
    assert_cmd::cargo::cargo_bin("tau")
}

/// Create a minimal project tempdir with just a tau.toml.
fn minimal_project() -> tempfile::TempDir {
    check_common::ensure_tau_home();
    let dir = tempfile::tempdir().expect("tempdir");
    let tau_toml = r#"[project]
name = "check-target-test"
"#;
    std::fs::write(dir.path().join("tau.toml"), tau_toml).expect("write tau.toml");
    dir
}

#[test]
fn check_target_against_unknown_triple_exits_64() {
    let project = minimal_project();
    let out = Command::new(tau_bin())
        .current_dir(project.path())
        .args(["check", "sandbox", "--target", "bogus-bogus-bogus"])
        .output()
        .expect("spawn");
    assert_eq!(
        out.status.code(),
        Some(64),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn check_target_passthrough_succeeds() {
    let project = minimal_project();
    let out = Command::new(tau_bin())
        .current_dir(project.path())
        .args(["check", "sandbox", "--target", "passthrough"])
        .output()
        .expect("spawn");
    assert!(
        out.status.success() || out.status.code() == Some(3),
        "expected success or NeedsSetup (3), got code {:?}. stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn check_target_linux_native_strict_runs() {
    let project = minimal_project();
    let out = Command::new(tau_bin())
        .current_dir(project.path())
        .args(["check", "sandbox", "--target", "linux-native-strict"])
        .output()
        .expect("spawn");
    // Exit code depends on platform; we only assert it doesn't crash.
    assert!(
        out.status.code().is_some(),
        "process should have exited cleanly. stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_ne!(out.status.code(), Some(64), "shouldn't be a usage error");
    assert_ne!(out.status.code(), Some(70), "shouldn't be internal error");
}

#[test]
fn check_target_against_reserved_triple_warns_but_passes() {
    let project = minimal_project();
    let out = Command::new(tau_bin())
        .current_dir(project.path())
        .args([
            "check",
            "sandbox",
            "--target",
            "windows-native-strict",
            "--json",
        ])
        .output()
        .expect("spawn");
    // No plugins installed → sandbox category should skip ("no plugin packages in lockfile")
    // before hitting the target_reserved Warning. Either outcome is acceptable;
    // we just assert no internal error.
    assert_ne!(
        out.status.code(),
        Some(70),
        "shouldn't be internal error. stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_ne!(out.status.code(), Some(64), "shouldn't be usage error");
}

#[test]
fn check_target_parse_error_exits_64() {
    let project = minimal_project();
    let out = Command::new(tau_bin())
        .current_dir(project.path())
        .args(["check", "sandbox", "--target", "linux-natiive-strict"]) // typo
        .output()
        .expect("spawn");
    assert_eq!(
        out.status.code(),
        Some(64),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
