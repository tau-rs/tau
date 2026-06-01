# β.3 MCP facilitator — PR-2: stdio transport + host lifecycle + fixture server

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship PR-2 of six in the β.3 sub-project. Implement the subprocess (stdio) MCP transport in `tau-mcp-tokio::transport_stdio`, the host-lifecycle layer (`host_lifecycle::open` + handshake driver + `McpClient` handle) that drives `initialize` + `tools/list` end-to-end, and an in-tree mock-mcp-server fixture binary used by the integration tests. Sandbox integration goes through `tau_runtime_tokio::process_gate::ProcessCapabilityGate::wrap_spawn` exactly like the existing plugin_host does.

**Architecture:** A `McpStdioServer` wraps a sandboxed `tokio::process::Child` plus line-delimited JSON-RPC framing on its stdin/stdout. It impls the `tau_mcp::transport::Transport` trait from PR-1. `host_lifecycle::open(url, plan, options)` discriminates `"stdio:<cmd>"` URLs, builds a `tokio::process::Command`, calls `gate.wrap_spawn(&plan, cmd.as_std_mut())` (same shape as `plugin_host::process::spawn`), spawns under tokio, drives the MCP handshake (initialize → wait for response → tools/list → wait), and returns an `McpClient` carrying the live `McpStdioServer` (as `Arc<dyn Transport>`) and the captured `ServerContract`. The in-tree fixture binary (`tests/fixtures/mock-mcp-server/`) is a small standalone Rust bin that reads JSON-RPC from stdin, writes responses to stdout, exposes one deterministic `echo` tool, supports a `--scenario` env-var-driven flag set (`handshake_slow`, `crash_on_call`, `refuse_initialize`) for failure-shape tests.

**Tech Stack:** Rust 2021, `tokio` (`process`, `io`, `time`), `serde_json`, `tau-mcp` (Transport trait, protocol + contract types), `tau-runtime-tokio` (`process_gate::ProcessCapabilityGate`), `tau-ports` (`CapabilityPlan`).

**Branch:** `feat/beta-3-pr-2-stdio-transport` (already created off `origin/main`; main now contains PR-1 at `33c3de3`).

**Worktree:** `/Users/titouanlebocq/code/tau-worktrees/beta-3-pr-2-stdio`.

**Spec reference:** `docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md` — particularly §2 (crate layout — `tau-mcp-tokio` deps now include `tau-runtime-tokio`), §8.1 (boot flow), §9 (sandbox model — stdio = `ProcessGate::wrap_spawn`), §15 (PR-2 scope).

**Locked architectural decisions consumed from the spec:**
- Q2 stdio transport in v0.
- Q7 reuse of `ProcessGate::wrap_spawn` for stdio MCP servers; URL-scheme discriminator (`stdio:<cmd>`).
- Q8 cassette format already exists in `tau-mcp::cassette`; PR-2 does NOT add a "cassette-as-transport" adapter (deferred to PR-3 alongside HTTP).

---

## Files map

### Modified
| File | Responsibility |
|---|---|
| `crates/tau-mcp-tokio/Cargo.toml` | Add `tau-runtime-tokio` dep (workspace); add runtime deps (`thiserror`, additional `tokio` features `process`, `io-util`, `time`, `macros`); add dev-deps for tests. |
| `crates/tau-mcp-tokio/src/lib.rs` | Re-export `host_lifecycle::open`, `McpClient`, `McpClientOptions`, `LifecycleError`. Re-export `transport_stdio::{McpStdioServer, StdioSpawnError}`. |
| `crates/tau-mcp-tokio/src/transport_stdio/mod.rs` | Replace doc-only stub with module split (`spawn.rs`, `framer.rs`, `server.rs`) + re-exports. |
| `crates/tau-mcp-tokio/src/host_lifecycle/mod.rs` | Replace doc-only stub with module split (`url.rs`, `client.rs`, `handshake.rs`, `open.rs`) + re-exports. |

### Created (NEW)
| File | Responsibility |
|---|---|
| `crates/tau-mcp-tokio/src/transport_stdio/spawn.rs` | `spawn(cmd: tokio::process::Command, gate: &dyn DynProcessCapabilityGate, plan: &CapabilityPlan) -> Result<tokio::process::Child, StdioSpawnError>`. Wraps the std::Command via `gate.wrap_spawn`, then spawns under tokio. |
| `crates/tau-mcp-tokio/src/transport_stdio/framer.rs` | Line-delimited JSON-RPC framer over `AsyncRead`/`AsyncWrite`. `JsonLineFramer` struct with `read_message()` / `write_message()` methods. |
| `crates/tau-mcp-tokio/src/transport_stdio/server.rs` | `McpStdioServer` struct holding the child + framers. Impls `tau_mcp::transport::Transport`. |
| `crates/tau-mcp-tokio/src/transport_stdio/error.rs` | `StdioSpawnError` + (re-exported) `StdioTransportError`. |
| `crates/tau-mcp-tokio/src/host_lifecycle/url.rs` | URL discriminator: `parse_url(s: &str) -> Result<McpUrl, UrlParseError>` returning `Stdio { cmd: Vec<String> }` for `stdio:<command>` (HTTP variants land in PR-3). |
| `crates/tau-mcp-tokio/src/host_lifecycle/handshake.rs` | `drive_handshake(transport: &dyn Transport, options: &McpClientOptions) -> Result<ServerContract, HandshakeError>` — sends `initialize` + `tools/list`, builds a `ServerContract` from the responses. |
| `crates/tau-mcp-tokio/src/host_lifecycle/client.rs` | `McpClient` struct holding `Arc<dyn Transport>` + `ServerContract` + control channels (cancel, shutdown). |
| `crates/tau-mcp-tokio/src/host_lifecycle/open.rs` | `open(url, plan, gate, options) -> Result<McpClient, LifecycleError>` — top-level entrypoint composing parse-url → spawn → framers → handshake → ready client. |
| `crates/tau-mcp-tokio/src/host_lifecycle/error.rs` | `LifecycleError`, `HandshakeError`, `UrlParseError`. |
| `crates/tau-mcp-tokio/tests/fixtures/mock-mcp-server/Cargo.toml` | Tiny bin crate, NOT a workspace member. |
| `crates/tau-mcp-tokio/tests/fixtures/mock-mcp-server/src/main.rs` | Mock server — reads JSON-RPC lines from stdin, writes responses to stdout, exposes `echo` tool, supports `TAU_MCP_FIXTURE_SCENARIO` env var. |
| `crates/tau-mcp-tokio/tests/fixtures/build_mock_server.rs` | Build-script-style helper (a `tests/common/` module, not literally a build.rs) that locates / builds the fixture binary and returns its path for integration tests. |
| `crates/tau-mcp-tokio/tests/common/mod.rs` | Test-helper module: `mock_server_path()` returns the absolute path to the compiled fixture binary; `passthrough_gate()` returns an `Arc<dyn DynProcessCapabilityGate>` using `PassthroughSandbox` for tests that don't need real OS enforcement. |
| `crates/tau-mcp-tokio/tests/stdio_lifecycle.rs` | End-to-end stdio tests: handshake-happy-path, handshake-timeout, mid-call-cancel, transport-closed-mid-call, server-refuses-initialize, sandbox-validation-rejection. |

### Deleted
- None — all files in `transport_stdio/` and `host_lifecycle/` are scaffold-only stubs from PR-1 and get content added, not deleted.

---

## Standing constraints (re-read before EVERY cargo / git command)

From `CLAUDE.md` — non-negotiable. Same shape as PR-1's standing constraints; abbreviated here:

| Command | Shape |
|---|---|
| Build / check | `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo {check,build} -p <crate>` |
| Test (nextest) | `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo nextest run -p <crate>` |
| Clippy | `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo clippy -p <crate> --all-targets -- -D warnings` |
| Fmt check | `timeout 30 env CARGO_TARGET_DIR=target/agent-<role> cargo fmt --check -p <crate>` |
| Commits | `git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "..."` |
| Push | `git push --no-verify -u origin feat/beta-3-pr-2-stdio-transport` |
| Auto-merge | `gh pr merge <N> --auto` BARE (the repo runs a merge queue; do NOT pass `--squash`/`--delete-branch`/`--merge` — the queue rejects them). |

`<role>` per task: `impl` for the implementer; `verify` for verifications.

Per-PR-1-experience addenda:
- **Auto-merge drops after a fail-then-rerun.** If any non-required check fails (incl. infra flakes like the llvm-cov "no space on device" we hit in PR-1), GitHub disables auto-merge enrollment. Re-enroll explicitly: `gh pr merge <N> --auto` after rerun.
- **The merge queue uses `mergeQueueEntry` (GraphQL), NOT `autoMergeRequest`.** `gh pr view --json autoMergeRequest` will show null even when enrolled. Use `gh api graphql -f query='query{repository(owner:"tau-rs",name:"tau"){pullRequest(number:<N>){mergeQueueEntry{state position}}}}'` to confirm enrollment.
- **macOS `tau-cli::cmd_chat_persistence::chat_ephemeral_writes_no_file`** is a recurring flake (hit in PR-1's merge-queue run; documented in 2026-05-27 doctests-round-2 + round-3 memory entries). On failure: rerun via `gh run rerun <run-id> --failed` + re-enroll auto-merge.

---

## Phase 1 — Cargo.toml + workspace deps

### Task 1.1: Extend `crates/tau-mcp-tokio/Cargo.toml`

**Files:**
- Modify: `crates/tau-mcp-tokio/Cargo.toml`

- [ ] **Step 1: Read the current file to see the PR-1 baseline.**

Run: `Read crates/tau-mcp-tokio/Cargo.toml`.

- [ ] **Step 2: Replace the `[dependencies]` and `[dev-dependencies]` blocks with the following.**

```toml
[dependencies]
tau-mcp           = { workspace = true }
tau-domain        = { workspace = true, features = ["serde"] }
tau-ports         = { workspace = true, features = ["serde"] }
# Sandbox integration: tau-runtime-tokio::process_gate::wrap_spawn (PR-2).
# HTTP transport (PR-3) doesn't need this; the dep is stdio-driven for now.
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
] }
tracing           = { workspace = true }

[dev-dependencies]
tau-runtime-tokio = { workspace = true, features = ["test-support"] }
tokio             = { workspace = true, features = ["test-util", "fs"] }
tempfile          = { workspace = true }
```

If `tempfile` is not yet in `[workspace.dependencies]` of the root Cargo.toml, check first — if absent, add it: `tempfile = "3"`. If it already exists, just reference it via `workspace = true`.

- [ ] **Step 3: `cargo check` to confirm the new deps resolve.**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio`

Expected: clean compile.

- [ ] **Step 4: Commit.**

```
git add crates/tau-mcp-tokio/Cargo.toml Cargo.toml Cargo.lock
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio): wire transport_stdio deps (tau-runtime-tokio + tokio process/io)"
```

---

## Phase 2 — URL discriminator + transport_stdio scaffolding

### Task 2.1: `host_lifecycle/url.rs` — `parse_url`

**Files:**
- Create: `crates/tau-mcp-tokio/src/host_lifecycle/url.rs`
- Create: `crates/tau-mcp-tokio/src/host_lifecycle/error.rs`

- [ ] **Step 1: Write `error.rs` with the lifecycle/handshake/url error enums.**

```rust
//! Error types for the host_lifecycle layer.

use thiserror::Error;

/// Failure to parse an MCP server URL from `[tools.<name>] mcp = "..."`.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum UrlParseError {
    /// Empty URL after stripping whitespace.
    #[error("MCP URL is empty")]
    Empty,
    /// URL scheme is not recognized in v0 (`stdio:` lands in PR-2,
    /// `http`/`https` land in PR-3, all others are rejected).
    #[error("unsupported MCP URL scheme: {scheme:?}")]
    UnsupportedScheme {
        /// The scheme observed (e.g. `"ws"`, `"file"`).
        scheme: String,
    },
    /// `stdio:` URL had an empty command after the prefix.
    #[error("stdio: URL has empty command after prefix")]
    EmptyStdioCommand,
}

/// Failure during the MCP handshake (initialize / tools/list).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HandshakeError {
    /// Transport-level error during handshake.
    #[error("transport error during handshake: {0}")]
    Transport(String),
    /// Server returned an error response to `initialize` or `tools/list`.
    #[error("server returned error during handshake: code={code} message={message}")]
    ServerError {
        /// JSON-RPC error code.
        code: i32,
        /// JSON-RPC error message.
        message: String,
    },
    /// Handshake exceeded the configured timeout.
    #[error("handshake timed out after {millis}ms")]
    Timeout {
        /// Configured timeout in milliseconds.
        millis: u64,
    },
    /// Server's response shape was malformed.
    #[error("malformed handshake response: {0}")]
    Malformed(String),
}

/// Failure during host_lifecycle::open.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LifecycleError {
    /// URL parse failure.
    #[error("URL parse: {0}")]
    UrlParse(#[from] UrlParseError),
    /// Subprocess spawn failure (stdio transport).
    #[error("stdio spawn: {0}")]
    StdioSpawn(#[from] crate::transport_stdio::StdioSpawnError),
    /// Handshake failure.
    #[error("handshake: {0}")]
    Handshake(#[from] HandshakeError),
}
```

- [ ] **Step 2: Write `url.rs`.**

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

use alloc::string::String;
use alloc::vec::Vec;

use crate::host_lifecycle::error::UrlParseError;

extern crate alloc;

/// Parsed MCP server URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpUrl {
    /// Subprocess MCP server. The vec is the command argv.
    Stdio {
        /// argv to spawn (first element is the binary).
        cmd: Vec<String>,
    },
}

/// Parse an MCP URL string into a typed `McpUrl`.
///
/// Currently accepts only `stdio:<command>` — HTTP variants land in PR-3.
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
        // Shell-split the command. v0 uses naive whitespace splitting —
        // future may grow to handle quoted args, but real MCP server
        // commands (`npx --yes @modelcontextprotocol/server-weather`,
        // `uvx mcp-server-fetch`) don't need quoting.
        let cmd = rest.split_whitespace().map(String::from).collect();
        return Ok(McpUrl::Stdio { cmd });
    }
    // PR-3 will add http/https arms here.
    let scheme = s
        .split(':')
        .next()
        .unwrap_or("")
        .to_string();
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
    fn http_rejected_in_pr2_with_correct_scheme() {
        let err = parse_url("https://mcp.example.com").expect_err("should reject in PR-2");
        match err {
            UrlParseError::UnsupportedScheme { scheme } => {
                assert_eq!(scheme, "https");
            }
            other => panic!("expected UnsupportedScheme, got {other:?}"),
        }
    }

    #[test]
    fn unknown_scheme_rejected() {
        let err = parse_url("ws://example.com").expect_err("should reject");
        match err {
            UrlParseError::UnsupportedScheme { scheme } => {
                assert_eq!(scheme, "ws");
            }
            other => panic!("expected UnsupportedScheme, got {other:?}"),
        }
    }
}
```

- [ ] **Step 3: Update `host_lifecycle/mod.rs` to declare submodules + re-export.**

Replace the entire stub `host_lifecycle/mod.rs` content with:

```rust
//! Host lifecycle for a contracted MCP server.
//!
//! `open(url, plan, gate, options)` is the v0 entrypoint: parse the URL,
//! spawn (stdio) or dial (HTTP — PR-3), drive the MCP handshake, return
//! a live `McpClient`.

pub mod client;
pub mod error;
pub mod handshake;
pub mod open;
pub mod url;

pub use client::{McpClient, McpClientOptions};
pub use error::{HandshakeError, LifecycleError, UrlParseError};
pub use open::open;
pub use url::{parse_url, McpUrl};
```

Note: `client::McpClient`, `client::McpClientOptions`, `handshake::*`, and `open::open` don't exist yet — they're added in subsequent tasks. To keep `cargo check` happy mid-phase, add minimal stub files for `client.rs`, `handshake.rs`, `open.rs` now (each `// placeholder; filled in Task X`), and replace them in subsequent tasks. The pattern matches PR-1's Phase 1 stub-then-overwrite.

Create `crates/tau-mcp-tokio/src/host_lifecycle/{client,handshake,open}.rs` with the placeholder body:

```rust
//! Placeholder; filled in Task 3.x of the PR-2 plan.
```

For client.rs, also add minimal stub types so the `pub use` block resolves:

```rust
//! Placeholder McpClient + McpClientOptions stubs; filled in Task 4.2.
pub struct McpClient;
pub struct McpClientOptions;
```

For open.rs:

```rust
//! Placeholder open() stub; filled in Task 4.3.
pub async fn open() -> () { () }
```

- [ ] **Step 4: Run the URL tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp-tokio -E 'test(host_lifecycle::url::tests)'`

Expected: 5 tests pass.

- [ ] **Step 5: Commit.**

```
git add crates/tau-mcp-tokio/src/host_lifecycle/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/host_lifecycle): URL discriminator + error types + module shells"
```

### Task 2.2: `transport_stdio/error.rs` — `StdioSpawnError`

**Files:**
- Create: `crates/tau-mcp-tokio/src/transport_stdio/error.rs`

- [ ] **Step 1: Write `error.rs`.**

```rust
//! Error types for the stdio transport.

use thiserror::Error;
use tau_ports::CapabilityError;

/// Failure during stdio MCP server spawn.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StdioSpawnError {
    /// `ProcessCapabilityGate::validate_plan` (which `wrap_spawn` calls
    /// internally) refused the plan.
    #[error("capability gate refused plan: {0}")]
    SandboxRefused(#[from] CapabilityError),
    /// `tokio::process::Command::spawn` failed (binary missing,
    /// permission denied, etc.).
    #[error("tokio spawn failed: {0}")]
    TokioSpawn(String),
}

impl From<std::io::Error> for StdioSpawnError {
    fn from(e: std::io::Error) -> Self {
        StdioSpawnError::TokioSpawn(format!("{e}"))
    }
}

/// Failure during a stdio transport read/write.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StdioTransportError {
    /// I/O error on child stdin/stdout.
    #[error("I/O on stdio transport: {0}")]
    Io(String),
    /// One of the framed JSON-RPC lines was not valid JSON.
    #[error("malformed JSON on stdio transport: {0}")]
    Json(String),
    /// The child process exited mid-conversation.
    #[error("child process exited (status: {status})")]
    ChildExited {
        /// Child exit status as a string (cross-platform).
        status: String,
    },
}

impl From<std::io::Error> for StdioTransportError {
    fn from(e: std::io::Error) -> Self {
        StdioTransportError::Io(format!("{e}"))
    }
}

impl From<serde_json::Error> for StdioTransportError {
    fn from(e: serde_json::Error) -> Self {
        StdioTransportError::Json(format!("{e}"))
    }
}
```

- [ ] **Step 2: Update `transport_stdio/mod.rs` to declare submodules + re-export.**

Replace the stub content with:

```rust
//! Subprocess stdio MCP transport.
//!
//! `McpStdioServer` wraps a sandboxed `tokio::process::Child` plus
//! line-delimited JSON-RPC framing on its stdin/stdout. It impls
//! `tau_mcp::transport::Transport`.

pub mod error;
pub mod framer;
pub mod server;
pub mod spawn;

pub use error::{StdioSpawnError, StdioTransportError};
pub use server::McpStdioServer;
pub use spawn::spawn;
```

Then create placeholder `framer.rs`, `server.rs`, `spawn.rs` with `//! Placeholder; filled in Task 2.x.`

- [ ] **Step 3: `cargo check -p tau-mcp-tokio` to confirm the scaffold builds.**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio`

Expected: clean compile.

- [ ] **Step 4: Commit.**

```
git add crates/tau-mcp-tokio/src/transport_stdio/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/transport_stdio): error types + module shells"
```

---

## Phase 3 — line-delimited JSON-RPC framer + spawn

### Task 3.1: `framer.rs` — line-delimited JSON-RPC

**Files:**
- Modify: `crates/tau-mcp-tokio/src/transport_stdio/framer.rs`

MCP over stdio uses **line-delimited JSON-RPC** (per MCP spec rev 2025-03-26 — every message is one JSON object terminated by `\n`). Content-Length framing exists in JSON-RPC tradition but MCP picked line-delim for simplicity. The framer reads/writes those lines.

- [ ] **Step 1: Write the failing test first (TDD).**

Append to `framer.rs`:

```rust
//! Line-delimited JSON-RPC framer over async I/O.
//!
//! Per MCP stdio transport spec: every JSON-RPC message is one JSON
//! object terminated by `\n`. The framer reads lines from an
//! `AsyncBufRead` and writes them to an `AsyncWrite`, deserializing
//! / serializing through `JsonRpcMessage`.

use tau_mcp::protocol::JsonRpcMessage;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::transport_stdio::error::StdioTransportError;

/// Line-delimited JSON-RPC framer.
pub struct JsonLineFramer<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    reader: BufReader<R>,
    writer: W,
    line_buf: String,
}

impl<R, W> JsonLineFramer<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    /// Construct a framer over the given reader+writer.
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
            line_buf: String::new(),
        }
    }

    /// Read one MCP message. Returns `Ok(None)` on EOF (clean close).
    pub async fn read_message(&mut self) -> Result<Option<JsonRpcMessage>, StdioTransportError> {
        self.line_buf.clear();
        let n = self.reader.read_line(&mut self.line_buf).await?;
        if n == 0 {
            return Ok(None);
        }
        let trimmed = self.line_buf.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            // Skip blank lines as a robustness measure.
            return self.read_message_boxed().await;
        }
        let msg: JsonRpcMessage = serde_json::from_str(trimmed)?;
        Ok(Some(msg))
    }

    fn read_message_boxed<'a>(
        &'a mut self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<JsonRpcMessage>, StdioTransportError>> + Send + 'a>> {
        Box::pin(self.read_message())
    }

    /// Write one MCP message followed by `\n`.
    pub async fn write_message(&mut self, msg: &JsonRpcMessage) -> Result<(), StdioTransportError> {
        let bytes = serde_json::to_vec(msg)?;
        self.writer.write_all(&bytes).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_mcp::protocol::jsonrpc::{
        JsonRpcMessage, JsonRpcRequest, JsonRpcResponse, RequestId, JSONRPC_VERSION,
    };
    use tokio::io::duplex;

    #[tokio::test(flavor = "current_thread")]
    async fn write_then_read_round_trips_request() {
        let (peer_r, mut peer_w) = duplex(4096);
        let (mut my_r, _my_w) = duplex(4096);

        // Use peer's write as MY read.
        let msg = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::Number(7),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({"name":"echo"})),
        });

        let bytes = serde_json::to_vec(&msg).unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut peer_w, &bytes).await.unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut peer_w, b"\n").await.unwrap();
        drop(peer_w);

        let mut framer = JsonLineFramer::new(peer_r, &mut my_r);
        let received = framer.read_message().await.unwrap().expect("got a message");
        assert_eq!(received, msg);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn eof_returns_none() {
        let (peer_r, peer_w) = duplex(4096);
        let (mut my_r, _my_w) = duplex(4096);
        drop(peer_w);  // EOF immediately

        let mut framer = JsonLineFramer::new(peer_r, &mut my_r);
        let received = framer.read_message().await.unwrap();
        assert!(received.is_none(), "EOF should yield Ok(None), got {received:?}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn malformed_json_errors() {
        let (peer_r, mut peer_w) = duplex(4096);
        let (mut my_r, _my_w) = duplex(4096);
        tokio::io::AsyncWriteExt::write_all(&mut peer_w, b"not json\n").await.unwrap();
        drop(peer_w);

        let mut framer = JsonLineFramer::new(peer_r, &mut my_r);
        let err = framer.read_message().await.expect_err("should error");
        assert!(matches!(err, StdioTransportError::Json(_)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn skip_blank_lines_then_read_message() {
        let (peer_r, mut peer_w) = duplex(4096);
        let (mut my_r, _my_w) = duplex(4096);

        let msg = JsonRpcMessage::Response(JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::Number(1),
            result: Some(serde_json::json!({"ok":true})),
            error: None,
        });
        let bytes = serde_json::to_vec(&msg).unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut peer_w, b"\n\n").await.unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut peer_w, &bytes).await.unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut peer_w, b"\n").await.unwrap();
        drop(peer_w);

        let mut framer = JsonLineFramer::new(peer_r, &mut my_r);
        let received = framer.read_message().await.unwrap().expect("got a message");
        assert_eq!(received, msg);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn write_message_writes_one_line_with_trailing_newline() {
        let (mut peer_r, peer_w) = duplex(4096);
        let (_my_r, my_w) = duplex(4096);
        let mut framer = JsonLineFramer::new(_my_r, peer_w);

        let msg = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::Number(0),
            method: "initialize".to_string(),
            params: None,
        });
        framer.write_message(&msg).await.unwrap();

        let mut buf = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut peer_r, &mut buf).await.unwrap();
        let s = std::str::from_utf8(&buf).unwrap();
        assert!(s.ends_with('\n'));
        assert_eq!(s.matches('\n').count(), 1);
        drop(my_w);
    }
}
```

- [ ] **Step 2: Run the framer tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp-tokio -E 'test(transport_stdio::framer::tests)'`

Expected: 5 tests pass.

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp-tokio/src/transport_stdio/framer.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/transport_stdio): line-delimited JSON-RPC framer"
```

### Task 3.2: `spawn.rs` — sandboxed subprocess spawn

**Files:**
- Modify: `crates/tau-mcp-tokio/src/transport_stdio/spawn.rs`

- [ ] **Step 1: Write `spawn.rs`.**

```rust
//! Sandboxed subprocess spawn for stdio MCP servers.
//!
//! Wraps a `tokio::process::Command` via
//! `tau_runtime_tokio::process_gate::DynProcessCapabilityGate::wrap_spawn`
//! exactly the same way `plugin_host::process::spawn` does — the
//! `CapabilityPlan` is honored at the OS boundary
//! (landlock/seccomp/sandbox-exec/podman per the four sandbox adapters).
//!
//! After `wrap_spawn` succeeds, the command is spawned under tokio.
//! Stdin / stdout / stderr handles are piped so the caller can wire
//! them into a `JsonLineFramer`.

use std::process::Stdio;
use std::sync::Arc;

use tau_ports::CapabilityPlan;
use tau_runtime_tokio::process_gate::DynProcessCapabilityGate;
use tokio::process::{Child, Command};

use crate::transport_stdio::error::StdioSpawnError;

/// Spawn an MCP server subprocess under the given capability gate.
///
/// The command's stdin/stdout/stderr are piped; the caller wires
/// `child.stdin.take()` and `child.stdout.take()` into a
/// `JsonLineFramer`.
///
/// # Errors
///
/// - [`StdioSpawnError::SandboxRefused`] — the capability gate refused
///   the plan (e.g. the plan demands a sandbox shape the gate adapter
///   doesn't support on this target).
/// - [`StdioSpawnError::TokioSpawn`] — `tokio::process::Command::spawn`
///   failed (binary missing, permission denied, etc.).
pub async fn spawn(
    mut cmd: Command,
    gate: Arc<dyn DynProcessCapabilityGate>,
    plan: &CapabilityPlan,
) -> Result<Child, StdioSpawnError> {
    // 1. Pipe stdin/stdout. Stderr is piped too so tests can assert on it
    //    if needed; production wiring may swap to inherit() for logs.
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    // 2. The sandbox gate operates on `std::process::Command`. Access
    //    via the same `as_std_mut()` shape `plugin_host::process::spawn`
    //    uses.
    let _handle = gate
        .wrap_spawn(plan, cmd.as_std_mut())
        .await
        .map_err(StdioSpawnError::SandboxRefused)?;

    // 3. Spawn under tokio. The CapabilityHandle returned by wrap_spawn
    //    is dropped here in PR-2 — the existing plugin_host's spawn
    //    flow drops it after handshake completes too. (apply_post_spawn
    //    + handle lifetime management land in a future cleanup if
    //    needed; v0 stdio MCP doesn't need post-spawn cap probes.)
    let child = cmd.spawn().map_err(StdioSpawnError::from)?;
    Ok(child)
}
```

- [ ] **Step 2: `cargo check` to confirm.**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio`

Expected: clean compile.

- [ ] **Step 3: Commit.**

(Tests for `spawn` are integration-level — they need a real binary to spawn — so they land in Phase 6's `stdio_lifecycle.rs`. No unit test here.)

```
git add crates/tau-mcp-tokio/src/transport_stdio/spawn.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/transport_stdio): sandboxed subprocess spawn via wrap_spawn"
```

### Task 3.3: `server.rs` — `McpStdioServer` + Transport impl

**Files:**
- Modify: `crates/tau-mcp-tokio/src/transport_stdio/server.rs`

- [ ] **Step 1: Write `server.rs`.**

```rust
//! `McpStdioServer` — stdio MCP server handle.
//!
//! Owns the spawned `tokio::process::Child` plus a `JsonLineFramer`
//! over its stdin/stdout. Impls `tau_mcp::transport::Transport`.

use std::pin::Pin;
use std::sync::Arc;

use tau_mcp::protocol::JsonRpcMessage;
use tau_mcp::transport::Transport;
use tau_mcp::McpError;
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

use crate::transport_stdio::framer::JsonLineFramer;
use crate::transport_stdio::error::StdioTransportError;

/// Live stdio MCP server.
///
/// Constructed by `host_lifecycle::open` after `spawn` completes;
/// passed by `Arc` into the `McpClient`.
pub struct McpStdioServer {
    /// The framer wraps the child's stdin/stdout. Mutex-guarded because
    /// `Transport::send_message` and `Transport::next_message` are
    /// `&self` (not `&mut`) per the trait shape.
    framer: Mutex<JsonLineFramer<ChildStdout, ChildStdin>>,
    /// The child handle — kept so dropping `McpStdioServer` kills the
    /// child (`Command::kill_on_drop(true)` was set in `spawn`).
    _child: Mutex<Child>,
}

impl McpStdioServer {
    /// Construct from a spawned child. Steals stdin/stdout out of the
    /// child handle.
    ///
    /// # Errors
    ///
    /// - Returns `McpError::Transport` if the child's stdin or stdout
    ///   were not piped (caller bug — `spawn` is supposed to pipe them).
    pub fn from_child(mut child: Child) -> Result<Arc<Self>, McpError> {
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Transport("child has no stdin pipe".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Transport("child has no stdout pipe".into()))?;
        let framer = JsonLineFramer::new(stdout, stdin);
        Ok(Arc::new(Self {
            framer: Mutex::new(framer),
            _child: Mutex::new(child),
        }))
    }
}

impl Transport for McpStdioServer {
    fn send_message<'a>(
        &'a self,
        msg: &'a JsonRpcMessage,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), McpError>> + Send + 'a>> {
        Box::pin(async move {
            let mut framer = self.framer.lock().await;
            framer
                .write_message(msg)
                .await
                .map_err(|e| convert_transport_error(e))
        })
    }

    fn next_message<'a>(
        &'a self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Option<JsonRpcMessage>, McpError>> + Send + 'a>>
    {
        Box::pin(async move {
            let mut framer = self.framer.lock().await;
            framer
                .read_message()
                .await
                .map_err(|e| convert_transport_error(e))
        })
    }
}

fn convert_transport_error(e: StdioTransportError) -> McpError {
    match e {
        StdioTransportError::Io(s) => McpError::Transport(s),
        StdioTransportError::Json(s) => McpError::Serde(s),
        StdioTransportError::ChildExited { status } => {
            McpError::Transport(format!("child exited: {status}"))
        }
    }
}
```

- [ ] **Step 2: `cargo check + clippy`.**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp-tokio -- -D warnings
```

Expected: clean.

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp-tokio/src/transport_stdio/server.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/transport_stdio): McpStdioServer + Transport impl"
```

---

## Phase 4 — host_lifecycle: handshake + McpClient + open()

### Task 4.1: `handshake.rs` — drive initialize + tools/list

**Files:**
- Modify: `crates/tau-mcp-tokio/src/host_lifecycle/handshake.rs`

- [ ] **Step 1: Write `handshake.rs`.**

```rust
//! MCP handshake driver.
//!
//! Sends `initialize` + `tools/list` over a Transport, captures the
//! responses, and builds a `ServerContract` from them.

use std::time::Duration;

use tau_mcp::contract::{ContractTool, ServerContract};
use tau_mcp::protocol::{
    initialize::{ClientInfo, InitializeRequest, InitializeResponse},
    jsonrpc::{
        JsonRpcMessage, JsonRpcRequest, JsonRpcResponse, RequestId, JSONRPC_VERSION,
    },
    tools::{ToolsListRequest, ToolsListResponse},
};
use tau_mcp::transport::Transport;
use tokio::time::timeout;
use tracing::{debug, instrument};

use crate::host_lifecycle::error::HandshakeError;

/// MCP protocol version tau speaks.
pub const MCP_PROTOCOL_VERSION: &str = "2025-03-26";

/// Handshake options (timeout, client info).
#[derive(Debug, Clone)]
pub struct HandshakeOptions {
    /// Timeout for the entire handshake (initialize + tools/list).
    pub handshake_timeout: Duration,
    /// Client name reported to the server.
    pub client_name: String,
    /// Client version reported to the server.
    pub client_version: String,
}

impl Default for HandshakeOptions {
    fn default() -> Self {
        Self {
            handshake_timeout: Duration::from_secs(30),
            client_name: "tau".to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// Drive the MCP handshake. Returns a `ServerContract` capturing the
/// server's reported info + tools/list snapshot.
#[instrument(name = "mcp_handshake", skip(transport, options), fields(
    client_name = %options.client_name,
    handshake_timeout_ms = options.handshake_timeout.as_millis() as u64,
))]
pub async fn drive_handshake(
    transport: &dyn Transport,
    options: &HandshakeOptions,
) -> Result<ServerContract, HandshakeError> {
    let inner = async {
        // 1. initialize
        let init_req = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::Number(0),
            method: "initialize".to_string(),
            params: Some(
                serde_json::to_value(InitializeRequest {
                    protocol_version: MCP_PROTOCOL_VERSION.to_string(),
                    client_info: ClientInfo {
                        name: options.client_name.clone(),
                        version: options.client_version.clone(),
                        additional: Default::default(),
                    },
                    capabilities: None,
                })
                .map_err(|e| HandshakeError::Malformed(format!("encode initialize: {e}")))?,
            ),
        });
        send(transport, &init_req).await?;
        let init_resp = recv_response_for(transport, &RequestId::Number(0)).await?;
        let init_result: InitializeResponse = serde_json::from_value(init_resp)
            .map_err(|e| HandshakeError::Malformed(format!("decode initialize response: {e}")))?;
        debug!(
            server_name = %init_result.server_info.name,
            server_version = %init_result.server_info.version,
            "initialize response decoded"
        );

        // 2. tools/list
        let list_req = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::Number(1),
            method: "tools/list".to_string(),
            params: Some(serde_json::to_value(ToolsListRequest::default()).unwrap_or_default()),
        });
        send(transport, &list_req).await?;
        let list_resp = recv_response_for(transport, &RequestId::Number(1)).await?;
        let list_result: ToolsListResponse = serde_json::from_value(list_resp)
            .map_err(|e| HandshakeError::Malformed(format!("decode tools/list response: {e}")))?;
        debug!(
            tools_count = list_result.tools.len(),
            "tools/list response decoded"
        );

        // 3. Build the ServerContract. v0 leaves per-tool caps empty
        // (no MCP-spec field for them yet); PR-4's lowering pass
        // intersects them with the author's envelope.
        let contract = ServerContract::from_handshake(init_result, list_result, |_| Vec::new());
        Ok::<_, HandshakeError>(contract)
    };

    timeout(options.handshake_timeout, inner)
        .await
        .map_err(|_| HandshakeError::Timeout {
            millis: options.handshake_timeout.as_millis() as u64,
        })?
}

async fn send(transport: &dyn Transport, msg: &JsonRpcMessage) -> Result<(), HandshakeError> {
    transport
        .send_message(msg)
        .await
        .map_err(|e| HandshakeError::Transport(format!("{e}")))
}

/// Receive messages until we see a response matching `expected_id`.
/// Ignores notifications and out-of-order responses (logs but skips).
async fn recv_response_for(
    transport: &dyn Transport,
    expected_id: &RequestId,
) -> Result<serde_json::Value, HandshakeError> {
    loop {
        let msg = transport
            .next_message()
            .await
            .map_err(|e| HandshakeError::Transport(format!("{e}")))?
            .ok_or_else(|| HandshakeError::Transport("peer closed mid-handshake".into()))?;
        match msg {
            JsonRpcMessage::Response(JsonRpcResponse { id, result, error, .. })
                if &id == expected_id =>
            {
                if let Some(e) = error {
                    return Err(HandshakeError::ServerError {
                        code: e.code,
                        message: e.message,
                    });
                }
                return Ok(result.unwrap_or(serde_json::Value::Null));
            }
            other => {
                debug!(
                    received_kind = ?std::mem::discriminant(&other),
                    expected_id = ?expected_id,
                    "skipping unexpected handshake message"
                );
            }
        }
    }
}
```

- [ ] **Step 2: `cargo check + clippy`.**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp-tokio -- -D warnings
```

Expected: clean.

- [ ] **Step 3: Commit.**

(Handshake tests live in Phase 6 integration — they need a real or mock server.)

```
git add crates/tau-mcp-tokio/src/host_lifecycle/handshake.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/host_lifecycle): handshake driver (initialize + tools/list → ServerContract)"
```

### Task 4.2: `client.rs` — `McpClient` + `McpClientOptions`

**Files:**
- Modify: `crates/tau-mcp-tokio/src/host_lifecycle/client.rs`

- [ ] **Step 1: Replace the placeholder with the real client.**

```rust
//! `McpClient` — live MCP server handle returned by `open`.
//!
//! Carries the `Arc<dyn Transport>` (a `McpStdioServer` in PR-2, a
//! Streamable HTTP transport in PR-3, possibly cassette-replay in
//! tests) and the captured `ServerContract` from the handshake.
//!
//! PR-5 will add per-tool routing and the inbound-dispatch task; PR-2
//! ships the bare client + a `call_tool` convenience for tests.

use std::sync::Arc;
use std::time::Duration;

use tau_mcp::contract::ServerContract;
use tau_mcp::protocol::{
    jsonrpc::{
        JsonRpcMessage, JsonRpcRequest, JsonRpcResponse, RequestId, JSONRPC_VERSION,
    },
    tools::{ToolsCallRequest, ToolsCallResponse},
};
use tau_mcp::transport::Transport;
use tau_mcp::McpError;
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::host_lifecycle::handshake::HandshakeOptions;

/// Options for an `McpClient` (handshake settings + tool-call defaults).
#[derive(Debug, Clone)]
pub struct McpClientOptions {
    /// Handshake settings (timeout, client info).
    pub handshake: HandshakeOptions,
    /// Default per-tool-call timeout.
    pub call_timeout: Duration,
}

impl Default for McpClientOptions {
    fn default() -> Self {
        Self {
            handshake: HandshakeOptions::default(),
            call_timeout: Duration::from_secs(60),
        }
    }
}

/// Live MCP server handle.
pub struct McpClient {
    transport: Arc<dyn Transport>,
    contract: ServerContract,
    options: McpClientOptions,
    next_id: Mutex<i64>,
}

impl McpClient {
    /// Construct from a live transport + already-completed handshake.
    /// PR-2 internal; callers use `host_lifecycle::open`.
    pub(crate) fn new(
        transport: Arc<dyn Transport>,
        contract: ServerContract,
        options: McpClientOptions,
    ) -> Self {
        Self {
            transport,
            contract,
            options,
            // ids 0 and 1 were consumed by handshake; next is 2.
            next_id: Mutex::new(2),
        }
    }

    /// The captured server contract (initialize + tools/list snapshot).
    pub fn contract(&self) -> &ServerContract {
        &self.contract
    }

    /// Call a server-side tool by name with the given JSON args.
    /// Honors `McpClientOptions::call_timeout`.
    pub async fn call_tool(
        &self,
        server_tool_name: &str,
        args: serde_json::Value,
    ) -> Result<ToolsCallResponse, McpError> {
        let id = {
            let mut next = self.next_id.lock().await;
            let id = *next;
            *next += 1;
            id
        };
        let req = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::Number(id),
            method: "tools/call".to_string(),
            params: Some(serde_json::to_value(ToolsCallRequest {
                name: server_tool_name.to_string(),
                arguments: Some(args),
            })?),
        });
        let inner = async {
            self.transport.send_message(&req).await?;
            let resp = recv_response_for(&*self.transport, &RequestId::Number(id)).await?;
            let result: ToolsCallResponse = serde_json::from_value(resp)?;
            Ok::<_, McpError>(result)
        };
        timeout(self.options.call_timeout, inner)
            .await
            .map_err(|_| McpError::Transport(format!(
                "tools/call {server_tool_name:?} timed out after {}ms",
                self.options.call_timeout.as_millis()
            )))?
    }

    /// Borrow the live transport (PR-5 wires the inbound-dispatch task
    /// here).
    pub fn transport(&self) -> &Arc<dyn Transport> {
        &self.transport
    }
}

async fn recv_response_for(
    transport: &dyn Transport,
    expected_id: &RequestId,
) -> Result<serde_json::Value, McpError> {
    loop {
        let msg = transport
            .next_message()
            .await?
            .ok_or_else(|| McpError::Transport("peer closed mid-call".into()))?;
        match msg {
            JsonRpcMessage::Response(JsonRpcResponse { id, result, error, .. })
                if &id == expected_id =>
            {
                if let Some(e) = error {
                    return Err(McpError::Protocol(format!(
                        "server returned error code={} msg={}",
                        e.code, e.message
                    )));
                }
                return Ok(result.unwrap_or(serde_json::Value::Null));
            }
            _ => continue,
        }
    }
}
```

- [ ] **Step 2: `cargo check + clippy`.**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp-tokio -- -D warnings
```

Expected: clean.

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp-tokio/src/host_lifecycle/client.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/host_lifecycle): McpClient + call_tool convenience"
```

### Task 4.3: `open.rs` — top-level entrypoint

**Files:**
- Modify: `crates/tau-mcp-tokio/src/host_lifecycle/open.rs`

- [ ] **Step 1: Replace the placeholder.**

```rust
//! `open(url, plan, gate, options)` — v0 entrypoint.
//!
//! Composes URL parse → spawn → framers → handshake → live `McpClient`.

use std::sync::Arc;

use tau_ports::CapabilityPlan;
use tau_runtime_tokio::process_gate::DynProcessCapabilityGate;
use tokio::process::Command;
use tracing::{info, instrument};

use crate::host_lifecycle::client::{McpClient, McpClientOptions};
use crate::host_lifecycle::error::LifecycleError;
use crate::host_lifecycle::handshake::drive_handshake;
use crate::host_lifecycle::url::{parse_url, McpUrl};
use crate::transport_stdio::{server::McpStdioServer, spawn};

/// Open a connection to an MCP server.
///
/// Returns a live `McpClient` once the MCP handshake has completed.
#[instrument(name = "mcp_open", skip(plan, gate, options), fields(url = url))]
pub async fn open(
    url: &str,
    plan: &CapabilityPlan,
    gate: Arc<dyn DynProcessCapabilityGate>,
    options: McpClientOptions,
) -> Result<McpClient, LifecycleError> {
    let parsed = parse_url(url)?;
    match parsed {
        McpUrl::Stdio { cmd } => {
            // Build a tokio Command from argv. cmd[0] = program; cmd[1..] = args.
            let mut command = Command::new(&cmd[0]);
            command.args(&cmd[1..]);

            info!(stdio_cmd = ?cmd, "spawning stdio MCP server");
            let child = spawn(command, gate, plan).await?;

            // Wrap in McpStdioServer (steals stdin/stdout into the framer).
            let transport = McpStdioServer::from_child(child).map_err(|e| {
                LifecycleError::Handshake(crate::host_lifecycle::error::HandshakeError::Transport(
                    format!("{e}"),
                ))
            })?;

            // Drive the handshake.
            let contract = drive_handshake(&*transport, &options.handshake).await?;
            info!(
                server_name = %contract.server_info.name,
                tools_count = contract.tools.len(),
                "MCP handshake complete"
            );

            Ok(McpClient::new(transport, contract, options))
        }
    }
}
```

- [ ] **Step 2: `cargo check + clippy`.**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp-tokio -- -D warnings
```

Expected: clean.

- [ ] **Step 3: Update `crates/tau-mcp-tokio/src/lib.rs` to re-export the new public surface.**

Replace the existing `lib.rs` with:

```rust
//! tau-mcp-tokio — tokio runtime + transports for tau-mcp.
//!
//! PR-2 ships the stdio transport + host lifecycle. PR-3 adds HTTP +
//! cassette-as-transport. PR-5 wires the `McpBridge` ToolDispatcher.

pub mod bridge;
pub mod host_lifecycle;
pub mod transport_http;
pub mod transport_stdio;

pub use host_lifecycle::{
    open, HandshakeError, LifecycleError, McpClient, McpClientOptions, McpUrl, UrlParseError,
};
pub use transport_stdio::{McpStdioServer, StdioSpawnError, StdioTransportError};
```

- [ ] **Step 4: Commit.**

```
git add crates/tau-mcp-tokio/src/host_lifecycle/open.rs crates/tau-mcp-tokio/src/lib.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/host_lifecycle): top-level open() + lib.rs re-exports"
```

---

## Phase 5 — mock-mcp-server fixture binary

The fixture is a small standalone Rust binary that lives under `tests/fixtures/` and is **not a workspace member** (so it doesn't pull workspace deps unnecessarily, and so the tests can build it on demand with `cargo build --manifest-path ...`).

### Task 5.1: mock-mcp-server `Cargo.toml`

**Files:**
- Create: `crates/tau-mcp-tokio/tests/fixtures/mock-mcp-server/Cargo.toml`

- [ ] **Step 1: Write the manifest.**

```toml
[package]
name = "tau-mcp-mock-server"
version = "0.0.0"
edition = "2021"
publish = false
# Intentionally NOT a workspace member — keeps its dep tree minimal and
# lets the integration tests build it on demand at test time.

[[bin]]
name = "tau-mcp-mock-server"
path = "src/main.rs"

[dependencies]
serde_json = "1"

[workspace]
# Empty workspace block: cargo treats this manifest as its own
# workspace root so it doesn't try to attach to the outer workspace.
```

- [ ] **Step 2: Tell the outer workspace to EXCLUDE this directory.**

Modify the root `Cargo.toml`'s `[workspace]` block — add an `exclude` entry (if not already present add the key):

```toml
[workspace]
resolver = "2"
members = [ ... existing ... ]
exclude = [
    "crates/tau-mcp-tokio/tests/fixtures/mock-mcp-server",
]
```

Verify the existing root Cargo.toml — if `exclude` already exists, add to it; if not, add the key under `[workspace]`.

- [ ] **Step 3: `cargo metadata` on the root to confirm the exclude works.**

Run: `timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo metadata --format-version 1 --no-deps > /dev/null`

Expected: exit 0, no errors about the fixture crate.

- [ ] **Step 4: Commit.**

```
git add crates/tau-mcp-tokio/tests/fixtures/mock-mcp-server/Cargo.toml Cargo.toml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/fixtures): mock-mcp-server crate scaffold (out-of-workspace)"
```

### Task 5.2: mock-mcp-server `main.rs`

**Files:**
- Create: `crates/tau-mcp-tokio/tests/fixtures/mock-mcp-server/src/main.rs`

- [ ] **Step 1: Write the fixture server.**

```rust
//! Mock MCP server for tau-mcp-tokio integration tests.
//!
//! Speaks line-delimited JSON-RPC over stdin/stdout. Supports:
//!
//! - `initialize` — returns `{name:"mock", version:"0.0.0"}` + protocol.
//! - `tools/list` — returns one tool, `echo`, schema {message: string}.
//! - `tools/call name=echo` — returns the message in a text content block.
//! - Anything else — JSON-RPC error code -32601 method not found.
//!
//! Scenarios via `TAU_MCP_FIXTURE_SCENARIO` env var:
//!
//! - `happy` (default) — normal behavior.
//! - `handshake_slow` — sleeps 5s before responding to initialize.
//! - `refuse_initialize` — returns JSON-RPC error to initialize.
//! - `crash_on_call` — exits with code 137 on first tools/call.

use std::io::{BufRead, Write};
use std::time::Duration;

fn main() {
    let scenario = std::env::var("TAU_MCP_FIXTURE_SCENARIO").unwrap_or_else(|_| "happy".into());
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) if l.trim().is_empty() => continue,
            Ok(l) => l,
            Err(_) => break,
        };
        let req: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,  // malformed, ignore
        };
        let id = req.get("id").cloned();
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");

        let resp_payload = match method {
            "initialize" => {
                if scenario == "handshake_slow" {
                    std::thread::sleep(Duration::from_secs(5));
                }
                if scenario == "refuse_initialize" {
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32603, "message": "refused by scenario"}
                    })
                } else {
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "protocolVersion": "2025-03-26",
                            "serverInfo": {"name": "mock", "version": "0.0.0"}
                        }
                    })
                }
            }
            "tools/list" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [
                        {
                            "name": "echo",
                            "description": "Echo a message",
                            "inputSchema": {
                                "type": "object",
                                "properties": {"message": {"type": "string"}},
                                "required": ["message"]
                            }
                        }
                    ]
                }
            }),
            "tools/call" => {
                if scenario == "crash_on_call" {
                    std::process::exit(137);
                }
                let params = req.get("params").cloned().unwrap_or_default();
                let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or_default();
                if name == "echo" {
                    let msg = args.get("message").and_then(|m| m.as_str()).unwrap_or("");
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {"content": [{"type": "text", "text": msg}]}
                    })
                } else {
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32602, "message": format!("unknown tool: {name}")}
                    })
                }
            }
            _ => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": format!("method not found: {method}")}
            }),
        };

        let bytes = serde_json::to_vec(&resp_payload).unwrap();
        out.write_all(&bytes).unwrap();
        out.write_all(b"\n").unwrap();
        out.flush().unwrap();
    }
}
```

- [ ] **Step 2: Build the fixture to verify it compiles.**

```
timeout 60 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl/mock-mcp-server-build cargo build --manifest-path crates/tau-mcp-tokio/tests/fixtures/mock-mcp-server/Cargo.toml
```

Expected: clean build; binary at `target/agent-impl/mock-mcp-server-build/debug/tau-mcp-mock-server`.

- [ ] **Step 3: Smoke-test the binary manually (optional but cheap).**

```
echo '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}' | target/agent-impl/mock-mcp-server-build/debug/tau-mcp-mock-server | head -1
```

Expected: a JSON line with `result.protocolVersion = "2025-03-26"`.

- [ ] **Step 4: Commit.**

```
git add crates/tau-mcp-tokio/tests/fixtures/mock-mcp-server/src/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/fixtures): mock-mcp-server binary (echo tool + scenarios)"
```

### Task 5.3: `tests/common/mod.rs` — fixture-binary discovery + passthrough gate

**Files:**
- Create: `crates/tau-mcp-tokio/tests/common/mod.rs`

- [ ] **Step 1: Write the common helper.**

```rust
//! Shared test helpers for tau-mcp-tokio integration tests.
//!
//! - `mock_server_path()` — builds the in-tree fixture binary on demand
//!   and returns its path.
//! - `passthrough_gate()` — returns a `DynProcessCapabilityGate` impl
//!   that doesn't enforce anything (for tests that aren't exercising
//!   sandbox refusal).

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, OnceLock};

use tau_runtime_tokio::process_gate::DynProcessCapabilityGate;

static MOCK_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Path to the fixture binary. First call builds it; subsequent calls
/// return the cached path.
pub fn mock_server_path() -> &'static PathBuf {
    MOCK_PATH.get_or_init(|| {
        let manifest = format!(
            "{}/tests/fixtures/mock-mcp-server/Cargo.toml",
            env!("CARGO_MANIFEST_DIR")
        );
        let target_dir = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| {
            format!("{}/../../target", env!("CARGO_MANIFEST_DIR"))
        });
        let fixture_target = format!("{target_dir}/mock-mcp-server-build");
        let status = Command::new(env!("CARGO"))
            .args(["build", "--manifest-path", &manifest])
            .env("CARGO_TARGET_DIR", &fixture_target)
            .env("CARGO_INCREMENTAL", "0")
            .status()
            .expect("build fixture");
        assert!(status.success(), "fixture build failed");
        PathBuf::from(format!(
            "{fixture_target}/debug/tau-mcp-mock-server"
        ))
    })
}

/// A `DynProcessCapabilityGate` that allows everything. Reuses the
/// existing `PassthroughSandbox` from `tau-runtime-tokio::process_gate`.
pub fn passthrough_gate() -> Arc<dyn DynProcessCapabilityGate> {
    use tau_runtime_tokio::process_gate::passthrough::PassthroughSandbox;
    Arc::new(PassthroughSandbox)
}
```

- [ ] **Step 2: Commit.**

```
git add crates/tau-mcp-tokio/tests/common/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "test(tau-mcp-tokio): common helpers (mock_server_path + passthrough_gate)"
```

---

## Phase 6 — Integration tests

### Task 6.1: `tests/stdio_lifecycle.rs` — happy-path + failure scenarios

**Files:**
- Create: `crates/tau-mcp-tokio/tests/stdio_lifecycle.rs`

- [ ] **Step 1: Write the integration test file.**

```rust
//! End-to-end stdio MCP tests against the in-tree mock-mcp-server.

mod common;

use std::time::Duration;

use tau_mcp_tokio::host_lifecycle::handshake::HandshakeOptions;
use tau_mcp_tokio::{open, LifecycleError, McpClientOptions};
use tau_ports::CapabilityPlan;

use crate::common::{mock_server_path, passthrough_gate};

fn stdio_url() -> String {
    format!("stdio:{}", mock_server_path().to_string_lossy())
}

fn empty_plan() -> CapabilityPlan {
    CapabilityPlan::default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_happy_path() {
    let client = open(
        &stdio_url(),
        &empty_plan(),
        passthrough_gate(),
        McpClientOptions::default(),
    )
    .await
    .expect("open succeeds");
    assert_eq!(client.contract().server_info.name, "mock");
    assert_eq!(client.contract().tools.len(), 1);
    assert_eq!(client.contract().tools[0].name, "echo");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tools_call_echo_round_trips() {
    let client = open(
        &stdio_url(),
        &empty_plan(),
        passthrough_gate(),
        McpClientOptions::default(),
    )
    .await
    .expect("open succeeds");
    let resp = client
        .call_tool("echo", serde_json::json!({"message":"hi"}))
        .await
        .expect("call succeeds");
    assert_eq!(resp.content.len(), 1);
    match &resp.content[0] {
        tau_mcp::protocol::tools::ContentBlock::Text { text } => assert_eq!(text, "hi"),
        other => panic!("expected Text block, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_timeout_fires_under_slow_scenario() {
    let mut cmd_with_env = stdio_url();
    // Naive: inject env var via setting it in the parent process.
    // tokio::process::Command inherits the parent env by default;
    // we set the scenario inline so the spawned child sees it.
    std::env::set_var("TAU_MCP_FIXTURE_SCENARIO", "handshake_slow");

    let options = McpClientOptions {
        handshake: HandshakeOptions {
            handshake_timeout: Duration::from_millis(500),
            ..HandshakeOptions::default()
        },
        ..McpClientOptions::default()
    };
    let err = open(
        &cmd_with_env,
        &empty_plan(),
        passthrough_gate(),
        options,
    )
    .await
    .expect_err("should time out");
    match err {
        LifecycleError::Handshake(
            tau_mcp_tokio::host_lifecycle::HandshakeError::Timeout { .. },
        ) => {}
        other => panic!("expected Timeout, got {other:?}"),
    }
    std::env::remove_var("TAU_MCP_FIXTURE_SCENARIO");
    let _ = cmd_with_env;  // silence unused warning across helper changes
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_refuses_initialize_surfaces_server_error() {
    std::env::set_var("TAU_MCP_FIXTURE_SCENARIO", "refuse_initialize");
    let err = open(
        &stdio_url(),
        &empty_plan(),
        passthrough_gate(),
        McpClientOptions::default(),
    )
    .await
    .expect_err("should fail");
    match err {
        LifecycleError::Handshake(
            tau_mcp_tokio::host_lifecycle::HandshakeError::ServerError { code, .. },
        ) => {
            assert_eq!(code, -32603);
        }
        other => panic!("expected ServerError, got {other:?}"),
    }
    std::env::remove_var("TAU_MCP_FIXTURE_SCENARIO");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_crash_mid_call_surfaces_transport_error() {
    std::env::set_var("TAU_MCP_FIXTURE_SCENARIO", "crash_on_call");
    let client = open(
        &stdio_url(),
        &empty_plan(),
        passthrough_gate(),
        McpClientOptions::default(),
    )
    .await
    .expect("handshake succeeds");
    let result = client
        .call_tool("echo", serde_json::json!({"message":"x"}))
        .await;
    assert!(result.is_err(), "call should fail when child crashed");
    std::env::remove_var("TAU_MCP_FIXTURE_SCENARIO");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn url_parse_failure_propagates() {
    let err = open(
        "ws://invalid",
        &empty_plan(),
        passthrough_gate(),
        McpClientOptions::default(),
    )
    .await
    .expect_err("should fail");
    match err {
        LifecycleError::UrlParse(_) => {}
        other => panic!("expected UrlParse, got {other:?}"),
    }
}
```

**Important — env-var-driven scenarios are racy under parallel test execution.** nextest runs each `#[tokio::test]` in its own process by default for crate-level tests, so the env-var pattern works. If a future nextest config changes that, replace the env var with a command-line arg the fixture binary parses (parsing `argv[1]` for the scenario name).

- [ ] **Step 2: Run the integration tests.**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp-tokio --test stdio_lifecycle
```

Expected: 6 tests pass. First run builds the fixture binary (~30-60s); subsequent runs use the cached binary.

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp-tokio/tests/stdio_lifecycle.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "test(tau-mcp-tokio): stdio_lifecycle integration tests (6 scenarios)"
```

### Task 6.2: sandbox-refusal integration test

**Files:**
- Create: `crates/tau-mcp-tokio/tests/stdio_sandbox.rs`

This test confirms that a `ProcessCapabilityGate` that REFUSES a plan during `wrap_spawn` surfaces as `StdioSpawnError::SandboxRefused` → `LifecycleError::StdioSpawn`.

- [ ] **Step 1: Write the test.**

```rust
//! Confirms `host_lifecycle::open` propagates a sandbox refusal as
//! `LifecycleError::StdioSpawn(StdioSpawnError::SandboxRefused)`.

mod common;

use std::pin::Pin;
use std::sync::Arc;

use tau_ports::{CapabilityError, CapabilityHandle, CapabilityPlan};
use tau_runtime_tokio::process_gate::DynProcessCapabilityGate;
use tau_runtime_core::builder::DynCapabilityGate;

use tau_mcp_tokio::{open, LifecycleError, McpClientOptions, StdioSpawnError};

use crate::common::mock_server_path;

/// A `DynProcessCapabilityGate` that always refuses `wrap_spawn` with a
/// fixed error.
struct AlwaysRefuseGate;

impl DynCapabilityGate for AlwaysRefuseGate {}

impl DynProcessCapabilityGate for AlwaysRefuseGate {
    fn wrap_spawn<'a>(
        &'a self,
        _plan: &'a CapabilityPlan,
        _cmd: &'a mut std::process::Command,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<CapabilityHandle, CapabilityError>> + 'a>>
    {
        Box::pin(async {
            Err(CapabilityError::new("test gate refuses every plan"))
        })
    }

    fn apply_post_spawn<'a>(
        &'a self,
        _plan: &'a CapabilityPlan,
        _child_pid: i32,
        _handle: &'a mut CapabilityHandle,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), CapabilityError>> + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sandbox_refusal_propagates() {
    let url = format!("stdio:{}", mock_server_path().to_string_lossy());
    let gate: Arc<dyn DynProcessCapabilityGate> = Arc::new(AlwaysRefuseGate);

    let err = open(
        &url,
        &CapabilityPlan::default(),
        gate,
        McpClientOptions::default(),
    )
    .await
    .expect_err("should refuse");
    match err {
        LifecycleError::StdioSpawn(StdioSpawnError::SandboxRefused(_)) => {}
        other => panic!("expected SandboxRefused, got {other:?}"),
    }
}
```

**Note on `CapabilityError::new`** — verify the actual constructor name by reading `tau_ports::CapabilityError` source. If `new()` doesn't exist, use whichever constructor / variant the type exposes. Most tau-ports error types follow the `Error::new(msg: impl Into<String>)` convention. If a different shape is needed (e.g. a typed variant), adapt the test to match.

- [ ] **Step 2: Run.**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp-tokio --test stdio_sandbox
```

Expected: 1 test passes.

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp-tokio/tests/stdio_sandbox.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "test(tau-mcp-tokio): sandbox refusal propagates as LifecycleError::StdioSpawn"
```

---

## Phase 7 — Final integration check + push + PR

### Task 7.1: Workspace-level checks

- [ ] **Step 1: Full `cargo check + nextest + clippy + fmt` for tau-mcp-tokio.**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp-tokio
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-mcp-tokio
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp-tokio --all-targets -- -D warnings
timeout 30  env CARGO_TARGET_DIR=target/agent-impl cargo fmt --check -p tau-mcp-tokio
```

Expected: all green. ~5 unit tests (framer) + ~5 url tests + ~7 integration tests = ~17 tests.

- [ ] **Step 2: Confirm tau-mcp (the upstream crate) didn't regress.**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp
```

Expected: 53 tests still passing (no PR-2 changes to tau-mcp).

- [ ] **Step 3: Canary three downstream crates to confirm pure-add.**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-pkg
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-tokio
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-cli
```

Expected: all clean.

- [ ] **Step 4: If `cargo fmt --check` flags anything, apply + commit.**

```
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-mcp-tokio
git status
```

If any files changed:

```
git add -A
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "style(tau-mcp-tokio): apply cargo fmt"
```

### Task 7.2: Push + open PR + auto-merge

- [ ] **Step 1: Push.**

```
git push --no-verify -u origin feat/beta-3-pr-2-stdio-transport
```

- [ ] **Step 2: Open the PR.**

```
gh pr create --title "β.3 MCP facilitator — PR-2: stdio transport + host lifecycle + fixture server" --body "$(cat <<'EOF'
## Summary

Second of six PRs in the β.3 MCP facilitator sub-project. Implements the subprocess (stdio) MCP transport in `tau-mcp-tokio::transport_stdio` (spawn via `ProcessGate::wrap_spawn` matching the existing plugin_host shape), the host-lifecycle layer (`open()` + handshake driver + `McpClient`) that drives `initialize` + `tools/list` end-to-end, and an in-tree mock-mcp-server fixture binary used by 7 integration tests covering: happy-path handshake, tools/call round-trip, handshake timeout, server-refuses-initialize, child-crash-mid-call, URL parse failure, and sandbox refusal propagation.

Spec: `docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md` §2/§8/§9/§15  
Plan: `docs/superpowers/plans/2026-06-01-beta-3-mcp-facilitator-pr-2.md`  
Previous PR: #280 (β.3 PR-1).

Stacks-on: nothing (independent of PR-3, PR-4 per spec's PR-2/3/4 fan-out).

## Test plan

- [ ] `cargo nextest run -p tau-mcp-tokio` green (~17 tests: 5 framer + 5 url + 7 integration).
- [ ] `cargo nextest run -p tau-mcp` still green (53 tests).
- [ ] `cargo clippy -p tau-mcp-tokio --all-targets -- -D warnings` clean.
- [ ] `cargo fmt --check -p tau-mcp-tokio` clean.
- [ ] No-op check on downstream crates (`tau-pkg`, `tau-runtime-tokio`, `tau-cli`).
- [ ] CI green on linux/macos/windows for both new crates.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Capture the PR number.

- [ ] **Step 3: Enroll auto-merge.**

```
gh pr merge <N> --auto
```

Bare form only — repo uses a merge queue; do NOT pass `--squash` or `--delete-branch` (queue rejects them).

- [ ] **Step 4: Watch CI; on merge-queue flake (e.g. `cmd_chat_persistence` on macOS), rerun + re-enroll auto-merge per the PR-1 dance.**

```
gh run rerun <run-id> --failed
gh pr merge <N> --auto
```

---

## Self-review checklist (run before declaring PR-2 done)

| Check | Status |
|---|---|
| `tau-mcp-tokio/Cargo.toml` carries `tau-runtime-tokio` dep + extra tokio features | Task 1.1 |
| `transport_stdio/` has framer.rs + spawn.rs + server.rs + error.rs (4 files) | Phase 3 |
| `host_lifecycle/` has url.rs + handshake.rs + client.rs + open.rs + error.rs (5 files) | Phase 2/4 |
| mock-mcp-server is NOT a workspace member (excluded in root Cargo.toml) | Task 5.1 |
| mock-mcp-server supports 4 scenarios (happy / handshake_slow / refuse_initialize / crash_on_call) | Task 5.2 |
| `tests/common/mod.rs` provides `mock_server_path()` + `passthrough_gate()` | Task 5.3 |
| 7 integration tests in `stdio_lifecycle.rs` + `stdio_sandbox.rs` | Phase 6 |
| Sandbox integration uses `gate.wrap_spawn(plan, cmd.as_std_mut())` — matches `plugin_host::process::spawn` shape | Task 3.2 |
| Total test count ≥17 | Task 7.1 |
| `cargo clippy --all-targets -- -D warnings` clean on tau-mcp-tokio | Task 7.1 |
| `cargo fmt --check` clean | Task 7.1 |
| Downstream canary checks clean (tau-pkg, tau-runtime-tokio, tau-cli) | Task 7.1 |
| Push used `--no-verify` (agent-runtime silent-kill avoidance) | Task 7.2 |
| Auto-merge enrolled via `gh pr merge <N> --auto` BARE | Task 7.2 |

---

## What's next: PR-3 through PR-6

PR-3 (HTTP transport + cassette replay) is independent of PR-2 and can start in parallel — fresh worktree off `origin/main`, brainstorm-or-skip → plan → execute, same shape as PR-2. PR-4 (lowering + lockfile) is also independent; PR-5 stacks on PR-4; PR-6 stacks on PR-4 + PR-5.

If you want to fan out 3-way: each subsequent PR gets its own worktree + branch + plan file, authored when its session starts.
