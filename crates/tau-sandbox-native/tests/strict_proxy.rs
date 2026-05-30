//! Layer 4 integration tests for the sandbox proxy.
//!
//! Replaces the deleted strict_net_filter.rs (sub-project F). These tests
//! exercise the full proxy + bridge + plugin chain via real spawn,
//! real netns, real seccomp.
//!
//! Linux-only; gated by feature `integration-tests`. Run via:
//!   cargo nextest run -p tau-sandbox-native --features integration-tests --test strict_proxy

#![cfg(target_os = "linux")]
#![cfg(feature = "integration-tests")]

use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::Command;
use tau_domain::fixtures::{cap_fs_read, cap_net_http};
use tau_ports::fixtures::plan_from_capabilities;
use tau_ports::{CapabilityGate, ProcessCapabilityGate, CapabilityPlan, CapabilityTier};
use tau_sandbox_native::NativeSandbox;

// ---------------------------------------------------------------------------
// Helpers (mirrors strict_seccomp.rs patterns)
// ---------------------------------------------------------------------------

fn locate_controlled_env_bin() -> PathBuf {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let bin = workspace_root.join(
        "crates/tau-plugin-compat/fixtures/controlled-env-binary/target/release/tau-controlled-env",
    );
    if !bin.exists() {
        panic!(
            "controlled-env binary not found at {}. Run: \
             cargo build --manifest-path \
             crates/tau-plugin-compat/fixtures/controlled-env-binary/Cargo.toml --release",
            bin.display()
        );
    }
    bin
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

fn plan_no_network() -> CapabilityPlan {
    let bin_parent = bin_parent_str();
    plan_from_capabilities(vec![cap_fs_read(&[&bin_parent])])
}

fn plan_with_http_cap(hosts: &[&str]) -> CapabilityPlan {
    let bin_parent = bin_parent_str();
    plan_from_capabilities(vec![
        cap_net_http(hosts, &["GET"]),
        cap_fs_read(&[&bin_parent]),
    ])
}

/// Set TAU_NET_BRIDGE_PATH to the bin target's compile-time path so the
/// container/native adapter can find tau-net-bridge during tests.
fn ensure_bridge_path() {
    if std::env::var_os("TAU_NET_BRIDGE_PATH").is_none() {
        let path = env!("CARGO_BIN_EXE_tau-net-bridge");
        std::env::set_var("TAU_NET_BRIDGE_PATH", path);
    }
}

/// Polls `temp_dir` until none of `names` remain, or the deadline expires.
/// Replaces a fixed sleep that previously waited "a beat" for the kernel
/// to unlink proxy sockets on handle drop; that sleep was flake-prone on
/// loaded CI.
async fn wait_for_paths_removed(
    temp_dir: &std::path::Path,
    names: &[std::ffi::OsString],
    deadline: std::time::Duration,
) -> Result<(), String> {
    let start = std::time::Instant::now();
    loop {
        let current: std::collections::HashSet<_> = std::fs::read_dir(temp_dir)
            .map_err(|e| format!("read_dir {}: {e}", temp_dir.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .collect();
        let still_present: Vec<_> = names.iter().filter(|n| current.contains(*n)).collect();
        if still_present.is_empty() {
            return Ok(());
        }
        if start.elapsed() > deadline {
            return Err(format!(
                "after {:?}, still present in {}: {:?}",
                deadline,
                temp_dir.display(),
                still_present
                    .iter()
                    .map(|n| n.to_string_lossy().into_owned())
                    .collect::<Vec<_>>(),
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Without a Network(Http) capability, the strict-tier seccomp filter must
/// block socket() with SIGSYS (or EACCES exit). Mirrors
/// strict_seccomp::socket_blocked_without_network_capability but verifies
/// the proxy code-path is correctly absent (no proxy socket, no wrapping).
#[tokio::test]
async fn no_network_cap_socket_denied_by_seccomp() {
    ensure_bridge_path();
    let plan = plan_no_network();

    let mut cmd = Command::new(locate_controlled_env_bin());
    cmd.env("TAU_FIXTURE_MODE", "open-socket");

    let sandbox = NativeSandbox::new("test-strict", CapabilityTier::Strict);
    let _handle = sandbox
        .wrap_spawn(&plan, &mut cmd)
        .await
        .expect("wrap_spawn must succeed for no-network plan");

    let output = cmd.output().expect("child spawn must succeed");

    // seccomp at strict tier without Network(Http) must block socket().
    assert!(
        !output.status.success(),
        "expected non-zero/signal exit; got status={:?}, stdout={:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
    );
    // SIGSYS = 31 from seccomp KillProcess; tolerate either signal or
    // the controlled-env binary's own non-zero exit on EACCES.
    if let Some(sig) = output.status.signal() {
        assert_eq!(sig, 31, "expected SIGSYS (31); got signal {sig}");
    }
}

/// When a Network(Http) plan is used, wrap_spawn spawns a proxy and creates
/// a temp socket file. Dropping the returned CapabilityHandle must unlink it.
#[tokio::test]
async fn proxy_handle_drop_cleans_up_temp_socket() {
    ensure_bridge_path();
    let plan = plan_with_http_cap(&["127.0.0.1"]);

    let mut cmd = Command::new(locate_controlled_env_bin());
    cmd.env("TAU_FIXTURE_MODE", "default");

    // Snapshot the temp dir BEFORE wrap_spawn so we can identify which
    // tau-proxy-*.sock files this test introduces (vs ones owned by other
    // tests running in parallel under nextest — e.g. strict_bridge.rs).
    let temp_dir = std::env::temp_dir();
    let baseline_files: std::collections::HashSet<_> = std::fs::read_dir(&temp_dir)
        .expect("read temp dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("tau-proxy-"))
        .map(|e| e.file_name())
        .collect();

    let sandbox = NativeSandbox::new("test-strict", CapabilityTier::Strict);
    let handle = sandbox
        .wrap_spawn(&plan, &mut cmd)
        .await
        .expect("wrap_spawn must succeed for Network(Http) plan");

    // After wrap_spawn, find OUR proxy socket(s): files that weren't in
    // the baseline snapshot. Naming pattern: tau-proxy-{pid}-{n}.sock
    // (see tau-sandbox-proxy).
    let after_spawn_files: std::collections::HashSet<_> = std::fs::read_dir(&temp_dir)
        .expect("read temp dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("tau-proxy-"))
        .map(|e| e.file_name())
        .collect();
    let new_files: Vec<_> = after_spawn_files
        .difference(&baseline_files)
        .cloned()
        .collect();
    assert!(
        !new_files.is_empty(),
        "expected at least one new tau-proxy-*.sock in {} after wrap_spawn \
         (baseline had {} matching files)",
        temp_dir.display(),
        baseline_files.len(),
    );

    drop(handle);
    // Poll for unlink instead of sleeping a fixed duration; the OS
    // unlinks asynchronously after the proxy process exits, and a fixed
    // 100ms wait was flake-prone on loaded CI. 2s deadline is generous
    // enough for heavy contention, short enough to fail fast on real bugs.
    //
    // This also subsumes the post-sleep re-read + assertion: the helper
    // only returns Ok once every entry in `new_files` is gone from
    // `temp_dir`, which is the property the test wants to enforce.
    wait_for_paths_removed(&temp_dir, &new_files, std::time::Duration::from_secs(2))
        .await
        .expect("proxy sockets not unlinked after handle drop");
}

// ---------------------------------------------------------------------------
// Tests that need a real cassette server (deferred to T9)
// ---------------------------------------------------------------------------

// `localhost_socket_allowed_with_http_cap`, `external_host_blocked_when_not_in_allowlist`,
// and `sni_mismatch_rejected` would each need:
//   - a cassette HTTP server running on the host (use tau-plugin-test-support if it exposes
//     a constructor; otherwise a simple tokio TcpListener test fixture)
//   - the controlled-env binary must be configured to make an actual HTTPS request via
//     HTTPS_PROXY (which it gets from cmd.env)
//
// These are deferred to layer4_container.rs (T9) because:
//   1. The controlled-env binary's existing TAU_FIXTURE_MODE values don't include
//      "make-https-request"; adding that mode is out of scope here
//   2. End-to-end TLS testing in a sandboxed netns requires careful cassette setup
//      that's better tested at the layer4_container.rs level (T9)
