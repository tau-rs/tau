# β.3 MCP facilitator — PR-5: Bridge + WiredHostHandlers + runtime drift check + dev/bundle dispatch

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship PR-5 of six in the β.3 sub-project. Wire the MCP runtime: spawn the inbound-dispatch task on `McpClient` (PR-3 shipped outbound only); implement `tau-mcp-tokio::bridge::McpBackedTool` (impls `DynTool`) so MCP entries plug into `ForwardingDispatcher`'s existing `BTreeMap<ToolId, Arc<dyn DynTool>>`; build `WiredHostHandlers` in tau-cli composing `LlmBackend` + per-server `sampling.models` allowlist + `roots`; add the boot-time drift check (re-handshake + canonical-hash compare vs lockfile); compose all of this in a shared `setup_mcp_runtime()` helper called from both `tau run` (dev) and `tau run --bundle` paths.

**Architecture:** Each lockfile MCP entry → one `host_lifecycle::open` handshake at boot → live `tools/list` re-hashed against `lockfile.mcp_entries[i].contract_hash` (drift = refuse to start). `McpClient::start_inbound_dispatch(handlers)` spawns a tokio task that demuxes server-initiated `sampling/createMessage` + `roots/list` requests through `WiredHostHandlers` and writes responses back through `transport.send_message`. Per server-tool (`<entry>.<name>` ToolId), one `McpBackedTool { client, server_tool_name, capability_subset }` is inserted into the same `BTreeMap<ToolId, Arc<dyn DynTool>>` `ForwardingDispatcher` already owns — no special-case routing, no sibling dispatcher.

**Tech Stack:** Rust 2021. `tau-mcp` (HostHandlers, sampling + roots protocol types, contract canonical-hash, CassetteTransport for tests), `tau-mcp-tokio` (host_lifecycle::open, McpClient, transport_stdio/http), `tau-runtime-core` (DynTool, ToolDispatcher, BoxFuture), `tau-cli` (ForwardingDispatcher, LlmBackend wiring, run/bundle paths, lockfile/project access).

**Branch:** `feat/beta-3-pr-5-bridge` (off origin/main at `d98dff6` — PR-4 just landed).

**Worktree:** `/Users/titouanlebocq/code/tau-worktrees/beta-3-pr-5-bridge`.

**Spec reference:** `docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md` — §2 (bridge.rs in tau-mcp-tokio; HostHandlers in tau-mcp), §8.1 (boot flow), §8.2 (per-turn dispatch), §8.3 (inbound sampling), §8.4 (cancellation — DEFERRED to PR-5.1 per locked decisions), §9 (cap enforcement two-pointer model), §12 (testing — default-deny / drift / envelope / bridge unit), §15 (PR-5 row).

**Locked architectural decisions (approved 2026-06-09 in chat; this plan IS the PR-5 design record):**
1. `McpBackedTool` impls `DynTool` and lands in the existing `BTreeMap<ToolId, Arc<dyn DynTool>>`. NO sibling dispatcher; NO ForwardingDispatcher structural change.
2. Cancellation propagation DEFERRED to PR-5.1 (`tools/call` is uncancellable in PR-5; caller blocks until response or transport close).
3. `WiredHostHandlers` in tau-cli composes `Arc<dyn DynLlmBackend>` + `sampling_models: Vec<String>` + `roots: Vec<PathBuf>`. v0 ignores `modelPreferences` (β.3.1 wires it).
4. Boot-time drift check lives in tau-cli's setup (uses `tau_mcp::contract::canonical_hash` + comparison against `LockedMcpEntry.contract_hash`).
5. Dev + Bundle paths share `setup_mcp_runtime()` helper in `tau-cli::cmd::ir_dispatcher`.
6. `McpClient::start_inbound_dispatch(handlers)` is a new method that spawns the inbound pump. Outbound-only McpClient remains the default.

---

## Files map

### Modified
| File | Change |
|---|---|
| `crates/tau-mcp-tokio/src/host_lifecycle/client.rs` | Add `start_inbound_dispatch(self: &Arc<Self>, handlers: Arc<dyn HostHandlers>) -> InboundDispatchHandle`. |
| `crates/tau-mcp-tokio/src/host_lifecycle/mod.rs` | Re-export `InboundDispatchHandle`. |
| `crates/tau-mcp-tokio/src/bridge.rs` | Replace 10-line stub with `McpBackedTool` + `DynTool` impl + helpers. |
| `crates/tau-mcp-tokio/src/lib.rs` | Re-export `bridge::McpBackedTool`. |
| `crates/tau-cli/src/cmd/ir_dispatcher.rs` | Add `WiredHostHandlers`, `setup_mcp_runtime()`, `verify_lockfile_against_live()`. |
| `crates/tau-cli/src/error.rs` (or wherever `RuntimeError` lives) | Add variants `McpContractDriftAtBoot`, `InboundSamplingNotAllowed`, `McpSetupFailed`. |
| `crates/tau-cli/src/cmd/run.rs` (dev path) | Call `setup_mcp_runtime` before constructing `ForwardingDispatcher`; merge MCP entries into `tools_by_id`. |
| `crates/tau-cli/src/cmd/run_bundle.rs` (bundle path; verify actual filename) | Same wiring as dev path. |

### Created (NEW)
| File | Responsibility |
|---|---|
| `crates/tau-mcp-tokio/src/host_lifecycle/inbound_dispatch.rs` | `InboundDispatchHandle` + the spawn helper for the inbound pump task. |
| `crates/tau-cli/tests/cmd_run_mcp.rs` | E2E tests: dev-mode + bundle-mode MCP tool round-trip via cassette transport. |

### Deleted
- None. The PR-1 `bridge.rs` stub gets fully replaced (NOT deleted).

---

## Standing constraints (re-read before EVERY cargo / git command)

Same shape as PR-2/3/4:

| Command | Shape |
|---|---|
| Build / check | `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo {check,build} -p <crate>` |
| Test (nextest) | `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo nextest run -p <crate>` |
| Clippy | `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo clippy -p <crate> --all-targets -- -D warnings` |
| Fmt check | `timeout 30 env CARGO_TARGET_DIR=target/agent-<role> cargo fmt --check -p <crate>` |
| Commits | `git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "..."` |
| Push | `git push --no-verify -u origin feat/beta-3-pr-5-bridge` |
| Auto-merge | `gh pr merge <N> --auto` BARE. (Repo IS a merge queue.) |

PR-2/3/4 addenda baked in:
- DO NOT enable `features = ["test-support"]` on tau-runtime-tokio dev-dep (workspace feature unification trap).
- CI's stable rustc may be newer than local — `Option::is_some_and(...)` over `map_or(false, ...)`.
- `#[non_exhaustive]` types need explicit `::new()` constructors for downstream construction.
- When extending a permissive cache/lookup pattern, preserve cache-miss semantics; enforce strictness at the entry point.

---

## Phase 1 — McpClient::start_inbound_dispatch

### Task 1.1: Create `inbound_dispatch.rs`

**Files:**
- Create: `crates/tau-mcp-tokio/src/host_lifecycle/inbound_dispatch.rs`

- [ ] **Step 1: Read** `crates/tau-mcp-tokio/src/host_lifecycle/client.rs` (full file, ~110 lines), `crates/tau-mcp/src/protocol/jsonrpc.rs` (JsonRpcMessage + JsonRpcRequest + JsonRpcResponse + JsonRpcError shapes), and `crates/tau-mcp/src/host/handlers.rs` (HostHandlers + InboundError shape, both confirmed in PR-1).

Confirm method names + field names before writing:
- `JsonRpcRequest { jsonrpc, id, method, params }` per PR-2.
- `JsonRpcResponse { jsonrpc, id, result: Option<Value>, error: Option<JsonRpcError> }`.
- `JsonRpcError { code: i32, message: String, data: Option<Value> }`.
- `transport.next_message().await -> Result<Option<JsonRpcMessage>, McpError>`.
- `transport.send_message(&msg).await -> Result<(), McpError>`.

- [ ] **Step 2: Write `inbound_dispatch.rs`:**

```rust
//! Inbound-dispatch task for `McpClient`.
//!
//! PR-3 shipped `McpClient` with outbound-only `call_tool` semantics:
//! the inbound side of `transport.next_message()` was drained ad-hoc
//! by `call_tool`'s response loop. PR-5 adds the server-initiated
//! request side: a tokio task that loops on `transport.next_message`,
//! routes `sampling/createMessage` + `roots/list` requests through
//! `HostHandlers`, and writes responses back via `transport.send_message`.
//!
//! Cancellation propagation (PR-5.1) is NOT in this PR — the pump
//! exits cleanly when the transport closes (next_message returns None
//! or McpError::Transport), and the parent can `shutdown()` the handle
//! to abort the task.

use std::sync::Arc;

use tau_mcp::host::handlers::{HostHandlers, InboundError};
use tau_mcp::protocol::jsonrpc::{
    JsonRpcError, JsonRpcMessage, JsonRpcRequest, JsonRpcResponse, RequestId, JSONRPC_VERSION,
};
use tau_mcp::protocol::roots::RootsListResponse;
use tau_mcp::protocol::sampling::{SamplingCreateMessageRequest, SamplingCreateMessageResponse};
use tau_mcp::transport::Transport;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

/// JSON-RPC custom error code for refused inbound requests.
///
/// Per MCP spec rev 2025-03-26: `-32000` to `-32099` is the custom
/// server-error range. We use `-32000` for HostHandlers refusals.
pub const INBOUND_REFUSED_ERROR_CODE: i32 = -32000;

/// Handle returned by `McpClient::start_inbound_dispatch`. Drop or
/// call `shutdown()` to abort the pump task.
#[must_use = "drop the handle or call shutdown() to abort the inbound pump"]
pub struct InboundDispatchHandle {
    task: JoinHandle<()>,
}

impl InboundDispatchHandle {
    /// Construct from a spawned task.
    pub(crate) fn new(task: JoinHandle<()>) -> Self {
        Self { task }
    }

    /// Abort the pump task. Idempotent.
    pub fn shutdown(self) {
        self.task.abort();
    }
}

/// Spawn the inbound-dispatch task for a given transport + handlers.
///
/// The task loops on `transport.next_message()`, routes server-initiated
/// requests through `handlers`, writes the response back via
/// `transport.send_message`. Exits cleanly on EOF or transport error
/// (logged at warn level).
pub fn spawn_inbound_dispatch(
    transport: Arc<dyn Transport>,
    handlers: Arc<dyn HostHandlers>,
) -> InboundDispatchHandle {
    let task = tokio::spawn(async move {
        loop {
            let msg = match transport.next_message().await {
                Ok(Some(m)) => m,
                Ok(None) => {
                    debug!("inbound-dispatch: transport closed cleanly");
                    return;
                }
                Err(e) => {
                    warn!(error = %e, "inbound-dispatch: transport error; exiting");
                    return;
                }
            };
            // Only server-initiated REQUESTS need routing here.
            // Responses to our outbound calls are consumed by call_tool's
            // own recv loop in PR-3. Notifications are TODO (β.3.1 wires
            // progress + log tracing).
            let JsonRpcMessage::Request(req) = msg else {
                continue;
            };
            if let Err(e) = route_request(&*transport, &*handlers, req).await {
                warn!(error = %e, "inbound-dispatch: route_request failed");
            }
        }
    });
    InboundDispatchHandle::new(task)
}

async fn route_request(
    transport: &dyn Transport,
    handlers: &dyn HostHandlers,
    req: JsonRpcRequest,
) -> Result<(), tau_mcp::McpError> {
    let id = req.id.clone();
    let response_payload = match req.method.as_str() {
        "sampling/createMessage" => {
            let parsed: SamplingCreateMessageRequest = match serde_json::from_value(
                req.params.unwrap_or(serde_json::Value::Null),
            ) {
                Ok(p) => p,
                Err(e) => return send_error(transport, id, format!("decode sampling: {e}")).await,
            };
            match handlers.sampling(parsed).await {
                Ok(resp) => match serde_json::to_value(resp) {
                    Ok(v) => Ok(v),
                    Err(e) => return send_error(transport, id, format!("encode sampling: {e}")).await,
                },
                Err(e) => return send_inbound_error(transport, id, e).await,
            }
        }
        "roots/list" => {
            match handlers.roots().await {
                Ok(roots) => {
                    let resp = RootsListResponse { roots };
                    match serde_json::to_value(resp) {
                        Ok(v) => Ok(v),
                        Err(e) => return send_error(transport, id, format!("encode roots: {e}")).await,
                    }
                }
                Err(e) => return send_inbound_error(transport, id, e).await,
            }
        }
        other => {
            return send_error(
                transport,
                id,
                format!("unsupported server-initiated method: {other}"),
            )
            .await;
        }
    };
    let result = response_payload?;
    let msg = JsonRpcMessage::Response(JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id,
        result: Some(result),
        error: None,
    });
    transport.send_message(&msg).await
}

async fn send_inbound_error(
    transport: &dyn Transport,
    id: RequestId,
    e: InboundError,
) -> Result<(), tau_mcp::McpError> {
    send_error(transport, id, format!("{e}")).await
}

async fn send_error(
    transport: &dyn Transport,
    id: RequestId,
    message: String,
) -> Result<(), tau_mcp::McpError> {
    let msg = JsonRpcMessage::Response(JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id,
        result: None,
        error: Some(JsonRpcError {
            code: INBOUND_REFUSED_ERROR_CODE,
            message,
            data: None,
        }),
    });
    transport.send_message(&msg).await
}
```

**Implementer notes:**
- `RootsListResponse { roots: Vec<Root> }` — verify shape in `crates/tau-mcp/src/protocol/roots.rs` and adjust if the response is named differently or carries other fields.
- `JsonRpcError { code, message, data }` — confirm field types. PR-3's tests instantiated this struct; crib if needed.
- `tau_mcp::McpError` re-export path — should be `tau_mcp::McpError` per PR-1 lib.rs.

- [ ] **Step 3: Wire into `host_lifecycle/mod.rs`:**

Add submodule + re-export:

```rust
pub mod inbound_dispatch;
pub use inbound_dispatch::{spawn_inbound_dispatch, InboundDispatchHandle, INBOUND_REFUSED_ERROR_CODE};
```

- [ ] **Step 4: Add `McpClient::start_inbound_dispatch` method.** Edit `crates/tau-mcp-tokio/src/host_lifecycle/client.rs` — append to `impl McpClient`:

```rust
    /// Spawn the inbound-dispatch task for this client.
    ///
    /// Routes server-initiated `sampling/createMessage` + `roots/list`
    /// requests through `handlers`. Returns a handle whose `shutdown()`
    /// or `Drop` aborts the pump.
    ///
    /// Call AT MOST ONCE per McpClient — the pump owns the inbound side
    /// of the transport. Calling twice would race with `call_tool`'s
    /// response loop. In v0 the pattern is: spawn the pump for clients
    /// that need server-initiated routing; do nothing for outbound-only
    /// clients (e.g. the live resolver from PR-4).
    pub fn start_inbound_dispatch(
        &self,
        handlers: std::sync::Arc<dyn tau_mcp::host::handlers::HostHandlers>,
    ) -> crate::host_lifecycle::InboundDispatchHandle {
        crate::host_lifecycle::spawn_inbound_dispatch(self.transport.clone(), handlers)
    }
```

(Note: `transport: Arc<dyn Transport>` is already a field on McpClient — confirm by reading client.rs.)

- [ ] **Step 5: cargo check + clippy.**

```sh
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp-tokio --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 6: Commit.**

```sh
git add crates/tau-mcp-tokio/src/host_lifecycle/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/host_lifecycle): start_inbound_dispatch — routes server-initiated sampling/roots through HostHandlers"
```

### Task 1.2: Cassette-based inbound-dispatch test

**Files:**
- Create: `crates/tau-mcp-tokio/tests/inbound_dispatch.rs`

A test that feeds a cassette with a server-initiated `sampling/createMessage` request, attaches a `DefaultDenyHandlers`, and asserts the response written back via the transport is a JSON-RPC error with `code = -32000`.

- [ ] **Step 1: Read** `crates/tau-mcp/src/cassette/transport.rs` (added in PR-3, gated on `with-std-adapters`) to confirm the `CassetteTransport::from_jsonl_bytes` API + how it exposes outbound (what the host writes back).

The cassette test pattern: build a `CassetteTransport` from a cassette with a sampling-request entry in the OUT direction (server → host), feed it to `spawn_inbound_dispatch`, then read the next OUT-direction message from the cassette (which the inbound pump should have written back).

This test will fail unless `CassetteTransport` supports reading host-written messages. If it doesn't, this test stays as a `#[ignore]` with a TODO note pointing at PR-6's broader test surface. The simpler test below uses `tokio::io::duplex` to simulate a bidirectional channel directly — that's the recommended path for PR-5.

- [ ] **Step 2: Write the duplex-based test** (avoids depending on cassette-as-bidirectional-mock):

```rust
//! Tests for the inbound-dispatch task.
//!
//! Uses a custom Transport impl over `tokio::sync::mpsc` channels so
//! the test fully controls inbound feed + outbound assert. Avoids the
//! cassette transport, which is read-only.

use std::pin::Pin;
use std::sync::Arc;

use tau_mcp::host::handlers::DefaultDenyHandlers;
use tau_mcp::protocol::jsonrpc::{
    JsonRpcMessage, JsonRpcRequest, JsonRpcResponse, RequestId, JSONRPC_VERSION,
};
use tau_mcp::transport::Transport;
use tau_mcp::McpError;
use tau_mcp_tokio::host_lifecycle::{spawn_inbound_dispatch, INBOUND_REFUSED_ERROR_CODE};
use tokio::sync::{mpsc, Mutex};

/// Bidirectional mpsc-backed Transport for tests.
struct MpscTransport {
    inbound: Mutex<mpsc::UnboundedReceiver<JsonRpcMessage>>,
    outbound: mpsc::UnboundedSender<JsonRpcMessage>,
}

impl Transport for MpscTransport {
    fn send_message<'a>(
        &'a self,
        msg: &'a JsonRpcMessage,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), McpError>> + Send + 'a>> {
        let msg = msg.clone();
        let tx = self.outbound.clone();
        Box::pin(async move {
            tx.send(msg)
                .map_err(|_| McpError::Transport("outbound channel closed".into()))?;
            Ok(())
        })
    }

    fn next_message<'a>(
        &'a self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Option<JsonRpcMessage>, McpError>> + Send + 'a>>
    {
        Box::pin(async move {
            let mut rx = self.inbound.lock().await;
            Ok(rx.recv().await)
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_deny_sampling_yields_jsonrpc_error_with_code_neg_32000() {
    let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
    let transport: Arc<dyn Transport> = Arc::new(MpscTransport {
        inbound: Mutex::new(inbound_rx),
        outbound: outbound_tx,
    });

    // Feed a sampling/createMessage request to the inbound side.
    let req = JsonRpcMessage::Request(JsonRpcRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: RequestId::Number(99),
        method: "sampling/createMessage".to_string(),
        params: Some(serde_json::json!({
            "messages": [{"role": "user", "content": {"type": "text", "text": "x"}}],
            "modelPreferences": {}
        })),
    });
    inbound_tx.send(req).unwrap();

    let _handle = spawn_inbound_dispatch(transport, Arc::new(DefaultDenyHandlers));

    // The pump should write a JsonRpcResponse back.
    let resp = tokio::time::timeout(std::time::Duration::from_secs(2), outbound_rx.recv())
        .await
        .expect("response written within 2s")
        .expect("channel still open");

    let JsonRpcMessage::Response(JsonRpcResponse { id, result, error, .. }) = resp else {
        panic!("expected Response, got {resp:?}");
    };
    assert_eq!(id, RequestId::Number(99));
    assert!(result.is_none(), "default-deny should set result=None");
    let err = error.expect("default-deny should set error");
    assert_eq!(err.code, INBOUND_REFUSED_ERROR_CODE);
    assert!(err.message.contains("sampling"), "error message mentions sampling: {}", err.message);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_deny_roots_returns_empty_list() {
    let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
    let transport: Arc<dyn Transport> = Arc::new(MpscTransport {
        inbound: Mutex::new(inbound_rx),
        outbound: outbound_tx,
    });

    let req = JsonRpcMessage::Request(JsonRpcRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: RequestId::Number(7),
        method: "roots/list".to_string(),
        params: None,
    });
    inbound_tx.send(req).unwrap();

    let _handle = spawn_inbound_dispatch(transport, Arc::new(DefaultDenyHandlers));

    let resp = tokio::time::timeout(std::time::Duration::from_secs(2), outbound_rx.recv())
        .await
        .expect("response written within 2s")
        .expect("channel still open");

    let JsonRpcMessage::Response(JsonRpcResponse { id, result, error, .. }) = resp else {
        panic!("expected Response, got {resp:?}");
    };
    assert_eq!(id, RequestId::Number(7));
    assert!(error.is_none(), "roots/list should succeed: {error:?}");
    let result = result.expect("roots/list should return result");
    let roots = result.get("roots").and_then(|v| v.as_array()).expect("result.roots is an array");
    assert!(roots.is_empty(), "default-deny roots returns []");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_method_yields_jsonrpc_error() {
    let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
    let transport: Arc<dyn Transport> = Arc::new(MpscTransport {
        inbound: Mutex::new(inbound_rx),
        outbound: outbound_tx,
    });

    let req = JsonRpcMessage::Request(JsonRpcRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: RequestId::Number(1),
        method: "future/method".to_string(),
        params: None,
    });
    inbound_tx.send(req).unwrap();

    let _handle = spawn_inbound_dispatch(transport, Arc::new(DefaultDenyHandlers));

    let resp = tokio::time::timeout(std::time::Duration::from_secs(2), outbound_rx.recv())
        .await
        .expect("response written within 2s")
        .expect("channel still open");

    let JsonRpcMessage::Response(JsonRpcResponse { error, .. }) = resp else {
        panic!("expected Response");
    };
    let err = error.expect("error variant");
    assert!(err.message.contains("unsupported server-initiated"));
}
```

- [ ] **Step 3: Run.**

```sh
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp-tokio --test inbound_dispatch
```

Expected: 3 tests pass.

- [ ] **Step 4: Commit.**

```sh
git add crates/tau-mcp-tokio/tests/inbound_dispatch.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "test(tau-mcp-tokio): inbound-dispatch routes sampling/roots through HostHandlers (default-deny)"
```

---

## Phase 2 — McpBackedTool (bridge.rs)

### Task 2.1: Replace bridge.rs stub with McpBackedTool

**Files:**
- Modify: `crates/tau-mcp-tokio/src/bridge.rs` (currently 10-line stub)
- Modify: `crates/tau-mcp-tokio/src/lib.rs`

`McpBackedTool` impls `tau_runtime_core::builder::DynTool`. Each MCP-expanded ToolId (`<entry>.<server-tool>`) maps to one McpBackedTool sharing the same underlying `Arc<McpClient>`.

- [ ] **Step 1: Read** `crates/tau-runtime-core/src/builder.rs` lines 125-200 for the exact `DynTool` shape:
  - `fn name(&self) -> &str`
  - `fn schema(&self) -> ToolSpec`
  - `fn capabilities(&self) -> &[tau_domain::Capability]`
  - `fn init<'a>(&'a self, ctx: SessionContext) -> BoxFuture<'a, Result<(), ToolError>>`
  - `fn invoke<'a>(&'a self, ctx: &'a SessionContext, session: &'a mut (), args: tau_domain::Value) -> BoxFuture<'a, Result<ToolResult, ToolError>>`
  - `fn teardown<'a>(&'a self, session: ()) -> BoxFuture<'a, Result<(), ToolError>>`

Also read `crates/tau-mcp/src/protocol/tools.rs` for `ToolsCallResponse` + `ContentBlock` (the response from `client.call_tool`).

- [ ] **Step 2: Write `bridge.rs`:**

```rust
//! `McpBackedTool` — implements `DynTool` over an `Arc<McpClient>` so
//! MCP-expanded tools register in the standard
//! `BTreeMap<ToolId, Arc<dyn DynTool>>` that `ForwardingDispatcher`
//! already owns.
//!
//! One `McpBackedTool` per IR ToolId (`<entry>.<server-tool>`).
//! All entries for the same `[tools.<entry>]` share the same
//! `Arc<McpClient>`; only `server_tool_name` + `capability_subset`
//! differ.

use std::sync::Arc;

use tau_domain::{Capability, Value};
use tau_mcp::protocol::tools::{ContentBlock, ToolsCallResponse};
use tau_runtime_core::builder::{BoxFuture, DynTool};
use tau_ports::{SessionContext, ToolError, ToolResult, ToolSpec};

use crate::host_lifecycle::McpClient;

/// One MCP-expanded server-tool exposed as a `DynTool`.
pub struct McpBackedTool {
    /// IR ToolId in the form `<entry>.<server-tool>` (e.g. `weather.get_forecast`).
    /// Held as the tool's `name()` per DynTool contract.
    ir_tool_id: String,
    /// Shared MCP client for this server entry.
    client: Arc<McpClient>,
    /// Server-side tool name sent on `tools/call`.
    server_tool_name: String,
    /// Capabilities the tool requires (per PR-4 expansion's intersection).
    capabilities: Vec<Capability>,
    /// JSON Schema for the tool's input (passed through from the contract).
    input_schema: serde_json::Value,
    /// Human-readable description, propagated to the LLM tools/list.
    description: String,
}

impl McpBackedTool {
    /// Construct one MCP-backed tool.
    pub fn new(
        ir_tool_id: impl Into<String>,
        client: Arc<McpClient>,
        server_tool_name: impl Into<String>,
        capabilities: Vec<Capability>,
        input_schema: serde_json::Value,
        description: impl Into<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            ir_tool_id: ir_tool_id.into(),
            client,
            server_tool_name: server_tool_name.into(),
            capabilities,
            input_schema,
            description: description.into(),
        })
    }

    /// Server-side tool name (for diagnostics / tests).
    pub fn server_tool_name(&self) -> &str {
        &self.server_tool_name
    }
}

impl tau_ports::Tool for McpBackedTool {
    type Session = ();

    fn name(&self) -> &str {
        &self.ir_tool_id
    }

    fn schema(&self) -> ToolSpec {
        // ToolSpec shape — adapt to whatever tau-ports actually exposes.
        // The implementer should crib from any existing in-tree DynTool
        // (e.g. tau-plugin's ipc tool) for the right constructor.
        ToolSpec {
            name: self.ir_tool_id.clone(),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
        }
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    fn init(
        &self,
        _ctx: SessionContext,
    ) -> BoxFuture<'_, Result<Self::Session, ToolError>> {
        // McpBackedTool is stateless; no per-session setup. The MCP
        // client itself was opened at runtime boot.
        Box::pin(async { Ok(()) })
    }

    fn invoke<'a>(
        &'a self,
        _session: &'a mut Self::Session,
        args: Value,
    ) -> BoxFuture<'a, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            // tau_domain::Value → serde_json::Value
            let json_args = serde_json::to_value(&args).map_err(|e| ToolError::Internal {
                message: format!("encode MCP args: {e}"),
            })?;
            let resp: ToolsCallResponse = self
                .client
                .call_tool(&self.server_tool_name, json_args)
                .await
                .map_err(|e| ToolError::Internal {
                    message: format!("MCP call_tool {:?}: {e}", self.server_tool_name),
                })?;
            // Convert MCP Content[] to a single text body (PR #277 body-shape
            // symmetry — match ForwardingDispatcher's joined-text pattern).
            let joined = content_to_text(&resp.content);
            // Try to round-trip as JSON; fall back to raw text. Same
            // semantics as ForwardingDispatcher::invoke per the comment
            // at agent_loop.rs:248-263.
            let body: serde_json::Value = serde_json::from_str(&joined)
                .unwrap_or(serde_json::Value::String(joined));
            let domain_body: Value = serde_json::from_value(body).map_err(|e| ToolError::Internal {
                message: format!("decode tools/call body: {e}"),
            })?;
            Ok(ToolResult {
                body: domain_body,
                // ToolResult may carry other fields; the implementer
                // should read tau_ports::ToolResult's actual shape and
                // adapt. Common shape from PR-2 was `ToolResult { body }`.
            })
        })
    }

    fn teardown(&self, _session: Self::Session) -> BoxFuture<'_, Result<(), ToolError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Extract joined text from MCP content blocks. Non-text blocks
/// (image, resource) are rendered as `"[non-text content]"` placeholders
/// in v0; β.3.1 may surface them structurally.
fn content_to_text(blocks: &[ContentBlock]) -> String {
    let mut out = String::new();
    for b in blocks {
        if !out.is_empty() {
            out.push('\n');
        }
        match b {
            ContentBlock::Text { text } => out.push_str(text),
            _ => out.push_str("[non-text content]"),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_to_text_joins_with_newline() {
        let blocks = vec![
            ContentBlock::Text { text: "hello".to_string() },
            ContentBlock::Text { text: "world".to_string() },
        ];
        assert_eq!(content_to_text(&blocks), "hello\nworld");
    }

    #[test]
    fn content_to_text_handles_single_block() {
        let blocks = vec![ContentBlock::Text { text: "solo".to_string() }];
        assert_eq!(content_to_text(&blocks), "solo");
    }

    #[test]
    fn content_to_text_handles_empty() {
        let blocks: Vec<ContentBlock> = vec![];
        assert_eq!(content_to_text(&blocks), "");
    }
}
```

**Implementer notes:**
- `tau_ports::ToolSpec` may have a different constructor / field names than shown — read `crates/tau-ports/src/tool.rs` or similar to confirm. Adapt the `schema()` impl accordingly.
- `tau_ports::ToolResult` may carry more than just `body` (e.g. an `is_error: bool` field from earlier PRs). Read + adapt.
- The blanket `impl<T: Tool<Session = ()> + 'static> DynTool for T` (in `builder.rs` line 157) means we can impl `tau_ports::Tool` and get `DynTool` for free. That's what the snippet above does.
- `tau_ports::Tool` may not exist with that exact name — search `git grep "pub trait Tool\b" crates/tau-ports/`. If it lives elsewhere, adapt the import.
- Add an integration test in Phase 2 OR defer to the Phase 6 cassette E2E. v0 leans on the latter — the unit tests above (3) cover `content_to_text` correctness; full `invoke` semantics are covered by the dev-mode cassette E2E in Phase 6.

- [ ] **Step 3: Wire into lib.rs.**

Add to `crates/tau-mcp-tokio/src/lib.rs`:

```rust
pub use bridge::McpBackedTool;
```

(`pub mod bridge;` already exists from PR-1.)

- [ ] **Step 4: cargo check + clippy.**

```sh
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp-tokio --all-targets -- -D warnings
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp-tokio --test inbound_dispatch -E 'test(content_to_text)' 2>/dev/null
```

The third command exercises the bridge.rs unit tests in-place (via the standard nextest run; the per-test selector is illustrative).

Actually run the full crate's tests:

```sh
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp-tokio
```

Expected: existing tests still pass (PR-3's 37 + PR-5 Phase 1's 3 = 40) + 3 new bridge::tests.

- [ ] **Step 5: Commit.**

```sh
git add crates/tau-mcp-tokio/src/bridge.rs crates/tau-mcp-tokio/src/lib.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/bridge): McpBackedTool impls DynTool over Arc<McpClient> (PR #277 body-shape symmetry)"
```

---

## Phase 3 — WiredHostHandlers in tau-cli

### Task 3.1: WiredHostHandlers struct + HostHandlers impl + unit tests

**Files:**
- Modify: `crates/tau-cli/src/cmd/ir_dispatcher.rs`

- [ ] **Step 1: Read** `crates/tau-cli/src/cmd/ir_dispatcher.rs` to see existing imports + structure. Confirm `Arc<dyn DynLlmBackend>` is the LlmBackend handle type (PR-1 forward).

Read `crates/tau-mcp/src/protocol/sampling.rs` for the exact `SamplingCreateMessageResponse` shape:
```
SamplingCreateMessageResponse { role, content: SamplingContent::Text { text }, model, stop_reason, additional }
```
(Adjust based on real fields.)

- [ ] **Step 2: Append `WiredHostHandlers`** to `ir_dispatcher.rs`:

```rust
// ---------------------------------------------------------------------------
// WiredHostHandlers (β.3 PR-5)
// ---------------------------------------------------------------------------

use std::path::PathBuf;
use tau_mcp::host::handlers::{HostHandlers, InboundError, BoxFuture as HostBoxFuture};
use tau_mcp::protocol::roots::Root;
use tau_mcp::protocol::sampling::{
    SamplingContent, SamplingCreateMessageRequest, SamplingCreateMessageResponse,
};

/// Inbound MCP handler impl wired against an agent's LlmBackend + the
/// per-server sampling.models allowlist + roots declaration from tau.toml.
pub(crate) struct WiredHostHandlers {
    /// LLM backend the agent owns — sampling delegates to this.
    backend: Arc<dyn DynLlmBackend>,
    /// Allowlisted model ids. Empty = sampling refused.
    sampling_models: Vec<String>,
    /// Roots returned to the server on `roots/list`.
    roots: Vec<PathBuf>,
}

impl WiredHostHandlers {
    pub(crate) fn new(
        backend: Arc<dyn DynLlmBackend>,
        sampling_models: Vec<String>,
        roots: Vec<PathBuf>,
    ) -> Arc<Self> {
        Arc::new(Self {
            backend,
            sampling_models,
            roots,
        })
    }
}

impl HostHandlers for WiredHostHandlers {
    fn sampling<'a>(
        &'a self,
        req: SamplingCreateMessageRequest,
    ) -> HostBoxFuture<'a, Result<SamplingCreateMessageResponse, InboundError>> {
        Box::pin(async move {
            if self.sampling_models.is_empty() {
                return Err(InboundError::SamplingNotAllowed);
            }
            // v0 model picker: pick first allowlisted. β.3.1 reads
            // req.model_preferences and picks accordingly.
            let model = self.sampling_models[0].clone();

            // Translate MCP SamplingMessage[] → backend's prompt shape.
            // The actual LlmBackend method may take `&str` system + `Vec<Message>` user;
            // implementer must adapt to crates/tau-runtime-core/src/builder.rs's
            // DynLlmBackend trait. Crib from existing tau-cli call sites.
            let prompt_text = req.messages.iter().map(|m| match &m.content {
                SamplingContent::Text { text } => text.clone(),
                #[allow(unreachable_patterns)]
                _ => String::new(),
            }).collect::<Vec<_>>().join("\n");

            // INVOKE THE BACKEND. The exact signature is backend-dependent.
            // Sketch (implementer adapts):
            //
            //   let completion = self.backend.complete(&model, &prompt_text)
            //       .await
            //       .map_err(|e| InboundError::Backend(format!("{e}")))?;
            //
            // If DynLlmBackend doesn't have a sync-style complete() method
            // and uses a streaming API instead, drain the stream into a
            // joined string before returning.
            //
            // For v0, if the integration is non-trivial, return an
            // InboundError::Backend with a descriptive message and gate
            // the real call behind a #[cfg(any(test, feature = "..."))]
            // path. Phase 3.1 test below uses the empty-allowlist refuse
            // path, which exercises the gate WITHOUT calling the backend.

            // STUB shape (replace with real backend call when wired):
            let text = format!("[sampling stub for model {model}; prompt={prompt_text:?}]");

            Ok(SamplingCreateMessageResponse {
                role: "assistant".to_string(),
                content: SamplingContent::Text { text },
                model,
                stop_reason: Some("endTurn".to_string()),
                additional: Default::default(),
            })
        })
    }

    fn roots<'a>(&'a self) -> HostBoxFuture<'a, Result<Vec<Root>, InboundError>> {
        Box::pin(async move {
            Ok(self.roots.iter().map(|p| Root {
                uri: format!("file://{}", p.display()),
                name: p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()),
            }).collect())
        })
    }
}
```

**Implementer notes — be honest about scope:**
- The "STUB shape" comment is deliberately a placeholder for the real LlmBackend invocation. The unit tests below only exercise the EMPTY-ALLOWLIST refuse path, which doesn't touch the backend. A real sampling round-trip needs the actual backend wired in — that's β.3.1 work, not PR-5. **If clippy refuses to compile the stub** (e.g. unreachable_pattern warnings on the catch-all `_ => ` arm for SamplingContent that only has one variant), drop the `#[allow]` and the catch-all.
- `SamplingContent` may have multiple variants (Text, Image). Pattern-match exhaustively or use `match` with `_`.
- `Root { uri, name }` — verify field names in `crates/tau-mcp/src/protocol/roots.rs`.

- [ ] **Step 3: Add three unit tests at the bottom of `ir_dispatcher.rs`:**

```rust
#[cfg(test)]
mod wired_handlers_tests {
    use super::*;
    use tau_mcp::protocol::sampling::{SamplingMessage, SamplingContent};

    fn req(text: &str) -> SamplingCreateMessageRequest {
        SamplingCreateMessageRequest {
            messages: vec![SamplingMessage {
                role: "user".to_string(),
                content: SamplingContent::Text { text: text.to_string() },
            }],
            model_preferences: None,
            system_prompt: None,
            include_context: None,
            max_tokens: None,
            additional: Default::default(),
        }
    }

    fn backend_stub() -> Arc<dyn DynLlmBackend> {
        // Reuse existing tau-cli test backend (search for `MockLlmBackend` or
        // similar in tau-cli/tests/common/). If unavailable, define a
        // minimal no-op impl inline.
        unimplemented!("crib from existing tau-cli MockLlmBackend pattern")
    }

    #[tokio::test]
    async fn empty_allowlist_refuses_sampling() {
        let h = WiredHostHandlers::new(backend_stub(), Vec::new(), Vec::new());
        let err = h.sampling(req("hi")).await.expect_err("should refuse");
        assert!(matches!(err, InboundError::SamplingNotAllowed));
    }

    #[tokio::test]
    async fn empty_roots_returns_empty_list() {
        let h = WiredHostHandlers::new(backend_stub(), Vec::new(), Vec::new());
        let roots = h.roots().await.expect("ok");
        assert!(roots.is_empty());
    }

    #[tokio::test]
    async fn roots_serializes_paths_as_file_uri() {
        let h = WiredHostHandlers::new(
            backend_stub(),
            Vec::new(),
            vec![PathBuf::from("/tmp/mcp-cache")],
        );
        let roots = h.roots().await.expect("ok");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].uri, "file:///tmp/mcp-cache");
    }
}
```

**Implementer note:** the `backend_stub` helper requires a `DynLlmBackend` impl. Look for `MockLlmBackend` in `crates/tau-cli/tests/common/` (PR #83 introduced it) and crib. If it's not pub-reachable from this test mod, define a minimal no-op stub inline (4-5 lines).

- [ ] **Step 4: cargo check + tests.**

```sh
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-cli
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-cli --all-targets -- -D warnings
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli -E 'test(wired_handlers_tests)'
```

Expected: 3 tests pass.

- [ ] **Step 5: Commit.**

```sh
git add crates/tau-cli/src/cmd/ir_dispatcher.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-cli/ir_dispatcher): WiredHostHandlers — sampling allowlist + roots (sampling backend wiring stub; β.3.1)"
```

---

## Phase 4 — Boot-time drift check

### Task 4.1: `verify_lockfile_against_live` helper + RuntimeError variants

**Files:**
- Modify: `crates/tau-cli/src/cmd/ir_dispatcher.rs`
- Modify: `crates/tau-cli/src/error.rs` (or wherever `RuntimeError` lives — confirm via `git grep`)

- [ ] **Step 1: Locate `RuntimeError` enum.** Run:

```sh
git grep -n "pub enum RuntimeError" crates/tau-cli/src/
```

Likely in `crates/tau-cli/src/error.rs` or inline in a runtime module. Note the existing thiserror pattern and add new variants:

```rust
    /// Boot-time drift: live tools/list hash differs from lockfile.
    #[error(
        "MCP contract drift at boot for entry {entry:?}: expected hash {expected_hash}, got {actual_hash}"
    )]
    McpContractDriftAtBoot {
        /// `[tools.<entry>]` name.
        entry: String,
        /// Hex hash from the lockfile.
        expected_hash: String,
        /// Hex hash from the live handshake.
        actual_hash: String,
    },
    /// MCP setup failed (handshake / spawn / etc.) — fold inbound resolver errors.
    #[error("MCP setup failed for entry {entry:?}: {reason}")]
    McpSetupFailed {
        /// `[tools.<entry>]` name.
        entry: String,
        /// Reason rendered.
        reason: String,
    },
```

(Add a `InboundSamplingNotAllowed` variant too if any caller needs to surface it specifically — likely not needed since the InboundError flows through the inbound dispatch task and never reaches RuntimeError.)

- [ ] **Step 2: Add the verify helper** to `ir_dispatcher.rs`:

```rust
use tau_mcp::contract::canonical::canonical_hash;
use tau_pkg::lockfile::LockedMcpEntry;
use tau_mcp_tokio::host_lifecycle::McpClient;

/// Verify that the live MCP handshake matches the lockfile-recorded
/// hash for one entry. Returns `Ok(())` on match; `RuntimeError::McpContractDriftAtBoot`
/// on mismatch.
pub(crate) fn verify_lockfile_against_live(
    entry: &LockedMcpEntry,
    client: &McpClient,
) -> Result<(), RuntimeError> {
    let actual_hash = canonical_hash(client.contract()).map_err(|e| RuntimeError::McpSetupFailed {
        entry: entry.entry.clone(),
        reason: format!("canonical_hash failed: {e}"),
    })?;
    let actual_hex = hex_lower(&actual_hash);
    if actual_hex != entry.contract_hash {
        return Err(RuntimeError::McpContractDriftAtBoot {
            entry: entry.entry.clone(),
            expected_hash: entry.contract_hash.clone(),
            actual_hash: actual_hex,
        });
    }
    Ok(())
}

/// Lowercase hex encode (reuse existing helper if one exists; PR-4 had one).
fn hex_lower(bytes: &[u8; 32]) -> String {
    // PR-4 ir_dispatcher had a `hex_lower` helper. If still present,
    // remove the duplicate here. If absent, this is the canonical impl.
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
```

If `hex_lower` already exists in build.rs from PR-4, skip the duplicate and import it.

- [ ] **Step 3: Add unit tests.**

```rust
#[cfg(test)]
mod drift_tests {
    use super::*;

    // Constructs a LockedMcpEntry with a hash that matches a known
    // ServerContract; calls verify_lockfile_against_live and asserts ok.
    //
    // Negative case constructs an entry with a deliberately-wrong hash
    // and asserts McpContractDriftAtBoot error variant.
    //
    // The implementer needs to construct a real McpClient for these
    // tests — use the CassetteTransport + a stub handshake response,
    // or factor out the hash-check into a fn that takes the contract
    // directly (cleaner — test the inner fn).
}
```

Recommended refactor: extract `verify_lockfile_against_live`'s body into `verify_hash_against_lockfile(entry: &LockedMcpEntry, contract: &ServerContract) -> Result<(), RuntimeError>` so tests don't need a real McpClient. Tests just construct a `ServerContract` literal + entry + hash + assert.

- [ ] **Step 4: Run.**

```sh
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli -E 'test(drift_tests)'
```

Expected: 2 tests pass.

- [ ] **Step 5: Commit.**

```sh
git add crates/tau-cli/src/cmd/ir_dispatcher.rs crates/tau-cli/src/error.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-cli/ir_dispatcher): verify_lockfile_against_live drift check + RuntimeError variants"
```

---

## Phase 5 — setup_mcp_runtime() shared helper

### Task 5.1: Compose Phase 1-4 pieces into the runtime helper

**Files:**
- Modify: `crates/tau-cli/src/cmd/ir_dispatcher.rs`

`setup_mcp_runtime()` is the centerpiece. It:
1. Reads `lockfile.mcp_entries` + the project's `[tools.<entry>]` blocks
2. Per entry: opens via `host_lifecycle::open` (handshake)
3. Verifies live contract hash vs lockfile hash (drift check)
4. Constructs `WiredHostHandlers` per entry from tau.toml `sampling.models` + `roots` + the agent's `LlmBackend`
5. Spawns inbound-dispatch on each client
6. Per server-tool in the contract: builds one `McpBackedTool` Arc<dyn DynTool>
7. Returns a struct: `{ tools_by_id_extension: Vec<(ToolId, Arc<dyn DynTool>)>, inbound_handles: Vec<InboundDispatchHandle> }`

- [ ] **Step 1: Write the helper.**

```rust
use tau_mcp_tokio::host_lifecycle::{open as mcp_open, McpClient, McpClientOptions, InboundDispatchHandle};
use tau_mcp_tokio::bridge::McpBackedTool;
use tau_pkg::lockfile::LockFile;
use tau_pkg::project::project::{ProjectConfig, ToolBody};
use tau_ports::CapabilityPlan;
use tau_runtime_tokio::process_gate::passthrough::PassthroughSandbox;

/// Outcome of `setup_mcp_runtime` — the `tools_by_id` extension map
/// + handles whose `Drop` aborts the inbound pump tasks.
pub(crate) struct McpRuntimeSetup {
    /// Entries to merge into `ForwardingDispatcher`'s `tools_by_id`.
    pub tools: Vec<(tau_ir::ids::ToolId, Arc<dyn DynTool>)>,
    /// Inbound-dispatch task handles. Drop to abort.
    #[allow(dead_code)] // held by caller for lifetime; drop = abort
    pub inbound_handles: Vec<InboundDispatchHandle>,
}

/// Boot the MCP runtime: per-entry handshake + drift check + WiredHostHandlers
/// + inbound dispatch + McpBackedTool registration.
///
/// Errors out before ForwardingDispatcher is constructed if any entry
/// fails (drift, network, parse).
pub(crate) async fn setup_mcp_runtime(
    config: &ProjectConfig,
    lockfile: &LockFile,
    backend: Arc<dyn DynLlmBackend>,
) -> Result<McpRuntimeSetup, RuntimeError> {
    let mut tools: Vec<(tau_ir::ids::ToolId, Arc<dyn DynTool>)> = Vec::new();
    let mut inbound_handles: Vec<InboundDispatchHandle> = Vec::new();

    for locked in &lockfile.mcp_entries {
        // Locate the corresponding tau.toml entry (sampling.models + roots).
        let tool_entry = config.tools.get(&locked.entry).ok_or_else(|| {
            RuntimeError::McpSetupFailed {
                entry: locked.entry.clone(),
                reason: format!(
                    "lockfile names entry {:?} but [tools.{:?}] missing in tau.toml",
                    locked.entry, locked.entry
                ),
            }
        })?;
        let url = match &tool_entry.body {
            ToolBody::Mcp(u) => u.clone(),
            other => {
                return Err(RuntimeError::McpSetupFailed {
                    entry: locked.entry.clone(),
                    reason: format!("[tools.{:?}] body is not Mcp: {other:?}", locked.entry),
                });
            }
        };
        let sampling_models = tool_entry
            .sampling
            .as_ref()
            .map(|s| s.models.clone())
            .unwrap_or_default();
        let roots = tool_entry.roots.clone();

        // Open the MCP server (handshake).
        let gate = Arc::new(PassthroughSandbox::new());
        let client = mcp_open(&url, &CapabilityPlan::default(), gate, McpClientOptions::default())
            .await
            .map_err(|e| RuntimeError::McpSetupFailed {
                entry: locked.entry.clone(),
                reason: format!("open failed: {e}"),
            })?;

        // Drift check.
        verify_lockfile_against_live(locked, &client)?;

        // Wrap McpClient in Arc; spawn inbound dispatch.
        let arc_client = Arc::new(client);
        let handlers = WiredHostHandlers::new(backend.clone(), sampling_models, roots);
        let handle = arc_client.start_inbound_dispatch(handlers);
        inbound_handles.push(handle);

        // Per server-tool, register one McpBackedTool.
        for st in &arc_client.contract().tools {
            let ir_tool_id = tau_ir::ids::ToolId(format!("{}.{}", locked.entry, st.name));
            let mcp_tool = McpBackedTool::new(
                ir_tool_id.0.clone(),
                arc_client.clone(),
                st.name.clone(),
                st.caps.clone(),
                st.input_schema.0.clone(),
                st.description.clone().unwrap_or_default(),
            );
            tools.push((ir_tool_id, mcp_tool as Arc<dyn DynTool>));
        }
    }

    Ok(McpRuntimeSetup {
        tools,
        inbound_handles,
    })
}
```

**Implementer notes:**
- `PassthroughSandbox` is the v0 default. PR-5.1 will plumb the real sandbox per the per-entry CapabilityPlan (built from tau.toml `capabilities`).
- `tool_entry.sampling` + `tool_entry.roots` come from PR-4 (Task 3.1 added them to ToolEntry).
- `st.input_schema.0` — `McpToolInputSchema` is a newtype wrapper per PR-4's note; access via `.0`.
- `st.description` is `Option<String>` per PR-4's report.
- `st.caps` is `Vec<Capability>` from tau_domain — passes through to `McpBackedTool::new`'s `capabilities` field.
- `lockfile.mcp_entries` is the v7 vec from PR-4.

- [ ] **Step 2: cargo check + clippy.**

```sh
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-cli
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-cli --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 3: Commit.**

```sh
git add crates/tau-cli/src/cmd/ir_dispatcher.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-cli/ir_dispatcher): setup_mcp_runtime — per-entry handshake + drift + inbound dispatch + McpBackedTool"
```

(Unit tests for setup_mcp_runtime are E2E by nature — covered in Phase 6 via cassette.)

---

## Phase 6 — Dev-mode integration + E2E test

### Task 6.1: Wire setup_mcp_runtime into the dev-mode run path

**Files:**
- Modify: `crates/tau-cli/src/cmd/run.rs` (or wherever the dev-mode entrypoint is — confirm via `git grep "ForwardingDispatcher::new"`)

- [ ] **Step 1: Find the dev-run construction site.** `git grep -n "ForwardingDispatcher::new" crates/tau-cli/src/` and read the surrounding `run_dev` or equivalent function.

- [ ] **Step 2: Add the MCP setup before ForwardingDispatcher::new:**

```rust
// After ProjectConfig + LockFile are loaded; before tools_by_id is built:
let mcp_setup = setup_mcp_runtime(&config, &lockfile, backend.clone()).await?;

// After native tools_by_id is built:
for (id, tool) in mcp_setup.tools {
    tools_by_id.insert(id, tool);
}

// Construct ForwardingDispatcher with the EXTENDED map.
let dispatcher = Arc::new(ForwardingDispatcher::new(backend.clone(), tools_by_id));

// Keep mcp_setup.inbound_handles alive for the rest of the run — drop
// at the end terminates the inbound pumps.
let _mcp_lifetime = mcp_setup.inbound_handles;
```

(Adapt to the actual local variable names + async context.)

- [ ] **Step 3: cargo check + clippy.**

```sh
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-cli
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-cli --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 4: Run the full tau-cli suite.**

```sh
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli
```

Expected: all existing tests still pass (lockfiles without `mcp_entries` produce an empty extension; the dev path is unchanged for non-MCP projects).

- [ ] **Step 5: Commit.**

```sh
git add crates/tau-cli/src/cmd/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-cli/cmd/run): wire setup_mcp_runtime into dev-mode before ForwardingDispatcher::new"
```

### Task 6.2: E2E cassette test

**Files:**
- Create: `crates/tau-cli/tests/cmd_run_mcp.rs`

This test exercises the full dev-mode path with an MCP entry. The cassette transport stands in for a real stdio/http server.

- [ ] **Step 1: Write the test.**

The test should:
1. Construct a tempdir with a tau.toml that has one `[tools.weather]` MCP entry
2. Write a v7 lockfile with one `LockedMcpEntry` referencing the cassette
3. Write a cassette JSONL file that captures: initialize response + tools/list response (one tool `get_forecast`) + tools/call response
4. Wire `setup_mcp_runtime` against a CassetteTransport-backed McpClient
5. Invoke `dispatcher.invoke(ToolId("weather.get_forecast"), args)`
6. Assert the result body is the expected text

**Tricky bit**: PR-3's CassetteTransport reads cassette JSONL but doesn't integrate with `host_lifecycle::open` (which only handles stdio/http URLs). To exercise setup_mcp_runtime end-to-end, the test would need a `cassette:` URL scheme support — which we explicitly deferred to PR-6 in PR-3's scope.

**Pragmatic v0 alternative**: skip the full E2E test. Instead, write a NARROWER unit test that:
1. Constructs a CassetteTransport directly
2. Wraps in `McpClient::new(transport, contract, options)`
3. Wraps in `Arc<McpClient>`
4. Constructs one `McpBackedTool` directly (no setup_mcp_runtime)
5. Calls `mcp_backed_tool.invoke(...)` and asserts result body

This narrower test exercises the McpBackedTool path without needing the URL-scheme wiring. The full setup_mcp_runtime path is left for PR-6's conformance fixture #07 (cassette-replay weather scenario; the spec calls out a `cassette:` URL scheme arm there).

Sketch:

```rust
//! Narrow E2E: McpBackedTool → CassetteTransport → ToolResult.
//! Full setup_mcp_runtime path is covered by PR-6's conformance fixture #07.

#![cfg(feature = "with-std-adapters")]

use std::sync::Arc;
use tau_mcp::cassette::CassetteTransport;
use tau_mcp::contract::server_contract::{ContractTool, ServerContract, ServerInfo};
use tau_mcp_tokio::bridge::McpBackedTool;
use tau_mcp_tokio::host_lifecycle::{McpClient, McpClientOptions};

fn minimal_cassette() -> Vec<u8> {
    // Cassette JSONL: one tools/call request → text response.
    let lines = [
        r#"{"version":1}"#,
        r#"{"dir":"in","kind":"request","id":2,"method":"tools/call","payload":{"name":"echo","arguments":{"message":"hi"}}}"#,
        r#"{"dir":"out","kind":"response","id":2,"payload":{"content":[{"type":"text","text":"hi back"}]}}"#,
    ];
    lines.join("\n").into_bytes()
}

#[tokio::test]
async fn mcp_backed_tool_round_trips_via_cassette() {
    let transport = CassetteTransport::from_jsonl_bytes(&minimal_cassette()).expect("parse cassette");
    // Build a minimal ServerContract (PR-2/PR-4 pattern — adapt).
    let contract = ServerContract {
        protocol_version: "2025-03-26".to_string(),
        server_info: ServerInfo {
            name: "mock".to_string(),
            version: "0.0.0".to_string(),
            additional: Default::default(),
        },
        tools: vec![],  // empty — McpBackedTool carries its own schema
    };
    let client = Arc::new(McpClient::new(transport, contract, McpClientOptions::default()));

    let tool = McpBackedTool::new(
        "weather.echo",
        client,
        "echo",
        Vec::new(),
        serde_json::json!({}),
        "echo tool".to_string(),
    );

    // Invoke via Tool trait (McpBackedTool impls tau_ports::Tool).
    // The implementer should construct a SessionContext + Value + call
    // `tool.invoke(&mut (), args).await` and assert the result body
    // is `Value::String("hi back")` per PR #277 body-shape symmetry.
    //
    // Detailed assertion shape depends on the actual ToolResult struct.
    let _ = tool; // placeholder — implementer fills in.
}
```

The above is a sketch — the implementer should crib the exact invoke call shape from `crates/tau-mcp-tokio/tests/stdio_lifecycle.rs::tools_call_echo_round_trips` (PR-2) and adapt to the McpBackedTool surface.

If wiring the test is non-trivial in time-box, mark `#[ignore]` with TODO and report — Phase 7 can pick it up or defer to PR-6.

- [ ] **Step 2: Run.**

```sh
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli --test cmd_run_mcp
```

Expected: 1 test passes (or 1 ignored if deferred).

- [ ] **Step 3: Commit.**

```sh
git add crates/tau-cli/tests/cmd_run_mcp.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "test(tau-cli): McpBackedTool round-trips via cassette transport"
```

---

## Phase 7 — Bundle-mode integration + workspace checks + push + PR

### Task 7.1: Wire setup_mcp_runtime into the bundle-mode run path

**Files:**
- Modify: `crates/tau-cli/src/cmd/run_bundle.rs` (or wherever `tau run --bundle` lives — confirm via `git grep "run.*bundle\|BundleMode"`)

The bundle path needs the SAME wiring as the dev path. The lockfile + project come from the bundle's embedded files (PR refs: tau build bundles embed `tau.toml` + `Tau.lock` per ADR-0035).

- [ ] **Step 1: Find the bundle-run construction.** Same pattern as Task 6.1 but in the bundle path.

- [ ] **Step 2: Apply the same edit** as Task 6.1, against the bundle path's locals.

- [ ] **Step 3: cargo check + clippy.**

```sh
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-cli
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-cli --all-targets -- -D warnings
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli
```

Expected: clean + all existing tests pass.

- [ ] **Step 4: Commit.**

```sh
git add crates/tau-cli/src/cmd/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-cli/cmd/run_bundle): wire setup_mcp_runtime — bundle path matches dev path"
```

### Task 7.2: Workspace checks + downstream canary

- [ ] **Step 1: Full check / nextest / doc / clippy / fmt for every touched crate.**

```sh
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-cli

timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp-tokio
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp --features with-std-adapters

timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-mcp-tokio
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-cli

timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp-tokio --all-targets -- -D warnings
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-cli --all-targets -- -D warnings

timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt --check -p tau-mcp-tokio
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt --check -p tau-cli
```

Expected: all green.

- [ ] **Step 2: Downstream canary.**

```sh
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-tokio
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-workflow
```

Expected: clean.

- [ ] **Step 3: Apply fmt if anything's off.**

```sh
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-mcp-tokio -p tau-cli
git status
```

If files changed:

```sh
git add -A
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "style(tau-mcp-tokio,tau-cli): apply cargo fmt across PR-5 changes"
```

### Task 7.3: Push + open PR + auto-merge

- [ ] **Step 1: Push.**

```sh
git push --no-verify -u origin feat/beta-3-pr-5-bridge
```

- [ ] **Step 2: Open the PR.**

```sh
gh pr create --title "β.3 MCP facilitator — PR-5: Bridge + WiredHostHandlers + runtime drift check + dispatch wiring" --body "$(cat <<'EOF'
## Summary

Fifth of six PRs in the β.3 MCP facilitator sub-project. Wires the MCP runtime end-to-end:

- **\`tau-mcp-tokio\`** — \`McpClient::start_inbound_dispatch(handlers)\` spawns the inbound pump that routes server-initiated \`sampling/createMessage\` + \`roots/list\` through \`HostHandlers\`. \`bridge::McpBackedTool\` impls \`DynTool\` so MCP-expanded tools register in the standard \`BTreeMap<ToolId, Arc<dyn DynTool>>\` \`ForwardingDispatcher\` already owns (no special-case routing).
- **\`tau-cli\`** — \`WiredHostHandlers\` composes \`Arc<dyn DynLlmBackend>\` + per-server \`sampling.models\` allowlist + \`roots\`. \`verify_lockfile_against_live\` re-hashes live \`tools/list\` vs lockfile \`contract_hash\` and refuses to start on drift. \`setup_mcp_runtime()\` composes all of the above; called from both \`tau run\` (dev) and \`tau run --bundle\` paths before \`ForwardingDispatcher::new\`.
- ~28 new tests across both crates.

Spec: \`docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md\` §2/§8.1/§8.2/§8.3/§9/§12/§15
Plan: \`docs/superpowers/plans/2026-06-09-beta-3-mcp-facilitator-pr-5.md\`
Previous PR: #284 (β.3 PR-4).

## Test plan

- [ ] tau-mcp-tokio nextest green (PR-3 baseline + Phase 1 inbound_dispatch + Phase 2 bridge unit)
- [ ] tau-cli nextest green (existing + Phase 3 wired_handlers + Phase 4 drift + Phase 6 e2e)
- [ ] tau-mcp nextest green (no changes; baseline)
- [ ] All 2 touched crates clippy + fmt clean
- [ ] Downstream canary (tau-runtime-tokio, tau-workflow) clean
- [ ] CI green on linux / macos / windows

## Known follow-ups (DEFERRED per locked decisions)

- **Cancellation propagation → PR-5.1.** Two-way \`notifications/cancelled\` plumbing (parent abort → MCP servers; server cancel → in-flight \`tools/call\`) is a separate design problem.
- **Real LLM-backend wiring in WiredHostHandlers::sampling → β.3.1.** v0 ships an allowlist-refusal-only impl; the actual backend invocation is stubbed.
- **\`cassette:\` URL scheme for setup_mcp_runtime E2E → PR-6 conformance fixture #07.** Full end-to-end test of the dev/bundle paths needs a cassette-driven host_lifecycle::open arm.
- **Real CapabilityPlan in setup_mcp_runtime → PR-5.1.** v0 uses PassthroughSandbox; real plan-from-envelope plumbing comes with the sandbox-routing pass.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 3: Enroll auto-merge.**

```sh
gh pr merge <N> --auto
```

- [ ] **Step 4: Confirm queue enrollment.**

```sh
gh api graphql -f query='query{repository(owner:"tau-rs",name:"tau"){pullRequest(number:<N>){mergeQueueEntry{state position} autoMergeRequest{enabledAt}}}}'
```

---

## Self-review checklist (run before declaring PR-5 done)

| Check | Status |
|---|---|
| `McpClient::start_inbound_dispatch(handlers)` exists | Task 1.1 |
| Inbound pump routes `sampling/createMessage` + `roots/list` via HostHandlers | Task 1.1 |
| Inbound pump returns JSON-RPC error (code -32000) on InboundError | Task 1.1 |
| Pump handle: drop = abort task | Task 1.1 |
| 3 inbound_dispatch unit tests pass | Task 1.2 |
| `McpBackedTool` impls `DynTool` via blanket `tau_ports::Tool` impl | Task 2.1 |
| `McpBackedTool::invoke` calls `client.call_tool(server_tool_name, args)` | Task 2.1 |
| Content[] → joined-text → JSON-or-String body matches ForwardingDispatcher symmetry | Task 2.1 |
| `WiredHostHandlers` refuses on empty allowlist | Task 3.1 |
| `WiredHostHandlers` returns `Vec<Root>` mapping `PathBuf → file:// URI` | Task 3.1 |
| `verify_lockfile_against_live` raises `McpContractDriftAtBoot` on hash mismatch | Task 4.1 |
| `setup_mcp_runtime` walks lockfile.mcp_entries, opens each, drift-checks, spawns inbound, builds `Vec<(ToolId, Arc<dyn DynTool>)>` | Task 5.1 |
| Dev-mode run path calls setup_mcp_runtime before ForwardingDispatcher::new | Task 6.1 |
| Bundle-mode run path mirrors dev path | Task 7.1 |
| All existing tau-cli + tau-mcp-tokio tests still pass | Task 7.2 |
| Push used `--no-verify` | Task 7.3 |
| Auto-merge enrolled via BARE `gh pr merge <N> --auto` | Task 7.3 |

---

## What's next: PR-6 + PR-5.1

**PR-6** (CLI verbs + conformance fixture #07 + ADR-0038 finalize + docs) stacks on PR-4 + PR-5:
- `tau mcp {pin, ls, show, refresh, diff}` reusing PR-4's `PinnedResolver` + PR-5's lockfile types
- `tau check mcp_contracts` phase
- Conformance fixture #07 (cassette-replay weather; needs `cassette:` URL scheme support for setup_mcp_runtime E2E)
- ADR-0038 finalize + 2 mdBook docs pages

**PR-5.1** (deferred from this PR):
- Two-way cancellation propagation (`notifications/cancelled` both directions)
- Real `LlmBackend` wiring in `WiredHostHandlers::sampling`
- Real `CapabilityPlan` derivation from tau.toml `capabilities` for setup_mcp_runtime (replaces PassthroughSandbox stub)

PR-5 lessons to fold forward:
- The `Arc<dyn DynTool>` plug-in pattern is the right shape for any future tool source (skill-backed tools, etc.); avoid sibling-dispatcher complexity.
- Cassette transport gaps (no bidirectional support) mean E2E tests for setup_mcp_runtime have to wait for PR-6's `cassette:` URL scheme. Don't try to bridge the gap in PR-5 — it's PR-6 scope.
- The blanket `impl<T: Tool<Session = ()> + 'static> DynTool for T` is the cheapest path to register new tool kinds; impl Tool, get DynTool free.
