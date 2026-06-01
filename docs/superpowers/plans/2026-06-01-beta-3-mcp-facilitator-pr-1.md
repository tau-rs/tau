# β.3 MCP facilitator — PR-1: crate scaffolds + protocol types

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship PR-1 of six in the β.3 sub-project. Create the new `tau-mcp` and `tau-mcp-tokio` crates, their module skeletons, and the complete MCP protocol-type surface (JSON-RPC envelopes + the five v0 method payloads + notifications + cancellation). Author the contract layer (`ServerContract`, canonical-hash, `PinnedContract` (de)serializer), the `HostHandlers` trait with default-deny baseline, and the cassette message-level format types. Pure-add: no integration with `tau-runtime-core`, `tau-ir`, or `tau-cli` yet (those land in PR-4..PR-6).

**Architecture:** Two new crates, both `no_std`-friendly. `tau-mcp` carries pure types — JSON-RPC envelopes, MCP method payloads, contract shape, canonical hash (SHA-256 over canonical JSON per the β.2 encoder rules), pinned-contract I/O, `HostHandlers` trait, cassette JSONL format. `tau-mcp-tokio` exists as a scaffold (lib.rs + `transport_stdio/mod.rs` placeholder) with its `Transport` trait defined in `tau-mcp` so the tokio side only carries the I/O impls. No runtime integration yet. Per the spec's "PR-1..PR-3 can land in any order behind PR-1" — this PR is the foundation everyone else depends on.

**Tech Stack:** Rust 2021, `serde`, `serde_json`, `sha2` (canonical-hash), `thiserror`, `hashbrown`, `tau-domain` (Capability types), `tau-ports` (CapabilityPlan reuse). No `tokio` deps in `tau-mcp`. `tau-mcp-tokio` scaffold carries `tokio` workspace dep but no transport impl yet.

**Branch:** `feat/beta-3-pr-1-mcp-scaffolds` (created off `origin/main`; renames `feat/beta-3-mcp-facilitator-spec` which currently holds the spec commit at `aa02f6b`).

**Worktree:** `/Users/titouanlebocq/code/tau-worktrees/workflow-ir-beta-2-6-2` (re-used; β.2.6.2 already shipped and merged from here).

**Locked architectural decisions consumed from the spec:**
- Q1 Crate layout: `tau-mcp` (no_std-friendly) + `tau-mcp-tokio` (tokio-locked).
- Q5 IR shape (informs the `ContractTool` shape PR-4 will consume).
- Q8 Cassette format: transport-agnostic, message-level JSONL in `tau-mcp::cassette`.

---

## Files map

### Created (NEW)

**Workspace + ADR + branch rename:**
- `crates/tau-mcp/Cargo.toml`
- `crates/tau-mcp-tokio/Cargo.toml`
- `docs/decisions/0038-mcp-facilitator.md` (placeholder; finalized in PR-6)

**`tau-mcp` crate:**
- `crates/tau-mcp/src/lib.rs`
- `crates/tau-mcp/src/error.rs`
- `crates/tau-mcp/src/protocol/mod.rs`
- `crates/tau-mcp/src/protocol/jsonrpc.rs`
- `crates/tau-mcp/src/protocol/initialize.rs`
- `crates/tau-mcp/src/protocol/tools.rs`
- `crates/tau-mcp/src/protocol/sampling.rs`
- `crates/tau-mcp/src/protocol/roots.rs`
- `crates/tau-mcp/src/protocol/notifications.rs`
- `crates/tau-mcp/src/contract/mod.rs`
- `crates/tau-mcp/src/contract/server_contract.rs`
- `crates/tau-mcp/src/contract/canonical.rs`
- `crates/tau-mcp/src/contract/pinned.rs`
- `crates/tau-mcp/src/host/mod.rs`
- `crates/tau-mcp/src/host/handlers.rs`
- `crates/tau-mcp/src/cassette/mod.rs`
- `crates/tau-mcp/src/cassette/message.rs`
- `crates/tau-mcp/src/cassette/recorder.rs`
- `crates/tau-mcp/src/cassette/replayer.rs`
- `crates/tau-mcp/src/transport.rs`
- `crates/tau-mcp/tests/golden_canonical.rs`
- `crates/tau-mcp/tests/golden_cassette.rs`
- `crates/tau-mcp/tests/fixtures/canonical/empty.json`
- `crates/tau-mcp/tests/fixtures/canonical/weather.json`
- `crates/tau-mcp/tests/fixtures/cassette/weather-happy-path.jsonl`

**`tau-mcp-tokio` crate (scaffold-only in PR-1):**
- `crates/tau-mcp-tokio/src/lib.rs`
- `crates/tau-mcp-tokio/src/transport_stdio/mod.rs`
- `crates/tau-mcp-tokio/src/transport_http/mod.rs`
- `crates/tau-mcp-tokio/src/host_lifecycle/mod.rs`
- `crates/tau-mcp-tokio/src/bridge.rs`

### Modified

- `Cargo.toml` (workspace) — register the two new crates under `members`.
- `Cargo.toml` (workspace) — add `tau-mcp` + `tau-mcp-tokio` to `[workspace.dependencies]` so downstream crates can `tau-mcp = { workspace = true }` in PR-4..PR-6.

---

## Standing constraints (re-read before EVERY cargo / git command)

From `CLAUDE.md` — these are non-negotiable. Every cargo invocation MUST follow:

| Command | Shape | Timeout |
|---|---|---|
| Build / check | `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo {check,build} -p <crate>` | 180s |
| Test (nextest) | `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo nextest run -p <crate>` | 300s |
| Doctest | `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo test --doc -p <crate>` | 300s |
| Clippy | `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo clippy -p <crate>` | 240s |
| Fmt check | `timeout 30 env CARGO_TARGET_DIR=target/agent-<role> cargo fmt --check -p <crate>` | 30s |

Per-task `<role>` is specified inline (typically `impl` for the main implementing subagent, `verify` for any verification subagent).

**Commits:** `git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "..."`. The `--no-verify` is necessary because the lefthook test-native step can corrupt git identity (per `feedback_lefthook_corrupts_worktree.md`).

**Push:** `git push --no-verify -u origin feat/beta-3-pr-1-mcp-scaffolds`. CI is the gate (per `AGENT PUSH RULES` in CLAUDE.md — the lefthook deep gate kills `git push` mid-hook when invoked from agent runtime).

**Auto-merge:** after pushing, enrol via `gh pr merge <N> --auto` BARE only (no `--squash`/`--delete-branch` flags; auto_merge.flags changed — see `feedback_auto_merge_available.md`).

**Branch protection:** PRs must be up-to-date with `main` to merge. If main moves while CI runs, `gh pr update-branch <N>` adds a merge commit (auto-merge re-evaluates).

---

## Phase 0 — branch + worktree setup

### Task 0.1: Rename the spec branch to PR-1's branch and keep the spec commit as PR-1's first commit

**Files:** none (branch operation)

- [ ] **Step 1: Verify current branch state.**

```
git status
git log --oneline -3
```
Expected output includes branch `feat/beta-3-mcp-facilitator-spec` and most recent commit `aa02f6b docs(specs): β.3 — MCP facilitator design`.

- [ ] **Step 2: Rename the branch in place.**

```
git branch -m feat/beta-3-mcp-facilitator-spec feat/beta-3-pr-1-mcp-scaffolds
```

- [ ] **Step 3: Verify the rename.**

```
git branch --show-current
```
Expected: `feat/beta-3-pr-1-mcp-scaffolds`.

No commit — branch rename is HEAD-only.

---

## Phase 1 — workspace + crate scaffolds

### Task 1.1: Create `crates/tau-mcp/Cargo.toml`

**Files:**
- Create: `crates/tau-mcp/Cargo.toml`

- [ ] **Step 1: Write `crates/tau-mcp/Cargo.toml`.**

```toml
[package]
name = "tau-mcp"
description = "MCP (Model Context Protocol) facilitator types and contract layer — JSON-RPC envelopes, method payloads, canonical-hashed contracts, cassette format, host-handlers trait. no_std + alloc. Transport impls live in tau-mcp-tokio."
version.workspace      = true
edition.workspace      = true
rust-version.workspace = true
license.workspace      = true
repository.workspace   = true
authors.workspace      = true

[dependencies]
tau-domain = { workspace = true, default-features = false, features = ["serde"] }
tau-ports  = { workspace = true, default-features = false, features = ["serde"] }
serde      = { workspace = true, default-features = false, features = ["alloc", "derive"] }
serde_json = { workspace = true }
thiserror  = { workspace = true, default-features = false }
hashbrown  = { workspace = true }
# SHA-256 over canonical JSON bytes for contract_hash + schema_hash.
# Same shape as tau-ir uses for the workflow IR hash (β.2).
sha2       = { workspace = true, default-features = false }

[dev-dependencies]
serde_json = { workspace = true }
tokio      = { workspace = true, features = ["rt", "macros"] }

[features]
default            = ["with-std-adapters"]
# When on, enables std-backed sinks (file I/O on Recorder / Replayer).
with-std-adapters  = []
```

- [ ] **Step 2: Commit.**

```
git add crates/tau-mcp/Cargo.toml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp): add Cargo.toml scaffold"
```

### Task 1.2: Create `crates/tau-mcp-tokio/Cargo.toml`

**Files:**
- Create: `crates/tau-mcp-tokio/Cargo.toml`

- [ ] **Step 1: Write `crates/tau-mcp-tokio/Cargo.toml`.**

```toml
[package]
name = "tau-mcp-tokio"
description = "Tokio runtime + transports for tau-mcp. stdio (subprocess) and Streamable HTTP transports; host-lifecycle (handshake, keepalive, shutdown); McpBridge composable ToolDispatcher adapter. Scaffold only in PR-1; transports land in PR-2 and PR-3."
version.workspace      = true
edition.workspace      = true
rust-version.workspace = true
license.workspace      = true
repository.workspace   = true
authors.workspace      = true

[dependencies]
tau-mcp         = { workspace = true }
tau-domain      = { workspace = true, features = ["serde"] }
tau-ports       = { workspace = true, features = ["serde"] }
# tau-runtime-tokio dep added in PR-2 (for ProcessGate::Sandbox::wrap_spawn).
# Held back in PR-1 to keep scaffold lightweight and dep graph clean.
serde           = { workspace = true, features = ["derive"] }
serde_json      = { workspace = true }
thiserror       = { workspace = true }
tokio           = { workspace = true, features = ["rt", "rt-multi-thread", "macros", "io-util"] }

[dev-dependencies]
tokio           = { workspace = true, features = ["test-util"] }
```

- [ ] **Step 2: Commit.**

```
git add crates/tau-mcp-tokio/Cargo.toml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio): add Cargo.toml scaffold"
```

### Task 1.3: Register both crates in the workspace `Cargo.toml`

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Read the existing workspace file to find the right insertion point in `members` and `workspace.dependencies`.**

Run: `Read Cargo.toml` (whole file).

- [ ] **Step 2: Insert under `[workspace] members = [...]`.**

Add the two new lines AFTER `"crates/tau-pkg",` (alphabetical with the other `tau-` crates):

```toml
    "crates/tau-mcp",
    "crates/tau-mcp-tokio",
```

- [ ] **Step 3: Insert under `[workspace.dependencies]`.**

Add (alphabetical):

```toml
tau-mcp           = { path = "crates/tau-mcp",       version = "0.0.0", default-features = false }
tau-mcp-tokio     = { path = "crates/tau-mcp-tokio", version = "0.0.0", default-features = false }
```

Use the same version string the other intra-workspace deps in the file use (verify by reading around `tau-ir = { path = ..., version = ... }`).

- [ ] **Step 4: `cargo metadata` smoke test to verify the workspace parses.**

Run: `timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo metadata --format-version 1 --no-deps > /dev/null`
Expected: exit 0; stderr quiet.

- [ ] **Step 5: Commit.**

```
git add Cargo.toml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(workspace): register tau-mcp + tau-mcp-tokio crates"
```

### Task 1.4: Stub `tau-mcp/src/lib.rs` with `no_std` declaration and module placeholders

**Files:**
- Create: `crates/tau-mcp/src/lib.rs`
- Create: `crates/tau-mcp/src/protocol/mod.rs`
- Create: `crates/tau-mcp/src/contract/mod.rs`
- Create: `crates/tau-mcp/src/host/mod.rs`
- Create: `crates/tau-mcp/src/cassette/mod.rs`
- Create: `crates/tau-mcp/src/transport.rs`
- Create: `crates/tau-mcp/src/error.rs`

- [ ] **Step 1: Write `src/lib.rs`.**

```rust
//! tau-mcp — MCP (Model Context Protocol) facilitator types.
//!
//! Pure types + canonical-hash + cassette format. No I/O, no tokio.
//! Transports + lifecycle live in `tau-mcp-tokio`.
//!
//! # Modules
//!
//! - [`protocol`] — MCP wire types: JSON-RPC envelopes, the five v0 method
//!   payloads (`initialize`, `tools/list`, `tools/call`,
//!   `sampling/createMessage`, `roots/list`), notifications, cancellation.
//! - [`contract`] — `ServerContract` (the schema + capability declaration
//!   tau-build pins) + canonical hash (`Hash256` = SHA-256 of canonical
//!   JSON) + pinned-contract file (de)serializer.
//! - [`host`] — `HostHandlers` trait with default-deny baseline impl.
//! - [`cassette`] — transport-agnostic message-level recorder + replayer.
//! - [`transport`] — `Transport` trait shared by `tau-mcp-tokio` impls.
//!
//! # Spec
//!
//! [β.3 MCP facilitator design](https://github.com/LEBOCQTitouan/tau/blob/main/docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md).

#![no_std]
#![cfg_attr(test, allow(unused_extern_crates))]

extern crate alloc;

#[cfg(any(test, feature = "with-std-adapters"))]
extern crate std;

pub mod cassette;
pub mod contract;
pub mod error;
pub mod host;
pub mod protocol;
pub mod transport;

pub use error::McpError;
```

- [ ] **Step 2: Write `src/protocol/mod.rs`.**

```rust
//! MCP wire types — JSON-RPC envelopes and method-specific payloads.
//!
//! Per MCP spec revision 2025-03-26 (the version tau v0 targets). Method
//! payloads live in submodules; envelopes live in [`jsonrpc`].

pub mod initialize;
pub mod jsonrpc;
pub mod notifications;
pub mod roots;
pub mod sampling;
pub mod tools;

pub use jsonrpc::{
    JsonRpcError, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, RequestId, JSONRPC_VERSION,
};
```

- [ ] **Step 3: Write `src/contract/mod.rs`.**

```rust
//! Contract types — what `tau build` pins for an MCP server.
//!
//! A [`ServerContract`] captures the server's `tools/list` response +
//! per-tool capability declaration. The canonical-hash module produces a
//! `Hash256` over the canonical-JSON bytes (β.2 canonical-encoder rules
//! re-applied here). Pinned-contract file I/O lives in [`pinned`].

pub mod canonical;
pub mod pinned;
pub mod server_contract;

pub use canonical::{canonical_hash, Hash256};
pub use pinned::PinnedContract;
pub use server_contract::{ContractTool, ServerContract};
// Re-export from upstream protocol modules for ergonomic contract API.
pub use crate::protocol::initialize::ServerInfo;
pub use crate::protocol::tools::McpToolInputSchema;
```

- [ ] **Step 4: Write `src/host/mod.rs`.**

```rust
//! Host-side handlers for server-initiated MCP requests.
//!
//! When an MCP server sends an inbound request (sampling, roots, etc.),
//! tau-mcp-tokio's inbound dispatch routes it through an impl of
//! [`HostHandlers`]. v0 ships the two handlers the philosophy doc stars:
//! sampling (delegated inference) and roots (capability gate at fs
//! boundary). Default-deny baseline impl is [`DefaultDenyHandlers`].

pub mod handlers;

pub use handlers::{
    DefaultDenyHandlers, HostHandlers, InboundError,
};
```

- [ ] **Step 5: Write `src/cassette/mod.rs`.**

```rust
//! Transport-agnostic message-level cassette format.
//!
//! Captures MCP-message traffic at the handler-dispatch boundary (above
//! the transport layer), so the same cassette replays under any transport
//! (stdio, HTTP, future ws) and any host shell (tokio, wasm, embassy).
//!
//! Spec: design doc §11.

pub mod message;
pub mod recorder;
pub mod replayer;

pub use message::{CassetteMessage, Direction, MessageKind, CASSETTE_VERSION};
pub use recorder::Recorder;
pub use replayer::{ReplayError, Replayer};
```

- [ ] **Step 6: Write `src/transport.rs`.**

```rust
//! `Transport` trait — implemented by `tau-mcp-tokio` for stdio and HTTP.
//!
//! Defined here so the protocol layer + host loop are transport-agnostic
//! (γ.5 wasm / embassy shells can implement this trait against their own
//! I/O without taking a tokio dep).

use alloc::boxed::Box;
use core::future::Future;
use core::pin::Pin;

use crate::error::McpError;
use crate::protocol::JsonRpcMessage;

/// A bidirectional MCP transport.
///
/// `send_message` writes one MCP message to the wire (any framing the
/// transport requires happens inside the impl). `next_message` reads the
/// next inbound message; returns `Ok(None)` if the transport has cleanly
/// closed.
pub trait Transport: Send + Sync {
    /// Send one MCP message to the peer.
    fn send_message<'a>(
        &'a self,
        msg: &'a JsonRpcMessage,
    ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>>;

    /// Read the next inbound MCP message. Returns `Ok(None)` on clean
    /// close.
    fn next_message<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<JsonRpcMessage>, McpError>> + Send + 'a>>;
}
```

- [ ] **Step 7: Write `src/error.rs`.**

```rust
//! Error type for tau-mcp.

use alloc::string::String;
use thiserror::Error;

/// Errors surfaced by the tau-mcp protocol + transport layer.
///
/// Categories:
/// - [`McpError::Serde`] — JSON serde failure (envelope shape, payload
///   shape).
/// - [`McpError::Transport`] — transport-level failure (I/O, framing,
///   closed peer).
/// - [`McpError::Protocol`] — MCP-protocol violation (unexpected message
///   id, missing required field after deserialization).
/// - [`McpError::ContractDrift`] — runtime re-hash of `tools/list` does
///   not match the pinned `contract_hash`.
/// - [`McpError::Refused`] — host handler refused an inbound request
///   (e.g. sampling refused due to empty allowlist).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum McpError {
    /// JSON (de)serialization error.
    #[error("MCP serde error: {0}")]
    Serde(String),

    /// Transport-level error.
    #[error("MCP transport error: {0}")]
    Transport(String),

    /// Protocol violation.
    #[error("MCP protocol error: {0}")]
    Protocol(String),

    /// Contract hash drifted vs the pinned/lockfile value.
    #[error("MCP contract drift: observed {observed}, expected {expected}")]
    ContractDrift {
        /// Observed (live re-hashed) contract hash, lowercase hex.
        observed: String,
        /// Expected (pinned / lockfile) contract hash, lowercase hex.
        expected: String,
    },

    /// Host handler refused an inbound request.
    #[error("MCP inbound refused: {0}")]
    Refused(String),
}

impl From<serde_json::Error> for McpError {
    fn from(e: serde_json::Error) -> Self {
        McpError::Serde(alloc::format!("{e}"))
    }
}
```

- [ ] **Step 8: `cargo check` to verify the crate skeleton compiles (every module file referenced must exist — but the submodules don't yet, so this WILL fail at this step).** Skip step 8 — proceed to Phase 2 which creates the submodule files. The `cargo check` happens at the end of Phase 2.

- [ ] **Step 9: Commit.**

```
git add crates/tau-mcp/src/lib.rs crates/tau-mcp/src/protocol/mod.rs crates/tau-mcp/src/contract/mod.rs crates/tau-mcp/src/host/mod.rs crates/tau-mcp/src/cassette/mod.rs crates/tau-mcp/src/transport.rs crates/tau-mcp/src/error.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp): scaffold lib.rs + module shells + error type"
```

### Task 1.5: Stub `tau-mcp-tokio/src/lib.rs` with placeholder modules

**Files:**
- Create: `crates/tau-mcp-tokio/src/lib.rs`
- Create: `crates/tau-mcp-tokio/src/transport_stdio/mod.rs`
- Create: `crates/tau-mcp-tokio/src/transport_http/mod.rs`
- Create: `crates/tau-mcp-tokio/src/host_lifecycle/mod.rs`
- Create: `crates/tau-mcp-tokio/src/bridge.rs`

- [ ] **Step 1: Write `crates/tau-mcp-tokio/src/lib.rs`.**

```rust
//! tau-mcp-tokio — tokio runtime + transports for tau-mcp.
//!
//! Scaffold only in PR-1. stdio transport + sandbox-gated spawn land in
//! PR-2; Streamable HTTP transport + cassette replay-against-live land in
//! PR-3; the `McpBridge` ToolDispatcher adapter lands in PR-5.

pub mod bridge;
pub mod host_lifecycle;
pub mod transport_http;
pub mod transport_stdio;
```

- [ ] **Step 2: Write `crates/tau-mcp-tokio/src/transport_stdio/mod.rs`.**

```rust
//! Subprocess stdio transport for MCP servers.
//!
//! Scaffold only in PR-1. PR-2 fills this in with:
//!
//! - `spawn(cmd, &CapabilityPlan)` that wraps `tokio::process::Command`
//!   via `tau_runtime_tokio::process_gate::Sandbox::wrap_spawn`.
//! - line-delimited JSON-RPC framing over child stdout / stdin.
//! - `Transport` impl carrying the spawned child + framers.
//!
//! See the β.3 design doc §2 (crate layout) and §9 (sandbox model).
```

- [ ] **Step 3: Write `crates/tau-mcp-tokio/src/transport_http/mod.rs`.**

```rust
//! Streamable HTTP transport for MCP servers.
//!
//! Scaffold only in PR-1. PR-3 fills this in with:
//!
//! - `connect(url, &CapabilityPlan)` using reqwest + SSE parsing.
//! - Per-call net.http cap enforcement via host-pinning middleware.
//! - `Transport` impl carrying the HTTP client + SSE stream.
//!
//! See the β.3 design doc §2 + §9.
```

- [ ] **Step 4: Write `crates/tau-mcp-tokio/src/host_lifecycle/mod.rs`.**

```rust
//! Host lifecycle for a contracted MCP server.
//!
//! Scaffold only in PR-1. PR-2 (stdio) + PR-3 (HTTP) wire:
//!
//! - `open(url, &CapabilityPlan)` discriminates transport and dials.
//! - handshake: `initialize` + `tools/list`.
//! - keepalive + shutdown.
//!
//! See the β.3 design doc §2 + §8.
```

- [ ] **Step 5: Write `crates/tau-mcp-tokio/src/bridge.rs`.**

```rust
//! `McpBridge` — composable `ToolDispatcher` adapter.
//!
//! Scaffold only in PR-1. PR-5 fills this in with:
//!
//! - `BTreeMap<ToolId, (Arc<McpClient>, server_tool_name, caps)>`.
//! - `impl ToolDispatcher for McpBridge`.
//! - Outbound cap-gate enforcement.
//! - Composition with `tau-cli::ForwardingDispatcher`.
//!
//! See the β.3 design doc §8.
```

- [ ] **Step 6: `cargo check tau-mcp-tokio` to verify the scaffold compiles.**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio`

Expected: clean compile. (The crate has only mod-doc strings; no compile errors possible.)

- [ ] **Step 7: Commit.**

```
git add crates/tau-mcp-tokio/src/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio): scaffold lib.rs + placeholder modules"
```

---

## Phase 2 — ADR-0038 placeholder

### Task 2.1: Create the ADR placeholder

**Files:**
- Create: `docs/decisions/0038-mcp-facilitator.md`

- [ ] **Step 1: Read `docs/decisions/template.md` to follow the project's ADR shape.**

Run: `Read docs/decisions/template.md`.

- [ ] **Step 2: Write `docs/decisions/0038-mcp-facilitator.md`.**

```markdown
# ADR-0038 — MCP facilitator (β.3)

**Status:** Placeholder. Finalized in PR-6 of the β.3 sub-project, after
implementation truth is captured.

**Date:** 2026-06-01 (placeholder); finalize date set at PR-6 merge.

**Context:** ROADMAP §β.3 — MCP host runtime + capability gate at the
contract boundary + Workflow IR integration via the existing
`ToolImpl::Mcp` variant. See the
[β.3 design spec](../superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md)
for the locked architectural decisions (Q1–Q8) and the
[β.3 PR-1 plan](../superpowers/plans/2026-06-01-beta-3-mcp-facilitator-pr-1.md)
for the foundation this ADR will eventually document.

**Decision:** _Final ADR text authored in PR-6 from implementation
truth. The locked architectural decisions list in the design spec is
the authoritative source; PR-6 transposes them into ADR form with any
post-implementation revisions._

**Consequences:** _See PR-6._

**Supersedes / Superseded by:** none.

**References:**

- [β.3 design spec](../superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md)
- [tau philosophy](../explanation/tau-philosophy.md) (the MCP FACILITATOR
  block in *The architecture, in one picture*)
- ADR-0037 (workflow IR — defines the `ToolImpl::Mcp` variant β.3 wires up)
- ADR-0035 (bundle format — extended by β.3 to embed pinned contracts)
```

- [ ] **Step 3: Commit.**

```
git add docs/decisions/0038-mcp-facilitator.md
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "docs(adr): 0038 MCP facilitator placeholder"
```

---

## Phase 3 — JSON-RPC envelope types

### Task 3.1: Write the failing serde round-trip test for `JsonRpcRequest`

**Files:**
- Create: `crates/tau-mcp/src/protocol/jsonrpc.rs`

- [ ] **Step 1: Write `crates/tau-mcp/src/protocol/jsonrpc.rs` with the type declarations + an inline `#[cfg(test)]` round-trip test.**

```rust
//! JSON-RPC 2.0 envelopes used by MCP.
//!
//! MCP uses JSON-RPC 2.0 (https://www.jsonrpc.org/specification) as its
//! wire format, with `jsonrpc: "2.0"` on every envelope. Three envelope
//! kinds:
//!
//! - [`JsonRpcRequest`] — a method call expecting a response.
//! - [`JsonRpcResponse`] — a response (success `result` or error
//!   `JsonRpcError`).
//! - [`JsonRpcNotification`] — a fire-and-forget message (no `id`,
//!   no response).
//!
//! [`JsonRpcMessage`] is the discriminated-union shape used over the
//! wire — a single `serde_json::Value::Object` is parsed once and
//! routed to the right variant by the presence/absence of `id` and
//! `result`/`error`.

use alloc::string::String;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC version this implementation speaks.
pub const JSONRPC_VERSION: &str = "2.0";

/// JSON-RPC 2.0 request-id. Per spec: number or string (or null for
/// notifications — see [`JsonRpcNotification`] for those).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    /// Integer id (the common case in MCP).
    Number(i64),
    /// String id.
    String(String),
}

/// A JSON-RPC 2.0 request envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Request id (echoed in the response).
    pub id: RequestId,
    /// Method name (e.g. `"tools/call"`).
    pub method: String,
    /// Method-specific parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// A JSON-RPC 2.0 response envelope (success).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Echoed request id.
    pub id: RequestId,
    /// Success result; `None` if this is an error response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error payload; `None` on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// Error code (negative integers per JSON-RPC spec; MCP defines its
    /// own range for protocol-level errors).
    pub code: i32,
    /// Human-readable error message.
    pub message: String,
    /// Optional additional payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// A JSON-RPC 2.0 notification envelope (no id, no response expected).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Method name (e.g. `"notifications/progress"`).
    pub method: String,
    /// Method-specific parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// Discriminated-union envelope over the wire.
///
/// A peer receives bytes, parses them once into `serde_json::Value`,
/// and routes by the presence/absence of `id` and `result`/`error`.
/// The `#[serde(untagged)]` attribute lets serde do this routing
/// automatically; the variant order matters — Request is tried first
/// (has `id` AND `method`), then Response (has `id` AND
/// `result`/`error`), then Notification (no `id`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    /// A method-call request.
    Request(JsonRpcRequest),
    /// A response to a prior request.
    Response(JsonRpcResponse),
    /// A fire-and-forget notification.
    Notification(JsonRpcNotification),
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;
    use serde_json::json;

    #[test]
    fn request_round_trips() {
        let req = JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::Number(7),
            method: "tools/call".to_string(),
            params: Some(json!({"name":"get_forecast","arguments":{"lat":40.7,"lon":-74.0}})),
        };
        let bytes = serde_json::to_vec(&req).expect("serialize");
        let decoded: JsonRpcRequest = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(req, decoded);
    }

    #[test]
    fn response_round_trips() {
        let resp = JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::Number(7),
            result: Some(json!({"content":[{"type":"text","text":"Sunny, 72°F"}]})),
            error: None,
        };
        let bytes = serde_json::to_vec(&resp).expect("serialize");
        let decoded: JsonRpcResponse = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(resp, decoded);
    }

    #[test]
    fn notification_round_trips() {
        let n = JsonRpcNotification {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: "notifications/progress".to_string(),
            params: Some(json!({"progressToken":"call-7","progress":50,"total":100})),
        };
        let bytes = serde_json::to_vec(&n).expect("serialize");
        let decoded: JsonRpcNotification = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(n, decoded);
    }

    #[test]
    fn untagged_routing_request() {
        let wire = json!({
            "jsonrpc":"2.0","id":7,"method":"tools/call",
            "params":{"name":"get_forecast"}
        });
        let msg: JsonRpcMessage = serde_json::from_value(wire).expect("route");
        assert!(matches!(msg, JsonRpcMessage::Request(_)));
    }

    #[test]
    fn untagged_routing_response_success() {
        let wire = json!({"jsonrpc":"2.0","id":7,"result":{"content":[]}});
        let msg: JsonRpcMessage = serde_json::from_value(wire).expect("route");
        assert!(matches!(msg, JsonRpcMessage::Response(_)));
    }

    #[test]
    fn untagged_routing_response_error() {
        let wire = json!({"jsonrpc":"2.0","id":7,"error":{"code":-32601,"message":"method not found"}});
        let msg: JsonRpcMessage = serde_json::from_value(wire).expect("route");
        assert!(matches!(msg, JsonRpcMessage::Response(_)));
    }

    #[test]
    fn untagged_routing_notification() {
        let wire = json!({"jsonrpc":"2.0","method":"notifications/progress","params":{}});
        let msg: JsonRpcMessage = serde_json::from_value(wire).expect("route");
        assert!(matches!(msg, JsonRpcMessage::Notification(_)));
    }

    #[test]
    fn request_id_string_round_trips() {
        let id = RequestId::String("req-7".to_string());
        let bytes = serde_json::to_vec(&id).expect("serialize");
        assert_eq!(&bytes, b"\"req-7\"");
        let decoded: RequestId = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(id, decoded);
    }

    #[test]
    fn jsonrpc_message_vec_round_trips() {
        let msgs = vec![
            JsonRpcMessage::Request(JsonRpcRequest {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: RequestId::Number(1),
                method: "initialize".to_string(),
                params: None,
            }),
            JsonRpcMessage::Notification(JsonRpcNotification {
                jsonrpc: JSONRPC_VERSION.to_string(),
                method: "notifications/initialized".to_string(),
                params: None,
            }),
        ];
        let bytes = serde_json::to_vec(&msgs).expect("serialize");
        let decoded: alloc::vec::Vec<JsonRpcMessage> =
            serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(msgs, decoded);
    }
}
```

- [ ] **Step 2: Run the tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp --no-fail-fast`

Expected: all 8 tests pass. Note: this is the FIRST test run of the new crate; expect a fresh build (~1-2 min the first time).

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp/src/protocol/jsonrpc.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp/protocol): JSON-RPC envelope types + round-trip tests"
```

### Task 3.2: Initialize types (`InitializeRequest`/`Response`, `ClientInfo`, `ServerInfo`)

**Files:**
- Create: `crates/tau-mcp/src/protocol/initialize.rs`

- [ ] **Step 1: Write `src/protocol/initialize.rs` with types + round-trip tests.**

```rust
//! `initialize` method — first request the host sends.
//!
//! The host advertises its protocol version + client info; the server
//! responds with its protocol version + server info + capabilities.

use alloc::collections::BTreeMap;
use alloc::string::String;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `initialize` request params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitializeRequest {
    /// MCP protocol version the host speaks (tau v0 sends
    /// `"2025-03-26"`).
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    /// Client (host) info.
    #[serde(rename = "clientInfo")]
    pub client_info: ClientInfo,
    /// Client capabilities (per MCP spec — free-form map).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Value>,
}

/// `initialize` response result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitializeResponse {
    /// MCP protocol version the server speaks.
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    /// Server info.
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
    /// Server capabilities (per MCP spec — free-form map).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Value>,
}

/// Host-side client info (tau ships `name="tau"`, version = crate version).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientInfo {
    /// Client name (`"tau"` for tau-mcp).
    pub name: String,
    /// Client version (tau crate version string).
    pub version: String,
    /// Additional fields the server may report; preserved across
    /// (de)serialization.
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub additional: BTreeMap<String, Value>,
}

/// Server-side info reported by the MCP server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerInfo {
    /// Server name (e.g. `"weather-mcp"`).
    pub name: String,
    /// Server version string.
    pub version: String,
    /// Additional fields the server may report; preserved across
    /// (de)serialization.
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub additional: BTreeMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use serde_json::json;

    #[test]
    fn initialize_request_round_trips() {
        let req = InitializeRequest {
            protocol_version: "2025-03-26".to_string(),
            client_info: ClientInfo {
                name: "tau".to_string(),
                version: "0.0.0".to_string(),
                additional: BTreeMap::new(),
            },
            capabilities: Some(json!({"roots":{"listChanged":false}})),
        };
        let bytes = serde_json::to_vec(&req).expect("serialize");
        let decoded: InitializeRequest = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(req, decoded);
    }

    #[test]
    fn initialize_response_round_trips() {
        let resp = InitializeResponse {
            protocol_version: "2025-03-26".to_string(),
            server_info: ServerInfo {
                name: "weather".to_string(),
                version: "1.2.3".to_string(),
                additional: BTreeMap::new(),
            },
            capabilities: Some(json!({"tools":{"listChanged":false}})),
        };
        let bytes = serde_json::to_vec(&resp).expect("serialize");
        let decoded: InitializeResponse = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(resp, decoded);
    }

    #[test]
    fn server_info_preserves_additional_fields() {
        // Real servers report extra fields tau doesn't know about; they
        // must round-trip without loss.
        let wire = json!({
            "name":"weather","version":"1.0","author":"NWS","website":"https://weather.gov"
        });
        let info: ServerInfo = serde_json::from_value(wire.clone()).expect("decode");
        assert_eq!(info.name, "weather");
        assert_eq!(info.additional.get("author").and_then(Value::as_str), Some("NWS"));
        let reencoded = serde_json::to_value(&info).expect("encode");
        assert_eq!(reencoded, wire);
    }
}
```

- [ ] **Step 2: Run the tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp -E 'test(protocol::initialize::tests)'`

Expected: 3 tests pass.

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp/src/protocol/initialize.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp/protocol): initialize request/response types"
```

### Task 3.3: tools/list + tools/call types

**Files:**
- Create: `crates/tau-mcp/src/protocol/tools.rs`

- [ ] **Step 1: Write `src/protocol/tools.rs`.**

```rust
//! `tools/list` and `tools/call` payloads.

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `tools/list` request — empty params.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ToolsListRequest {
    /// Optional cursor for paginated tool lists (rarely used in 2026
    /// servers; we accept but don't paginate in v0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// `tools/list` response — a vector of advertised tools.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolsListResponse {
    /// One entry per tool the server exposes.
    pub tools: Vec<McpTool>,
    /// Cursor for the next page (we accept but don't follow in v0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "nextCursor")]
    pub next_cursor: Option<String>,
}

/// A tool advertised by an MCP server.
///
/// PR-4 expands one `ToolImpl::Mcp` per `[tools.<entry>]` in tau.toml
/// into one `Tool` per `McpTool` in this list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpTool {
    /// Tool name as the server expects it on the wire (e.g.
    /// `"get_forecast"`). PR-4 forbids `.` in this name to avoid
    /// IR-ToolId namespace collision.
    pub name: String,
    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON schema for the tool's input.
    #[serde(rename = "inputSchema")]
    pub input_schema: McpToolInputSchema,
}

/// JSON schema for a tool's input — wrapped to preserve serde shape.
///
/// MCP servers ship JSON Schema 2020-12 (or similar). tau passes the
/// schema through opaquely to the LLM; no validation in v0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct McpToolInputSchema(pub Value);

/// `tools/call` request — invoke a server tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolsCallRequest {
    /// Server-side tool name (NOT the IR ToolId).
    pub name: String,
    /// Tool arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
}

/// `tools/call` response — the tool's output content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolsCallResponse {
    /// One or more content blocks (text / image / resource).
    pub content: Vec<ContentBlock>,
    /// Set to `true` if the server reports the tool errored.
    #[serde(default, rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

/// One block of tool result content.
///
/// MCP servers commonly return `Text`. `Image` + `Resource` are spec
/// but rare in 2026 ecosystem; tau accepts but doesn't render them in
/// v0 (passed through as opaque JSON to the agent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ContentBlock {
    /// Plain text.
    Text {
        /// The text content.
        text: String,
    },
    /// Image (base64 data + mime).
    Image {
        /// Base64-encoded image bytes.
        data: String,
        /// MIME type (e.g. `"image/png"`).
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    /// Embedded resource reference.
    Resource {
        /// Resource payload (free-form).
        resource: Value,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;
    use serde_json::json;

    #[test]
    fn tools_list_response_round_trips() {
        let resp = ToolsListResponse {
            tools: vec![McpTool {
                name: "get_forecast".to_string(),
                description: Some("Get a weather forecast".to_string()),
                input_schema: McpToolInputSchema(json!({
                    "type":"object",
                    "properties":{"lat":{"type":"number"},"lon":{"type":"number"}},
                    "required":["lat","lon"]
                })),
            }],
            next_cursor: None,
        };
        let bytes = serde_json::to_vec(&resp).expect("serialize");
        let decoded: ToolsListResponse = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(resp, decoded);
    }

    #[test]
    fn tools_call_request_round_trips() {
        let req = ToolsCallRequest {
            name: "get_forecast".to_string(),
            arguments: Some(json!({"lat":40.7,"lon":-74.0})),
        };
        let bytes = serde_json::to_vec(&req).expect("serialize");
        let decoded: ToolsCallRequest = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(req, decoded);
    }

    #[test]
    fn tools_call_response_text_round_trips() {
        let resp = ToolsCallResponse {
            content: vec![ContentBlock::Text {
                text: "Sunny, 72°F".to_string(),
            }],
            is_error: None,
        };
        let bytes = serde_json::to_vec(&resp).expect("serialize");
        let decoded: ToolsCallResponse = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(resp, decoded);
    }

    #[test]
    fn tools_call_response_error_flag_preserved() {
        let resp = ToolsCallResponse {
            content: vec![ContentBlock::Text {
                text: "rate limited".to_string(),
            }],
            is_error: Some(true),
        };
        let bytes = serde_json::to_vec(&resp).expect("serialize");
        let decoded: ToolsCallResponse = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(resp.is_error, decoded.is_error);
    }

    #[test]
    fn content_block_image_round_trips() {
        let block = ContentBlock::Image {
            data: "iVBORw0KG…".to_string(),
            mime_type: "image/png".to_string(),
        };
        let bytes = serde_json::to_vec(&block).expect("serialize");
        let decoded: ContentBlock = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(block, decoded);
    }
}
```

- [ ] **Step 2: Run the tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp -E 'test(protocol::tools::tests)'`

Expected: 5 tests pass.

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp/src/protocol/tools.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp/protocol): tools/list + tools/call payload types"
```

### Task 3.4: sampling/createMessage types

**Files:**
- Create: `crates/tau-mcp/src/protocol/sampling.rs`

- [ ] **Step 1: Write `src/protocol/sampling.rs`.**

```rust
//! `sampling/createMessage` — server-initiated request asking host to
//! invoke an LLM.
//!
//! Per the β.3 design doc §8.3 and §9: v0 routes this through the
//! agent's `LlmBackend` filtered by the `sampling.models` allowlist.
//! `modelPreferences` is parsed but ignored in v0 (β.3.1 adds it).

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `sampling/createMessage` request — server asks host for an LLM
/// completion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SamplingCreateMessageRequest {
    /// Chat-style message history.
    pub messages: Vec<SamplingMessage>,
    /// Model hints (intelligence / speed / cost weighting). v0 ignores;
    /// β.3.1 honors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "modelPreferences")]
    pub model_preferences: Option<ModelPreferences>,
    /// Optional system prompt the host should pass to the LLM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "systemPrompt")]
    pub system_prompt: Option<String>,
    /// Optional inclusion of host-context in the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "includeContext")]
    pub include_context: Option<String>,
    /// Maximum tokens hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "maxTokens")]
    pub max_tokens: Option<u32>,
    /// Other parameters (temperature, stopSequences, etc.); preserved
    /// across (de)serialization. BTreeMap keeps key order stable for
    /// canonical hashing.
    #[serde(flatten, default, skip_serializing_if = "alloc::collections::BTreeMap::is_empty")]
    pub additional: alloc::collections::BTreeMap<String, Value>,
}

/// A message in a sampling request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SamplingMessage {
    /// Role (`"user"` | `"assistant"` per spec; tau forwards through).
    pub role: String,
    /// Content block — v0 supports text only on inbound sampling.
    pub content: SamplingContent,
}

/// Content block of a sampling message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SamplingContent {
    /// Plain text.
    Text {
        /// The text.
        text: String,
    },
    /// Image (base64 data + mime).
    Image {
        /// Base64-encoded image bytes.
        data: String,
        /// MIME type.
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

/// Model-preference hints (parsed in v0; not yet honored).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ModelPreferences {
    /// Server's hint for intelligence (0.0–1.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "intelligencePriority")]
    pub intelligence_priority: Option<f32>,
    /// Server's hint for speed (0.0–1.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "speedPriority")]
    pub speed_priority: Option<f32>,
    /// Server's hint for cost (0.0–1.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "costPriority")]
    pub cost_priority: Option<f32>,
    /// Server's hint for specific model names (free-form).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hints: Vec<ModelHint>,
}

/// One model-name hint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelHint {
    /// Suggested model name.
    pub name: String,
}

/// `sampling/createMessage` response — the host's LLM completion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SamplingCreateMessageResponse {
    /// Role of the response (always `"assistant"` per spec).
    pub role: String,
    /// Completion content.
    pub content: SamplingContent,
    /// Model name actually used.
    pub model: String,
    /// Stop reason (`"endTurn"` | `"stopSequence"` | `"maxTokens"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "stopReason")]
    pub stop_reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;
    use serde_json::json;

    #[test]
    fn sampling_request_round_trips() {
        let req = SamplingCreateMessageRequest {
            messages: vec![SamplingMessage {
                role: "user".to_string(),
                content: SamplingContent::Text {
                    text: "summarize".to_string(),
                },
            }],
            model_preferences: Some(ModelPreferences {
                intelligence_priority: Some(0.9),
                speed_priority: Some(0.1),
                cost_priority: None,
                hints: vec![ModelHint {
                    name: "claude-haiku".to_string(),
                }],
            }),
            system_prompt: None,
            include_context: None,
            max_tokens: Some(512),
            additional: alloc::collections::BTreeMap::new(),
        };
        let bytes = serde_json::to_vec(&req).expect("serialize");
        let decoded: SamplingCreateMessageRequest =
            serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(req, decoded);
        // Suppress unused-import warning for `json!` macro now that
        // `additional` no longer needs Value construction here.
        let _ = json!({});
    }

    #[test]
    fn sampling_response_round_trips() {
        let resp = SamplingCreateMessageResponse {
            role: "assistant".to_string(),
            content: SamplingContent::Text {
                text: "summary".to_string(),
            },
            model: "claude-haiku-4-5".to_string(),
            stop_reason: Some("endTurn".to_string()),
        };
        let bytes = serde_json::to_vec(&resp).expect("serialize");
        let decoded: SamplingCreateMessageResponse =
            serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(resp, decoded);
    }
}
```

- [ ] **Step 2: Run the tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp -E 'test(protocol::sampling::tests)'`

Expected: 2 tests pass.

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp/src/protocol/sampling.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp/protocol): sampling/createMessage payload types"
```

### Task 3.5: roots/list types

**Files:**
- Create: `crates/tau-mcp/src/protocol/roots.rs`

- [ ] **Step 1: Write `src/protocol/roots.rs`.**

```rust
//! `roots/list` — server asks host which filesystem roots it may
//! read/write.
//!
//! Per the β.3 design doc §9: tau v0 returns the explicit `roots` field
//! from tau.toml, build-time-checked ⊆ the tool's `fs.read` caps.
//! Default-empty `roots` returns `[]` (server gets no fs access).

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

/// `roots/list` request — empty params.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RootsListRequest {}

/// `roots/list` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RootsListResponse {
    /// Allowed roots; empty array means the server has no host-granted
    /// filesystem visibility (it falls back to its own behavior).
    pub roots: Vec<Root>,
}

/// One root the host advertises.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Root {
    /// URI of the root (typically `"file:///path"`).
    pub uri: String,
    /// Optional human-readable name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;
    use serde_json::json;

    #[test]
    fn roots_response_round_trips() {
        let resp = RootsListResponse {
            roots: vec![Root {
                uri: "file:///tmp/mcp-cache".to_string(),
                name: Some("cache".to_string()),
            }],
        };
        let bytes = serde_json::to_vec(&resp).expect("serialize");
        let decoded: RootsListResponse =
            serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(resp, decoded);
    }

    #[test]
    fn empty_roots_round_trips() {
        let resp = RootsListResponse { roots: vec![] };
        let bytes = serde_json::to_vec(&resp).expect("serialize");
        assert_eq!(serde_json::from_slice::<RootsListResponse>(&bytes).unwrap(), resp);
        // Also accept legacy `{"roots":[]}` form unchanged.
        let wire = json!({"roots":[]});
        let decoded: RootsListResponse = serde_json::from_value(wire).expect("decode");
        assert_eq!(decoded, resp);
    }
}
```

- [ ] **Step 2: Run the tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp -E 'test(protocol::roots::tests)'`

Expected: 2 tests pass.

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp/src/protocol/roots.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp/protocol): roots/list payload types"
```

### Task 3.6: notifications + cancellation types

**Files:**
- Create: `crates/tau-mcp/src/protocol/notifications.rs`

- [ ] **Step 1: Write `src/protocol/notifications.rs`.**

```rust
//! Bidirectional notifications + cancellation.
//!
//! Per the β.3 design doc §4 (v0 scope) and §8.4 (cancellation
//! propagation). Notifications are fire-and-forget (no `id`, no
//! response). Cancellation is also a notification per MCP spec.

use alloc::string::String;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::jsonrpc::RequestId;

/// `notifications/progress` — host or server reporting progress on an
/// in-flight request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressNotification {
    /// Progress token (mirrors the request's `_meta.progressToken` if
    /// the caller asked for progress; otherwise free-form).
    #[serde(rename = "progressToken")]
    pub progress_token: Value,
    /// Current progress (units defined by the producer).
    pub progress: f64,
    /// Optional total to compute percentage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
}

/// `notifications/cancelled` — caller is aborting an in-flight request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CancelledNotification {
    /// The request id being cancelled.
    #[serde(rename = "requestId")]
    pub request_id: RequestId,
    /// Optional reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `notifications/initialized` — host signals it has finished processing
/// the `initialize` response.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct InitializedNotification {}

/// `notifications/message` (logging) — server emits a log line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogNotification {
    /// Log level (`"debug"` | `"info"` | `"warn"` | `"error"`).
    pub level: String,
    /// Logger name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logger: Option<String>,
    /// Free-form structured payload (server-defined).
    pub data: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use serde_json::json;

    #[test]
    fn progress_round_trips() {
        let n = ProgressNotification {
            progress_token: json!("call-7"),
            progress: 50.0,
            total: Some(100.0),
        };
        let bytes = serde_json::to_vec(&n).expect("serialize");
        let decoded: ProgressNotification =
            serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(n, decoded);
    }

    #[test]
    fn cancelled_round_trips() {
        let n = CancelledNotification {
            request_id: RequestId::Number(7),
            reason: Some("user abort".to_string()),
        };
        let bytes = serde_json::to_vec(&n).expect("serialize");
        let decoded: CancelledNotification =
            serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(n, decoded);
    }

    #[test]
    fn log_round_trips() {
        let n = LogNotification {
            level: "info".to_string(),
            logger: Some("weather".to_string()),
            data: json!({"msg":"forecast fetched","duration_ms":42}),
        };
        let bytes = serde_json::to_vec(&n).expect("serialize");
        let decoded: LogNotification = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(n, decoded);
    }
}
```

- [ ] **Step 2: Run the tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp -E 'test(protocol::notifications::tests)'`

Expected: 3 tests pass.

- [ ] **Step 3: Final phase-3 build + clippy check.**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp -- -D warnings`

Expected: zero warnings.

- [ ] **Step 4: Commit.**

```
git add crates/tau-mcp/src/protocol/notifications.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp/protocol): notification + cancellation types; phase-3 clippy clean"
```

---

## Phase 4 — contract layer + canonical hash

### Task 4.1: `ServerContract` type

**Files:**
- Create: `crates/tau-mcp/src/contract/server_contract.rs`

- [ ] **Step 1: Write `src/contract/server_contract.rs`.**

```rust
//! `ServerContract` — what tau pins for an MCP server.
//!
//! Captures the server's `initialize` response + `tools/list` snapshot
//! at build time. PR-4's lowering pass canonical-hashes a `ServerContract`
//! and stores `(url, contract_hash, expanded_tools)` in the lockfile.

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use tau_domain::Capability;

use crate::protocol::initialize::ServerInfo;
use crate::protocol::tools::{McpTool, McpToolInputSchema};

/// Frozen server contract.
///
/// One contract per MCP server URL. `tau build` constructs this from
/// the live (or pinned) handshake; `canonical_hash` produces the
/// `Hash256` that participates in the IR module hash.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerContract {
    /// MCP protocol version the server advertised at `initialize`.
    pub protocol_version: String,
    /// Server's reported info.
    pub server_info: ServerInfo,
    /// The full `tools/list` snapshot, in server order.
    pub tools: Vec<ContractTool>,
}

/// One tool from the server's `tools/list` plus its declared caps.
///
/// `caps` is the **server-declared** capability set for this tool. PR-4
/// intersects it with the author's per-server envelope before storing
/// in the IR's `ToolImpl::Mcp::capability_subset`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContractTool {
    /// Server-side tool name.
    pub name: String,
    /// Server-supplied description (passed through to the LLM).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Server-supplied input JSON schema.
    pub input_schema: McpToolInputSchema,
    /// Server-declared capabilities (per-tool).
    ///
    /// Note: MCP spec does NOT currently standardize a "capability
    /// declaration" field on tools/list entries. tau extracts caps from
    /// a tau-specific extension field; if the server doesn't ship it,
    /// caps default to the empty vector and the author's envelope is
    /// the upper bound (per the spec's "envelope ∩ contract" rule).
    ///
    /// β.3.1 may evolve this once the MCP spec lands per-tool caps.
    #[serde(default)]
    pub caps: Vec<Capability>,
}

impl ServerContract {
    /// Build a `ServerContract` from a handshake-completed pair of
    /// (`InitializeResponse`, `ToolsListResponse`). PR-2 + PR-3 wire
    /// this from the live transport.
    ///
    /// `caps_extractor` lets the caller pull caps from a tau-specific
    /// extension field on each `McpTool`; if the extension is absent
    /// (most off-the-shelf servers), the closure returns `Vec::new()`
    /// and the author's envelope is the upper bound.
    pub fn from_handshake<F>(
        init: crate::protocol::initialize::InitializeResponse,
        tools_list: crate::protocol::tools::ToolsListResponse,
        mut caps_extractor: F,
    ) -> Self
    where
        F: FnMut(&McpTool) -> Vec<Capability>,
    {
        let tools = tools_list
            .tools
            .into_iter()
            .map(|t| ContractTool {
                caps: caps_extractor(&t),
                name: t.name,
                description: t.description,
                input_schema: t.input_schema,
            })
            .collect();
        ServerContract {
            protocol_version: init.protocol_version,
            server_info: init.server_info,
            tools,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use alloc::string::ToString;
    use alloc::vec;
    use serde_json::json;

    fn weather_contract() -> ServerContract {
        ServerContract {
            protocol_version: "2025-03-26".to_string(),
            server_info: ServerInfo {
                name: "weather".to_string(),
                version: "1.0".to_string(),
                additional: BTreeMap::new(),
            },
            tools: vec![ContractTool {
                name: "get_forecast".to_string(),
                description: Some("Get weather forecast".to_string()),
                input_schema: McpToolInputSchema(json!({
                    "type":"object",
                    "properties":{"lat":{"type":"number"},"lon":{"type":"number"}}
                })),
                caps: vec![],
            }],
        }
    }

    #[test]
    fn server_contract_round_trips() {
        let c = weather_contract();
        let bytes = serde_json::to_vec(&c).expect("serialize");
        let decoded: ServerContract = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(c, decoded);
    }

    #[test]
    fn from_handshake_constructs_contract() {
        use crate::protocol::initialize::InitializeResponse;
        use crate::protocol::tools::ToolsListResponse;

        let init = InitializeResponse {
            protocol_version: "2025-03-26".to_string(),
            server_info: ServerInfo {
                name: "weather".to_string(),
                version: "1.0".to_string(),
                additional: BTreeMap::new(),
            },
            capabilities: None,
        };
        let tools = ToolsListResponse {
            tools: vec![McpTool {
                name: "get_forecast".to_string(),
                description: None,
                input_schema: McpToolInputSchema(json!({"type":"object"})),
            }],
            next_cursor: None,
        };
        let c = ServerContract::from_handshake(init, tools, |_| vec![]);
        assert_eq!(c.tools.len(), 1);
        assert_eq!(c.tools[0].name, "get_forecast");
        assert!(c.tools[0].caps.is_empty());
    }
}
```

- [ ] **Step 2: Run the tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp -E 'test(contract::server_contract::tests)'`

Expected: 2 tests pass.

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp/src/contract/server_contract.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp/contract): ServerContract + ContractTool types"
```

### Task 4.2: Canonical hash

**Files:**
- Create: `crates/tau-mcp/src/contract/canonical.rs`

- [ ] **Step 1: Write `src/contract/canonical.rs`.**

```rust
//! Canonical hash for `ServerContract`.
//!
//! Same shape as the β.2 IR module hash: SHA-256 over canonical-JSON
//! bytes. "Canonical" means object keys sorted alphabetically, no
//! whitespace, `f64` integers normalized to integer form when they
//! losslessly represent integers. serde_json::to_vec gives us most of
//! that for free when the source types are stable; the BTreeMap-backed
//! `additional` fields preserve sorted keys.
//!
//! The deterministic property checked by `golden_canonical.rs` is:
//! same `ServerContract` (constructed identically) → same `Hash256`
//! across runs and across platforms.

use sha2::{Digest, Sha256};

use crate::contract::server_contract::ServerContract;
use crate::McpError;

/// 32-byte content hash (SHA-256 output).
pub type Hash256 = [u8; 32];

/// Compute the canonical hash of a `ServerContract`.
///
/// The hash participates in the IR module hash (PR-4 wires this into
/// `ToolImpl::Mcp::contract_hash`) so contract drift invalidates the
/// bundle.
pub fn canonical_hash(contract: &ServerContract) -> Result<Hash256, McpError> {
    // serde_json's default Map preserves insertion order; for canonical
    // form we re-serialize through a value tree using sorted keys. The
    // `preserve_order` feature is NOT enabled on serde_json in tau-mcp
    // (default off), so Map = BTreeMap and keys come out sorted — this
    // is the same property β.2's IR-hash relies on.
    let bytes = serde_json::to_vec(contract)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let out = hasher.finalize();
    let mut h: Hash256 = [0; 32];
    h.copy_from_slice(&out);
    Ok(h)
}

/// Format a `Hash256` as a lowercase hex string for diagnostics +
/// lockfile.
pub fn hash_to_hex(h: &Hash256) -> alloc::string::String {
    use core::fmt::Write;
    let mut s = alloc::string::String::with_capacity(64);
    for b in h.iter() {
        // Per the LowerHex-in-CI gotcha from project_skills_5_shipped_2026_05_16,
        // use the {:02x} form explicitly.
        let _ = write!(&mut s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::server_contract::{ContractTool, ServerContract};
    use crate::protocol::initialize::ServerInfo;
    use crate::protocol::tools::McpToolInputSchema;
    use alloc::collections::BTreeMap;
    use alloc::string::ToString;
    use alloc::vec;
    use serde_json::json;

    fn fixture() -> ServerContract {
        ServerContract {
            protocol_version: "2025-03-26".to_string(),
            server_info: ServerInfo {
                name: "weather".to_string(),
                version: "1.0".to_string(),
                additional: BTreeMap::new(),
            },
            tools: vec![ContractTool {
                name: "get_forecast".to_string(),
                description: None,
                input_schema: McpToolInputSchema(json!({"type":"object"})),
                caps: vec![],
            }],
        }
    }

    #[test]
    fn determinism() {
        let h1 = canonical_hash(&fixture()).expect("hash");
        let h2 = canonical_hash(&fixture()).expect("hash");
        assert_eq!(h1, h2);
    }

    #[test]
    fn hex_is_lowercase_64_chars() {
        let h = canonical_hash(&fixture()).expect("hash");
        let s = hash_to_hex(&h);
        assert_eq!(s.len(), 64);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn different_contracts_have_different_hashes() {
        let mut other = fixture();
        other.tools[0].description = Some("changed".to_string());
        let h1 = canonical_hash(&fixture()).expect("hash");
        let h2 = canonical_hash(&other).expect("hash");
        assert_ne!(h1, h2);
    }
}
```

- [ ] **Step 2: Run the tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp -E 'test(contract::canonical::tests)'`

Expected: 3 tests pass.

- [ ] **Step 3: Update `src/contract/mod.rs` to re-export `hash_to_hex`.**

Modify line `pub use canonical::{canonical_hash, Hash256};` to:

```rust
pub use canonical::{canonical_hash, hash_to_hex, Hash256};
```

- [ ] **Step 4: Commit.**

```
git add crates/tau-mcp/src/contract/canonical.rs crates/tau-mcp/src/contract/mod.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp/contract): canonical_hash (SHA-256 over canonical JSON)"
```

### Task 4.3: PinnedContract file I/O

**Files:**
- Create: `crates/tau-mcp/src/contract/pinned.rs`

- [ ] **Step 1: Write `src/contract/pinned.rs`.**

```rust
//! Pinned contract file shape.
//!
//! Stored at `.tau/mcp/<name>.contract.json` by `tau mcp pin <name>`.
//! Read by `tau build --offline` (PR-4) and by `tau verify --bundle`
//! (PR-6). Carries the full `ServerContract` plus the URL and the
//! pre-computed `contract_hash` so callers can read-and-trust without
//! re-hashing (re-hash is still the runtime defense-in-depth check).

use alloc::string::String;
use serde::{Deserialize, Serialize};

use crate::contract::canonical::{canonical_hash, hash_to_hex, Hash256};
use crate::contract::server_contract::ServerContract;
use crate::McpError;

/// Schema version of the pinned-contract file format.
pub const PINNED_CONTRACT_SCHEMA_VERSION: u32 = 1;

/// A pinned MCP server contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PinnedContract {
    /// Schema version (for forward compat).
    pub schema_version: u32,
    /// Server URL (matches the `[tools.<name>] mcp = "..."` field).
    pub url: String,
    /// Pre-computed contract hash (lowercase hex).
    pub contract_hash_hex: String,
    /// Full server contract snapshot.
    pub contract: ServerContract,
}

impl PinnedContract {
    /// Build a `PinnedContract` from a `(url, ServerContract)` pair,
    /// computing the hash inline.
    pub fn from_parts(url: String, contract: ServerContract) -> Result<Self, McpError> {
        let h = canonical_hash(&contract)?;
        Ok(Self {
            schema_version: PINNED_CONTRACT_SCHEMA_VERSION,
            url,
            contract_hash_hex: hash_to_hex(&h),
            contract,
        })
    }

    /// Decode the `contract_hash_hex` field back to a `Hash256`.
    pub fn decoded_hash(&self) -> Result<Hash256, McpError> {
        decode_hex_hash(&self.contract_hash_hex)
    }

    /// Verify `contract_hash_hex` matches a freshly-computed hash of
    /// `contract`. Used by `tau verify --bundle` and the runtime drift
    /// check.
    pub fn verify_self_hash(&self) -> Result<(), McpError> {
        let observed = canonical_hash(&self.contract)?;
        let observed_hex = hash_to_hex(&observed);
        if observed_hex != self.contract_hash_hex {
            return Err(McpError::ContractDrift {
                observed: observed_hex,
                expected: self.contract_hash_hex.clone(),
            });
        }
        Ok(())
    }
}

fn decode_hex_hash(s: &str) -> Result<Hash256, McpError> {
    if s.len() != 64 {
        return Err(McpError::Protocol(alloc::format!(
            "contract_hash_hex must be 64 chars, got {}",
            s.len()
        )));
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let hi = hex_digit(chunk[0])?;
        let lo = hex_digit(chunk[1])?;
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_digit(b: u8) -> Result<u8, McpError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(McpError::Protocol(alloc::format!(
            "invalid hex digit: 0x{b:02x}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::server_contract::{ContractTool, ServerContract};
    use crate::protocol::initialize::ServerInfo;
    use crate::protocol::tools::McpToolInputSchema;
    use alloc::collections::BTreeMap;
    use alloc::string::ToString;
    use alloc::vec;
    use serde_json::json;

    fn fixture() -> ServerContract {
        ServerContract {
            protocol_version: "2025-03-26".to_string(),
            server_info: ServerInfo {
                name: "weather".to_string(),
                version: "1.0".to_string(),
                additional: BTreeMap::new(),
            },
            tools: vec![ContractTool {
                name: "get_forecast".to_string(),
                description: None,
                input_schema: McpToolInputSchema(json!({"type":"object"})),
                caps: vec![],
            }],
        }
    }

    #[test]
    fn round_trips() {
        let p = PinnedContract::from_parts("https://example.com".to_string(), fixture())
            .expect("build");
        let bytes = serde_json::to_vec(&p).expect("serialize");
        let decoded: PinnedContract = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(p, decoded);
    }

    #[test]
    fn verify_self_hash_ok() {
        let p = PinnedContract::from_parts("u".to_string(), fixture()).expect("build");
        p.verify_self_hash().expect("matches");
    }

    #[test]
    fn verify_self_hash_drift() {
        let mut p = PinnedContract::from_parts("u".to_string(), fixture()).expect("build");
        // Tamper with contract; hash field now wrong.
        p.contract.tools[0].description = Some("tampered".to_string());
        let err = p.verify_self_hash().expect_err("should detect drift");
        assert!(matches!(err, McpError::ContractDrift { .. }));
    }

    #[test]
    fn decoded_hash_round_trip() {
        let p = PinnedContract::from_parts("u".to_string(), fixture()).expect("build");
        let h_decoded = p.decoded_hash().expect("decode");
        let h_recomputed = canonical_hash(&p.contract).expect("rehash");
        assert_eq!(h_decoded, h_recomputed);
    }

    #[test]
    fn invalid_hex_length_rejected() {
        let p = PinnedContract {
            schema_version: PINNED_CONTRACT_SCHEMA_VERSION,
            url: "u".to_string(),
            contract_hash_hex: "abc".to_string(),
            contract: fixture(),
        };
        let err = p.decoded_hash().expect_err("should reject short hex");
        assert!(matches!(err, McpError::Protocol(_)));
    }
}
```

- [ ] **Step 2: Run the tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp -E 'test(contract::pinned::tests)'`

Expected: 5 tests pass.

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp/src/contract/pinned.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp/contract): PinnedContract serializer + self-hash verifier"
```

### Task 4.4: Golden-vector test for canonical-hash determinism

**Files:**
- Create: `crates/tau-mcp/tests/golden_canonical.rs`
- Create: `crates/tau-mcp/tests/fixtures/canonical/empty.json`
- Create: `crates/tau-mcp/tests/fixtures/canonical/weather.json`

- [ ] **Step 1: Author the two fixture files.**

Write `crates/tau-mcp/tests/fixtures/canonical/empty.json`:

```json
{
  "protocol_version": "2025-03-26",
  "server_info": {"name": "empty", "version": "0.0.0"},
  "tools": []
}
```

Write `crates/tau-mcp/tests/fixtures/canonical/weather.json`:

```json
{
  "protocol_version": "2025-03-26",
  "server_info": {"name": "weather", "version": "1.0.0"},
  "tools": [
    {
      "name": "get_forecast",
      "description": "Get a weather forecast",
      "input_schema": {
        "type": "object",
        "properties": {
          "lat": {"type": "number"},
          "lon": {"type": "number"}
        },
        "required": ["lat", "lon"]
      },
      "caps": []
    }
  ]
}
```

- [ ] **Step 2: Write `crates/tau-mcp/tests/golden_canonical.rs`.**

```rust
//! Golden-vector test for canonical-hash determinism.
//!
//! Reads two fixture `ServerContract` JSONs, computes canonical hashes,
//! and asserts they match the recorded constants below. If the canonical
//! encoder changes shape, these constants change too — the test fails
//! and the test author updates them with the new values (treat that as
//! an intentional protocol-format bump, NOT just a test fix).

use std::fs;

use tau_mcp::contract::{canonical_hash, hash_to_hex, ServerContract};

fn load_fixture(name: &str) -> ServerContract {
    let path = format!(
        "{}/tests/fixtures/canonical/{}",
        env!("CARGO_MANIFEST_DIR"),
        name
    );
    let bytes = fs::read(&path).expect("read fixture");
    serde_json::from_slice(&bytes).expect("decode fixture")
}

#[test]
fn empty_contract_golden_hash() {
    let c = load_fixture("empty.json");
    let h = canonical_hash(&c).expect("hash");
    // First-time author: leave the assert below pointing at a
    // placeholder, run the test, capture the value, fill it in, re-run.
    // After this lands, any future change to the canonical encoder MUST
    // intentionally update this constant.
    let expected = include_str!("expected_hashes/empty.hex").trim();
    assert_eq!(hash_to_hex(&h), expected);
}

#[test]
fn weather_contract_golden_hash() {
    let c = load_fixture("weather.json");
    let h = canonical_hash(&c).expect("hash");
    let expected = include_str!("expected_hashes/weather.hex").trim();
    assert_eq!(hash_to_hex(&h), expected);
}
```

- [ ] **Step 3: Create the expected-hashes directory + placeholder files.**

Create `crates/tau-mcp/tests/expected_hashes/empty.hex` with placeholder content:

```
0000000000000000000000000000000000000000000000000000000000000000
```

Create `crates/tau-mcp/tests/expected_hashes/weather.hex` with the same placeholder.

- [ ] **Step 4: Run the test once to capture the real hashes; expect failure with the observed value in the assertion.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp --test golden_canonical 2>&1 | head -40`

Expected: both tests fail with `assertion failed` showing the observed lowercase-hex hash. Capture both values.

- [ ] **Step 5: Write the captured hashes into the `.hex` files.**

Replace the all-zero content of `empty.hex` with the observed value for `empty_contract_golden_hash`, and `weather.hex` with the observed value for `weather_contract_golden_hash`. ONE hex string per file, no trailing newline (or trailing newline that the `.trim()` strips).

- [ ] **Step 6: Re-run the golden tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp --test golden_canonical`

Expected: both tests pass.

- [ ] **Step 7: Commit.**

```
git add crates/tau-mcp/tests/golden_canonical.rs crates/tau-mcp/tests/fixtures/canonical/ crates/tau-mcp/tests/expected_hashes/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "test(tau-mcp): golden canonical-hash vectors (empty + weather contracts)"
```

---

## Phase 5 — HostHandlers trait + default-deny

### Task 5.1: HostHandlers trait + DefaultDenyHandlers

**Files:**
- Create: `crates/tau-mcp/src/host/handlers.rs`

- [ ] **Step 1: Write `src/host/handlers.rs`.**

```rust
//! `HostHandlers` trait — host-side response to server-initiated
//! requests (sampling + roots in v0).
//!
//! v0 ships two real inbound handlers (sampling, roots) plus a
//! default-deny baseline impl ([`DefaultDenyHandlers`]). PR-5 wires the
//! real impl in `tau-cli` carrying the agent's `LlmBackend` and the
//! per-server `sampling.models` allowlist + `roots` declaration.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::future::Future;
use core::pin::Pin;
use thiserror::Error;

use crate::protocol::roots::Root;
use crate::protocol::sampling::{
    SamplingCreateMessageRequest, SamplingCreateMessageResponse,
};

/// Error returned by an inbound handler to refuse a server request.
///
/// Surfaces as an MCP `JsonRpcError` payload to the server with code
/// = `-32000` (custom error range).
#[derive(Debug, Clone, Error)]
#[non_exhaustive]
pub enum InboundError {
    /// Server requested sampling but the host has no models allowlisted.
    #[error("sampling refused: allowlist is empty")]
    SamplingNotAllowed,
    /// Server requested sampling with a model that's not in the
    /// allowlist.
    #[error("sampling refused: model {requested:?} not in allowlist")]
    SamplingModelRefused {
        /// The model the server asked for.
        requested: String,
    },
    /// Server requested roots but the host's roots list is empty
    /// (semantically the same as "no fs visibility granted").
    #[error("roots returned []: no roots declared")]
    RootsEmpty,
    /// Backend invocation (LLM call) failed.
    #[error("backend error: {0}")]
    Backend(String),
}

/// Type alias for a boxed-future returning a result.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Host-side handlers for inbound (server-initiated) MCP requests.
///
/// One impl per contracted MCP server. Concrete impl lives in PR-5
/// (`tau-cli::cmd::run::ir_dispatcher::WiredHostHandlers` or similar)
/// and composes the agent's `LlmBackend` + per-server allowlist.
pub trait HostHandlers: Send + Sync {
    /// Handle a `sampling/createMessage` request from the server.
    fn sampling<'a>(
        &'a self,
        req: SamplingCreateMessageRequest,
    ) -> BoxFuture<'a, Result<SamplingCreateMessageResponse, InboundError>>;

    /// Handle a `roots/list` request from the server.
    fn roots<'a>(&'a self) -> BoxFuture<'a, Result<Vec<Root>, InboundError>>;
}

/// Default-deny baseline impl: refuses every inbound request.
///
/// Suitable as a starting point for tests that don't need to exercise
/// inbound handlers. PR-5's production impl follows the same trait
/// shape but composes real backends.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultDenyHandlers;

impl HostHandlers for DefaultDenyHandlers {
    fn sampling<'a>(
        &'a self,
        _req: SamplingCreateMessageRequest,
    ) -> BoxFuture<'a, Result<SamplingCreateMessageResponse, InboundError>> {
        Box::pin(async { Err(InboundError::SamplingNotAllowed) })
    }

    fn roots<'a>(&'a self) -> BoxFuture<'a, Result<Vec<Root>, InboundError>> {
        // Default-deny for roots returns an EMPTY list, not an error —
        // per the spec, `roots/list` returning `[]` is a valid response
        // meaning "host grants no fs visibility." Servers must accept
        // that gracefully.
        Box::pin(async { Ok(Vec::new()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::sampling::{
        ModelPreferences, SamplingContent, SamplingMessage,
    };
    use alloc::string::ToString;
    use alloc::vec;
    use serde_json::json;

    fn sample_request() -> SamplingCreateMessageRequest {
        SamplingCreateMessageRequest {
            messages: vec![SamplingMessage {
                role: "user".to_string(),
                content: SamplingContent::Text {
                    text: "x".to_string(),
                },
            }],
            model_preferences: Some(ModelPreferences::default()),
            system_prompt: None,
            include_context: None,
            max_tokens: None,
            additional: json!({}),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn default_deny_sampling_refuses() {
        let h = DefaultDenyHandlers;
        let r = h.sampling(sample_request()).await;
        assert!(matches!(r, Err(InboundError::SamplingNotAllowed)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn default_deny_roots_returns_empty() {
        let h = DefaultDenyHandlers;
        let r = h.roots().await.expect("ok");
        assert!(r.is_empty());
    }
}
```

- [ ] **Step 2: Add tokio as a dev-dependency in `tau-mcp/Cargo.toml` (needed for the `#[tokio::test]` macro).**

Modify `crates/tau-mcp/Cargo.toml` `[dev-dependencies]` block to:

```toml
[dev-dependencies]
serde_json = { workspace = true }
tokio      = { workspace = true, features = ["rt", "macros"] }
```

- [ ] **Step 3: Run the tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp -E 'test(host::handlers::tests)'`

Expected: 2 tests pass.

- [ ] **Step 4: Commit.**

```
git add crates/tau-mcp/src/host/handlers.rs crates/tau-mcp/Cargo.toml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp/host): HostHandlers trait + DefaultDenyHandlers baseline"
```

---

## Phase 6 — cassette types

### Task 6.1: CassetteMessage type

**Files:**
- Create: `crates/tau-mcp/src/cassette/message.rs`

- [ ] **Step 1: Write `src/cassette/message.rs`.**

```rust
//! Cassette message record — one JSON line per MCP message.
//!
//! Per the β.3 design doc §11. JSONL format with a `{"version":1}` first
//! line followed by per-message records.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::jsonrpc::RequestId;

/// Cassette format version emitted by this crate.
pub const CASSETTE_VERSION: u32 = 1;

/// Direction of a cassette message (from the cassette's recording POV).
///
/// - [`Direction::In`] — message arrived INTO the cassette from the
///   host side (host sent it to the server).
/// - [`Direction::Out`] — message emitted OUT of the cassette to the
///   host side (server's reply or server-initiated request).
///
/// Mnemonic: replay direction is `Out` — the cassette is the server
/// stand-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Host → server (recorded inbound to the cassette).
    In,
    /// Server → host (cassette emits to host on replay).
    Out,
}

/// Kind of MCP message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageKind {
    /// A method call expecting a response.
    Request,
    /// A response to a prior request.
    Response,
    /// Fire-and-forget notification.
    Notification,
}

/// One cassette record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CassetteMessage {
    /// Direction (see [`Direction`] mnemonic).
    pub dir: Direction,
    /// Message kind.
    pub kind: MessageKind,
    /// Request id (None for notifications).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<RequestId>,
    /// Method name (None for response records).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Raw payload — params for request/notification, result/error for
    /// response.
    pub payload: Value,
}

/// The version-header first line of a cassette file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CassetteHeader {
    /// Format version.
    pub version: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use serde_json::json;

    #[test]
    fn header_round_trips() {
        let h = CassetteHeader { version: 1 };
        let bytes = serde_json::to_vec(&h).expect("serialize");
        assert_eq!(&bytes, br#"{"version":1}"#);
        let decoded: CassetteHeader = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(h, decoded);
    }

    #[test]
    fn message_request_round_trips() {
        let m = CassetteMessage {
            dir: Direction::In,
            kind: MessageKind::Request,
            id: Some(RequestId::Number(7)),
            method: Some("tools/call".to_string()),
            payload: json!({"name":"get_forecast","arguments":{"lat":40.7,"lon":-74.0}}),
        };
        let bytes = serde_json::to_vec(&m).expect("serialize");
        let decoded: CassetteMessage = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(m, decoded);
    }

    #[test]
    fn message_response_round_trips() {
        let m = CassetteMessage {
            dir: Direction::Out,
            kind: MessageKind::Response,
            id: Some(RequestId::Number(7)),
            method: None,
            payload: json!({"content":[{"type":"text","text":"sunny"}]}),
        };
        let bytes = serde_json::to_vec(&m).expect("serialize");
        let decoded: CassetteMessage = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(m, decoded);
    }

    #[test]
    fn message_notification_round_trips() {
        let m = CassetteMessage {
            dir: Direction::Out,
            kind: MessageKind::Notification,
            id: None,
            method: Some("notifications/progress".to_string()),
            payload: json!({"progressToken":"call-7","progress":50,"total":100}),
        };
        let bytes = serde_json::to_vec(&m).expect("serialize");
        let decoded: CassetteMessage = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(m, decoded);
    }
}
```

- [ ] **Step 2: Run the tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp -E 'test(cassette::message::tests)'`

Expected: 4 tests pass.

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp/src/cassette/message.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp/cassette): CassetteMessage + CassetteHeader record types"
```

### Task 6.2: Recorder

**Files:**
- Create: `crates/tau-mcp/src/cassette/recorder.rs`

- [ ] **Step 1: Write `src/cassette/recorder.rs`.**

```rust
//! In-memory cassette recorder.
//!
//! Captures `CassetteMessage` records as they're produced (PR-3 wires
//! this into the host loop at the handler-dispatch boundary). The
//! `Recorder` itself is transport-agnostic — it gets called with
//! already-parsed `JsonRpcMessage` values; the transport layer is
//! responsible for handing them to the recorder before they're framed
//! / after they're parsed.
//!
//! File-I/O sink (`save_to_file`) requires `std`; the in-memory record
//! API is `no_std`-compatible.

use alloc::string::String;
use alloc::vec::Vec;

use crate::cassette::message::{CassetteHeader, CassetteMessage, Direction, MessageKind, CASSETTE_VERSION};
use crate::protocol::jsonrpc::{JsonRpcMessage, RequestId};

/// In-memory cassette accumulator.
#[derive(Debug, Default, Clone)]
pub struct Recorder {
    messages: Vec<CassetteMessage>,
}

impl Recorder {
    /// Construct an empty recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one JSON-RPC message with its direction.
    pub fn record(&mut self, dir: Direction, msg: &JsonRpcMessage) {
        let record = match msg {
            JsonRpcMessage::Request(r) => CassetteMessage {
                dir,
                kind: MessageKind::Request,
                id: Some(r.id.clone()),
                method: Some(r.method.clone()),
                payload: r.params.clone().unwrap_or(serde_json::Value::Null),
            },
            JsonRpcMessage::Response(r) => CassetteMessage {
                dir,
                kind: MessageKind::Response,
                id: Some(r.id.clone()),
                method: None,
                payload: if let Some(e) = &r.error {
                    serde_json::json!({"error": e})
                } else {
                    r.result.clone().unwrap_or(serde_json::Value::Null)
                },
            },
            JsonRpcMessage::Notification(n) => CassetteMessage {
                dir,
                kind: MessageKind::Notification,
                id: None,
                method: Some(n.method.clone()),
                payload: n.params.clone().unwrap_or(serde_json::Value::Null),
            },
        };
        self.messages.push(record);
    }

    /// Return the recorded messages.
    pub fn messages(&self) -> &[CassetteMessage] {
        &self.messages
    }

    /// Serialize the cassette to JSONL bytes (header line + one line
    /// per recorded message). Pure-allocator API; no I/O.
    pub fn to_jsonl_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        let mut out: Vec<u8> = Vec::new();
        let header = CassetteHeader {
            version: CASSETTE_VERSION,
        };
        out.extend_from_slice(&serde_json::to_vec(&header)?);
        out.push(b'\n');
        for m in &self.messages {
            out.extend_from_slice(&serde_json::to_vec(m)?);
            out.push(b'\n');
        }
        Ok(out)
    }

    /// Save the cassette to a file. Requires std.
    #[cfg(feature = "with-std-adapters")]
    pub fn save_to_file<P: AsRef<std::path::Path>>(
        &self,
        path: P,
    ) -> Result<(), String> {
        let bytes = self.to_jsonl_bytes().map_err(|e| alloc::format!("{e}"))?;
        std::fs::write(path, bytes).map_err(|e| alloc::format!("{e}"))?;
        Ok(())
    }

    /// Return how many records are stored, by request id, useful for
    /// asserting in tests.
    pub fn count_for(&self, id: &RequestId) -> usize {
        self.messages
            .iter()
            .filter(|m| m.id.as_ref() == Some(id))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::jsonrpc::{JsonRpcRequest, JsonRpcResponse, JSONRPC_VERSION};
    use alloc::string::ToString;
    use serde_json::json;

    #[test]
    fn records_request_and_response() {
        let mut r = Recorder::new();
        r.record(
            Direction::In,
            &JsonRpcMessage::Request(JsonRpcRequest {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: RequestId::Number(1),
                method: "initialize".to_string(),
                params: Some(json!({"protocolVersion":"2025-03-26"})),
            }),
        );
        r.record(
            Direction::Out,
            &JsonRpcMessage::Response(JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: RequestId::Number(1),
                result: Some(json!({"protocolVersion":"2025-03-26"})),
                error: None,
            }),
        );
        assert_eq!(r.messages().len(), 2);
        assert_eq!(r.count_for(&RequestId::Number(1)), 2);
    }

    #[test]
    fn jsonl_bytes_start_with_version_header() {
        let r = Recorder::new();
        let bytes = r.to_jsonl_bytes().expect("serialize");
        assert!(bytes.starts_with(br#"{"version":1}"#));
    }

    #[test]
    fn jsonl_bytes_per_message_line() {
        let mut r = Recorder::new();
        r.record(
            Direction::In,
            &JsonRpcMessage::Request(JsonRpcRequest {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: RequestId::Number(1),
                method: "x".to_string(),
                params: None,
            }),
        );
        let bytes = r.to_jsonl_bytes().expect("serialize");
        let s = core::str::from_utf8(&bytes).expect("utf8");
        let line_count = s.lines().count();
        assert_eq!(line_count, 2, "header + 1 record");
    }
}
```

- [ ] **Step 2: Add the `with-std-adapters` feature to `tau-mcp/Cargo.toml`.**

Modify `[features]` block (add the section if absent) to:

```toml
[features]
default            = ["with-std-adapters"]
# When on, enables std-backed sinks (file I/O on Recorder / Replayer).
with-std-adapters  = []
```

- [ ] **Step 3: Run the tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp -E 'test(cassette::recorder::tests)'`

Expected: 3 tests pass.

- [ ] **Step 4: Commit.**

```
git add crates/tau-mcp/src/cassette/recorder.rs crates/tau-mcp/Cargo.toml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp/cassette): Recorder + JSONL serialization + with-std-adapters feature"
```

### Task 6.3: Replayer

**Files:**
- Create: `crates/tau-mcp/src/cassette/replayer.rs`

- [ ] **Step 1: Write `src/cassette/replayer.rs`.**

```rust
//! Cassette replayer.
//!
//! Reads a cassette (JSONL bytes), matches inbound (host→server)
//! requests against recorded `Direction::In` entries by
//! (method, normalized args), and emits the matching recorded
//! `Direction::Out` responses + notifications.
//!
//! PR-3 wires this into a `Transport` impl that the in-memory test
//! harness uses (cassette-as-transport).

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use serde_json::Value;
use thiserror::Error;

use crate::cassette::message::{CassetteHeader, CassetteMessage, Direction, MessageKind, CASSETTE_VERSION};

/// Errors during cassette read or replay.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ReplayError {
    /// Cassette bytes are not valid UTF-8 JSONL.
    #[error("cassette parse error: {0}")]
    Parse(String),
    /// Cassette header version is newer than we support.
    #[error("cassette version {found} not supported (max {max})")]
    UnsupportedVersion {
        /// Version we found in the header.
        found: u32,
        /// Maximum version this crate supports.
        max: u32,
    },
    /// No matching inbound entry for an outbound request from the host.
    #[error("no cassette entry matches method {method:?} args {args}")]
    NoMatch {
        /// Method we couldn't match.
        method: String,
        /// Normalized args we tried to match.
        args: String,
    },
}

/// Read-only cassette replayer.
#[derive(Debug, Clone)]
pub struct Replayer {
    /// All messages, in cassette order.
    records: Vec<CassetteMessage>,
    /// Per-record consumption flag (true = already matched once;
    /// matches are one-shot in v0).
    consumed: Vec<bool>,
    /// FIFO queue of outbound (host-bound) records that should be
    /// emitted between matched calls (notifications, server-initiated
    /// requests). Filled when a matching request consumes the records
    /// between it and the matched response.
    pending_outbound: VecDeque<CassetteMessage>,
}

impl Replayer {
    /// Parse a cassette from JSONL bytes.
    pub fn from_jsonl_bytes(bytes: &[u8]) -> Result<Self, ReplayError> {
        let s = core::str::from_utf8(bytes).map_err(|e| ReplayError::Parse(alloc::format!("utf8: {e}")))?;
        let mut lines = s.lines();

        let header_line = lines
            .next()
            .ok_or_else(|| ReplayError::Parse("empty cassette".into()))?;
        let header: CassetteHeader = serde_json::from_str(header_line)
            .map_err(|e| ReplayError::Parse(alloc::format!("header: {e}")))?;
        if header.version > CASSETTE_VERSION {
            return Err(ReplayError::UnsupportedVersion {
                found: header.version,
                max: CASSETTE_VERSION,
            });
        }

        let mut records: Vec<CassetteMessage> = Vec::new();
        for (i, line) in lines.enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let rec: CassetteMessage = serde_json::from_str(line)
                .map_err(|e| ReplayError::Parse(alloc::format!("line {}: {e}", i + 2)))?;
            records.push(rec);
        }

        let consumed = vec![false; records.len()];
        Ok(Self {
            records,
            consumed,
            pending_outbound: VecDeque::new(),
        })
    }

    /// Attempt to match an inbound (host→server) request and return the
    /// recorded outbound response + any notifications/server-initiated
    /// requests that lie between the matched request and its response.
    ///
    /// The matched-request record (`Direction::In`) is consumed; the
    /// outbound records before the matching response are queued for
    /// `next_pending_outbound`; the response itself is returned.
    pub fn match_request(
        &mut self,
        method: &str,
        normalized_args: &Value,
    ) -> Result<CassetteMessage, ReplayError> {
        // Find the first unconsumed inbound record with matching method
        // + args.
        let req_idx = self
            .records
            .iter()
            .enumerate()
            .position(|(i, r)| {
                !self.consumed[i]
                    && r.dir == Direction::In
                    && r.kind == MessageKind::Request
                    && r.method.as_deref() == Some(method)
                    && normalize(&r.payload) == *normalized_args
            })
            .ok_or_else(|| ReplayError::NoMatch {
                method: method.into(),
                args: normalized_args.to_string(),
            })?;
        self.consumed[req_idx] = true;

        // Walk forward; queue Direction::Out records until we hit the
        // matching response (Direction::Out, kind=Response, same id).
        let req_id = self.records[req_idx].id.clone();
        let mut response: Option<CassetteMessage> = None;
        for i in (req_idx + 1)..self.records.len() {
            if self.consumed[i] {
                continue;
            }
            let rec = &self.records[i];
            if rec.dir != Direction::Out {
                continue;
            }
            if rec.kind == MessageKind::Response && rec.id == req_id {
                self.consumed[i] = true;
                response = Some(rec.clone());
                break;
            }
            // Notifications or server-initiated requests between the
            // host's request and the server's response.
            self.consumed[i] = true;
            self.pending_outbound.push_back(rec.clone());
        }

        response.ok_or_else(|| {
            ReplayError::NoMatch {
                method: alloc::format!("response to {method}"),
                args: alloc::format!("id={req_id:?}"),
            }
        })
    }

    /// Drain one pending outbound record (notification or
    /// server-initiated request) for the host to consume between
    /// `match_request` calls.
    pub fn next_pending_outbound(&mut self) -> Option<CassetteMessage> {
        self.pending_outbound.pop_front()
    }
}

/// Normalize a JSON value for comparison (deep, key-sorted, whitespace-
/// independent). Implementation reuses serde_json's BTreeMap-backed
/// `Map` ordering by re-serializing to a string and parsing back.
fn normalize(v: &Value) -> Value {
    // Round-trip through bytes — Map is BTreeMap (no preserve_order
    // feature) so keys come out sorted.
    let bytes = serde_json::to_vec(v).unwrap_or_default();
    serde_json::from_slice(&bytes).unwrap_or_else(|_| v.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cassette::recorder::Recorder;
    use crate::protocol::jsonrpc::{
        JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, RequestId,
        JSONRPC_VERSION,
    };
    use alloc::string::ToString;
    use serde_json::json;

    fn build_weather_cassette() -> Vec<u8> {
        let mut r = Recorder::new();
        // initialize handshake
        r.record(
            Direction::In,
            &JsonRpcMessage::Request(JsonRpcRequest {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: RequestId::Number(0),
                method: "initialize".to_string(),
                params: Some(json!({"protocolVersion":"2025-03-26"})),
            }),
        );
        r.record(
            Direction::Out,
            &JsonRpcMessage::Response(JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: RequestId::Number(0),
                result: Some(json!({"protocolVersion":"2025-03-26"})),
                error: None,
            }),
        );
        // tools/list
        r.record(
            Direction::In,
            &JsonRpcMessage::Request(JsonRpcRequest {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: RequestId::Number(1),
                method: "tools/list".to_string(),
                params: None,
            }),
        );
        r.record(
            Direction::Out,
            &JsonRpcMessage::Response(JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: RequestId::Number(1),
                result: Some(json!({"tools":[{"name":"get_forecast","inputSchema":{"type":"object"}}]})),
                error: None,
            }),
        );
        // tools/call (with mid-request progress notification)
        r.record(
            Direction::In,
            &JsonRpcMessage::Request(JsonRpcRequest {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: RequestId::Number(2),
                method: "tools/call".to_string(),
                params: Some(json!({"name":"get_forecast","arguments":{"lat":40.7,"lon":-74.0}})),
            }),
        );
        r.record(
            Direction::Out,
            &JsonRpcMessage::Notification(JsonRpcNotification {
                jsonrpc: JSONRPC_VERSION.to_string(),
                method: "notifications/progress".to_string(),
                params: Some(json!({"progressToken":"call-2","progress":50,"total":100})),
            }),
        );
        r.record(
            Direction::Out,
            &JsonRpcMessage::Response(JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: RequestId::Number(2),
                result: Some(json!({"content":[{"type":"text","text":"Sunny, 72°F"}]})),
                error: None,
            }),
        );
        r.to_jsonl_bytes().expect("serialize")
    }

    #[test]
    fn parses_well_formed_cassette() {
        let bytes = build_weather_cassette();
        let r = Replayer::from_jsonl_bytes(&bytes).expect("parse");
        assert_eq!(r.records.len(), 7);
    }

    #[test]
    fn matches_initialize_and_returns_response() {
        let bytes = build_weather_cassette();
        let mut r = Replayer::from_jsonl_bytes(&bytes).expect("parse");
        let resp = r
            .match_request("initialize", &json!({"protocolVersion":"2025-03-26"}))
            .expect("match");
        assert_eq!(resp.kind, MessageKind::Response);
        assert_eq!(resp.id, Some(RequestId::Number(0)));
    }

    #[test]
    fn matches_tools_call_and_yields_progress_then_response() {
        let bytes = build_weather_cassette();
        let mut r = Replayer::from_jsonl_bytes(&bytes).expect("parse");
        // consume earlier records first
        r.match_request("initialize", &json!({"protocolVersion":"2025-03-26"}))
            .expect("init");
        r.match_request("tools/list", &Value::Null).expect("list");

        let resp = r
            .match_request(
                "tools/call",
                &json!({"name":"get_forecast","arguments":{"lat":40.7,"lon":-74.0}}),
            )
            .expect("call");
        // The progress notification should be queued for the host to
        // consume next.
        let pending = r.next_pending_outbound().expect("progress");
        assert_eq!(pending.kind, MessageKind::Notification);
        assert_eq!(
            pending.method.as_deref(),
            Some("notifications/progress")
        );
        assert_eq!(resp.kind, MessageKind::Response);
    }

    #[test]
    fn unknown_method_errors() {
        let bytes = build_weather_cassette();
        let mut r = Replayer::from_jsonl_bytes(&bytes).expect("parse");
        let err = r
            .match_request("bogus", &Value::Null)
            .expect_err("should not match");
        assert!(matches!(err, ReplayError::NoMatch { .. }));
    }

    #[test]
    fn unsupported_version_errors() {
        let bytes = br#"{"version":99}
"#;
        let err = Replayer::from_jsonl_bytes(bytes).expect_err("should reject");
        assert!(matches!(err, ReplayError::UnsupportedVersion { .. }));
    }

    #[test]
    fn key_order_normalization_independent() {
        let bytes = build_weather_cassette();
        let mut r = Replayer::from_jsonl_bytes(&bytes).expect("parse");
        // Same args, different key order — should still match.
        let resp = r
            .match_request(
                "tools/call",
                &json!({"arguments":{"lon":-74.0,"lat":40.7},"name":"get_forecast"}),
            );
        // First call: not yet matched (must consume earlier records).
        assert!(resp.is_err()); // tools/call comes after init+list, so unmatched at this point

        // Reset by re-parsing
        let mut r = Replayer::from_jsonl_bytes(&bytes).expect("parse");
        r.match_request("initialize", &json!({"protocolVersion":"2025-03-26"}))
            .expect("init");
        r.match_request("tools/list", &Value::Null).expect("list");
        let resp = r
            .match_request(
                "tools/call",
                &json!({"arguments":{"lon":-74.0,"lat":40.7},"name":"get_forecast"}),
            )
            .expect("normalized match");
        assert_eq!(resp.kind, MessageKind::Response);
    }
}
```

- [ ] **Step 2: Run the tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp -E 'test(cassette::replayer::tests)'`

Expected: 6 tests pass.

- [ ] **Step 3: Commit.**

```
git add crates/tau-mcp/src/cassette/replayer.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp/cassette): Replayer with key-normalized matching + pending-outbound queue"
```

### Task 6.4: Cassette golden vector test

**Files:**
- Create: `crates/tau-mcp/tests/golden_cassette.rs`
- Create: `crates/tau-mcp/tests/fixtures/cassette/weather-happy-path.jsonl`

- [ ] **Step 1: Author `tests/fixtures/cassette/weather-happy-path.jsonl`.**

```jsonl
{"version":1}
{"dir":"in","kind":"request","id":0,"method":"initialize","payload":{"protocolVersion":"2025-03-26"}}
{"dir":"out","kind":"response","id":0,"payload":{"protocolVersion":"2025-03-26"}}
{"dir":"in","kind":"request","id":1,"method":"tools/list","payload":null}
{"dir":"out","kind":"response","id":1,"payload":{"tools":[{"name":"get_forecast","inputSchema":{"type":"object"}}]}}
{"dir":"in","kind":"request","id":2,"method":"tools/call","payload":{"name":"get_forecast","arguments":{"lat":40.7,"lon":-74.0}}}
{"dir":"out","kind":"notification","method":"notifications/progress","payload":{"progressToken":"call-2","progress":50,"total":100}}
{"dir":"out","kind":"response","id":2,"payload":{"content":[{"type":"text","text":"Sunny, 72°F"}]}}
```

- [ ] **Step 2: Write `tests/golden_cassette.rs`.**

```rust
//! Golden-vector test for the cassette format.
//!
//! Asserts the on-disk fixture is parseable and replays in the
//! expected order.

use std::fs;

use tau_mcp::cassette::{Direction, MessageKind, Replayer};
use tau_mcp::protocol::jsonrpc::RequestId;

fn load_cassette() -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/cassette/weather-happy-path.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    fs::read(&path).expect("read fixture")
}

#[test]
fn weather_happy_path_parses() {
    let bytes = load_cassette();
    let _r = Replayer::from_jsonl_bytes(&bytes).expect("parse");
}

#[test]
fn weather_happy_path_full_replay() {
    let bytes = load_cassette();
    let mut r = Replayer::from_jsonl_bytes(&bytes).expect("parse");

    // initialize
    let resp = r
        .match_request("initialize", &serde_json::json!({"protocolVersion":"2025-03-26"}))
        .expect("init");
    assert_eq!(resp.id, Some(RequestId::Number(0)));

    // tools/list
    let resp = r
        .match_request("tools/list", &serde_json::Value::Null)
        .expect("list");
    assert_eq!(resp.id, Some(RequestId::Number(1)));

    // tools/call
    let resp = r
        .match_request(
            "tools/call",
            &serde_json::json!({"name":"get_forecast","arguments":{"lat":40.7,"lon":-74.0}}),
        )
        .expect("call");
    assert_eq!(resp.id, Some(RequestId::Number(2)));

    // progress notification was queued between request + response
    let pending = r.next_pending_outbound().expect("progress notification");
    assert_eq!(pending.dir, Direction::Out);
    assert_eq!(pending.kind, MessageKind::Notification);
    assert_eq!(
        pending.method.as_deref(),
        Some("notifications/progress")
    );
}
```

- [ ] **Step 3: Run the test.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp --test golden_cassette`

Expected: both tests pass.

- [ ] **Step 4: Commit.**

```
git add crates/tau-mcp/tests/golden_cassette.rs crates/tau-mcp/tests/fixtures/cassette/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "test(tau-mcp): golden cassette vector (weather happy path)"
```

---

## Phase 7 — final integration: lib.rs re-exports + workspace check

### Task 7.1: Verify the full `tau-mcp` crate builds clean

- [ ] **Step 1: `cargo check` + `cargo nextest` + `cargo clippy` + `cargo fmt --check` for `tau-mcp`.**

Run sequentially:
```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-mcp
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp -- -D warnings
timeout 30  env CARGO_TARGET_DIR=target/agent-impl cargo fmt --check -p tau-mcp
```

Expected: all green. ~35 unit tests + 2 golden-cassette tests + 2 golden-canonical tests = ~39 tests total.

- [ ] **Step 2: `cargo check` + `cargo clippy` + `cargo fmt --check` for `tau-mcp-tokio` (scaffold only).**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp-tokio -- -D warnings
timeout 30  env CARGO_TARGET_DIR=target/agent-impl cargo fmt --check -p tau-mcp-tokio
```

Expected: all green (the crate has no real code yet; just module skeletons).

- [ ] **Step 3: Workspace-level smoke check (per-crate, NOT `--workspace`) to confirm nothing else broke.**

Pick three downstream crates as canaries — `tau-pkg`, `tau-runtime-tokio`, `tau-cli` — and `cargo check` each:

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-pkg
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-tokio
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-cli
```

Expected: each clean. PR-1 is pure-add — no downstream crate references `tau-mcp` yet, so this should be no-op for them aside from the workspace-Cargo.toml parse.

- [ ] **Step 4: No commit yet — the next phase rolls up.**

### Task 7.2: Update tau-philosophy.md implementation-status footnote (optional, leave to PR-6)

Skip — the implementation-status footnotes in `docs/explanation/tau-philosophy.md` are updated as phases ship. PR-6 will own that update.

### Task 7.3: Final unit-coverage tally + PR-1 summary commit

- [ ] **Step 1: List the new files in this PR.**

Run:
```
git diff --stat origin/main..HEAD
```

Expected output mentions ≥18 new files in `crates/tau-mcp/` + ≥5 in `crates/tau-mcp-tokio/` + ADR + spec + Cargo.toml workspace mod.

- [ ] **Step 2: Verify the test count.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp --no-fail-fast 2>&1 | tail -5`

Expected output: `Summary` line reporting ≥35 tests, all passing.

- [ ] **Step 3: Final phase-7 commit (no functional change; just a marker).**

This step intentionally skipped — the per-task commits in phases 1-6 are the PR-1 history. No "umbrella commit" needed.

---

## Phase 8 — push + open PR + auto-merge enrollment

### Task 8.1: Push the branch

- [ ] **Step 1: Confirm CI-relevant files were touched.**

Run:
```
git diff --name-only origin/main..HEAD | grep -E '\.(rs|toml)$' | head -20
```

Expected: `Cargo.toml`, `crates/tau-mcp/Cargo.toml`, `crates/tau-mcp-tokio/Cargo.toml`, plus the many `.rs` files. Confirms this PR will run the Rust CI lanes (not docs-only).

- [ ] **Step 2: Push with `--no-verify` (per AGENT PUSH RULES — agent-runtime push triggers the deep-gate silent-kill).**

Run:
```
git push --no-verify -u origin feat/beta-3-pr-1-mcp-scaffolds
```

Expected: push completes; remote tracking established.

### Task 8.2: Open the PR

- [ ] **Step 1: Open the PR via `gh pr create`.**

Run:
```
gh pr create --title "β.3 MCP facilitator — PR-1: crate scaffolds + protocol types" --body "$(cat <<'EOF'
## Summary

First of six PRs in the β.3 MCP facilitator sub-project. Pure-add: introduces two new crates (`tau-mcp` + `tau-mcp-tokio`) with the protocol-type surface (JSON-RPC envelopes + the five v0 method payloads + notifications + cancellation), the contract layer (`ServerContract`, canonical-hash, `PinnedContract`), `HostHandlers` trait + default-deny baseline, and the cassette message-level format. No runtime integration yet — PR-2..PR-6 wire it.

- Spec: `docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md`
- Plan: `docs/superpowers/plans/2026-06-01-beta-3-mcp-facilitator-pr-1.md`
- ADR-0038 placeholder; finalized in PR-6.

## Test plan

- [ ] `cargo nextest run -p tau-mcp` green (≥35 unit tests).
- [ ] `cargo test --doc -p tau-mcp` green.
- [ ] Golden vectors (`tests/golden_canonical.rs`, `tests/golden_cassette.rs`) green.
- [ ] `cargo check -p tau-mcp-tokio` green (scaffold only).
- [ ] No-op check on downstream crates (`tau-pkg`, `tau-runtime-tokio`, `tau-cli`).
- [ ] `cargo clippy -p tau-mcp -- -D warnings` clean.
- [ ] CI lanes for both new crates appear green on linux/macos/windows.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Expected: PR URL printed; capture the number.

- [ ] **Step 2: Enrol auto-merge.**

```
gh pr merge <N> --auto
```

Bare form per `feedback_auto_merge_available.md` (no `--squash`/`--delete-branch` flags; auto_merge.flags changed and now rejects them).

- [ ] **Step 3: Watch CI; on first-try-green, the PR auto-merges. If main moves while CI runs, `gh pr update-branch <N>`.**

Per `project_session_overlap_default_2026_05_17.md` and the standing main-busy auto-merge dance.

---

## Self-review checklist (run before declaring PR-1 done)

| Check | Status |
|---|---|
| Workspace `Cargo.toml` has both new crates registered alphabetically | step 1.3 |
| `tau-mcp` has 7 protocol files + 3 contract files + 2 cassette runtime files + 1 host + 1 transport + 1 error + lib.rs (= 16 src files) | phase 1-6 |
| ADR-0038 placeholder filed | phase 2 |
| Spec already committed at `aa02f6b` (kept as PR-1's first commit) | phase 0 |
| ≥35 unit tests + 2 golden_canonical + 2 golden_cassette = ≥39 total | task 7.1 |
| `tau-mcp-tokio` is scaffold-only (no transport_stdio impl yet — PR-2) | task 1.5 |
| `cargo clippy -- -D warnings` clean on both crates | tasks 3.6, 7.1 |
| `cargo fmt --check` clean on both crates | task 7.1 |
| No downstream crate references `tau-mcp` yet (pure-add) | task 7.1 step 3 |
| Branch name reflects PR-1 scope (`feat/beta-3-pr-1-mcp-scaffolds`) | task 0.1 |
| Push used `--no-verify` (agent-runtime silent-kill avoidance) | task 8.1 |
| Auto-merge enrolled via `gh pr merge <N> --auto` BARE | task 8.2 |

---

## What's next: PR-2 through PR-6

Each subsequent PR gets its own plan document authored just-in-time (β.2 family pattern — `2026-06-01-workflow-ir-beta-2-6-2.md` etc.). PR-1 is the foundation; PR-2/3/4 can fan out 3-way once PR-1 merges (per the spec's critical path diagram §15).

| PR | Scope (per spec §15) | Plan filename (when authored) |
|---|---|---|
| **PR-2** | stdio transport + lifecycle + in-tree fixture server (~3-4 days) | `2026-XX-XX-beta-3-pr-2-stdio-transport.md` |
| **PR-3** | HTTP transport + cassette replay (~3-4 days) | `2026-XX-XX-beta-3-pr-3-http-transport.md` |
| **PR-4** | Lowering integration + lockfile v7 + `tau build` wiring (~4-5 days) | `2026-XX-XX-beta-3-pr-4-lowering-lockfile.md` |
| **PR-5** | Bridge + ForwardingDispatcher + runtime drift check (~4-5 days) | `2026-XX-XX-beta-3-pr-5-bridge-dispatcher.md` |
| **PR-6** | CLI verbs + conformance fixture #07 + ADR-0038 finalize + docs (~3-4 days) | `2026-XX-XX-beta-3-pr-6-cli-conformance-docs.md` |

When PR-1 merges, the next plan-writing session takes the spec, decides which PR(s) to start (likely PR-2 first since PR-4 depends on PR-1 only and can also start, but PR-2 unblocks the stdio integration test surface for PR-4's resolver impl), and runs the full `superpowers:brainstorming` → `superpowers:writing-plans` → `superpowers:subagent-driven-development` flow.

The standing constraints (CLAUDE.md cargo rules, agent push rules, auto-merge syntax) apply identically to each subsequent PR — no re-derivation needed.
