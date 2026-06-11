# Sandbox Egress-Proxy Hardening (S6 + O3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close two LOW-severity hardening gaps on the same data path in `crates/tau-sandbox-proxy/src/lib.rs`: (S6) the world-writable control socket in shared `/tmp`, and (O3) silently swallowed byte-splice errors.

**Architecture:** S6 follows the ssh-agent/gpg-agent pattern — bind the Unix socket inside a per-run directory created at `0o700`. The directory gates filesystem traversal for other host users; the container bridge still reaches the socket through the bind-mount (`-v {sock}:/run/tau-proxy.sock`), which references the inode directly and is unaffected by host-side directory permissions; the native bridge runs as the same host user and traverses the dir under DAC (landlock grants the socket path, and landlock v1 does not gate traversal). The socket file mode stays permissive so rootless-userns container UIDs that differ from the owner can still dial it — the directory is the boundary. O3 replaces both `let _ = try_join!(...)` splice sites with one shared `splice_bidirectional` helper built on `tokio::join!` that logs each direction's outcome (`host`, `bytes` on success / `error` on failure) under target `tau::proxy`.

**Tech Stack:** Rust, tokio (Unix sockets + `tokio::io::copy`), `tracing`, `std::os::unix::fs::PermissionsExt`, `tracing-subscriber` (dev-dep, capturing Layer for the O3 test).

**Both touches live in the connection setup / data path of the same file.** S6 changes `spawn_proxy` + `make_temp_sock_path` + `ProxyHandle` (Drop). O3 changes the two splice tails (`handle_connect`, `handle_http`). They do not overlap textually but share the file; do S6 first (path/handle), then O3 (splice tails). Do NOT touch allowlist / CONNECT / HTTP-filter logic (that is the S4/S5 cluster, brief 40).

---

### Task 1: S6 — socket in a per-run `0o700` directory

**Files:**
- Modify: `crates/tau-sandbox-proxy/src/lib.rs` (`ProxyHandle`, `Drop`, `spawn_proxy`, `make_temp_sock_path`)
- Test: same file, `proxy_lifecycle_tests` module

- [ ] **Step 1: Write the failing test**

Add to `mod proxy_lifecycle_tests`:

```rust
#[tokio::test]
async fn socket_lives_in_private_0700_dir() {
    use std::os::unix::fs::PermissionsExt;
    let h = spawn_proxy(vec!["example.com".to_string()]).expect("spawn");
    let sock = h.sock_path().to_path_buf();
    let dir = sock.parent().expect("socket has a parent dir").to_path_buf();
    // The socket must NOT sit directly in the shared temp dir — it must be in
    // a dedicated per-run subdirectory so its perms can be locked down.
    assert_ne!(
        dir,
        std::env::temp_dir(),
        "socket must live in a private per-run dir, not shared temp"
    );
    // The per-run dir must be 0o700: no group/other access, so no other local
    // user can traverse into it to reach the socket (S6).
    let mode = std::fs::metadata(&dir).expect("dir metadata").permissions().mode();
    assert_eq!(mode & 0o777, 0o700, "per-run dir must be 0o700, got {:o}", mode & 0o777);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test -p tau-sandbox-proxy socket_lives_in_private_0700_dir`
Expected: FAIL (`assert_ne!` trips — socket currently sits directly in `std::env::temp_dir()`).

- [ ] **Step 3: Implement — per-run dir + handle tracks it**

Change `ProxyHandle` to also hold the directory, and clean it up on Drop:

```rust
#[cfg(unix)]
#[non_exhaustive]
pub struct ProxyHandle {
    sock_path: PathBuf,
    sock_dir: PathBuf,
    task: JoinHandle<()>,
}
```

```rust
#[cfg(unix)]
impl Drop for ProxyHandle {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.sock_path);
        // Remove the per-run directory too (best-effort; only succeeds once the
        // socket file inside it is gone).
        let _ = std::fs::remove_dir(&self.sock_dir);
    }
}
```

Rewrite `make_temp_sock_path` into a dir-creating helper and update `spawn_proxy`. Replace the `0o666`-on-socket chmod with `0o700`-on-dir:

```rust
#[cfg(unix)]
pub fn spawn_proxy(allowed_hosts: Vec<String>) -> std::io::Result<ProxyHandle> {
    let (sock_dir, sock_path) = make_run_dir_and_sock_path()?;
    let listener = UnixListener::bind(&sock_path)?;
    let task = tokio::spawn(accept_loop(listener, allowed_hosts));
    Ok(ProxyHandle { sock_path, sock_dir, task })
}

/// Create a private per-run directory (mode `0o700`) in the system temp dir
/// and return `(dir, socket_path)`. The socket itself is left at the OS
/// default mode: the `0o700` directory is the access boundary, so no other
/// local user can traverse to the socket, while the container bridge reaches
/// it through a bind-mount of the inode (unaffected by directory perms) and
/// the native bridge runs as the same host user (DAC-permitted). This mirrors
/// the ssh-agent / gpg-agent socket-in-a-private-dir pattern and replaces the
/// former world-writable (`0o666`) socket in shared `/tmp` (audit S6).
#[cfg(unix)]
fn make_run_dir_and_sock_path() -> std::io::Result<(PathBuf, PathBuf)> {
    use std::os::unix::fs::DirBuilderExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut dir = std::env::temp_dir();
    dir.push(format!("tau-proxy-{}-{}", std::process::id(), n));
    // Clean any stale dir from a prior aborted run, then create fresh at 0o700.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    let sock_path = dir.join("proxy.sock");
    Ok((dir, sock_path))
}
```

Delete the old `make_temp_sock_path` and the `use std::os::unix::fs::PermissionsExt;` line + the `set_permissions(... 0o666)` call and its now-stale comment in `spawn_proxy`.

- [ ] **Step 4: Run the new + existing lifecycle tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test -p tau-sandbox-proxy`
Expected: PASS — including `socket_lives_in_private_0700_dir` and the existing `proxy_handle_drop_unlinks_socket_file` (which still holds: `remove_file` then `remove_dir`).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-sandbox-proxy/src/lib.rs docs/superpowers/plans/2026-06-11-sandbox-proxy-hardening.md
git commit -m "fix(tau-sandbox-proxy): place egress-proxy socket in private 0700 dir (audit S6)"
```

---

### Task 2: O3 — log splice errors with host + byte counts

**Files:**
- Modify: `crates/tau-sandbox-proxy/src/lib.rs` (`handle_connect` tail, `handle_http` tail, new `splice_bidirectional` helper)
- Modify: `crates/tau-sandbox-proxy/Cargo.toml` (dev-dep `tracing-subscriber`)
- Test: same lib.rs, `proxy_lifecycle_tests` module

- [ ] **Step 1: Add the dev-dependency**

In `crates/tau-sandbox-proxy/Cargo.toml` under `[dev-dependencies]`:

```toml
tracing-subscriber = { workspace = true }
```

- [ ] **Step 2: Write the failing test (capturing subscriber over a real splice)**

Add to `mod proxy_lifecycle_tests` a small capturing Layer plus a deterministic clean-transfer test that asserts the splice emits a `tau::proxy` debug event carrying `host` and `bytes`:

```rust
use std::sync::{Arc, Mutex};
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;

#[derive(Clone, Default)]
struct CapturedEvent {
    target: String,
    message: String,
    host: Option<String>,
    bytes: Option<u64>,
}

#[derive(Default)]
struct CaptureVisitor {
    ev: CapturedEvent,
}
impl Visit for CaptureVisitor {
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "bytes" {
            self.ev.bytes = Some(value);
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let v = format!("{value:?}");
        match field.name() {
            "message" => self.ev.message = v.trim_matches('"').to_string(),
            "host" => self.ev.host = Some(v.trim_matches('"').to_string()),
            _ => {}
        }
    }
}

#[derive(Clone, Default)]
struct CaptureLayer {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}
impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut v = CaptureVisitor::default();
        v.ev.target = event.metadata().target().to_string();
        event.record(&mut v);
        self.events.lock().unwrap().push(v.ev);
    }
}

#[tokio::test]
async fn splice_emits_debug_with_host_and_bytes() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let cap = CaptureLayer::default();
    let events = cap.events.clone();
    let subscriber = tracing_subscriber::registry()
        .with(cap.with_filter(tracing_subscriber::filter::LevelFilter::DEBUG));

    tracing::subscriber::with_default(subscriber, || {
        // run the whole exchange inside this subscriber scope
    });

    // Drive a real plaintext-HTTP exchange through the proxy to a loopback
    // upstream so both copy directions complete cleanly and log at debug.
    let exchange = async {
        // Upstream responder on 127.0.0.1: read the request, reply, close.
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = upstream.local_addr().unwrap().port();
        let up = tokio::spawn(async move {
            let (mut s, _) = upstream.accept().await.unwrap();
            let mut b = [0u8; 1024];
            let _ = s.read(&mut b).await.unwrap();
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi").await.unwrap();
            // drop -> clean close, EOF on the remote->client copy
        });
        let h = spawn_proxy(vec!["127.0.0.1".to_string()]).expect("spawn");
        let mut conn = UnixStream::connect(h.sock_path()).await.expect("connect");
        let req = format!("GET http://127.0.0.1:{port}/ HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
        conn.write_all(req.as_bytes()).await.unwrap();
        let mut resp = Vec::new();
        let _ = conn.read_to_end(&mut resp).await; // read until proxy closes
        up.await.unwrap();
    };

    // Re-run the exchange *inside* the subscriber scope.
    let subscriber = tracing_subscriber::registry()
        .with(CaptureLayer { events: events.clone() }
            .with_filter(tracing_subscriber::filter::LevelFilter::DEBUG));
    let _g = tracing::subscriber::set_default(subscriber);
    exchange.await;
    // Give the proxy's spawned task a beat to finish the splice + log.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let evs = events.lock().unwrap();
    let splice = evs.iter().find(|e| e.target == "tau::proxy" && e.host.as_deref() == Some("127.0.0.1") && e.bytes.is_some());
    assert!(splice.is_some(), "expected a tau::proxy splice debug event with host+bytes, got: {:?}", *evs);
}
```

> Note: the first `with_default(..., || {})` stub above is illustrative; the real test uses the `set_default` guard form shown in the second half. When implementing, keep only the `set_default`-guard version (drop the empty `with_default` block).

- [ ] **Step 3: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test -p tau-sandbox-proxy splice_emits_debug_with_host_and_bytes`
Expected: FAIL — no `tau::proxy` event with `bytes` is emitted yet (current code discards the `try_join!` result).

- [ ] **Step 4: Implement the shared splice helper + log on both directions**

Add the helper near the splice sites:

```rust
/// Splice both directions of a proxied connection and log each direction's
/// outcome under target `tau::proxy`. Uses `tokio::join!` (not `try_join!`) so
/// a failure in one direction does not discard the other direction's byte
/// count — both data-path outcomes are always recorded (audit O3). On success
/// the transferred byte count is logged at debug; on error a `warn!` carries
/// the host and the io error so a mid-stream truncation is no longer silent.
#[cfg(unix)]
async fn splice_bidirectional<CR, CW, RR, RW>(
    mut client_r: CR,
    mut client_w: CW,
    mut remote_r: RR,
    mut remote_w: RW,
    host: &str,
) where
    CR: AsyncReadExt + Unpin,
    CW: AsyncWriteExt + Unpin,
    RR: AsyncReadExt + Unpin,
    RW: AsyncWriteExt + Unpin,
{
    let (up, down) = tokio::join!(
        tokio::io::copy(&mut client_r, &mut remote_w),
        tokio::io::copy(&mut remote_r, &mut client_w),
    );
    match up {
        Ok(bytes) => tracing::debug!(
            target: "tau::proxy", host = %host, bytes, direction = "client->remote",
            "proxy splice direction complete"
        ),
        Err(e) => tracing::warn!(
            target: "tau::proxy", host = %host, error = %e, direction = "client->remote",
            "proxy splice direction failed mid-stream"
        ),
    }
    match down {
        Ok(bytes) => tracing::debug!(
            target: "tau::proxy", host = %host, bytes, direction = "remote->client",
            "proxy splice direction complete"
        ),
        Err(e) => tracing::warn!(
            target: "tau::proxy", host = %host, error = %e, direction = "remote->client",
            "proxy splice direction failed mid-stream"
        ),
    }
}
```

In `handle_connect`, replace lines `198-201`:

```rust
    let (pr, pw) = plugin_sock.split();
    let (rr, rw) = remote.split();
    splice_bidirectional(pr, pw, rr, rw, &req.host).await;
    Ok(())
```

In `handle_http`, replace lines `279-282`:

```rust
    let (pr, pw) = plugin_sock.split();
    let (rr, rw) = remote.split();
    splice_bidirectional(pr, pw, rr, rw, &req.host).await;
    Ok(())
```

(`AsyncReadExt`/`AsyncWriteExt` are already imported at the top of the file.)

- [ ] **Step 5: Run the test to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test -p tau-sandbox-proxy`
Expected: PASS — `splice_emits_debug_with_host_and_bytes` plus all pre-existing tests.

- [ ] **Step 6: Verification — capture a real error-path warn**

O3's core is the *error* branch. Capture real output: run the suite with the proxy logs visible and a forced mid-stream client drop, confirming a `warn!` with `host` is emitted. Document the captured line in the PR body (verification-before-completion).

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main RUST_LOG=tau::proxy=debug cargo test -p tau-sandbox-proxy -- --nocapture`
Expected: real `tau::proxy` debug/warn lines visible in output.

- [ ] **Step 7: Commit**

```bash
git add crates/tau-sandbox-proxy/src/lib.rs crates/tau-sandbox-proxy/Cargo.toml
git commit -m "fix(tau-sandbox-proxy): log byte-splice errors with host + byte counts (audit O3)"
```

---

### Task 3: Final verification + clippy/fmt

- [ ] **Step 1: clippy clean**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo clippy -p tau-sandbox-proxy --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 2: fmt check**

Run: `timeout 30 env CARGO_TARGET_DIR=target/main cargo fmt -p tau-sandbox-proxy -- --check`
Expected: clean.

- [ ] **Step 3: full proxy test suite green**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test -p tau-sandbox-proxy`
Expected: all pass.

- [ ] **Step 4: requesting-code-review (scope check), then push + PR**

Cite S6 and O3; confirm no allowlist/CONNECT/HTTP-filter logic touched. `gh pr create -R tau-rs/tau --base main`. STOP — no merge.
