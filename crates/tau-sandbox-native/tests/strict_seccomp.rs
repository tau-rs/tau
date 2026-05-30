//! Sub-project D Task 2 — real-kernel seccomp e2e tests.
//!
//! Verifies that the native adapter at Strict tier installs a seccomp
//! filter that SIGSYSes the child on syscalls outside the baseline +
//! capability-derived extensions.

#![cfg(feature = "integration-tests")]
#![cfg(target_os = "linux")]

use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::Command;
use tau_domain::fixtures::cap_fs_read;
use tau_ports::fixtures::plan_from_capabilities;
use tau_ports::{CapabilityGate, ProcessCapabilityGate, CapabilityPlan, CapabilityTier};
use tau_sandbox_native::NativeSandbox;

fn locate_controlled_env_bin() -> PathBuf {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    workspace_root.join(
        "crates/tau-plugin-compat/fixtures/controlled-env-binary/target/release/tau-controlled-env",
    )
}

/// Returns the controlled-env binary's parent dir as a string, for
/// inclusion in `fs.read` paths so landlock allows exec of the binary.
fn bin_parent_str() -> String {
    locate_controlled_env_bin()
        .parent()
        .expect("controlled-env binary has parent dir")
        .to_string_lossy()
        .into_owned()
}

fn plan_strict_no_network() -> CapabilityPlan {
    let bin_parent = bin_parent_str();
    plan_from_capabilities(vec![cap_fs_read(&[&bin_parent])])
}

#[tokio::test]
async fn socket_blocked_without_network_capability() {
    let plan = plan_strict_no_network();

    let mut cmd = Command::new(locate_controlled_env_bin());
    cmd.env("TAU_FIXTURE_MODE", "open-socket");

    let sandbox = NativeSandbox::new("test-strict", CapabilityTier::Strict);
    let _handle = sandbox
        .wrap_spawn(&plan, &mut cmd)
        .await
        .expect("wrap_spawn");
    let output = cmd.output().expect("spawn");

    // seccomp at strict tier without Network(Http) capability should
    // SIGSYS the process on socket(). Signal exit, NO stdout.
    assert!(
        !output.status.success(),
        "expected non-zero/signal exit; got status={:?}, stdout={:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        output.stdout.is_empty() || !String::from_utf8_lossy(&output.stdout).contains("SOCKET_OK"),
        "expected no SOCKET_OK; got stdout={:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    // Either signal-exit OR exit-1-from-EACCES is acceptable.
    if let Some(sig) = output.status.signal() {
        // SIGSYS = 31
        assert_eq!(sig, 31, "expected SIGSYS (31); got signal {sig}");
    }
}

#[tokio::test]
async fn baseline_syscalls_allowed() {
    // The default mode (no env vars) just emits CONTROLLED_ENV_OK,
    // exercising baseline syscalls (write, exit_group, etc.).
    let plan = plan_strict_no_network();

    let mut cmd = Command::new(locate_controlled_env_bin());

    let sandbox = NativeSandbox::new("test-strict", CapabilityTier::Strict);
    let _handle = sandbox
        .wrap_spawn(&plan, &mut cmd)
        .await
        .expect("wrap_spawn");
    let output = cmd.output().expect("spawn");

    assert!(output.status.success(), "baseline syscalls must succeed");
    assert!(String::from_utf8_lossy(&output.stdout).contains("CONTROLLED_ENV_OK"));
}
