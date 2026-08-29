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
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use tau_ports::{CapabilityPlan, ProcessCapabilityGate};
use tau_sandbox_windows::{test_support, WindowsSandbox};

/// Upper bound on the request head this fixture will buffer before it
/// answers regardless — a product bug must fail the assertion, never
/// grow the fixture's memory without limit.
const UPSTREAM_HEAD_CAP: usize = 64 * 1024;

/// One-shot upstream: accepts a single conn, drains the WHOLE request
/// head, answers 200 "hello", then half-closes. Returns (port, handle).
///
/// # Why the drain loop (#622 CI round 3)
///
/// `tau_sandbox_proxy::handle_http` forwards a request as **two**
/// writes: the rewritten (origin-form) request line, then the remaining
/// bytes of the head it had already buffered. An upstream that issues a
/// single `read()` therefore only reliably sees the first of them, and
/// the second is still sitting unread in the socket's receive buffer
/// when the socket is dropped.
///
/// On Windows, closing a socket that still holds unread received data
/// aborts the connection with an **RST** rather than a FIN. That
/// destroys the response already in flight in the other direction:
/// round 3's markers showed `PROXY http upstream connect OK` and
/// `PROXY http forwarded head line=16B rest=44B`, followed by
/// `PROXY splice remote->client FAILED err=... (os error 10054)` and
/// zero bytes at the probe. The product chain was working; this fixture
/// was tearing it down.
///
/// The fix belongs here, not in `handle_http`: writing a request in two
/// segments is legitimate, and any HTTP client that did the same would
/// hit the identical fixture bug. So read until the `\r\n\r\n` head
/// terminator — bounded by [`UPSTREAM_HEAD_CAP`] *and* a read timeout so
/// a regression fails the test instead of hanging CI — then answer, then
/// `shutdown(Write)` so the peer observes a clean FIN.
fn spawn_upstream() -> (u16, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("upstream bind");
    let port = listener.local_addr().expect("addr").port();
    let h = std::thread::spawn(move || {
        if let Ok((mut s, _)) = listener.accept() {
            // Bounded: a product bug must fail the assertion in the test
            // body, not wedge this thread (the test `join()`s it).
            let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
            let mut head = Vec::new();
            let mut chunk = [0u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < UPSTREAM_HEAD_CAP {
                match s.read(&mut chunk) {
                    Ok(0) => break,  // peer closed
                    Err(_) => break, // timeout or reset
                    Ok(n) => head.extend_from_slice(&chunk[..n]),
                }
            }
            let _ = s.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
            );
            let _ = s.flush();
            // FIN, not RST: tells the proxy's `remote -> client` splice
            // the response is complete, which is what lets the whole
            // chain tear down in order.
            let _ = s.shutdown(std::net::Shutdown::Write);
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
    // The host-side proxy has no tracing subscriber in these tests, so
    // its `tracing` events are invisible exactly when the data path
    // breaks. This opt-in flag mirrors every decision point to stderr as
    // a `PROXY ` marker (parsed host/port, allowlist verdict, port gate,
    // upstream connect, forwarded head size, per-direction splice byte
    // counts) — the missing third hop next to `BRIDGE ` and `PIPEPROXY `.
    // nextest runs one process per test, so this cannot leak between tests.
    std::env::set_var("TAU_SANDBOX_PROXY_TRACE", "1");
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
    // Pin the *proxy's* answer, not just the digits "403" appearing
    // anywhere in the output (an ephemeral port could contain them).
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("PROBE result=status detail=HTTP/1.1 403"),
        "expected proxy 403 status line, got:\n{}",
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

/// Sibling isolation stays: a path with NO grant is denied.
///
/// Positive-grant readability (the "cousin" side of this property) is
/// already covered end-to-end by `leaf_only_grant_readable_at_nested_path`
/// above. An earlier version of this test also created a `granted`
/// sibling directory that was never actually ACL-granted — inert
/// scaffolding that proved nothing and implied a positive-read
/// assertion this test doesn't make. Deleted rather than wired up: doing
/// so properly would mean a second `wrap_spawn`/probe invocation (a
/// second AppContainer profile) inside this test purely to re-prove
/// what `leaf_only_grant_readable_at_nested_path` already proves, for no
/// added coverage.
#[tokio::test]
async fn ungranted_sibling_path_still_denied() {
    let base = std::env::temp_dir().join(format!("tau-egress-sib-{}", std::process::id()));
    std::fs::create_dir_all(&base).expect("mkdirs");
    let secret = base.join("secret.txt");
    std::fs::write(&secret, "secret").expect("write");
    let plan = plan_with_hosts_and_read(
        &["127.0.0.1"],
        &[env!("CARGO_BIN_EXE_tau-sandbox-test-probe")],
    );
    let out = run_probe_wrapped(&plan, &["read-file", secret.to_str().unwrap()]).await;
    assert_ne!(
        out.status.code(),
        Some(0),
        "ungranted path must stay denied:\n{}",
        render(&out)
    );
    // Pin down *how* it failed: must be the probe's own access-denied
    // read error, not a spawn failure or watchdog timeout that would
    // also produce a non-zero exit without ever exercising the ACL.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("PROBE result=err detail=read"),
        "expected the probe to run and report a denied read, got:\n{}",
        render(&out)
    );
}

/// A grant on a DIRECTORY must reach files that already existed inside
/// it — the property the whole ACL design rests on, and the one nothing
/// tested until now.
///
/// `leaf_only_grant_readable_at_nested_path` grants on the leaf *file*
/// itself, so it proves bypass-traverse, not inheritance.
/// `wrap_spawn` however grants on directories (`<pkg>/**`,
/// `$CARGO_HOME/**`, `$RUSTUP_HOME/**` from `tau-pkg`'s build envelope)
/// and relies on `SetNamedSecurityInfoW` propagating the inheritable ACE
/// to the files that are ALREADY there — a toolchain, a source tree.
/// If that propagation does not happen, every such grant is inert and
/// the container sees `Access is denied` on pre-existing files, which is
/// precisely the shape of #622's `install_rust_cargo_acceptance` failure
/// (`rustc.exe ... Access is denied (os error 5)` with the `.rustup`
/// grant reported as successful).
///
/// So: create the file FIRST, grant on the ancestor directory SECOND,
/// then read the file from inside the container. Red here localises that
/// failure to ACL propagation and rules the `tau-pkg` path resolution
/// out; green rules propagation out and sends the search elsewhere.
#[tokio::test]
async fn dir_grant_reaches_preexisting_nested_file() {
    let profile = format!("tau-egress-inh-{}", std::process::id());
    let base = std::env::temp_dir().join(format!("tau-egress-inherit-{}", std::process::id()));
    let nested = base.join("x/y");
    std::fs::create_dir_all(&nested).expect("mkdirs");
    let leaf = nested.join("preexisting.txt");
    // Written BEFORE the grant: this is the whole point of the test.
    std::fs::write(&leaf, "hello").expect("write");

    test_support::create_profile(&profile).expect("profile");
    let probe = env!("CARGO_BIN_EXE_tau-sandbox-test-probe");
    test_support::grant_read(&profile, probe).expect("grant probe");
    // Grant on the ANCESTOR DIRECTORY only — never on the leaf.
    test_support::grant_read(&profile, base.to_str().expect("utf8")).expect("grant dir");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tau-appcontainer-launcher"));
    cmd.args([
        "--profile",
        &profile,
        "--",
        probe,
        "read-file",
        leaf.to_str().expect("utf8"),
    ]);
    let out = tokio::task::spawn_blocking(move || cmd.output())
        .await
        .expect("spawn_blocking join")
        .expect("launcher");
    test_support::delete_profile(&profile).ok();

    assert_eq!(
        out.status.code(),
        Some(0),
        "a read grant on a directory did NOT reach a file that already existed inside it \
         — SetNamedSecurityInfoW is not propagating the inheritable ACE, so every \
         directory-shaped grant in the build envelope is inert:\n{}",
        render(&out)
    );
}

/// Stage a temp tree that ALREADY contains a nested executable and a
/// nested data file, both created before any ACL grant touches the tree.
/// Returns `(base, exe, marker)`; only `base` is ever granted.
///
/// The copy is what makes the exe genuinely pre-existing *and* covered
/// only by the directory grant: `std::fs::copy` gives the new file the
/// destination directory's inherited ACL, and the grant lands afterwards.
fn stage_preexisting_exe_tree(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("tau-egress-{tag}-{}", std::process::id()));
    let bin = base.join("toolchain/bin");
    std::fs::create_dir_all(&bin).expect("mkdirs");
    let exe = bin.join("preexisting-probe.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_tau-sandbox-test-probe"), &exe).expect("copy probe");
    let marker = base.join("marker.txt");
    std::fs::write(&marker, "hello").expect("write marker");
    (base, exe, marker)
}

/// Run `program args...` inside the AppContainer `profile` via the
/// launcher, waiting on the blocking `output()` off-runtime (module doc).
async fn launch_in_container(profile: &str, program: &str, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tau-appcontainer-launcher"));
    cmd.args(["--profile", profile, "--", program]);
    cmd.args(args);
    tokio::task::spawn_blocking(move || cmd.output())
        .await
        .expect("spawn_blocking join")
        .expect("launcher")
}

/// A directory grant must confer **EXECUTE** (image activation) on a
/// pre-existing nested `.exe`, not merely READ.
///
/// `dir_grant_reaches_preexisting_nested_file` (above) proved the
/// inherited ACE reaches a pre-existing nested *data file* for reading.
/// Nothing proved it permits *image activation* of a pre-existing nested
/// executable — the property #622's `install_rust_cargo_acceptance`
/// actually depends on (`rustc.exe` lives inside the `$RUSTUP_HOME/**`
/// directory grant and is never granted in its own right).
///
/// This is the launcher-side half of the experiment: the process is
/// created by `tau-appcontainer-launcher`, which runs as the host user.
/// `in_container_spawn_reaches_dir_granted_preexisting_exe` covers the
/// other half, where the creator is itself inside the container.
#[tokio::test]
async fn dir_grant_confers_execute_on_preexisting_nested_exe() {
    let profile = format!("tau-egress-exe-{}", std::process::id());
    let (base, exe, marker) = stage_preexisting_exe_tree("exe");

    test_support::create_profile(&profile).expect("profile");
    // ONLY the ancestor directory is granted. The exe inside it never
    // receives an ACE of its own — an inherited one is all it can have.
    test_support::grant_read(&profile, base.to_str().expect("utf8")).expect("grant dir");

    let out = launch_in_container(
        &profile,
        exe.to_str().expect("utf8"),
        &["read-file", marker.to_str().expect("utf8")],
    )
    .await;
    test_support::delete_profile(&profile).ok();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("CreateProcessW"),
        "the OS refused to ACTIVATE a pre-existing nested .exe covered only by a directory \
         read grant, even though the same kind of grant makes a nested data file readable \
         (dir_grant_reaches_preexisting_nested_file) — the inherited ACE's access mask does \
         not permit image activation, which is exactly #622's `rustc.exe ... Access is \
         denied (os error 5)`; the fix then belongs in acl.rs:\n{}",
        render(&out)
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "dir-granted pre-existing exe ran but did not succeed:\n{}",
        render(&out)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("PROBE result=ok detail=read"),
        "expected the dir-granted exe to run and read the dir-granted marker:\n{}",
        render(&out)
    );
}

/// The same question, asked from *inside* the container — the shape
/// #622's FAILURE B actually has.
///
/// When `tau-appcontainer-launcher` creates a process it runs as the
/// host user, so a successful activation there does not prove the
/// AppContainer's own SID can activate that image. `rustc.exe`, by
/// contrast, is activated by cargo, which is already inside the
/// container. This test runs the probe in `spawn` mode so the
/// `CreateProcess` under test is issued by a container process.
///
/// It is a *controlled* experiment, which is why it launches twice:
///
/// - **control** — spawn an exe that carries its own explicit grant. If
///   this fails, in-container process creation is broken for reasons
///   that have nothing to do with ACL inheritance and the subject result
///   carries no information.
/// - **subject** — spawn the pre-existing nested exe covered ONLY by the
///   directory grant.
///
/// control green + subject red ⇒ an inherited ACE does not confer image
/// activation ⇒ fix in `acl.rs` (mask or ACE shape). Both green ⇒ the
/// grant mechanism is sound for a small tree and FAILURE B is specific
/// to the `.rustup` tree itself (scale, protected DACLs, …), which is a
/// design question, not a bug in this module.
#[tokio::test]
async fn in_container_spawn_reaches_dir_granted_preexisting_exe() {
    let profile = format!("tau-egress-spawn-{}", std::process::id());
    let (base, exe, marker) = stage_preexisting_exe_tree("spawn");
    let probe = env!("CARGO_BIN_EXE_tau-sandbox-test-probe");
    let marker_str = marker.to_str().expect("utf8");

    test_support::create_profile(&profile).expect("profile");
    // The launcher's target (the *parent* probe) is granted explicitly,
    // as in every other test here.
    test_support::grant_read(&profile, probe).expect("grant probe");
    // The staged tree is granted at the DIRECTORY only.
    test_support::grant_read(&profile, base.to_str().expect("utf8")).expect("grant dir");

    let control =
        launch_in_container(&profile, probe, &["spawn", probe, "read-file", marker_str]).await;
    let subject = launch_in_container(
        &profile,
        probe,
        &[
            "spawn",
            exe.to_str().expect("utf8"),
            "read-file",
            marker_str,
        ],
    )
    .await;
    test_support::delete_profile(&profile).ok();

    assert_eq!(
        control.status.code(),
        Some(0),
        "CONTROL failed: a process inside the AppContainer could not spawn an EXPLICITLY \
         granted exe. In-container process creation is broken for a reason unrelated to ACL \
         inheritance, so the subject case proves nothing — diagnose this first:\n{}",
        render(&control)
    );
    assert_eq!(
        subject.status.code(),
        Some(0),
        "SUBJECT failed while the CONTROL passed: a process inside the AppContainer can \
         activate an explicitly-granted exe but NOT a pre-existing nested exe covered only \
         by a directory grant. The inherited ACE does not confer image activation to the \
         container SID — that is #622's `rustc.exe ... Access is denied (os error 5)`, and \
         the fix belongs in acl.rs (access mask / ACE shape):\n{}",
        render(&subject)
    );
    assert!(
        String::from_utf8_lossy(&subject.stdout).contains("PROBE result=spawn-exit code=Some(0)"),
        "expected the in-container parent to report a successful child activation:\n{}",
        render(&subject)
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
    // A non-zero exit alone doesn't prove the DACL was exercised — a
    // launcher-level failure (foreign AppContainer can't even launch the
    // probe: missing exe, image-load denial) or the probe's own 8s
    // watchdog would also produce a non-zero exit here, and would let
    // this test pass green without ever reaching the pipe-open call it
    // exists to guard. Require the probe's own marker for a denied open,
    // and explicitly rule out the watchdog path.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("PROBE result=err detail=open"),
        "expected the probe to run and report a denied pipe open, got:\n{}",
        render(&out)
    );
    assert!(
        !stdout.contains("watchdog-timeout"),
        "probe hit its watchdog instead of failing the pipe open — the DACL was never exercised:\n{}",
        render(&out)
    );
}
