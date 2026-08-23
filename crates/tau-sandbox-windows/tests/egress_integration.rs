//! Windows-only: end-to-end egress + positive-FS enforcement for #622.
//! Chain under test: WindowsSandbox::wrap_spawn -> launcher ->
//! tau-net-bridge-win (in-container, ephemeral loopback port) ->
//! SID-DACL'd named pipe -> host-side HostAllow proxy -> upstream.
//!
//! Hermetic: the upstream is a host-loopback HTTP server; the proxy's
//! port policy allows loopback on any port, so no external network is
//! touched.
//!
//! ## Runtime gotcha: blocking `Command::output()` vs. the pipe proxy's
//! background task
//!
//! `WindowsSandbox::wrap_spawn` spawns the pipe-proxy accept loop via
//! `tokio::spawn` onto the *ambient* runtime (the ProcessCapabilityGate
//! trait is async, so `wrap_spawn` must run inside one). `#[tokio::test]`
//! defaults to the single-threaded (`current_thread`) flavor: if the test
//! body then calls the blocking `std::process::Command::output()`
//! directly, that call occupies the runtime's only worker thread for its
//! entire duration and the just-spawned accept-loop task never gets
//! polled — the wrapped child hangs until its own 8s watchdog fires,
//! which reads as a spurious failure rather than a real proxy/ACL bug.
//! `run_probe_wrapped` below runs the blocking wait via
//! `tokio::task::spawn_blocking` (a dedicated blocking-thread pool,
//! present under any runtime flavor) so the current-thread runtime stays
//! free to drive the proxy's accept loop and per-connection relay tasks
//! concurrently with the child process actually talking to them.
#![cfg(all(target_os = "windows", feature = "integration-tests"))]

use std::io::{Read, Write};
use std::process::Command;

use tau_ports::{CapabilityPlan, ProcessCapabilityGate};
use tau_sandbox_windows::{test_support, WindowsSandbox};

/// One-shot upstream: accepts a single conn, returns 200 "hello".
/// Returns (port, join-handle).
fn spawn_upstream() -> (u16, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("upstream bind");
    let port = listener.local_addr().expect("addr").port();
    let h = std::thread::spawn(move || {
        if let Ok((mut s, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let _ = s.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
            );
        }
    });
    (port, h)
}

fn plan_with_hosts_and_read(hosts: &[&str], read_paths: &[&str]) -> CapabilityPlan {
    serde_json::from_value(serde_json::json!({
        "capabilities": [
            { "kind": "net.http", "hosts": hosts, "methods": ["GET"] },
            { "kind": "fs.read", "paths": read_paths }
        ],
        "context": null,
        "limits": null,
    }))
    .expect("plan decode")
}

/// wrap_spawn the probe under the adapter and run it. Sets the
/// launcher/bridge env overrides to the cargo-built bins.
///
/// Runs the blocking `Command::output()` via `spawn_blocking` (see the
/// module doc) so the pipe proxy's background accept-loop task — spawned
/// by `wrap_spawn` onto this same (current-thread-by-default) runtime —
/// keeps making progress while the wrapped child talks to it.
async fn run_probe_wrapped(plan: &CapabilityPlan, probe_args: &[&str]) -> std::process::Output {
    std::env::set_var(
        "TAU_APPCONTAINER_LAUNCHER_PATH",
        env!("CARGO_BIN_EXE_tau-appcontainer-launcher"),
    );
    std::env::set_var(
        "TAU_NET_BRIDGE_WIN_PATH",
        env!("CARGO_BIN_EXE_tau-net-bridge-win"),
    );
    let gate = WindowsSandbox::new("windows");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tau-sandbox-test-probe"));
    cmd.args(probe_args);
    // `_guard` keeps the pipe proxy (and the AppContainer profile's ACL
    // grants) alive for the duration of the blocking wait below; it is
    // not moved into the spawn_blocking closure, only `cmd` is.
    let _guard = gate.wrap_spawn(plan, &mut cmd).await.expect("wrap_spawn");
    tokio::task::spawn_blocking(move || cmd.output())
        .await
        .expect("spawn_blocking join")
        .expect("spawn wrapped probe")
}

fn render(out: &std::process::Output) -> String {
    format!(
        "exit={:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    )
}

/// The whole chain: allowlisted loopback host fetch returns 200.
#[tokio::test]
async fn egress_allowlisted_host_succeeds_through_full_chain() {
    let (port, upstream) = spawn_upstream();
    let plan = plan_with_hosts_and_read(
        &["127.0.0.1"],
        &[env!("CARGO_BIN_EXE_tau-sandbox-test-probe")],
    );
    let url = format!("http://127.0.0.1:{port}/");
    let out = run_probe_wrapped(&plan, &["http-get", &url]).await;
    assert_eq!(
        out.status.code(),
        Some(0),
        "egress chain failed:\n{}",
        render(&out)
    );
    upstream.join().ok();
}

/// Negative guard: a host NOT in the allowlist gets the proxy's 403.
#[tokio::test]
async fn egress_unlisted_host_denied() {
    let plan = plan_with_hosts_and_read(
        &["allowed.example.com"],
        &[env!("CARGO_BIN_EXE_tau-sandbox-test-probe")],
    );
    let out = run_probe_wrapped(&plan, &["http-get", "http://denied.example.com/"]).await;
    assert_ne!(
        out.status.code(),
        Some(0),
        "unlisted host must be denied:\n{}",
        render(&out)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("403"),
        "expected proxy 403, got:\n{}",
        render(&out)
    );
}

/// Positive-FS (spike #626 H3, promoted): a leaf-only grant on a
/// nested path is readable — AppContainers retain bypass-traverse.
/// If a future Windows hardening strips SeChangeNotifyPrivilege from
/// AppContainer tokens, this fails and item 2 of ADR-0067's amendment
/// needs revisiting (FILE_TRAVERSE ancestor grants).
#[tokio::test]
async fn leaf_only_grant_readable_at_nested_path() {
    let dir = std::env::temp_dir()
        .join(format!("tau-egress-h3-{}", std::process::id()))
        .join("a/b/c");
    std::fs::create_dir_all(&dir).expect("mkdirs");
    let leaf = dir.join("leaf.txt");
    std::fs::write(&leaf, "hello").expect("write");
    let leaf_str = leaf.to_str().expect("utf8");
    let plan = plan_with_hosts_and_read(
        &["127.0.0.1"],
        &[env!("CARGO_BIN_EXE_tau-sandbox-test-probe"), leaf_str],
    );
    let out = run_probe_wrapped(&plan, &["read-file", leaf_str]).await;
    assert_eq!(
        out.status.code(),
        Some(0),
        "leaf-only grant no longer readable (bypass-traverse gone?):\n{}",
        render(&out)
    );
}

/// Sibling isolation stays: a path with NO grant is denied even while
/// its cousin is granted.
#[tokio::test]
async fn ungranted_sibling_path_still_denied() {
    let base = std::env::temp_dir().join(format!("tau-egress-sib-{}", std::process::id()));
    let granted = base.join("granted");
    let sibling = base.join("sibling");
    std::fs::create_dir_all(&granted).expect("mkdirs");
    std::fs::create_dir_all(&sibling).expect("mkdirs");
    let secret = sibling.join("secret.txt");
    std::fs::write(&secret, "secret").expect("write");
    let plan = plan_with_hosts_and_read(
        &["127.0.0.1"],
        &[env!("CARGO_BIN_EXE_tau-sandbox-test-probe")],
    );
    let out = run_probe_wrapped(&plan, &["read-file", secret.to_str().unwrap()]).await;
    assert_ne!(
        out.status.code(),
        Some(0),
        "ungranted sibling must stay denied:\n{}",
        render(&out)
    );
}

/// Security control (spike #626 H2-control, promoted): a pipe DACL'd
/// to container A must NOT be openable from container B. Guards the
/// per-spawn SID ACE — if someone ever "simplifies" the SDDL to
/// Everyone or ALL APPLICATION PACKAGES, this goes red.
///
/// Unlike the tests above, this one doesn't need the accept loop to be
/// polled: the DACL check happens at `CreateFile` time against the
/// already-created pipe instance (`spawn_pipe_proxy` creates the first
/// instance synchronously, before returning), so it's enforced whether
/// or not the tokio task behind it ever runs. The blocking
/// `Command::output()` is still routed through `spawn_blocking` to
/// match the other tests' pattern and avoid relying on that distinction.
#[tokio::test]
async fn foreign_container_cannot_open_pipe() {
    let owner = format!("tau-egress-own-{}", std::process::id());
    let foreign = format!("tau-egress-for-{}", std::process::id());
    test_support::create_profile(&owner).expect("owner profile");
    test_support::create_profile(&foreign).expect("foreign profile");
    let (pipe_name, _guard) =
        test_support::spawn_pipe_proxy(&owner, tau_sandbox_proxy::HostAllow::Any)
            .expect("pipe proxy");
    let probe = env!("CARGO_BIN_EXE_tau-sandbox-test-probe");
    test_support::grant_read(&foreign, probe).expect("grant probe to foreign");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tau-appcontainer-launcher"));
    cmd.args(["--profile", &foreign, "--", probe, "pipe-open", &pipe_name]);
    let out = tokio::task::spawn_blocking(move || cmd.output())
        .await
        .expect("spawn_blocking join")
        .expect("launcher");
    test_support::delete_profile(&owner).ok();
    test_support::delete_profile(&foreign).ok();
    assert_ne!(
        out.status.code(),
        Some(0),
        "foreign container opened another container's proxy pipe:\n{}",
        render(&out)
    );
}
