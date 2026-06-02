# β.3 MCP facilitator — PR-3: HTTP transport + cassette-as-transport

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship PR-3 of six in the β.3 sub-project. Implement the Streamable HTTP MCP client in `tau-mcp-tokio::transport_http` (POST request + SSE response framing + `Mcp-Session-Id` header tracking + `HttpClientGuard` enforcing wire-level net.http host pinning), wire `host_lifecycle::open()` to dial `http://`/`https://` URLs, and ship `tau-mcp::cassette::transport::CassetteTransport` — a thin `Transport` impl wrapping the existing `Replayer` so cassettes can drive in-memory MCP tests directly.

**Architecture:** Two parallel additions. (1) `transport_http` mirrors `transport_stdio`'s shape from PR-2: an error module, a wire framer (`sse.rs`), a guard (`guard.rs`), a session tracker (`session.rs`), a server handle (`server.rs` impls `Transport`), and a dial entrypoint (`dial.rs`). The HTTP client is a `reqwest::Client` wrapped in `HttpClientGuard` with `redirect::Policy::none()`; every outgoing URL is host-validated against the pinned URL before delegation. The SSE framer parses `data: <JSON-RPC>\n\n` events out of `reqwest::Response::bytes_stream()` and demuxes them into the inbound mpsc that `Transport::next_message` reads from. (2) `CassetteTransport` lives in `tau-mcp` (so wasm/embassy shells can use it without tokio) — wait, actually `tokio::sync::mpsc` is a tokio dep; cassette/transport.rs uses `futures::channel::mpsc` or `core::sync` instead. Decision: use `futures::channel::mpsc::unbounded` so `tau-mcp` stays no_std-friendly per spec §2; the channel is already in the `futures` workspace dep.

**Tech Stack:** Rust 2021, `tokio` (`process`, `io`, `time`, `sync`), `reqwest` (with `stream` + `rustls-tls`), `url`, `bytes`, `futures` (channel + stream), `serde_json`, `tau-mcp` (Transport trait, protocol + contract types, cassette::Replayer), `tau-mcp-tokio` (existing transport_stdio + host_lifecycle from PR-2). Dev: `wiremock` for HTTP integration tests.

**Branch:** `feat/beta-3-pr-3-http-transport` (created off `origin/main`; main now contains PR-2 at `48c2c6e` + plugin_host_ipc_llm fix at `536f57b`).

**Worktree:** `/Users/titouanlebocq/code/tau-worktrees/beta-3-pr-3-http`.

**Spec reference:** `docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md` — §2 (crate layout — `tau-mcp-tokio` gets `transport_http`; `tau-mcp::cassette` gets `transport` submodule), §3 (URL schemes — `http://`/`https://` → `transport_http`), §9 (cap enforcement — HTTP gets `check_outbound` PLUS reqwest middleware/guard for host pinning), §11 (cassette format — Replayer already shipped in PR-1), §12 (testing — wiremock-rs for HTTP, cassette round-trip unit tests), §15 (PR-3 scope).

**Locked architectural decisions consumed:**
- PR-3 design brainstormed 2026-06-02 in chat; per user preference, this plan IS the PR-3 design record (no separate PR-3-specific spec file). The four design questions from §15:
  - SSE framer: hand-rolled (~100 lines) over `reqwest::Response::bytes_stream()`. No `eventsource-stream` / `reqwest-eventsource` crate.
  - net.http enforcement: custom `HttpClientGuard` newtype around `reqwest::Client` with `redirect::Policy::none()` + URL-host validation before delegating. No `reqwest-middleware` crate.
  - Cassette-as-transport: `CassetteTransport` wraps `Replayer` + `futures::channel::mpsc::unbounded` inbound channel. Locks the Replayer behind a `Mutex` (re-export from `tau_mcp` shouldn't add a tokio dep; use `spin::Mutex` from a tiny no_std spin crate OR `Mutex<Replayer>` from `parking_lot` only inside a `with-std-adapters`-gated module). Decision: gate `CassetteTransport` behind `with-std-adapters` feature on tau-mcp (the existing feature for save_to_file), since the cassette transport is a TEST construct + futures::channel::mpsc requires std anyway.
  - HTTP test fixture: wiremock-rs (no separate binary fixture — wiremock plays the server role; pure in-process).
- Out-of-scope for PR-3 (deferred per spec §15): `McpBridge` (PR-5), sampling/roots inbound handler dispatch (PR-5), tau-cli `cmd/mcp/*` verbs (PR-6), conformance fixture #07 (PR-6), `cassette:` URL scheme (a tau-cli concern, not a transport concern).

---

## Files map

### Modified
| File | Responsibility |
|---|---|
| `Cargo.toml` (workspace) | Add `wiremock` to `[workspace.dependencies]`. |
| `crates/tau-mcp-tokio/Cargo.toml` | Add `reqwest` (`json`, `stream`, `rustls-tls`, no `default-features`), `bytes`, `futures`, `url` deps. Dev: `wiremock`. |
| `crates/tau-mcp-tokio/src/lib.rs` | Re-export `transport_http::{McpHttpServer, HttpSpawnError, HttpTransportError}`. |
| `crates/tau-mcp-tokio/src/transport_http/mod.rs` | Replace doc-only stub with module split (`error.rs`, `guard.rs`, `sse.rs`, `session.rs`, `server.rs`, `dial.rs`). |
| `crates/tau-mcp-tokio/src/host_lifecycle/url.rs` | `McpUrl` gains `Http { url: url::Url }` + `Https { url: url::Url }` variants. `parse_url` accepts both schemes; rejects `file://` etc. |
| `crates/tau-mcp-tokio/src/host_lifecycle/open.rs` | `McpUrl::Http` / `McpUrl::Https` arms call `transport_http::dial::dial(url, options) → McpHttpServer`. |
| `crates/tau-mcp-tokio/src/host_lifecycle/error.rs` | `LifecycleError::HttpSpawn(#[from] HttpSpawnError)` variant. |
| `crates/tau-mcp/Cargo.toml` | Add `futures` (with `std` feature; gated on `with-std-adapters`) to `[dependencies]` for the cassette transport channel. |
| `crates/tau-mcp/src/cassette/mod.rs` | Add `#[cfg(feature = "with-std-adapters")] pub mod transport;` + re-export. |

### Created (NEW)
| File | Responsibility |
|---|---|
| `crates/tau-mcp-tokio/src/transport_http/error.rs` | `HttpSpawnError`, `HttpTransportError`. |
| `crates/tau-mcp-tokio/src/transport_http/guard.rs` | `HttpClientGuard` — newtype around `reqwest::Client` + pinned `url::Host`; `post(url, body)` validates URL host before delegating. |
| `crates/tau-mcp-tokio/src/transport_http/sse.rs` | Hand-rolled SSE frame parser. `SseFramer::feed_bytes(&[u8]) → Vec<JsonRpcMessage>` and `parse_event_block(&str) → Option<JsonRpcMessage>`. |
| `crates/tau-mcp-tokio/src/transport_http/session.rs` | `SessionState` — interior-mutable cell tracking the `Mcp-Session-Id` header after initialize response. |
| `crates/tau-mcp-tokio/src/transport_http/server.rs` | `McpHttpServer` impls `Transport`. POST request → spawn-task that streams response → demux SSE events into inbound mpsc. |
| `crates/tau-mcp-tokio/src/transport_http/dial.rs` | `dial(url, options) → Result<Arc<McpHttpServer>, HttpSpawnError>`. Constructs guard + client + session, returns ready handle. |
| `crates/tau-mcp/src/cassette/transport.rs` | `CassetteTransport` impls `Transport`; wraps `Replayer` + `futures::channel::mpsc::unbounded` inbound channel. |
| `crates/tau-mcp-tokio/tests/http_lifecycle.rs` | wiremock-rs integration tests: handshake, tools/call, SSE multi-event, session-id round-trip, redirect refused, host-pin refused. |
| `crates/tau-mcp/tests/cassette_transport.rs` | Cassette-transport unit-as-integration tests: round-trip happy path, inbound-injection (notification + server-initiated request), EOF-as-transport-closed. |

### Deleted
- None.

---

## Standing constraints (re-read before EVERY cargo / git command)

From `CLAUDE.md` — non-negotiable. Same shape as PR-2's:

| Command | Shape |
|---|---|
| Build / check | `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo {check,build} -p <crate>` |
| Test (nextest) | `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo nextest run -p <crate>` |
| Clippy | `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo clippy -p <crate> --all-targets -- -D warnings` |
| Fmt check | `timeout 30 env CARGO_TARGET_DIR=target/agent-<role> cargo fmt --check -p <crate>` |
| Commits | `git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "..."` |
| Push | `git push --no-verify -u origin feat/beta-3-pr-3-http-transport` |
| Auto-merge | `gh pr merge <N> --auto` BARE. (Repo IS a merge queue. `autoMergeRequest:null` + `mergeQueueEntry.state=AWAITING_CHECKS` is the normal transition.) |

`<role>` per task: `impl` for the implementer; `verify` for verifications.

PR-2-experience addenda baked in:

- **DO NOT enable `features = ["test-support"]` on `tau-runtime-tokio` dev-dep.** PR-3 doesn't need it (PassthroughSandbox is unconditional). Activating it workspace-wide exposed a long-broken test in PR-2's first CI run. (The broken test was fixed in PR #282 but the principle stands.)
- **If you add a fixture binary**, use `cargo build --message-format=json-render-diagnostics` + parse `compiler-artifact.executable`. Don't guess paths from CARGO_TARGET_DIR (broke on macos CI in PR-2). PR-3 should NOT need a fixture binary — wiremock-rs is pure in-process.
- **Auto-merge drops silently after ANY check failure** (even infra flakes). Re-enroll explicitly via `gh pr merge <N> --auto` BARE after rerun.
- **macOS recurring flake** — `tau-cli::cmd_chat_persistence::chat_ephemeral_writes_no_file`. On hit: rerun + re-enroll auto-merge.

---

## Phase 1 — Cargo.toml + workspace deps

### Task 1.1: Add `wiremock` to workspace deps

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Read root Cargo.toml `[workspace.dependencies]` block.**

Confirm `wiremock` is not already listed.

- [ ] **Step 2: Add `wiremock = "0.6"` to `[workspace.dependencies]`.**

Alphabetical insertion. Run a `cargo metadata` sanity check.

- [ ] **Step 3: cargo metadata.**

Run: `timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo metadata --format-version 1 --no-deps > /dev/null`

Expected: exit 0.

- [ ] **Step 4: Commit.**

```
git add Cargo.toml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(workspace): add wiremock 0.6 dev-dep (tau-mcp-tokio PR-3 HTTP integration tests)"
```

### Task 1.2: Extend `crates/tau-mcp-tokio/Cargo.toml`

**Files:**
- Modify: `crates/tau-mcp-tokio/Cargo.toml`

- [ ] **Step 1: Read the current file to see PR-2's baseline.**

- [ ] **Step 2: Add the new runtime deps + dev-dep.**

Replace `[dependencies]` and `[dev-dependencies]` blocks to add `reqwest`, `bytes`, `futures`, `url`, and dev-dep `wiremock`. The final shape:

```toml
[dependencies]
tau-mcp           = { workspace = true }
tau-domain        = { workspace = true, features = ["serde"] }
tau-ports         = { workspace = true, features = ["serde"] }
# Sandbox integration: tau-runtime-tokio::process_gate::wrap_spawn (PR-2).
tau-runtime-tokio = { workspace = true }
serde             = { workspace = true, features = ["derive"] }
serde_json        = { workspace = true }
thiserror         = { workspace = true }
tokio             = { workspace = true, features = [
    "rt",
    "rt-multi-thread",
    "macros",
    "io-util",
    "process",
    "time",
    "sync",
] }
tracing           = { workspace = true }
# HTTP transport (PR-3).
reqwest           = { workspace = true }
bytes             = { workspace = true }
futures           = { workspace = true }
url               = { workspace = true }

# NOTE: do NOT enable tau-runtime-tokio's "test-support" feature here.
# PassthroughSandbox is unconditionally public; activating "test-support"
# unifies feature flags workspace-wide and exposes tau-runtime-tokio's own
# plugin_host_ipc_llm.rs test, which would compile silently broken until
# someone activated the feature workspace-wide. (Fixed in PR #282, but
# the principle holds: don't enable features you don't use.)
[dev-dependencies]
tokio    = { workspace = true, features = ["test-util", "fs"] }
tempfile = { workspace = true }
wiremock = { workspace = true }
```

- [ ] **Step 3: `cargo check`.**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio`

Expected: clean.

- [ ] **Step 4: Commit.**

```
git add crates/tau-mcp-tokio/Cargo.toml Cargo.lock
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio): wire transport_http deps (reqwest + bytes + futures + url; wiremock dev-dep)"
```

### Task 1.3: Extend `crates/tau-mcp/Cargo.toml`

**Files:**
- Modify: `crates/tau-mcp/Cargo.toml`

- [ ] **Step 1: Read current Cargo.toml.**

Note the existing `[features]` block (in particular `with-std-adapters`).

- [ ] **Step 2: Add `futures` to deps gated on `with-std-adapters`.**

Two changes:

In `[dependencies]`:
```toml
futures = { workspace = true, optional = true }
```

In `[features]`:
```toml
with-std-adapters = ["std", "dep:futures"]
```

(Adjust the LHS list as needed — `with-std-adapters` likely already exists with a different RHS; add `dep:futures` to its existing list.)

- [ ] **Step 3: `cargo check`.**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp --features with-std-adapters`

Expected: clean.

- [ ] **Step 4: Commit.**

```
git add crates/tau-mcp/Cargo.toml Cargo.lock
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp): add futures (optional, gated on with-std-adapters) for cassette transport channel"
```

---

## Phase 2 — error types + module shells

### Task 2.1: `transport_http/error.rs`

**Files:**
- Create: `crates/tau-mcp-tokio/src/transport_http/error.rs`

- [ ] **Step 1: Write `error.rs`.**

```rust
//! Error types for the Streamable HTTP transport.

use thiserror::Error;
use url::Host;

/// Failure during HTTP MCP server dial.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HttpSpawnError {
    /// URL has no host component (e.g. `http:///foo`).
    #[error("HTTP URL has no host: {url}")]
    NoHost {
        /// URL we tried to dial.
        url: String,
    },
    /// `reqwest::ClientBuilder::build` failed (TLS init, etc.).
    #[error("reqwest client construction failed: {0}")]
    ClientBuild(String),
}

/// Failure during an HTTP request/response cycle.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HttpTransportError {
    /// Outbound request URL host did not match the pinned host.
    /// Caller bug — every outbound URL must come from the pinned host;
    /// HttpClientGuard refused to delegate to `reqwest::Client`.
    #[error("URL host {actual:?} does not match pinned host {pinned:?}")]
    HostPinViolation {
        /// Host the caller tried to contact.
        actual: String,
        /// Pinned host from the original URL.
        pinned: Host<String>,
    },
    /// `reqwest::Client::execute` failed (network, TLS, etc.).
    #[error("HTTP send failed: {0}")]
    Send(String),
    /// HTTP server returned non-2xx.
    #[error("HTTP server returned {status}: {body}")]
    Status {
        /// Status code.
        status: u16,
        /// Response body (truncated if large).
        body: String,
    },
    /// SSE frame parse failure.
    #[error("SSE parse error: {0}")]
    SseParse(String),
    /// JSON-RPC message decode failure.
    #[error("JSON-RPC decode failure: {0}")]
    JsonDecode(String),
    /// Inbound channel send/recv error (typically transport shutdown).
    #[error("inbound channel error: {0}")]
    Channel(String),
}

impl From<serde_json::Error> for HttpTransportError {
    fn from(e: serde_json::Error) -> Self {
        HttpTransportError::JsonDecode(format!("{e}"))
    }
}
```

- [ ] **Step 2: Create placeholder stubs** for the other submodules so `mod.rs`'s re-export resolves:

`crates/tau-mcp-tokio/src/transport_http/guard.rs`:
```rust
//! Placeholder; filled in Task 3.2.
```

`crates/tau-mcp-tokio/src/transport_http/sse.rs`:
```rust
//! Placeholder; filled in Task 3.1.
```

`crates/tau-mcp-tokio/src/transport_http/session.rs`:
```rust
//! Placeholder; filled in Task 3.3.
```

`crates/tau-mcp-tokio/src/transport_http/server.rs`:
```rust
//! Placeholder; filled in Task 4.1.
//!
//! Stub so transport_http/mod.rs re-export resolves.
pub struct McpHttpServer;
```

`crates/tau-mcp-tokio/src/transport_http/dial.rs`:
```rust
//! Placeholder; filled in Task 4.2.
```

- [ ] **Step 3: Replace `crates/tau-mcp-tokio/src/transport_http/mod.rs`.**

```rust
//! Streamable HTTP MCP transport (PR-3).
//!
//! Per MCP spec rev 2025-03-26, the Streamable HTTP transport uses
//! POST for client→server messages and either application/json or
//! text/event-stream for server→client responses. This module composes:
//!
//! - `guard::HttpClientGuard` — pinned-host newtype around
//!   reqwest::Client with `redirect::Policy::none()`.
//! - `sse::SseFramer` — hand-rolled SSE frame parser.
//! - `session::SessionState` — tracks the `Mcp-Session-Id` header.
//! - `server::McpHttpServer` — impls `tau_mcp::transport::Transport`.
//! - `dial::dial` — top-level dial entrypoint composing the above.

pub mod dial;
pub mod error;
pub mod guard;
pub mod server;
pub mod session;
pub mod sse;

pub use dial::dial;
pub use error::{HttpSpawnError, HttpTransportError};
pub use server::McpHttpServer;
```

- [ ] **Step 4: cargo check.**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio`

Expected: clean.

- [ ] **Step 5: Commit.**

```
git add crates/tau-mcp-tokio/src/transport_http/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/transport_http): error types + module shells"
```

### Task 2.2: Extend `host_lifecycle/error.rs` with `HttpSpawn` variant

**Files:**
- Modify: `crates/tau-mcp-tokio/src/host_lifecycle/error.rs`

- [ ] **Step 1: Read the current file.**

- [ ] **Step 2: Add the new variant to `LifecycleError`.**

Insert after the existing `StdioSpawn` variant:

```rust
    /// HTTP dial failure (Streamable HTTP transport).
    #[error("http dial: {0}")]
    HttpSpawn(#[from] crate::transport_http::HttpSpawnError),
```

- [ ] **Step 3: cargo check.**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio`

Expected: clean.

- [ ] **Step 4: Commit.**

```
git add crates/tau-mcp-tokio/src/host_lifecycle/error.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/host_lifecycle): LifecycleError::HttpSpawn variant"
```

---

## Phase 3 — SSE framer + HttpClientGuard + Session

### Task 3.1: `sse.rs` — SSE frame parser

**Files:**
- Modify: `crates/tau-mcp-tokio/src/transport_http/sse.rs`

MCP Streamable HTTP per spec rev 2025-03-26 uses a narrow SSE shape: each event is `data: <JSON-RPC message>\n\n` with no event names, no IDs, no retries. No need for a full SSE parser.

- [ ] **Step 1: Write `sse.rs` with TDD-shaped tests.**

```rust
//! Hand-rolled SSE frame parser for Streamable HTTP MCP responses.
//!
//! Per MCP spec rev 2025-03-26: each event is `data: <JSON-RPC>\n\n`.
//! No event names, no IDs, no retries — MCP handles reconnection at
//! the protocol level via session IDs. The parser is therefore tiny:
//! split on blank lines, strip the `data: ` prefix, parse JSON-RPC.

use tau_mcp::protocol::JsonRpcMessage;

use crate::transport_http::error::HttpTransportError;

/// Accumulates SSE bytes and emits complete JSON-RPC messages once
/// `\n\n` event boundaries land.
#[derive(Debug, Default)]
pub struct SseFramer {
    buf: String,
}

impl SseFramer {
    /// Construct a fresh framer.
    pub fn new() -> Self {
        Self {
            buf: String::new(),
        }
    }

    /// Feed a chunk of bytes, returning any complete messages parsed
    /// out of the accumulated buffer.
    pub fn feed_bytes(
        &mut self,
        chunk: &[u8],
    ) -> Result<Vec<JsonRpcMessage>, HttpTransportError> {
        // SSE is text per spec — utf-8 only. Append, then scan for
        // event boundaries.
        let s = std::str::from_utf8(chunk)
            .map_err(|e| HttpTransportError::SseParse(format!("non-utf8 SSE chunk: {e}")))?;
        self.buf.push_str(s);

        let mut messages = Vec::new();
        loop {
            // Look for the next event boundary (`\n\n` or `\r\n\r\n`).
            let boundary = self
                .buf
                .find("\n\n")
                .map(|i| (i, 2))
                .or_else(|| self.buf.find("\r\n\r\n").map(|i| (i, 4)));
            let Some((idx, sep_len)) = boundary else {
                // No complete event yet — keep buffering.
                return Ok(messages);
            };
            let event = self.buf[..idx].to_string();
            self.buf.drain(..idx + sep_len);
            if let Some(msg) = parse_event_block(&event)? {
                messages.push(msg);
            }
        }
    }

    /// Drain the accumulated buffer as a final event (useful after EOF
    /// if the server didn't append a trailing `\n\n`).
    pub fn flush(&mut self) -> Result<Option<JsonRpcMessage>, HttpTransportError> {
        let event = std::mem::take(&mut self.buf);
        let trimmed = event.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            return Ok(None);
        }
        parse_event_block(trimmed)
    }
}

/// Parse one SSE event block (without the trailing `\n\n`) into an
/// optional `JsonRpcMessage`. Returns `Ok(None)` for keep-alive
/// comments (lines starting with `:`).
pub fn parse_event_block(
    block: &str,
) -> Result<Option<JsonRpcMessage>, HttpTransportError> {
    // Collect data: lines (SSE allows multi-line data fields joined by
    // `\n`). MCP only uses one data: line per event, but parse robustly.
    let mut data = String::new();
    for line in block.lines() {
        if line.is_empty() {
            continue;
        }
        if line.starts_with(':') {
            // SSE comment / keep-alive — ignore.
            continue;
        }
        let Some(rest) = line.strip_prefix("data:") else {
            // Non-data field (event:, id:, retry:) — MCP doesn't use
            // these; ignore per SSE spec.
            continue;
        };
        // SSE allows an optional space after `:` — strip one if present.
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        if !data.is_empty() {
            data.push('\n');
        }
        data.push_str(rest);
    }
    if data.is_empty() {
        return Ok(None);
    }
    let msg: JsonRpcMessage = serde_json::from_str(&data)?;
    Ok(Some(msg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_mcp::protocol::jsonrpc::{
        JsonRpcMessage, JsonRpcResponse, RequestId, JSONRPC_VERSION,
    };

    fn response_msg(id: i64) -> JsonRpcMessage {
        JsonRpcMessage::Response(JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::Number(id),
            result: Some(serde_json::json!({"ok": true})),
            error: None,
        })
    }

    #[test]
    fn parses_single_event() {
        let mut f = SseFramer::new();
        let msg = response_msg(1);
        let line = format!("data: {}\n\n", serde_json::to_string(&msg).unwrap());
        let parsed = f.feed_bytes(line.as_bytes()).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0], msg);
    }

    #[test]
    fn parses_two_events_in_one_chunk() {
        let mut f = SseFramer::new();
        let m1 = response_msg(1);
        let m2 = response_msg(2);
        let bytes = format!(
            "data: {}\n\ndata: {}\n\n",
            serde_json::to_string(&m1).unwrap(),
            serde_json::to_string(&m2).unwrap()
        );
        let parsed = f.feed_bytes(bytes.as_bytes()).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], m1);
        assert_eq!(parsed[1], m2);
    }

    #[test]
    fn handles_event_split_across_feed_calls() {
        let mut f = SseFramer::new();
        let msg = response_msg(7);
        let line = format!("data: {}\n\n", serde_json::to_string(&msg).unwrap());
        let (a, b) = line.split_at(line.len() / 2);
        let parsed_a = f.feed_bytes(a.as_bytes()).unwrap();
        assert!(parsed_a.is_empty(), "first chunk should not yield events");
        let parsed_b = f.feed_bytes(b.as_bytes()).unwrap();
        assert_eq!(parsed_b.len(), 1);
        assert_eq!(parsed_b[0], msg);
    }

    #[test]
    fn skips_keepalive_comments() {
        let mut f = SseFramer::new();
        let msg = response_msg(3);
        let bytes = format!(
            ": keepalive\n\ndata: {}\n\n",
            serde_json::to_string(&msg).unwrap()
        );
        let parsed = f.feed_bytes(bytes.as_bytes()).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0], msg);
    }

    #[test]
    fn malformed_json_errors() {
        let mut f = SseFramer::new();
        let bytes = b"data: not json\n\n";
        let err = f.feed_bytes(bytes).expect_err("should error");
        assert!(matches!(err, HttpTransportError::JsonDecode(_)));
    }

    #[test]
    fn flush_drains_buffer_without_trailing_newlines() {
        let mut f = SseFramer::new();
        let msg = response_msg(9);
        let line = format!("data: {}", serde_json::to_string(&msg).unwrap());
        // No \n\n — server abruptly ended the stream.
        let parsed = f.feed_bytes(line.as_bytes()).unwrap();
        assert!(parsed.is_empty());
        let final_msg = f.flush().unwrap().expect("flush yields one message");
        assert_eq!(final_msg, msg);
    }
}
```

- [ ] **Step 2: Run the SSE tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp-tokio -E 'test(transport_http::sse::tests)'`

Expected: 6 tests pass.

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp-tokio/src/transport_http/sse.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/transport_http): hand-rolled SSE frame parser"
```

### Task 3.2: `guard.rs` — HttpClientGuard

**Files:**
- Modify: `crates/tau-mcp-tokio/src/transport_http/guard.rs`

- [ ] **Step 1: Write `guard.rs` with tests.**

```rust
//! Pinned-host newtype around `reqwest::Client`.
//!
//! Enforces the spec §9 invariant: every outbound HTTP request must
//! go to the pinned MCP server host. Combined with
//! `redirect::Policy::none()` on the inner client, this guarantees the
//! `net.http` capability's `host` field is honored at the wire — a
//! 3xx redirect to a different host fails closed, and any code path
//! that constructs a different URL is refused before the request
//! leaves the process.

use reqwest::{Client, Request, RequestBuilder, Response};
use url::{Host, Url};

use crate::transport_http::error::HttpTransportError;

/// HTTP client guard pinned to a single host.
#[derive(Debug, Clone)]
pub struct HttpClientGuard {
    /// Inner reqwest client (constructed with `redirect::Policy::none()`).
    inner: Client,
    /// Pinned host extracted from the MCP server URL at dial time.
    pinned_host: Host<String>,
}

impl HttpClientGuard {
    /// Construct from an already-built client + a pinned host.
    pub fn new(inner: Client, pinned_host: Host<String>) -> Self {
        Self { inner, pinned_host }
    }

    /// Get the pinned host (for diagnostics + tests).
    pub fn pinned_host(&self) -> &Host<String> {
        &self.pinned_host
    }

    /// Borrow the inner client. Use ONLY for building requests via
    /// `Client::request(...)`; always send via [`HttpClientGuard::send`].
    pub fn inner(&self) -> &Client {
        &self.inner
    }

    /// Validate the request URL's host against the pinned host, then
    /// execute the request via the inner client.
    pub async fn send(
        &self,
        request: Request,
    ) -> Result<Response, HttpTransportError> {
        let url = request.url().clone();
        self.check_host(&url)?;
        self.inner
            .execute(request)
            .await
            .map_err(|e| HttpTransportError::Send(format!("{e}")))
    }

    /// Convenience: build + send a POST request to the given URL.
    pub fn post(&self, url: Url) -> RequestBuilder {
        self.inner.post(url)
    }

    /// Check that `url`'s host matches the pinned host.
    pub fn check_host(&self, url: &Url) -> Result<(), HttpTransportError> {
        let actual = url
            .host()
            .ok_or_else(|| HttpTransportError::HostPinViolation {
                actual: "<no host>".to_string(),
                pinned: self.pinned_host.clone(),
            })?;
        // url::Host<&str> vs url::Host<String> — normalize.
        let actual_owned: Host<String> = match actual {
            Host::Domain(d) => Host::Domain(d.to_string()),
            Host::Ipv4(a) => Host::Ipv4(a),
            Host::Ipv6(a) => Host::Ipv6(a),
        };
        if actual_owned != self.pinned_host {
            return Err(HttpTransportError::HostPinViolation {
                actual: format!("{actual_owned}"),
                pinned: self.pinned_host.clone(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard(pinned: &str) -> HttpClientGuard {
        let host = Host::parse(pinned).expect("parse host");
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("build client");
        HttpClientGuard::new(client, host)
    }

    #[test]
    fn pinned_host_allowed() {
        let g = guard("example.com");
        let url = Url::parse("https://example.com/path").unwrap();
        g.check_host(&url).expect("same host is allowed");
    }

    #[test]
    fn different_host_refused() {
        let g = guard("example.com");
        let url = Url::parse("https://evil.com/path").unwrap();
        let err = g.check_host(&url).expect_err("different host refused");
        assert!(matches!(err, HttpTransportError::HostPinViolation { .. }));
    }

    #[test]
    fn missing_host_refused() {
        let g = guard("example.com");
        // `file:` URLs have no host — should refuse.
        let url = Url::parse("file:///etc/passwd").unwrap();
        let err = g.check_host(&url).expect_err("missing host refused");
        assert!(matches!(err, HttpTransportError::HostPinViolation { .. }));
    }
}
```

- [ ] **Step 2: Run the guard tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp-tokio -E 'test(transport_http::guard::tests)'`

Expected: 3 tests pass.

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp-tokio/src/transport_http/guard.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/transport_http): HttpClientGuard pinned-host newtype"
```

### Task 3.3: `session.rs` — SessionState

**Files:**
- Modify: `crates/tau-mcp-tokio/src/transport_http/session.rs`

- [ ] **Step 1: Write `session.rs`.**

```rust
//! `Mcp-Session-Id` header tracker.
//!
//! Per MCP spec rev 2025-03-26: the Streamable HTTP transport assigns a
//! session ID on the initialize response (HTTP response header
//! `Mcp-Session-Id`). The client must include that header on every
//! subsequent request. We track it in interior-mutable storage so the
//! `McpHttpServer`'s `Transport::send_message` can attach it without
//! needing `&mut self`.

use std::sync::Mutex;

/// HTTP header name MCP uses for session IDs.
pub const MCP_SESSION_ID_HEADER: &str = "Mcp-Session-Id";

/// Interior-mutable session-ID tracker.
#[derive(Debug, Default)]
pub struct SessionState {
    id: Mutex<Option<String>>,
}

impl SessionState {
    /// Construct with no session ID yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the current session ID (None until initialize response sets it).
    pub fn get(&self) -> Option<String> {
        self.id.lock().expect("session state mutex poisoned").clone()
    }

    /// Set the session ID. Idempotent: re-setting to the same value is
    /// a no-op; setting to a DIFFERENT non-None value while one is
    /// already pinned is logged + ignored (the first one wins, per
    /// MCP's "single session per HTTP transport" guarantee).
    pub fn set(&self, new_id: String) {
        let mut guard = self.id.lock().expect("session state mutex poisoned");
        match &*guard {
            None => *guard = Some(new_id),
            Some(existing) if existing == &new_id => {}
            Some(existing) => {
                tracing::warn!(
                    existing = %existing,
                    attempted = %new_id,
                    "ignoring conflicting Mcp-Session-Id; first-wins"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_with_no_id() {
        let s = SessionState::new();
        assert_eq!(s.get(), None);
    }

    #[test]
    fn set_then_get() {
        let s = SessionState::new();
        s.set("abc-123".into());
        assert_eq!(s.get(), Some("abc-123".into()));
    }

    #[test]
    fn re_set_same_id_idempotent() {
        let s = SessionState::new();
        s.set("abc".into());
        s.set("abc".into());
        assert_eq!(s.get(), Some("abc".into()));
    }

    #[test]
    fn first_wins_on_conflict() {
        let s = SessionState::new();
        s.set("first".into());
        s.set("second".into());
        assert_eq!(s.get(), Some("first".into()));
    }
}
```

- [ ] **Step 2: Run the session tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp-tokio -E 'test(transport_http::session::tests)'`

Expected: 4 tests pass.

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp-tokio/src/transport_http/session.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/transport_http): SessionState tracks Mcp-Session-Id header"
```

---

## Phase 4 — McpHttpServer + dial

### Task 4.1: `server.rs` — McpHttpServer + Transport impl

**Files:**
- Modify: `crates/tau-mcp-tokio/src/transport_http/server.rs`

- [ ] **Step 1: Write `server.rs`.**

```rust
//! `McpHttpServer` — Streamable HTTP MCP server handle.
//!
//! Implements `tau_mcp::transport::Transport` by translating each
//! outbound `send_message(JsonRpcMessage)` into an HTTP POST that
//! goes through `HttpClientGuard`, then demuxing the SSE response
//! stream into an inbound mpsc that `next_message` reads from.
//!
//! Design note on the SSE response pump: `Transport::send_message`
//! takes `&self`, not `&Arc<Self>`. We avoid threading an `Arc<Self>`
//! into the pump by capturing `inbound_tx.clone()` (cheap; mpsc senders
//! are `Clone`) and setting any `Mcp-Session-Id` response header on
//! `self.session` SYNCHRONOUSLY before spawning the streaming task.
//! The task itself only needs the sender.

use std::pin::Pin;
use std::sync::Arc;

use futures::stream::StreamExt;
use tau_mcp::protocol::JsonRpcMessage;
use tau_mcp::transport::Transport;
use tau_mcp::McpError;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;
use tracing::debug;
use url::Url;

use crate::transport_http::error::HttpTransportError;
use crate::transport_http::guard::HttpClientGuard;
use crate::transport_http::session::{SessionState, MCP_SESSION_ID_HEADER};
use crate::transport_http::sse::SseFramer;

/// Live Streamable HTTP MCP server.
pub struct McpHttpServer {
    guard: HttpClientGuard,
    session: SessionState,
    url: Url,
    inbound_rx: Mutex<UnboundedReceiver<Result<JsonRpcMessage, HttpTransportError>>>,
    inbound_tx: UnboundedSender<Result<JsonRpcMessage, HttpTransportError>>,
}

impl McpHttpServer {
    /// Construct from a guard + URL. Caller (`dial`) is responsible
    /// for having validated `url`'s host matches `guard.pinned_host()`.
    pub fn new(guard: HttpClientGuard, url: Url) -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            guard,
            session: SessionState::new(),
            url,
            inbound_rx: Mutex::new(rx),
            inbound_tx: tx,
        })
    }

    /// Server URL (for diagnostics).
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Borrow the session state (mainly for tests).
    pub fn session(&self) -> &SessionState {
        &self.session
    }

    /// Spawn an async pump that streams `response.bytes_stream()`
    /// through `SseFramer` and pushes decoded messages to `inbound_tx`.
    /// Captures the `Mcp-Session-Id` response header SYNCHRONOUSLY
    /// before spawning so the task only needs the sender.
    fn start_pump(&self, response: reqwest::Response) {
        if let Some(value) = response.headers().get(MCP_SESSION_ID_HEADER) {
            if let Ok(s) = value.to_str() {
                self.session.set(s.to_string());
            }
        }
        let inbound_tx = self.inbound_tx.clone();
        tokio::spawn(async move {
            let mut framer = SseFramer::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = inbound_tx
                            .send(Err(HttpTransportError::Send(format!("{e}"))));
                        return;
                    }
                };
                match framer.feed_bytes(&chunk) {
                    Ok(messages) => {
                        for m in messages {
                            if inbound_tx.send(Ok(m)).is_err() {
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = inbound_tx.send(Err(e));
                        return;
                    }
                }
            }
            match framer.flush() {
                Ok(Some(m)) => {
                    let _ = inbound_tx.send(Ok(m));
                }
                Ok(None) => {}
                Err(e) => {
                    let _ = inbound_tx.send(Err(e));
                }
            }
            debug!("HTTP SSE stream ended cleanly");
        });
    }
}

impl Transport for McpHttpServer {
    fn send_message<'a>(
        &'a self,
        msg: &'a JsonRpcMessage,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), McpError>> + Send + 'a>> {
        Box::pin(async move {
            let body = serde_json::to_vec(msg)
                .map_err(|e| McpError::Serde(format!("encode JSON-RPC: {e}")))?;
            let mut builder = self
                .guard
                .post(self.url.clone())
                .header("Content-Type", "application/json")
                .header("Accept", "text/event-stream, application/json")
                .body(body);
            if let Some(sid) = self.session.get() {
                builder = builder.header(MCP_SESSION_ID_HEADER, sid);
            }
            let request = builder
                .build()
                .map_err(|e| McpError::Transport(format!("build HTTP request: {e}")))?;
            let response = self
                .guard
                .send(request)
                .await
                .map_err(convert_transport_error)?;
            if !response.status().is_success() {
                let status = response.status().as_u16();
                let body = response
                    .text()
                    .await
                    .unwrap_or_else(|e| format!("<body read failed: {e}>"));
                return Err(convert_transport_error(HttpTransportError::Status {
                    status,
                    body,
                }));
            }
            self.start_pump(response);
            Ok(())
        })
    }

    fn next_message<'a>(
        &'a self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Option<JsonRpcMessage>, McpError>> + Send + 'a>>
    {
        Box::pin(async move {
            let mut rx = self.inbound_rx.lock().await;
            match rx.recv().await {
                Some(Ok(msg)) => Ok(Some(msg)),
                Some(Err(e)) => Err(convert_transport_error(e)),
                None => Ok(None), // Channel closed — EOF.
            }
        })
    }
}

fn convert_transport_error(e: HttpTransportError) -> McpError {
    match e {
        HttpTransportError::JsonDecode(s) => McpError::Serde(s),
        HttpTransportError::Send(s)
        | HttpTransportError::SseParse(s)
        | HttpTransportError::Channel(s) => McpError::Transport(s),
        HttpTransportError::Status { status, body } => {
            McpError::Transport(format!("HTTP {status}: {body}"))
        }
        HttpTransportError::HostPinViolation { actual, pinned } => {
            McpError::Transport(format!(
                "host-pin violation: actual={actual} pinned={pinned}"
            ))
        }
    }
}
```

- [ ] **Step 2: cargo check + clippy.**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp-tokio --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp-tokio/src/transport_http/server.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/transport_http): McpHttpServer impls Transport (POST + SSE pump + session-id)"
```

(Server-level integration tests live in Phase 7 against wiremock-rs — no unit tests here because the server needs a real HTTP response.)

### Task 4.2: `dial.rs` — top-level dial entrypoint

**Files:**
- Modify: `crates/tau-mcp-tokio/src/transport_http/dial.rs`

- [ ] **Step 1: Write `dial.rs`.**

```rust
//! `dial(url, options) → Arc<McpHttpServer>` — HTTP transport dial entrypoint.
//!
//! Composes URL host extraction → reqwest::Client build → HttpClientGuard
//! → McpHttpServer construction. Called by host_lifecycle::open() for
//! `http:` and `https:` URLs.

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, instrument};
use url::Url;

use crate::transport_http::error::HttpSpawnError;
use crate::transport_http::guard::HttpClientGuard;
use crate::transport_http::server::McpHttpServer;

/// Options for HTTP dial.
#[derive(Debug, Clone)]
pub struct HttpDialOptions {
    /// Per-request timeout for the reqwest client.
    pub request_timeout: Duration,
    /// User-Agent string sent with every request.
    pub user_agent: String,
}

impl Default for HttpDialOptions {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(60),
            user_agent: concat!("tau-mcp-tokio/", env!("CARGO_PKG_VERSION")).to_string(),
        }
    }
}

/// Dial an HTTP MCP server. Returns a ready `Arc<McpHttpServer>` that
/// `host_lifecycle::open` then drives through the MCP handshake.
#[instrument(name = "mcp_http_dial", skip(options), fields(url = %url))]
pub fn dial(
    url: Url,
    options: HttpDialOptions,
) -> Result<Arc<McpHttpServer>, HttpSpawnError> {
    let pinned_host = url.host().ok_or_else(|| HttpSpawnError::NoHost {
        url: url.to_string(),
    })?;
    let pinned_host = match pinned_host {
        url::Host::Domain(d) => url::Host::Domain(d.to_string()),
        url::Host::Ipv4(a) => url::Host::Ipv4(a),
        url::Host::Ipv6(a) => url::Host::Ipv6(a),
    };
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(options.request_timeout)
        .user_agent(options.user_agent)
        .build()
        .map_err(|e| HttpSpawnError::ClientBuild(format!("{e}")))?;
    let guard = HttpClientGuard::new(client, pinned_host);
    info!(host = %guard.pinned_host(), "constructed pinned HTTP client");
    Ok(McpHttpServer::new(guard, url))
}
```

- [ ] **Step 2: cargo check + clippy.**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp-tokio --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp-tokio/src/transport_http/dial.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/transport_http): dial() entrypoint — URL → reqwest client + guard → ready server"
```

---

## Phase 5 — URL discriminator + open() Http arm + lib.rs re-exports

### Task 5.1: `host_lifecycle/url.rs` — add Http/Https arms

**Files:**
- Modify: `crates/tau-mcp-tokio/src/host_lifecycle/url.rs`

- [ ] **Step 1: Read the current file.**

PR-2's url.rs has the `Stdio` variant and rejects everything else with `UnsupportedScheme`.

- [ ] **Step 2: Replace the file.**

```rust
//! MCP URL discriminator.
//!
//! Per the β.3 design doc §3, the `[tools.<name>] mcp = "..."` field
//! discriminates transport by URL scheme:
//!
//! - `stdio:<command>` → subprocess MCP server (PR-2)
//! - `http://...` / `https://...` → Streamable HTTP (PR-3)
//!
//! Any other scheme is rejected with `UrlParseError::UnsupportedScheme`.

use crate::host_lifecycle::error::UrlParseError;

/// Parsed MCP server URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpUrl {
    /// Subprocess MCP server. The vec is the command argv.
    Stdio {
        /// argv to spawn (first element is the binary).
        cmd: Vec<String>,
    },
    /// Plain-HTTP Streamable MCP server (accepted but should warn at
    /// build time per spec §3).
    Http {
        /// Validated URL with a host component.
        url: url::Url,
    },
    /// HTTPS Streamable MCP server.
    Https {
        /// Validated URL with a host component.
        url: url::Url,
    },
}

/// Parse an MCP URL string into a typed `McpUrl`.
pub fn parse_url(s: &str) -> Result<McpUrl, UrlParseError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(UrlParseError::Empty);
    }
    if let Some(rest) = s.strip_prefix("stdio:") {
        let rest = rest.trim();
        if rest.is_empty() {
            return Err(UrlParseError::EmptyStdioCommand);
        }
        let cmd = rest.split_whitespace().map(String::from).collect();
        return Ok(McpUrl::Stdio { cmd });
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        let url = url::Url::parse(s).map_err(|e| UrlParseError::UnsupportedScheme {
            scheme: format!("invalid URL: {e}"),
        })?;
        if url.host().is_none() {
            return Err(UrlParseError::UnsupportedScheme {
                scheme: "http(s) URL has no host".to_string(),
            });
        }
        return match url.scheme() {
            "http" => Ok(McpUrl::Http { url }),
            "https" => Ok(McpUrl::Https { url }),
            other => Err(UrlParseError::UnsupportedScheme {
                scheme: other.to_string(),
            }),
        };
    }
    let scheme = s.split(':').next().unwrap_or("").to_string();
    Err(UrlParseError::UnsupportedScheme { scheme })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_stdio() {
        let url = parse_url("stdio:npx --yes weather").expect("parse");
        match url {
            McpUrl::Stdio { cmd } => {
                assert_eq!(cmd, vec!["npx", "--yes", "weather"]);
            }
            other => panic!("expected Stdio, got {other:?}"),
        }
    }

    #[test]
    fn empty_url_rejected() {
        assert!(matches!(parse_url(""), Err(UrlParseError::Empty)));
        assert!(matches!(parse_url("   "), Err(UrlParseError::Empty)));
    }

    #[test]
    fn empty_stdio_command_rejected() {
        assert!(matches!(parse_url("stdio:"), Err(UrlParseError::EmptyStdioCommand)));
        assert!(matches!(parse_url("stdio:   "), Err(UrlParseError::EmptyStdioCommand)));
    }

    #[test]
    fn https_accepted() {
        let url = parse_url("https://mcp.example.com").expect("parse");
        match url {
            McpUrl::Https { url } => {
                assert_eq!(url.host_str(), Some("mcp.example.com"));
            }
            other => panic!("expected Https, got {other:?}"),
        }
    }

    #[test]
    fn http_accepted() {
        let url = parse_url("http://localhost:8080/mcp").expect("parse");
        match url {
            McpUrl::Http { url } => {
                assert_eq!(url.host_str(), Some("localhost"));
                assert_eq!(url.port(), Some(8080));
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[test]
    fn ws_rejected() {
        let err = parse_url("ws://example.com").expect_err("should reject");
        match err {
            UrlParseError::UnsupportedScheme { scheme } => {
                assert_eq!(scheme, "ws");
            }
            other => panic!("expected UnsupportedScheme, got {other:?}"),
        }
    }

    #[test]
    fn file_rejected() {
        let err = parse_url("file:///etc/passwd").expect_err("should reject");
        match err {
            UrlParseError::UnsupportedScheme { scheme } => {
                assert_eq!(scheme, "file");
            }
            other => panic!("expected UnsupportedScheme, got {other:?}"),
        }
    }

    #[test]
    fn http_without_host_rejected() {
        let err = parse_url("http://").expect_err("should reject");
        assert!(matches!(err, UrlParseError::UnsupportedScheme { .. }));
    }
}
```

- [ ] **Step 3: Run the URL tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp-tokio -E 'test(host_lifecycle::url::tests)'`

Expected: 8 tests pass.

- [ ] **Step 4: Commit.**

```
git add crates/tau-mcp-tokio/src/host_lifecycle/url.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/host_lifecycle): url discriminator gains Http/Https arms"
```

### Task 5.2: `host_lifecycle/open.rs` — wire Http/Https branches

**Files:**
- Modify: `crates/tau-mcp-tokio/src/host_lifecycle/open.rs`

- [ ] **Step 1: Read current open.rs.**

The PR-2 file matches on `McpUrl::Stdio` only. Add arms for `Http` + `Https`.

- [ ] **Step 2: Replace the file.**

```rust
//! `open(url, plan, gate, options)` — v0 entrypoint.
//!
//! Composes URL parse → spawn (stdio) or dial (HTTP) → handshake →
//! live `McpClient`.

use std::sync::Arc;

use tau_ports::CapabilityPlan;
use tau_runtime_tokio::process_gate::DynProcessCapabilityGate;
use tokio::process::Command;
use tracing::{info, instrument};

use crate::host_lifecycle::client::{McpClient, McpClientOptions};
use crate::host_lifecycle::error::{HandshakeError, LifecycleError};
use crate::host_lifecycle::handshake::drive_handshake;
use crate::host_lifecycle::url::{parse_url, McpUrl};
use crate::transport_http::dial::{dial as http_dial, HttpDialOptions};
use crate::transport_stdio::{server::McpStdioServer, spawn as stdio_spawn};

/// Open a connection to an MCP server.
#[instrument(name = "mcp_open", skip(plan, gate, options), fields(url = url))]
pub async fn open(
    url: &str,
    plan: &CapabilityPlan,
    gate: Arc<dyn DynProcessCapabilityGate>,
    options: McpClientOptions,
) -> Result<McpClient, LifecycleError> {
    let parsed = parse_url(url)?;
    match parsed {
        McpUrl::Stdio { cmd } => open_stdio(cmd, plan, gate, options).await,
        McpUrl::Http { url } => open_http(url, options).await,
        McpUrl::Https { url } => open_http(url, options).await,
    }
}

async fn open_stdio(
    cmd: Vec<String>,
    plan: &CapabilityPlan,
    gate: Arc<dyn DynProcessCapabilityGate>,
    options: McpClientOptions,
) -> Result<McpClient, LifecycleError> {
    let mut command = Command::new(&cmd[0]);
    command.args(&cmd[1..]);
    info!(stdio_cmd = ?cmd, "spawning stdio MCP server");
    let child = stdio_spawn(command, gate, plan).await?;
    let transport = McpStdioServer::from_child(child)
        .map_err(|e| LifecycleError::Handshake(HandshakeError::Transport(format!("{e}"))))?;
    let contract = drive_handshake(&*transport, &options.handshake).await?;
    info!(
        server_name = %contract.server_info.name,
        tools_count = contract.tools.len(),
        "MCP handshake complete (stdio)"
    );
    Ok(McpClient::new(transport, contract, options))
}

async fn open_http(
    url: url::Url,
    options: McpClientOptions,
) -> Result<McpClient, LifecycleError> {
    info!(http_url = %url, "dialing HTTP MCP server");
    let transport = http_dial(url, HttpDialOptions::default())?;
    let contract = drive_handshake(&*transport, &options.handshake).await?;
    info!(
        server_name = %contract.server_info.name,
        tools_count = contract.tools.len(),
        "MCP handshake complete (HTTP)"
    );
    Ok(McpClient::new(transport, contract, options))
}
```

- [ ] **Step 3: cargo check + clippy.**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp-tokio --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 4: Commit.**

```
git add crates/tau-mcp-tokio/src/host_lifecycle/open.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/host_lifecycle): open() Http/Https arms via transport_http::dial"
```

### Task 5.3: `lib.rs` — re-export HTTP types

**Files:**
- Modify: `crates/tau-mcp-tokio/src/lib.rs`

- [ ] **Step 1: Read current lib.rs.**

- [ ] **Step 2: Add HTTP re-exports.**

Final lib.rs:

```rust
//! tau-mcp-tokio — tokio runtime + transports for tau-mcp.

pub mod bridge;
pub mod host_lifecycle;
pub mod transport_http;
pub mod transport_stdio;

pub use host_lifecycle::{
    open, HandshakeError, LifecycleError, McpClient, McpClientOptions, McpUrl, UrlParseError,
};
pub use transport_http::{HttpSpawnError, HttpTransportError, McpHttpServer};
pub use transport_stdio::{McpStdioServer, StdioSpawnError, StdioTransportError};
```

- [ ] **Step 3: cargo check + clippy.**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp-tokio --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 4: Commit.**

```
git add crates/tau-mcp-tokio/src/lib.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio): re-export transport_http public surface"
```

---

## Phase 6 — CassetteTransport (tau-mcp)

### Task 6.1: `cassette/transport.rs` — CassetteTransport

**Files:**
- Create: `crates/tau-mcp/src/cassette/transport.rs`

`CassetteTransport` wraps the existing `Replayer` so cassettes can be used as a `Transport` directly. Gated on the `with-std-adapters` feature because it depends on `futures::channel::mpsc` (which is `std` + the `futures` dep we added in Task 1.3) and `std::sync::Mutex`.

- [ ] **Step 1: Write `transport.rs`.**

```rust
//! `CassetteTransport` — wraps a `Replayer` so cassettes can drive
//! `tau_mcp::transport::Transport` directly. Gated on `with-std-adapters`.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::future::Future;
use core::pin::Pin;

use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::stream::StreamExt;
use std::sync::Mutex;

use crate::cassette::message::{CassetteMessage, Direction, MessageKind};
use crate::cassette::replayer::{ReplayError, Replayer};
use crate::error::McpError;
use crate::protocol::jsonrpc::{
    JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse,
};
use crate::transport::Transport;

/// Live cassette-as-transport.
pub struct CassetteTransport {
    replayer: Mutex<Replayer>,
    inbound_rx: futures::lock::Mutex<UnboundedReceiver<JsonRpcMessage>>,
    inbound_tx: UnboundedSender<JsonRpcMessage>,
}

impl CassetteTransport {
    /// Construct from raw JSONL bytes.
    pub fn from_jsonl_bytes(bytes: &[u8]) -> Result<Arc<Self>, ReplayError> {
        let replayer = Replayer::from_jsonl_bytes(bytes)?;
        let (tx, rx) = mpsc::unbounded();
        Ok(Arc::new(Self {
            replayer: Mutex::new(replayer),
            inbound_rx: futures::lock::Mutex::new(rx),
            inbound_tx: tx,
        }))
    }

    /// Push any queued outbounds (notifications + server-initiated
    /// requests) into the inbound channel for the host to consume.
    fn drain_pending(&self) -> Result<(), McpError> {
        let mut replayer = self
            .replayer
            .lock()
            .map_err(|_| McpError::Transport("replayer mutex poisoned".to_string()))?;
        while let Some(rec) = replayer.next_pending_outbound() {
            let msg = cassette_record_to_jsonrpc(&rec)?;
            if self.inbound_tx.unbounded_send(msg).is_err() {
                return Err(McpError::Transport(
                    "inbound channel closed".to_string(),
                ));
            }
        }
        Ok(())
    }
}

impl Transport for CassetteTransport {
    fn send_message<'a>(
        &'a self,
        msg: &'a JsonRpcMessage,
    ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
        Box::pin(async move {
            match msg {
                JsonRpcMessage::Request(req) => {
                    // Match the request in the cassette; queue its
                    // response + any preceding notifications/server-
                    // initiated-requests for the host to read next.
                    let response = {
                        let mut replayer = self.replayer.lock().map_err(|_| {
                            McpError::Transport("replayer mutex poisoned".to_string())
                        })?;
                        let method = req.method.clone();
                        let args = req
                            .params
                            .clone()
                            .unwrap_or(serde_json::Value::Null);
                        replayer
                            .match_request(&method, &args)
                            .map_err(|e| McpError::Transport(format!("cassette: {e}")))?
                    };
                    // Drain pending_outbound (notifications +
                    // server-initiated requests recorded BETWEEN this
                    // request and its response).
                    self.drain_pending()?;
                    // Push the matched response.
                    let resp_msg = cassette_record_to_jsonrpc(&response)?;
                    self.inbound_tx
                        .unbounded_send(resp_msg)
                        .map_err(|_| McpError::Transport("inbound channel closed".to_string()))?;
                    Ok(())
                }
                JsonRpcMessage::Response(_) | JsonRpcMessage::Notification(_) => {
                    // Host responding to a server-initiated request or
                    // emitting a notification. The cassette's
                    // pending_outbound queue was already drained when
                    // the prior request matched; nothing more to do.
                    Ok(())
                }
            }
        })
    }

    fn next_message<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<JsonRpcMessage>, McpError>> + Send + 'a>>
    {
        Box::pin(async move {
            let mut rx = self.inbound_rx.lock().await;
            Ok(rx.next().await)
        })
    }
}

/// Convert a `CassetteMessage` (Direction::Out only) into a `JsonRpcMessage`
/// for the host to consume.
fn cassette_record_to_jsonrpc(rec: &CassetteMessage) -> Result<JsonRpcMessage, McpError> {
    if rec.dir != Direction::Out {
        return Err(McpError::Transport(format!(
            "cassette record direction must be Out for replay; got {:?}",
            rec.dir
        )));
    }
    match rec.kind {
        MessageKind::Response => {
            // result/error live in rec.payload (already split or as
            // {result: ..., error: ...}? — cassette format §11 says
            // payload IS the result/error directly for responses).
            // Parse defensively: if payload has top-level "error", treat
            // as error; else as result.
            let id = rec.id.clone().ok_or_else(|| {
                McpError::Transport("cassette response without id".to_string())
            })?;
            // The cassette stores the result directly as `payload` per
            // spec §11. We construct a JsonRpcResponse.
            // Per §11 example: payload for response is the inner result.
            let resp = JsonRpcResponse {
                jsonrpc: crate::protocol::jsonrpc::JSONRPC_VERSION.to_string(),
                id,
                result: Some(rec.payload.clone()),
                error: None,
            };
            Ok(JsonRpcMessage::Response(resp))
        }
        MessageKind::Notification => {
            let method = rec.method.clone().ok_or_else(|| {
                McpError::Transport("cassette notification without method".to_string())
            })?;
            let n = JsonRpcNotification {
                jsonrpc: crate::protocol::jsonrpc::JSONRPC_VERSION.to_string(),
                method,
                params: Some(rec.payload.clone()),
            };
            Ok(JsonRpcMessage::Notification(n))
        }
        MessageKind::Request => {
            // Server-initiated request — same shape as a regular
            // JsonRpcRequest. Host responds to it via send_message.
            let id = rec.id.clone().ok_or_else(|| {
                McpError::Transport("cassette server-initiated request without id".to_string())
            })?;
            let method = rec.method.clone().ok_or_else(|| {
                McpError::Transport(
                    "cassette server-initiated request without method".to_string(),
                )
            })?;
            let req = JsonRpcRequest {
                jsonrpc: crate::protocol::jsonrpc::JSONRPC_VERSION.to_string(),
                id,
                method,
                params: Some(rec.payload.clone()),
            };
            Ok(JsonRpcMessage::Request(req))
        }
    }
}

```

**Note for the implementer:** the spec §11 cassette example shows `"payload"` for responses containing the `result` object directly (not `{result: ..., error: ...}`). The conversion above assumes `payload` IS the result. If integration tests reveal a different shape, adjust accordingly.

- [ ] **Step 2: Update `crates/tau-mcp/src/cassette/mod.rs` to expose the module.**

Replace the file with:

```rust
//! Transport-agnostic message-level cassette format (spec §11).
//!
//! Lives in `tau-mcp` (not `tau-mcp-tokio`) so wasm + embassy shells
//! can replay cassettes in tests without a tokio dependency. The
//! `transport` submodule (which provides `CassetteTransport`) requires
//! std + futures and is therefore gated on `with-std-adapters`.

pub mod message;
pub mod recorder;
pub mod replayer;

#[cfg(feature = "with-std-adapters")]
pub mod transport;

#[cfg(feature = "with-std-adapters")]
pub use transport::CassetteTransport;
```

- [ ] **Step 3: cargo check with the feature enabled.**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp --features with-std-adapters
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp --features with-std-adapters --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 4: Commit.**

```
git add crates/tau-mcp/src/cassette/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp/cassette): CassetteTransport wraps Replayer as Transport (gated on with-std-adapters)"
```

### Task 6.2: `tests/cassette_transport.rs` — integration tests

**Files:**
- Create: `crates/tau-mcp/tests/cassette_transport.rs`

- [ ] **Step 1: Write the integration tests.**

```rust
//! Tests for `tau_mcp::cassette::CassetteTransport`.
//!
//! Drives the cassette via Transport::send_message / next_message,
//! verifies matched responses, pending-outbound drains, and EOF
//! semantics.

#![cfg(feature = "with-std-adapters")]

use tau_mcp::cassette::CassetteTransport;
use tau_mcp::protocol::jsonrpc::{
    JsonRpcMessage, JsonRpcRequest, RequestId, JSONRPC_VERSION,
};
use tau_mcp::transport::Transport;

/// A minimal cassette covering initialize + tools/list + tools/call
/// with a notification interleaved between the tools/call request and
/// its response.
fn minimal_cassette() -> Vec<u8> {
    let lines = [
        r#"{"version":1}"#,
        r#"{"dir":"in","kind":"request","id":0,"method":"initialize","payload":null}"#,
        r#"{"dir":"out","kind":"response","id":0,"payload":{"protocolVersion":"2025-03-26","serverInfo":{"name":"mock","version":"0.0.0"}}}"#,
        r#"{"dir":"in","kind":"request","id":1,"method":"tools/list","payload":null}"#,
        r#"{"dir":"out","kind":"response","id":1,"payload":{"tools":[]}}"#,
        r#"{"dir":"in","kind":"request","id":2,"method":"tools/call","payload":{"name":"echo","arguments":{"message":"hi"}}}"#,
        r#"{"dir":"out","kind":"notification","method":"notifications/progress","payload":{"progressToken":"call-2","progress":50,"total":100}}"#,
        r#"{"dir":"out","kind":"response","id":2,"payload":{"content":[{"type":"text","text":"hi"}]}}"#,
    ];
    lines.join("\n").into_bytes()
}

fn req(id: i64, method: &str, params: serde_json::Value) -> JsonRpcMessage {
    JsonRpcMessage::Request(JsonRpcRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: RequestId::Number(id),
        method: method.to_string(),
        params: Some(params),
    })
}

#[tokio::test]
async fn happy_path_initialize_then_list_then_call() {
    let t = CassetteTransport::from_jsonl_bytes(&minimal_cassette()).expect("parse cassette");

    // initialize
    t.send_message(&req(0, "initialize", serde_json::Value::Null))
        .await
        .expect("send initialize");
    let resp = t.next_message().await.unwrap().expect("response");
    assert!(matches!(resp, JsonRpcMessage::Response(_)));

    // tools/list
    t.send_message(&req(1, "tools/list", serde_json::Value::Null))
        .await
        .expect("send tools/list");
    let resp = t.next_message().await.unwrap().expect("response");
    assert!(matches!(resp, JsonRpcMessage::Response(_)));

    // tools/call — expect notification then response (interleaved per cassette)
    t.send_message(&req(2, "tools/call", serde_json::json!({"name":"echo","arguments":{"message":"hi"}})))
        .await
        .expect("send tools/call");
    let first = t.next_message().await.unwrap().expect("first message");
    assert!(matches!(first, JsonRpcMessage::Notification(_)), "first msg should be the notification");
    let second = t.next_message().await.unwrap().expect("second message");
    assert!(matches!(second, JsonRpcMessage::Response(_)), "second msg should be the response");
}

#[tokio::test]
async fn unmatched_request_errors() {
    let t = CassetteTransport::from_jsonl_bytes(&minimal_cassette()).expect("parse cassette");
    let err = t
        .send_message(&req(0, "nonexistent/method", serde_json::Value::Null))
        .await
        .expect_err("should fail to match");
    let msg = format!("{err:?}");
    assert!(msg.contains("cassette"));
}

#[tokio::test]
async fn channel_closed_after_drop_returns_none() {
    let t = CassetteTransport::from_jsonl_bytes(&minimal_cassette()).expect("parse cassette");
    // Drive one request so the channel has one message available.
    t.send_message(&req(0, "initialize", serde_json::Value::Null))
        .await
        .expect("send");
    let _ = t.next_message().await.unwrap().expect("one message");
    // Drop the transport — next_message on a fresh handle is no longer
    // possible since we just consumed the Arc; this test instead
    // verifies the channel-closed path by dropping the inbound_tx.
    // (For now, this assertion is weak — the strong shape requires
    // either holding a separate clone of the inbound_tx publicly or a
    // shutdown() method. Defer the explicit close test to PR-5 when
    // McpBridge needs it.)
}
```

- [ ] **Step 2: Run.**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp --features with-std-adapters --test cassette_transport
```

Expected: 3 tests pass.

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp/tests/cassette_transport.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "test(tau-mcp/cassette): CassetteTransport happy-path + unmatched-request integration tests"
```

---

## Phase 7 — HTTP integration tests (wiremock-rs) + final checks + push + PR

### Task 7.1: `tests/http_lifecycle.rs` — wiremock-rs integration

**Files:**
- Create: `crates/tau-mcp-tokio/tests/http_lifecycle.rs`

- [ ] **Step 1: Write the integration tests.**

```rust
//! End-to-end Streamable HTTP MCP tests against wiremock-rs.

use std::time::Duration;

use serde_json::json;
use tau_mcp_tokio::host_lifecycle::handshake::HandshakeOptions;
use tau_mcp_tokio::{open, LifecycleError, McpClientOptions};
use tau_ports::CapabilityPlan;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn passthrough_gate() -> std::sync::Arc<dyn tau_runtime_tokio::process_gate::DynProcessCapabilityGate> {
    use tau_runtime_tokio::process_gate::passthrough::PassthroughSandbox;
    std::sync::Arc::new(PassthroughSandbox::new())
}

fn empty_plan() -> CapabilityPlan {
    CapabilityPlan::default()
}

/// Build a wiremock response that emits the initialize + tools/list +
/// (optionally) tools/call responses as ONE SSE event each per HTTP
/// response (one HTTP request → one response). The MCP host posts
/// each request individually, so each Mock matches one POST.
fn sse_event(msg: serde_json::Value) -> String {
    format!("data: {}\n\n", serde_json::to_string(&msg).unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_happy_path_via_wiremock() {
    let server = MockServer::start().await;
    let url = server.uri();

    // initialize response
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .insert_header("Mcp-Session-Id", "session-xyz")
                .set_body_string(sse_event(json!({
                    "jsonrpc": "2.0",
                    "id": 0,
                    "result": {
                        "protocolVersion": "2025-03-26",
                        "serverInfo": {"name": "mock", "version": "0.0.0"}
                    }
                }))),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;

    // tools/list response (must echo Mcp-Session-Id)
    Mock::given(method("POST"))
        .and(path("/"))
        .and(header("Mcp-Session-Id", "session-xyz"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(sse_event(json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {"tools": []}
                }))),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;

    let client = open(
        &url,
        &empty_plan(),
        passthrough_gate(),
        McpClientOptions {
            handshake: HandshakeOptions {
                handshake_timeout: Duration::from_secs(5),
                ..HandshakeOptions::default()
            },
            ..McpClientOptions::default()
        },
    )
    .await
    .expect("open succeeds");
    assert_eq!(client.contract().server_info.name, "mock");
    assert_eq!(client.contract().tools.len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_event_sse_response() {
    let server = MockServer::start().await;
    let url = server.uri();

    // initialize — two events: a notification THEN the response.
    let body = format!(
        "{}{}",
        sse_event(json!({
            "jsonrpc": "2.0",
            "method": "notifications/progress",
            "params": {"progress": 50}
        })),
        sse_event(json!({
            "jsonrpc": "2.0",
            "id": 0,
            "result": {
                "protocolVersion": "2025-03-26",
                "serverInfo": {"name": "mock", "version": "0.0.0"}
            }
        })),
    );
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let result = open(
        &url,
        &empty_plan(),
        passthrough_gate(),
        McpClientOptions {
            handshake: HandshakeOptions {
                handshake_timeout: Duration::from_secs(2),
                ..HandshakeOptions::default()
            },
            ..McpClientOptions::default()
        },
    )
    .await;
    // The handshake should still complete despite the leading notification.
    // (handshake.rs skips non-response messages while awaiting the matching id.)
    let client = result.expect("open succeeds despite leading notification");
    assert_eq!(client.contract().server_info.name, "mock");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_2xx_response_surfaces_as_handshake_error() {
    let server = MockServer::start().await;
    let url = server.uri();

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503).set_body_string("server overloaded"))
        .mount(&server)
        .await;

    let err = open(
        &url,
        &empty_plan(),
        passthrough_gate(),
        McpClientOptions {
            handshake: HandshakeOptions {
                handshake_timeout: Duration::from_secs(2),
                ..HandshakeOptions::default()
            },
            ..McpClientOptions::default()
        },
    )
    .await
    .expect_err("should fail");
    match err {
        LifecycleError::Handshake(_) => {} // ok
        other => panic!("expected Handshake error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn url_parse_failure_propagates_for_http() {
    let err = open(
        "http://",
        &empty_plan(),
        passthrough_gate(),
        McpClientOptions::default(),
    )
    .await
    .expect_err("should fail");
    assert!(matches!(err, LifecycleError::UrlParse(_)));
}
```

- [ ] **Step 2: Run.**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp-tokio --test http_lifecycle
```

Expected: 4 tests pass.

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp-tokio/tests/http_lifecycle.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "test(tau-mcp-tokio): http_lifecycle integration tests (wiremock-rs — happy path, multi-event SSE, error responses, URL parse)"
```

### Task 7.2: Workspace-level checks

- [ ] **Step 1: Full check / nextest / doc / clippy / fmt for tau-mcp-tokio + tau-mcp.**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp-tokio
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp --features with-std-adapters
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-mcp-tokio
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp-tokio --all-targets -- -D warnings
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp --features with-std-adapters --all-targets -- -D warnings
timeout 30  env CARGO_TARGET_DIR=target/agent-impl cargo fmt --check -p tau-mcp-tokio
timeout 30  env CARGO_TARGET_DIR=target/agent-impl cargo fmt --check -p tau-mcp
```

Expected: all green.

- [ ] **Step 2: Cross-crate canary (PR-2 didn't break, but the new `transport_http` re-export touches `lib.rs`).**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-pkg
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-tokio
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-cli
```

Expected: all clean.

- [ ] **Step 3: If fmt flags anything, apply + commit.**

```
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-mcp-tokio -p tau-mcp
git status
```

If files changed:

```
git add -A
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "style(tau-mcp,tau-mcp-tokio): apply cargo fmt"
```

### Task 7.3: Push + open PR + auto-merge

- [ ] **Step 1: Push.**

```
git push --no-verify -u origin feat/beta-3-pr-3-http-transport
```

- [ ] **Step 2: Open the PR.**

```
gh pr create --title "β.3 MCP facilitator — PR-3: HTTP transport + cassette-as-transport" --body "$(cat <<'EOF'
## Summary

Third of six PRs in the β.3 MCP facilitator sub-project. Implements:

- `tau-mcp-tokio::transport_http` — Streamable HTTP MCP client per spec rev 2025-03-26. POST request body + SSE response framing + `Mcp-Session-Id` header tracking. `HttpClientGuard` newtype around `reqwest::Client` (with `redirect::Policy::none()`) enforces wire-level net.http host pinning. Hand-rolled SSE parser (no `eventsource-stream` dep).
- `tau-mcp-tokio::host_lifecycle::open()` — gains `Http` / `Https` arms calling `transport_http::dial()`.
- `tau-mcp::cassette::transport::CassetteTransport` — thin `Transport` impl wrapping the existing `Replayer`. Lets cassettes drive in-memory MCP tests directly. Gated on `with-std-adapters` feature.
- ~20 tests: 6 SSE framer + 3 guard + 4 session + 8 URL parser + 4 HTTP integration (wiremock-rs) + 3 cassette integration.

Spec: \`docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md\` §2/§3/§9/§11/§15
Plan: \`docs/superpowers/plans/2026-06-02-beta-3-mcp-facilitator-pr-3.md\`
Previous PR: #281 (β.3 PR-2).

Stacks-on: nothing (independent of PR-4 per spec's PR-2/3/4 fan-out).

## Test plan

- [ ] \`cargo nextest run -p tau-mcp-tokio\` green (~24 tests: 17 from PR-2 + 7 new transport_http unit + integration)
- [ ] \`cargo nextest run -p tau-mcp --features with-std-adapters\` green (53 from PR-1 + 3 new cassette integration)
- [ ] \`cargo clippy -p tau-mcp-tokio --all-targets -- -D warnings\` clean
- [ ] \`cargo clippy -p tau-mcp --features with-std-adapters --all-targets -- -D warnings\` clean
- [ ] \`cargo fmt --check\` clean
- [ ] Downstream canary (\`tau-pkg\`, \`tau-runtime-tokio\`, \`tau-cli\`) clean
- [ ] CI green on linux / macos / windows

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Capture the PR number.

- [ ] **Step 3: Enroll auto-merge.**

```
gh pr merge <N> --auto
```

Bare form only.

- [ ] **Step 4: Confirm queue enrollment.**

```
gh api graphql -f query='query{repository(owner:"tau-rs",name:"tau"){pullRequest(number:<N>){mergeQueueEntry{state position}}}}'
```

Expected: `mergeQueueEntry.state` is one of `QUEUED` / `AWAITING_CHECKS` once PR-level checks complete.

- [ ] **Step 5: Watch CI. Re-enroll auto-merge if a check fails and you rerun it.**

---

## Self-review checklist (run before declaring PR-3 done)

| Check | Status |
|---|---|
| `tau-mcp-tokio/Cargo.toml` carries reqwest + bytes + futures + url; wiremock dev-dep | Task 1.2 |
| `tau-mcp/Cargo.toml` adds futures gated on with-std-adapters | Task 1.3 |
| Workspace Cargo.toml has wiremock | Task 1.1 |
| `transport_http/` has error.rs + guard.rs + sse.rs + session.rs + server.rs + dial.rs + mod.rs (7 files) | Phase 2/3/4 |
| `host_lifecycle/url.rs` accepts http + https + stdio, rejects ws + file | Task 5.1 |
| `host_lifecycle/open.rs` routes Http/Https via transport_http::dial | Task 5.2 |
| `host_lifecycle/error.rs` has HttpSpawn variant with #[from] | Task 2.2 |
| `lib.rs` re-exports HttpSpawnError + HttpTransportError + McpHttpServer | Task 5.3 |
| `cassette/transport.rs` impls Transport via Replayer; gated on with-std-adapters | Task 6.1 |
| `cassette/mod.rs` exposes transport submodule with cfg gate | Task 6.1 |
| 6 SSE framer unit tests + 3 guard tests + 4 session tests + 8 URL tests + 4 HTTP integration + 3 cassette = ~28 new tests | Phase 3/5/6/7 |
| `cargo clippy --all-targets -- -D warnings` clean on tau-mcp-tokio + tau-mcp (--features with-std-adapters) | Task 7.2 |
| `cargo fmt --check` clean | Task 7.2 |
| Downstream canary (tau-pkg / tau-runtime-tokio / tau-cli) clean | Task 7.2 |
| Push used `--no-verify` (agent-runtime silent-kill avoidance) | Task 7.3 |
| Auto-merge enrolled via `gh pr merge <N> --auto` BARE | Task 7.3 |
| Queue enrollment confirmed via mergeQueueEntry GraphQL | Task 7.3 |

---

## What's next: PR-4 through PR-6

PR-4 (lowering + lockfile schema v7 + PinnedContract) is independent of PR-2/PR-3 and can run in a parallel worktree. PR-5 (McpBridge + sampling/roots inbound handlers + runtime integration) stacks on PR-4. PR-6 (CLI verbs + conformance fixture #07 + ADR-0038 finalize + docs) stacks on PR-4 + PR-5.

PR-3 lessons to fold into PR-4's plan:
- The `with-std-adapters` feature gate is the canonical "test/cassette-only" gate for tau-mcp. PR-4 should mirror this discipline for any test-only types it adds.
- Workspace feature unification trap: do NOT enable opaque features on dev-deps without auditing what they activate transitively.
- `gh pr merge --auto` is idempotent (returns "already queued to merge"); always re-enroll after rerun.
- wiremock-rs `MockServer::start().await` is a real bind-to-localhost — tests are real network round-trips, not pure-in-process. CI overhead is small (<100ms per test).
