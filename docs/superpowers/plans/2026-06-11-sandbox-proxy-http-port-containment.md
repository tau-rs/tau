# Sandbox proxy HTTP-path port containment (S5) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Mirror the CONNECT path's port restriction on the proxy's plaintext HTTP path so an allowlisted remote host is reachable over plaintext only on port 80, while loopback hosts keep any-port access for local servers.

**Architecture:** Add two small pure helpers (`is_loopback_host`, `http_port_allowed`) to `crates/tau-sandbox-proxy/src/lib.rs` and insert a port gate in `handle_http` after the existing host-allowlist check, returning `400` exactly as `handle_connect` does for `port != 443`.

**Tech Stack:** Rust, tokio, `std::net::IpAddr`, `tracing`.

---

### Task 1: Port gate on the plaintext HTTP path

**Files:**
- Modify: `crates/tau-sandbox-proxy/src/lib.rs` (add helpers near the `#[cfg(unix)]` block; insert gate in `handle_http` at lib.rs:220-225 region, after the allowlist check, before the upstream `TcpStream::connect`)
- Test: `crates/tau-sandbox-proxy/src/lib.rs` — pure-helper unit tests in a new `#[cfg(test)] mod port_gate_tests`; async lifecycle tests appended to the existing `proxy_lifecycle_tests` module

Per CARGO RULES, every cargo command below uses:
`timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo <cmd> -p tau-sandbox-proxy`

- [ ] **Step 1: Write the failing async lifecycle test**

Append to `mod proxy_lifecycle_tests` (after `http_malformed_returns_400`):

```rust
    #[tokio::test]
    async fn http_non_loopback_non_80_port_returns_400() {
        let h = spawn_proxy(vec!["allowed.example.com".to_string()]).expect("spawn");
        let mut conn = UnixStream::connect(h.sock_path()).await.expect("connect");
        // Allowlisted host, but plaintext on a non-80 port: must be rejected,
        // mirroring CONNECT's non-443 -> 400.
        conn.write_all(
            b"GET http://allowed.example.com:8080/ HTTP/1.1\r\nHost: allowed.example.com:8080\r\n\r\n",
        )
        .await
        .expect("write");
        let mut resp = [0u8; 256];
        let n = conn.read(&mut resp).await.expect("read");
        let s = std::str::from_utf8(&resp[..n]).expect("utf8");
        assert!(s.starts_with("HTTP/1.1 400"), "got: {s}");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-sandbox-proxy http_non_loopback_non_80_port_returns_400`

Expected: FAIL. Without the gate, the proxy tries to open an upstream TCP
connection to `allowed.example.com:8080`, so the response is `502 Bad Gateway`
(or a connect hang/timeout), not `400` — the assertion `starts_with("HTTP/1.1 400")` fails.

- [ ] **Step 3: Add the pure helpers**

Add immediately above `async fn handle_http` (still inside the `#[cfg(unix)]` region; place the `use` at the top of the helper or fully-qualify `std::net::IpAddr`):

```rust
/// True if `host` is an IP literal that is a loopback address
/// (`127.0.0.0/8` or `::1`). Non-IP hostnames are never loopback.
/// Matches the loopback semantics of `validate::validate_hosts`.
#[cfg(unix)]
fn is_loopback_host(host: &str) -> bool {
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// Plaintext HTTP egress port policy. Mirrors `handle_connect`'s 443-only
/// rule: a remote (non-loopback) host may only be reached on the well-known
/// HTTP port 80; loopback hosts may use any port so local servers (e.g. a
/// local model server on `http://127.0.0.1:11434`) keep working.
#[cfg(unix)]
fn http_port_allowed(host: &str, port: u16) -> bool {
    is_loopback_host(host) || port == 80
}
```

- [ ] **Step 4: Insert the gate in `handle_http`**

In `handle_http`, immediately after the existing forbidden-host block
(the `if !allowed_hosts.iter().any(|h| h == &req.host) { ... return Ok(()); }`)
and before the `// Open TCP to the destination host:port.` comment, insert:

```rust
    // Mirror handle_connect's port restriction on the plaintext path: a
    // remote allowlisted host is reachable over plaintext only on port 80;
    // loopback hosts may use any port (local servers).
    if !http_port_allowed(&req.host, req.port) {
        tracing::warn!(
            host = %req.host,
            port = req.port,
            "proxy rejected plaintext HTTP to non-80 port on non-loopback host"
        );
        plugin_sock
            .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
            .await?;
        return Ok(());
    }
```

- [ ] **Step 5: Run the failing test to verify it now passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-sandbox-proxy http_non_loopback_non_80_port_returns_400`

Expected: PASS.

- [ ] **Step 6: Add the loopback-preserved async test + pure unit tests**

Append to `mod proxy_lifecycle_tests`:

```rust
    #[tokio::test]
    async fn http_loopback_arbitrary_port_not_rejected_by_port_gate() {
        let h = spawn_proxy(vec!["127.0.0.1".to_string()]).expect("spawn");
        let mut conn = UnixStream::connect(h.sock_path()).await.expect("connect");
        // Loopback host on an arbitrary (closed) port: the port gate must NOT
        // reject it. It reaches the upstream-connect path, which fails to dial
        // the closed port and returns 502 — proving the gate let it through.
        conn.write_all(
            b"GET http://127.0.0.1:9/ HTTP/1.1\r\nHost: 127.0.0.1:9\r\n\r\n",
        )
        .await
        .expect("write");
        let mut resp = [0u8; 256];
        let n = conn.read(&mut resp).await.expect("read");
        let s = std::str::from_utf8(&resp[..n]).expect("utf8");
        assert!(s.starts_with("HTTP/1.1 502"), "expected 502 not 400, got: {s}");
    }
```

Add a new test module (place it next to `proxy_lifecycle_tests`):

```rust
#[cfg(unix)]
#[cfg(test)]
mod port_gate_tests {
    use super::{http_port_allowed, is_loopback_host};

    #[test]
    fn remote_host_port_80_allowed() {
        assert!(http_port_allowed("allowed.example.com", 80));
    }

    #[test]
    fn remote_host_non_80_rejected() {
        assert!(!http_port_allowed("allowed.example.com", 8080));
        assert!(!http_port_allowed("allowed.example.com", 443));
    }

    #[test]
    fn loopback_any_port_allowed() {
        assert!(http_port_allowed("127.0.0.1", 11434));
        assert!(http_port_allowed("::1", 9000));
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("::1"));
        assert!(!is_loopback_host("8.8.8.8"));
        assert!(!is_loopback_host("example.com"));
    }
}
```

- [ ] **Step 7: Run the full crate test suite**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-sandbox-proxy`

Expected: PASS — all new tests plus the pre-existing CONNECT/HTTP/parse tests
(`http_forbidden_host_returns_403`, `http_malformed_returns_400`,
`non_443_port_returns_400`, etc.) stay green.

- [ ] **Step 8: Clippy clean**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo clippy -p tau-sandbox-proxy --all-targets`

Expected: no warnings.

- [ ] **Step 9: Commit**

```bash
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -am "fix(sandbox-proxy): restrict plaintext HTTP egress to port 80 for non-loopback hosts

The plaintext HTTP path validated only the host allowlist and accepted any
destination port, while the CONNECT path restricts to 443. A sandboxed
plugin could open a plaintext channel to an allowlisted host on any port
(e.g. :22, :6379). Mirror CONNECT's port rule: non-loopback hosts are
reachable over plaintext only on port 80; loopback hosts keep any-port
access for local servers. Full host:port allowlist granularity is deferred
(needs a port field on NetCapability::Http).

Finding: audit/security.md S5.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Deferred (not in this plan)

- **S4 — serve mode runs plugins unsandboxed.** Separate superpowers cycle
  after this PR merges. Apply the CLI `plugin_loader` adapter-resolution path
  (`resolve_adapter` + per-plugin `build_plan`) inside `tau-app`'s serve
  `build_runtime`, or gate serve startup behind an explicit `--no-sandbox`.
- **Full `host:port` allowlist granularity.** Needs a port field on
  `tau_domain::NetCapability::Http` plus schema/lockfile/docs ripple.
