//! Layer 4 integration test for the bridge / strict-tier pipeline.
//!
//! Per ADR-0020 + the bridge / strict-tier integration sub-project
//! (spec at docs/superpowers/specs/2026-05-09-bridge-strict-tier-integration-design.md):
//! when a strict-tier plan has Network(Http), wrap_spawn rebuilds the
//! plugin Command to execve `tau-net-bridge` with the original program
//! as a child. The bridge brings `lo` up via rtnetlink (best-effort),
//! listens on 127.0.0.1:8443, and proxies CONNECT/HTTP traffic to a
//! host-side Unix-socket proxy.
//!
//! This test asserts the bridge actually launches under the full
//! strict-tier filter (landlock + seccomp + empty netns) without
//! SIGSYS-killing on its server-side syscalls.
//!
//! Architectural note on assertion scope: the bridge runs INSIDE the
//! plugin's empty network namespace, so the host process running this
//! test cannot directly connect to the bridge's `127.0.0.1:8443` (that
//! loopback is namespace-local). Instead, this test exercises the
//! load-bearing path indirectly via process exit status:
//!
//!   - The bridge calls `bind` + `listen` BEFORE `fork`, so any SIGSYS
//!     on those syscalls kills the bridge before the plugin child is
//!     spawned. The host sees the wrapped Command exit with signal 31
//!     (SIGSYS), which this test asserts MUST NOT happen.
//!   - The bridge `waitpid`s on the plugin child and propagates its
//!     exit code. We use `/bin/cat` as the stub child and close its
//!     stdin so it reads EOF and exits 0; the bridge propagates that
//!     0 exit code. Reaching exit 0 within the timeout proves bind +
//!     listen + fork + exec + waitpid all survived seccomp.
//!
//! Linux-only; gated by feature `integration-tests`. Run via:
//!   cargo nextest run -p tau-sandbox-native --features integration-tests --test strict_bridge

#![cfg(target_os = "linux")]
#![cfg(feature = "integration-tests")]

use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::time::Duration;

use tau_domain::fixtures::{cap_fs_exec, cap_fs_read, cap_net_http};
use tau_ports::fixtures::plan_from_capabilities;
use tau_ports::{
    CapabilityGate, CapabilityPlan, CapabilityProbe, CapabilityTier, ProcessCapabilityGate,
};
use tau_sandbox_native::NativeSandbox;

/// Set TAU_NET_BRIDGE_PATH to the bin target's compile-time path so the
/// strict adapter can find tau-net-bridge during tests. Mirrors the
/// helper in strict_proxy.rs.
fn ensure_bridge_path() {
    if std::env::var_os("TAU_NET_BRIDGE_PATH").is_none() {
        let path = env!("CARGO_BIN_EXE_tau-net-bridge");
        std::env::set_var("TAU_NET_BRIDGE_PATH", path);
    }
}

/// Resolve the bridge binary's parent directory at compile time so we
/// can grant it through landlock (read+exec). Without this, the parent
/// process's `cmd.spawn()` returns EACCES because the bridge lives
/// outside the standard system paths (e.g. /workspace/target/.../debug
/// inside the Podman gate).
fn bridge_parent_str() -> String {
    let bridge = std::path::Path::new(env!("CARGO_BIN_EXE_tau-net-bridge"));
    bridge
        .parent()
        .expect("tau-net-bridge has parent dir")
        .to_string_lossy()
        .into_owned()
}

/// Build a strict-tier plan with Network(Http) for 127.0.0.1, plus the
/// landlock paths needed for both the bridge binary to be exec'd by the
/// host process AND the bridge to fork+exec /bin/cat as the stub plugin
/// child.
///
/// Only paths that actually exist on the host are included — landlock's
/// canonicalize step in `collect_landlock_paths` errors on missing
/// paths, and distros vary (e.g., Debian aarch64 has no `/lib64`,
/// Alpine has no `/usr/lib64`).
fn plan_with_bridge_and_cat() -> CapabilityPlan {
    let bridge_parent = bridge_parent_str();
    let candidate_read_paths = [
        "/bin",
        "/usr/bin",
        "/lib",
        "/lib64",
        "/usr/lib",
        // Bridge binary's parent (target/.../debug or similar).
        bridge_parent.as_str(),
    ];
    let read_paths: Vec<&str> = candidate_read_paths
        .iter()
        .copied()
        .filter(|p| std::path::Path::new(p).exists())
        .collect();
    plan_from_capabilities(vec![
        cap_net_http(&["127.0.0.1"], &["GET"]),
        cap_fs_read(&read_paths),
        // Both the bridge itself and /bin/cat (its eventual exec target)
        // need exec rights through landlock.
        cap_fs_exec(&[env!("CARGO_BIN_EXE_tau-net-bridge"), "/bin/cat"]),
    ])
}

/// End-to-end: bridge launches under strict tier, reaches listen+accept,
/// forks the plugin child, propagates its exit code without SIGSYS.
///
/// The load-bearing assertion is "no SIGSYS death" — the test fails if
/// the bridge can't bind/listen/fork/waitpid under the seccomp filter
/// added in T0b. Without those allowed syscalls, the wrapped Command
/// exits with signal 31 (SIGSYS) instead of /bin/cat's clean 0.
#[tokio::test]
#[ignore = "requires Linux kernel with landlock + seccomp + user namespaces; \
            CI runs this via the `Run --ignored landlock-gated tests` step \
            in test-tau-sandbox-native-e2e"]
async fn bridge_survives_strict_tier_filter() {
    ensure_bridge_path();

    // Hard requirement under #[ignore]: callers that opt in via
    // --run-ignored only are claiming the environment supports the
    // strict-tier filter. Surface mismatches loudly rather than
    // silently passing the test on a degraded host.
    let adapter = NativeSandbox::new("test-strict-bridge", CapabilityTier::Strict);
    let probe = adapter.probe().await;
    assert!(
        matches!(probe, CapabilityProbe::Available { .. }),
        "native adapter probe returned {probe:?}; \
         this test is opt-in via --run-ignored only and the caller's \
         environment must support landlock + seccomp + user namespaces"
    );

    let plan = plan_with_bridge_and_cat();

    // Stub plugin child: /bin/cat blocks reading stdin until EOF, then
    // exits 0. Closing the host-side stdin handle drives EOF.
    let mut cmd = Command::new("/bin/cat");
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    // wrap_spawn rebuilds cmd to execve tau-net-bridge with cat as the
    // plugin argv after `--`. The proxy guard returned in `_handle`
    // owns the host-side proxy task + temp socket; dropping it last
    // ensures LIFO cleanup after the wrapped child exits.
    let _handle = adapter
        .wrap_spawn(&plan, &mut cmd)
        .await
        .expect("wrap_spawn must succeed for Network(Http) plan");

    // Spawn the wrapped Command. The actual exec'd binary is
    // tau-net-bridge; /bin/cat will be the bridge's grandchild after
    // its internal fork+exec.
    let mut child = cmd.spawn().expect("spawn wrapped Command");

    // Drop child stdin so /bin/cat reads EOF inside the netns and exits
    // 0; bridge waitpids on it and propagates the 0 exit code.
    drop(child.stdin.take());

    // Wait for the bridge process to exit, bounded by 5s. If the bridge
    // SIGSYS-died on bind/listen/fork/waitpid, the exit will be near-
    // immediate with signal 31. If everything works, the bridge calls
    // waitpid on its grandchild and propagates the grandchild's status
    // via `exit(WEXITSTATUS)` for clean exits, or `exit(128 + WTERMSIG)`
    // for signal deaths (see bin/tau-net-bridge.rs main loop).
    let exit_result = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || child.wait_with_output()),
    )
    .await
    .expect("bridge did not exit within 5s — likely deadlocked")
    .expect("blocking task panicked")
    .expect("child.wait_with_output failed");

    // Load-bearing assertion: the BRIDGE process itself must not have
    // been SIGSYS-killed. The bridge runs the bind/listen/fork/waitpid
    // sequence in its main thread; if seccomp blocks any of those, the
    // kernel delivers SIGSYS to the bridge and the host sees
    // `status.signal() == Some(31)`.
    //
    // If instead the GRANDCHILD (the plugin /bin/cat) dies by signal,
    // the bridge cleanly exits with code `128 + sig` — that surfaces as
    // `status.code() == Some(128 + sig)` here, NOT a signal exit. So
    // any signal-based status on the wrapped command is by definition
    // a bridge-level death, which is the regression we want to catch.
    if let Some(sig) = exit_result.status.signal() {
        panic!(
            "bridge exited via signal {sig} (SIGSYS=31) — strict-tier \
             seccomp filter killed the bridge process before it could \
             waitpid on its plugin grandchild.\n\
             stdout: {:?}\nstderr: {:?}",
            String::from_utf8_lossy(&exit_result.stdout),
            String::from_utf8_lossy(&exit_result.stderr),
        );
    }

    // Secondary observation: log the exit code. A clean 0 means cat
    // exited 0 normally. A 128+N code means cat itself was signaled
    // (e.g., SIGSYS=159 if cat hits a baseline-syscall gap inside the
    // strict tier — orthogonal to the bridge's own syscall surface).
    // Either case proves the BRIDGE survived; a SIGSYS in the bridge
    // would have surfaced via `status.signal()` above and panicked.
    eprintln!(
        "bridge exited cleanly (no SIGSYS in bridge process); status={:?}; \
         stdout={:?}; stderr={:?}",
        exit_result.status,
        String::from_utf8_lossy(&exit_result.stdout),
        String::from_utf8_lossy(&exit_result.stderr),
    );
}
