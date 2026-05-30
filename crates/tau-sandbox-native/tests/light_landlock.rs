//! Sub-project D Task 2 — real-kernel landlock e2e tests using the
//! controlled-env binary.
//!
//! Verifies that the native adapter installs a landlock V1 ruleset
//! that allows reads inside declared paths and blocks reads outside.
//!
//! These tests were originally drafted at priority-12 ship but removed
//! because Ubuntu's `/bin → /usr/bin` symlinks tripped landlock V1's
//! lack of symlink resolution. Sub-project B's
//! `resolve_symlinks_for_landlock` helper canonicalizes paths and adds
//! both the symlink and target to the ruleset; D re-introduces the
//! tests using the controlled-env binary for predictable I/O.

#![cfg(feature = "integration-tests")]
#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::process::Command;
use tau_domain::fixtures::cap_fs_read;
use tau_ports::fixtures::plan_from_capabilities;
use tau_ports::{CapabilityGate, ProcessCapabilityGate, CapabilityPlan, CapabilityTier};
use tau_sandbox_native::NativeSandbox;
use tempfile::TempDir;

fn locate_controlled_env_bin() -> PathBuf {
    // CARGO_MANIFEST_DIR is `crates/tau-sandbox-native`; walk up to repo root.
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let bin = workspace_root.join(
        "crates/tau-plugin-compat/fixtures/controlled-env-binary/target/release/tau-controlled-env",
    );
    if !bin.exists() {
        panic!(
            "controlled-env binary not found at {}. Run: cargo build --manifest-path crates/tau-plugin-compat/fixtures/controlled-env-binary/Cargo.toml --release",
            bin.display()
        );
    }
    bin
}

/// Build a CapabilityPlan with read access to the given paths PLUS the
/// controlled-env binary's parent directory. Without the binary's
/// parent in the read paths, landlock blocks exec of the binary itself
/// (EACCES on spawn) since the standard system_read_paths in
/// `tau-sandbox-native::light` only includes /bin, /usr/bin, /lib, etc.
fn plan_with_read_paths(paths: Vec<&str>) -> CapabilityPlan {
    let bin = locate_controlled_env_bin();
    let bin_parent = bin
        .parent()
        .expect("controlled-env binary has parent dir")
        .to_string_lossy()
        .into_owned();
    // Extend with the binary's parent so landlock allows exec of the binary.
    let mut all_paths_owned: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
    all_paths_owned.push(bin_parent);
    let all_path_refs: Vec<&str> = all_paths_owned.iter().map(|s| s.as_str()).collect();
    plan_from_capabilities(vec![cap_fs_read(&all_path_refs)])
}

#[tokio::test]
async fn allowed_read_succeeds() {
    let tmp = TempDir::new().expect("tempdir");
    let allowed = tmp.path().join("allowed.txt");
    std::fs::write(&allowed, b"OK").unwrap();

    let plan = plan_with_read_paths(vec![tmp.path().to_str().unwrap()]);

    let mut cmd = Command::new(locate_controlled_env_bin());
    cmd.env("TAU_FIXTURE_MODE", "read")
        .env("TAU_FIXTURE_INPUT_PATH", &allowed);

    let sandbox = NativeSandbox::new("test-light", CapabilityTier::Light);
    let _handle = sandbox
        .wrap_spawn(&plan, &mut cmd)
        .await
        .expect("wrap_spawn");
    let output = cmd.output().expect("spawn");

    assert!(
        output.status.success(),
        "expected exit 0; got status={:?}, stdout={:?}, stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("READ_OK OK"),
        "expected READ_OK OK in stdout; got {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[tokio::test]
async fn blocked_read_returns_eacces() {
    let tmp = TempDir::new().expect("tempdir");
    // Write a file OUTSIDE the allowed path.
    let blocked_dir = TempDir::new().expect("blocked tempdir");
    let blocked_file = blocked_dir.path().join("secret.txt");
    std::fs::write(&blocked_file, b"SECRET").unwrap();

    // Plan only allows reads inside `tmp`, not `blocked_dir`.
    let plan = plan_with_read_paths(vec![tmp.path().to_str().unwrap()]);

    let mut cmd = Command::new(locate_controlled_env_bin());
    cmd.env("TAU_FIXTURE_MODE", "read")
        .env("TAU_FIXTURE_INPUT_PATH", &blocked_file);

    let sandbox = NativeSandbox::new("test-light", CapabilityTier::Light);
    let _handle = sandbox
        .wrap_spawn(&plan, &mut cmd)
        .await
        .expect("wrap_spawn");
    let output = cmd.output().expect("spawn");

    assert!(
        !output.status.success(),
        "expected non-zero exit; got status={:?}, stdout={:?}, stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("controlled-env-binary error"),
        "expected controlled-env error message; got stderr={stderr:?}"
    );
}

#[tokio::test]
async fn multiple_paths_all_landlocked() {
    let tmp_a = TempDir::new().expect("tempdir A");
    let tmp_b = TempDir::new().expect("tempdir B");
    let file_a = tmp_a.path().join("a.txt");
    let file_b = tmp_b.path().join("b.txt");
    std::fs::write(&file_a, b"A").unwrap();
    std::fs::write(&file_b, b"B").unwrap();

    let plan = plan_with_read_paths(vec![
        tmp_a.path().to_str().unwrap(),
        tmp_b.path().to_str().unwrap(),
    ]);

    // Read from B (second path).
    let mut cmd = Command::new(locate_controlled_env_bin());
    cmd.env("TAU_FIXTURE_MODE", "read")
        .env("TAU_FIXTURE_INPUT_PATH", &file_b);

    let sandbox = NativeSandbox::new("test-light", CapabilityTier::Light);
    let _handle = sandbox
        .wrap_spawn(&plan, &mut cmd)
        .await
        .expect("wrap_spawn");
    let output = cmd.output().expect("spawn");

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("READ_OK B"));
}
