# Sandbox proxy: HTTP-path port containment (S5)

**Date:** 2026-06-11
**Finding:** `audit/security.md` S5 — *Sandbox proxy HTTP path enforces neither
destination port nor TLS (asymmetric with CONNECT)* (Medium, security).
**Scope:** First increment of the egress-containment cluster. S4 (serve-mode
sandboxing) is a deliberately separate follow-up cycle, listed in the PR body.

## Problem

`tau-sandbox-proxy` is the userspace egress filter for sandboxed plugins. A
sandboxed plugin's traffic reaches the proxy over a Unix socket as either:

- an HTTP `CONNECT host:port` request (HTTPS tunnel), or
- a plain HTTP request (`GET http://host/path HTTP/1.1`).

The two paths enforce **asymmetric** policy today
(`crates/tau-sandbox-proxy/src/lib.rs`):

| Check | `handle_connect` (HTTPS) | `handle_http` (plaintext) |
|---|---|---|
| Host in allowlist | yes | yes |
| Destination port | **must be 443** | **any port accepted** |
| TLS SNI matches host | **yes (pinned)** | n/a (plaintext) |

So a sandboxed plugin can open a plaintext channel to an allowlisted host on
**any** port — e.g. `http://allowed.example.com:22` or `:6379` — reaching
arbitrary services behind the hostname. The CONNECT path's `port != 443 → 400`
restriction has no plaintext analog. This is weaker egress containment than the
HTTPS path implies.

## Decision

Add a port gate to `handle_http` that mirrors CONNECT's `port != 443 → 400`,
adapted for plaintext semantics, while preserving the one legitimate
multi-port plaintext flow: **loopback local servers** (e.g. a local model
server like `http://127.0.0.1:11434`). `validate.rs` already encodes loopback
(`127.0.0.1` / `::1`) as the *only* permitted IP literals, so loopback is the
established escape hatch for local-only traffic.

Policy for the plaintext HTTP path, applied **after** the existing host
allowlist check:

- **Loopback host** (`127.0.0.1`, `::1`) → any port allowed.
- **Any other host** → destination port must be **80** (the plaintext analog of
  CONNECT's 443). Otherwise reply `HTTP/1.1 400 Bad Request\r\n\r\n` and return,
  exactly matching CONNECT's rejection shape and status.

Rationale:

- Closes "arbitrary-port reach to allowlisted hosts" for remote hosts: a remote
  allowlisted host is now reachable over plaintext only on the well-known HTTP
  port, structurally symmetric with CONNECT only reaching 443.
- Preserves already-permitted traffic: remote plaintext on port 80 still works,
  and loopback local servers on arbitrary ports still work.
- No TLS verification is added — the path is plaintext by definition. The
  containment guarantee on this path is "allowlisted host, well-known port (or
  loopback)", not confidentiality.

### Explicitly out of scope (deferred)

- **Full `host:port` allowlist granularity.** The finding also notes the
  allowlist is exact-hostname with no port component. Expressing per-entry
  `host:port` requires a port field on `tau_domain::NetCapability::Http`, with
  ripple through the capability JSON schema, the lockfile, `validate_hosts`,
  and docs. That is a larger capability-model change, not part of this cluster.
  This increment makes the plaintext path *no weaker than* CONNECT; per-port
  allowlisting is a future enhancement.
- **S4 — serve-mode sandboxing.** Separate superpowers cycle after this merges.

## Implementation

Single file: `crates/tau-sandbox-proxy/src/lib.rs`.

Add a small pure helper so the loopback decision is unit-testable without a
tokio runtime and shares one definition:

```rust
/// Plaintext HTTP egress is restricted to the well-known HTTP port for
/// remote hosts (mirroring CONNECT's 443-only rule); loopback hosts may use
/// any port so local servers (e.g. a local model server) keep working.
fn http_port_allowed(host: &str, port: u16) -> bool {
    if is_loopback_host(host) {
        return true;
    }
    port == 80
}
```

`is_loopback_host` parses the host as an `IpAddr` and returns
`ip.is_loopback()` (covers `127.0.0.0/8` and `::1`); non-IP hostnames are not
loopback. This matches `validate.rs`'s loopback semantics.

In `handle_http`, after the `allowed_hosts` membership check and before opening
the upstream TCP connection, insert:

```rust
if !http_port_allowed(&req.host, req.port) {
    plugin_sock
        .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
        .await?;
    return Ok(());
}
```

Emit a structured `tracing::warn!` on rejection (host, port), consistent with
the crate's existing `tracing::warn!` use on the accept/connection-failure
paths and the brief's "structured tracing, typed errors" constraint.

## Testing (TDD — failing tests first)

Unit tests (pure, no runtime) on `http_port_allowed` / `is_loopback_host`:

- remote host, port 80 → allowed
- remote host, port 8080 → rejected
- remote host, port 443 → rejected (plaintext on 443 is not a normal flow)
- `127.0.0.1`, arbitrary port → allowed
- `::1`, arbitrary port → allowed

Async lifecycle tests (mirroring the existing `proxy_lifecycle_tests`):

- `http_non_loopback_non_80_port_returns_400`: `GET http://allowed.example.com:8080/`
  against an allowlist containing `allowed.example.com` → `HTTP/1.1 400`.
  (This is the FAILING test that proves the gap before the fix.)
- `http_loopback_arbitrary_port_allowed`: a request to `127.0.0.1:<port>` on a
  closed port reaches the upstream-connect path and returns `502` (not `400`),
  proving the port gate did not reject it.

Existing tests (`http_forbidden_host_returns_403`, `http_malformed_returns_400`,
the CONNECT-path tests) must stay green — the host and parse checks are
unchanged and run before the new port gate.

## Verification

`cargo nextest run -p tau-sandbox-proxy` green; `cargo clippy -p tau-sandbox-proxy`
clean. (Per CARGO RULES: `CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main`,
`timeout`, `-p`.)
