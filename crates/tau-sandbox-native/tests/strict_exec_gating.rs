//! Sub-project E — per-command exec argument-filter e2e tests.
//!
//! Verifies that the landlock-based exec gating at Strict tier:
//! - accepts plans with `Filesystem(Exec)` and `Process(Spawn)` capabilities,
//! - actually allows execution of the controlled-env binary when its path
//!   is covered by the plan's exec-allowed list.
//!
//! These tests require `--features integration-tests` and a Linux kernel
//! with landlock V1 (>= 5.13). On macOS dev builds the file is compiled
//! (`--no-run`) but execution is gated by `cfg(target_os = "linux")`.

#![cfg(feature = "integration-tests")]
#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::process::Command;

use tau_domain::fixtures::{cap_fs_exec, cap_fs_read, cap_process_spawn};
use tau_ports::{
    fixtures::plan_from_capabilities, CapabilityGate, ProcessCapabilityGate, CapabilityPlan,
    CapabilityTier,
};
use tau_sandbox_native::NativeSandbox;

fn locate_controlled_env_bin() -> PathBuf {
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

fn bin_parent_str() -> String {
    locate_controlled_env_bin()
        .parent()
        .expect("controlled-env binary has parent dir")
        .to_string_lossy()
        .into_owned()
}

/// A plan that grants `fs.exec` on the controlled-env binary's parent dir
/// (allowing exec of the binary) AND `fs.read` on the same dir (required
/// by landlock to read the binary file during exec).
fn plan_with_fs_exec() -> CapabilityPlan {
    let bin_parent = bin_parent_str();
    plan_from_capabilities(vec![
        // Read access so landlock can load the binary into memory.
        cap_fs_read(&[bin_parent.as_str()]),
        // Exec access — the sub-project E enforcement.
        cap_fs_exec(&[bin_parent.as_str()]),
    ])
}

/// A plan that grants `process.spawn` with the full binary path AND `fs.read`
/// on the binary's parent so the binary can be loaded.
fn plan_with_process_spawn() -> CapabilityPlan {
    let bin = locate_controlled_env_bin();
    let bin_str = bin.to_string_lossy().into_owned();
    let bin_parent = bin_parent_str();
    plan_from_capabilities(vec![
        cap_fs_read(&[bin_parent.as_str()]),
        cap_process_spawn(&[bin_str.as_str()]),
    ])
}

/// Exec is allowed when the binary's path is in `Filesystem(Exec { paths })`.
///
/// The controlled-env binary exits 0 with "CONTROLLED_ENV_OK" on stdout when
/// no `TAU_FIXTURE_MODE` env var is set, making it a reliable positive-case probe.
#[tokio::test]
async fn exec_allowed_via_fs_exec_capability() {
    let plan = plan_with_fs_exec();
    let mut cmd = Command::new(locate_controlled_env_bin());

    let sandbox = NativeSandbox::new("test-strict-exec-e", CapabilityTier::Strict);
    let _handle = sandbox
        .wrap_spawn(&plan, &mut cmd)
        .await
        .expect("wrap_spawn must succeed");
    let output = cmd.output().expect("spawn");

    assert!(
        output.status.success(),
        "binary must exit 0 under fs.exec capability; status={:?}, stdout={:?}, stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("CONTROLLED_ENV_OK"),
        "expected CONTROLLED_ENV_OK; got stdout={:?}",
        String::from_utf8_lossy(&output.stdout),
    );
}

/// Exec is allowed when the binary's full path is in
/// `Process(Spawn { commands })`.
#[tokio::test]
async fn exec_allowed_via_process_spawn_full_path() {
    let plan = plan_with_process_spawn();
    let mut cmd = Command::new(locate_controlled_env_bin());

    let sandbox = NativeSandbox::new("test-strict-spawn-e", CapabilityTier::Strict);
    let _handle = sandbox
        .wrap_spawn(&plan, &mut cmd)
        .await
        .expect("wrap_spawn must succeed");
    let output = cmd.output().expect("spawn");

    assert!(
        output.status.success(),
        "binary must exit 0 under process.spawn capability; status={:?}, stdout={:?}, stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("CONTROLLED_ENV_OK"),
        "expected CONTROLLED_ENV_OK; got stdout={:?}",
        String::from_utf8_lossy(&output.stdout),
    );
}
