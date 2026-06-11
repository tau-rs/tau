# Serve Dispatcher Error Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close three LOW-severity dispatcher gaps in serve mode (D3 brittle/dead stringly-typed agent-not-found contract, O4 silently dropped outbound responses, O5 fabricated request id on parse errors).

**Architecture:** D3 — `Project::resolve` returns a typed `ResolveError::AgentNotFound` and the dispatcher routes the *single* typed path through `-32010`, deleting both the redundant `contains_key` pre-check and the dead `"agent not found: "` string contract. O4 — outbound sends that fail trip a shared `CancellationToken` (logged once) that the dispatch loop selects on to begin shutdown; the writer task logs + propagates flush/write errors instead of `.ok()`-swallowing them. O5 — `RequestId` gains a `Null` variant (JSON-RPC allows null id on parse/invalid-request errors) so those responses carry the spec-correct id rather than a fabricated `0`.

**Tech Stack:** Rust, `thiserror` (typed boundary errors), `tracing` (structured logs), `tokio-util::sync::CancellationToken`, `serde` untagged enums.

---

## File Structure

- `crates/tau-app/src/serve/project.rs` — add `ResolveError` enum, change `resolve` return type, fix doc comments (D3).
- `crates/tau-app/src/serve/dispatch_run.rs` — delete pre-check, route typed `ResolveError` (D3).
- `crates/tau-app/src/serve/protocol.rs` — add `RequestId::Null` + serde round-trip test (O5).
- `crates/tau-app/src/serve/dispatch.rs` — `Null` id on parse/invalid-request (O5); `writer_gone` token + `note_writer_gone` + send-failure logging (O4).
- `crates/tau-app/src/serve/framing.rs` — writer logs + propagates write/flush errors (O4).
- `crates/tau-app/src/serve/lifecycle.rs` — construct + wire `writer_gone` token into the dispatch select (O4).
- `crates/tau-app/src/serve/mod.rs` — re-export `ResolveError` (test reach).
- `crates/tau-app/tests/common/mod.rs` — `writer_gone` field + `kill_writer` helper (O4 test).
- `crates/tau-app/tests/serve_writer_gone.rs` — new O4 integration test.
- `crates/tau-app/tests/serve_run_batch.rs` — O5 invalid-request null-id test (add to existing file).

Cargo prefix for every command (CLAUDE.md Rules 1–4):
`timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-app`

---

## Task 1: O5 — `RequestId::Null` variant

**Files:**
- Modify: `crates/tau-app/src/serve/protocol.rs`

- [ ] **Step 1: Write the failing test** — append to `protocol.rs` `mod tests`:

```rust
#[test]
fn null_id_round_trips_to_json_null() {
    let out = Outbound::Error(ErrorResponse {
        jsonrpc: "2.0".into(),
        id: RequestId::Null,
        error: ErrorObject {
            code: -32700,
            message: "Parse error".into(),
            data: None,
        },
    });
    let s = serde_json::to_string(&out).unwrap();
    assert!(s.contains(r#""id":null"#), "got: {s}");
    // null id is distinct from int id 0
    let zero = serde_json::to_string(&RequestId::Int(0)).unwrap();
    assert_eq!(zero, "0");
    let null = serde_json::to_string(&RequestId::Null).unwrap();
    assert_eq!(null, "null");
    let parsed: RequestId = serde_json::from_str("null").unwrap();
    assert_eq!(parsed, RequestId::Null);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-app null_id_round_trips`
Expected: FAIL to compile — `RequestId::Null` does not exist.

- [ ] **Step 3: Add the variant** — replace the enum + its doc:

```rust
/// JSON-RPC 2.0 request id. Per spec, may be integer, string, or null.
/// Requests normally carry an integer or string id. The `Null` variant
/// is used for the spec-mandated null id on parse / invalid-request
/// error responses, where the id cannot be recovered from the input.
///
/// ```
/// use tau_app::serve::RequestId;
///
/// let int_id = RequestId::Int(42);
/// let str_id = RequestId::Str("uuid-abc".into());
///
/// // Hash + Eq allow use as map keys.
/// let mut map = std::collections::HashMap::new();
/// map.insert(int_id.clone(), "request-42");
/// assert_eq!(map[&RequestId::Int(42)], "request-42");
///
/// // Serde round-trip (untagged: integer → JSON number, string → JSON
/// // string, null → JSON null).
/// let json_int = serde_json::to_string(&int_id).expect("serialize int id");
/// assert_eq!(json_int, "42");
/// let json_str = serde_json::to_string(&str_id).expect("serialize str id");
/// assert_eq!(json_str, "\"uuid-abc\"");
/// let json_null = serde_json::to_string(&RequestId::Null).expect("serialize null id");
/// assert_eq!(json_null, "null");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    /// Integer id (most common).
    Int(i64),
    /// String id (UUIDs, etc.).
    Str(String),
    /// Null id — carried by parse / invalid-request error responses, where
    /// JSON-RPC 2.0 mandates a null id because the originating id is unknown.
    Null,
}
```

- [ ] **Step 4: Run to verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-app null_id_round_trips`
Expected: PASS. Also run `cargo test -p tau-app --doc` for the doctest.

- [ ] **Step 5: Commit** — `feat(serve): add RequestId::Null variant (O5)`

---

## Task 2: O5 — parse / invalid-request errors carry null id

**Files:**
- Modify: `crates/tau-app/src/serve/dispatch.rs:46-58` (parse), `:65-78` (invalid-request)
- Test: `crates/tau-app/tests/serve_run_batch.rs`

- [ ] **Step 1: Write the failing test** — append to `serve_run_batch.rs`:

```rust
/// A request missing the required `id` field fails to parse as a Request
/// and returns -32600 INVALID_REQUEST with the spec-correct null id —
/// not a fabricated id 0 that would collide with a legitimate id-0 request.
#[tokio::test]
async fn invalid_request_carries_null_id() {
    let mut h = Harness::new(fixture_dir()).await;
    // No id field → not a valid Request.
    h.send_raw(r#"{"jsonrpc":"2.0","method":"meta.ping"}"#).await;
    let resp = h.recv().await.expect("no response");
    assert_eq!(resp["error"]["code"], -32600, "expected INVALID_REQUEST, got: {resp}");
    assert!(resp["id"].is_null(), "expected null id, got: {resp}");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `... cargo nextest run -p tau-app invalid_request_carries_null_id`
Expected: FAIL — id is `0`, not null.

- [ ] **Step 3: Implement** — in `dispatch.rs`, parse-error arm (`run`): replace `RequestId::Int(0)` with `RequestId::Null` and update the comment:

```rust
                Inbound::ParseError(msg) => {
                    warn!(error = %msg, "parse error");
                    // Per JSON-RPC 2.0, parse errors carry a null id (the
                    // originating id can't be recovered from malformed input).
                    self.send_err(
                        RequestId::Null,
                        error_codes::PARSE_ERROR,
                        "Parse error".into(),
                        None,
                    )
                    .await;
                }
```

And the invalid-request arm in `handle_one`:

```rust
            Err(e) => {
                // Invalid JSON-RPC object: id unknown, so per spec use null.
                self.send_err(
                    RequestId::Null,
                    error_codes::INVALID_REQUEST,
                    format!("invalid request: {}", e),
                    None,
                )
                .await;
                return;
            }
```

- [ ] **Step 4: Run to verify pass** — `... cargo nextest run -p tau-app invalid_request_carries_null_id` → PASS.

- [ ] **Step 5: Commit** — `fix(serve): null id on parse/invalid-request errors (O5)`

---

## Task 3: D3 — typed `ResolveError::AgentNotFound`

**Files:**
- Modify: `crates/tau-app/src/serve/project.rs`
- Modify: `crates/tau-app/src/serve/mod.rs` (re-export)
- Test: `crates/tau-app/src/serve/project.rs` `#[cfg(test)]`

- [ ] **Step 1: Write the failing test** — add to `project.rs` (new `#[cfg(test)] mod tests`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/handshake-only")
    }

    #[tokio::test]
    async fn resolve_unknown_agent_returns_typed_variant() {
        // handshake-only fixture defines zero agents.
        std::env::set_var(
            "TAU_HOME",
            std::env::temp_dir().join("tau-project-unit-test"),
        );
        std::fs::create_dir_all(std::env::temp_dir().join("tau-project-unit-test")).unwrap();
        let project = Project::load(&fixture_dir()).await.expect("load fixture");
        let err = project.resolve("no-such-agent").unwrap_err();
        assert!(
            matches!(err, ResolveError::AgentNotFound { ref agent_id, .. } if agent_id == "no-such-agent"),
            "expected typed AgentNotFound, got: {err:?}"
        );
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `... cargo nextest run -p tau-app resolve_unknown_agent_returns_typed_variant`
Expected: FAIL to compile — `ResolveError` does not exist.

- [ ] **Step 3: Implement** — in `project.rs`:

Replace the import line:
```rust
use anyhow::{Context, Result};
```

Update the module doc reconciliation note (lines ~10-20) — replace the bullet about string-prefix matching with:
```rust
//! - Unknown-agent detection is done in [`Project::resolve`] by returning
//!   the typed [`ResolveError::AgentNotFound`] variant; the dispatcher
//!   (Task 10) maps that single typed path to JSON-RPC `-32010 Unknown
//!   agent`. There is no string-prefix error contract.
```

Add the error enum above `impl Project`:
```rust
/// Error returned by [`Project::resolve`].
///
/// Typed so the dispatcher routes unknown agents to `-32010 Unknown agent`
/// through one match arm — no string-prefix sniffing.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// `agent_id` is not defined in the project's `tau.toml`.
    #[error("agent {agent_id:?} is not defined in {root}")]
    AgentNotFound {
        /// The unknown agent id, echoed back in the JSON-RPC error data.
        agent_id: String,
        /// Project root, for the human-readable message.
        root: String,
    },
    /// Package resolution failed (not installed, version unsatisfied,
    /// manifest invalid). Flows to `-32006 RUNTIME_ERROR`.
    #[error(transparent)]
    Resolution(#[from] AgentResolutionError),
}
```

Replace `resolve` and its doc:
```rust
    /// Resolve an agent id to `(AgentDefinition, PackageManifest)`.
    ///
    /// Returns [`ResolveError::AgentNotFound`] for an unknown agent id and
    /// [`ResolveError::Resolution`] for package/manifest failures. The
    /// dispatcher maps these to `-32010` and `-32006` respectively.
    pub fn resolve(
        &self,
        agent_id: &str,
    ) -> std::result::Result<(AgentDefinition, PackageManifest), ResolveError> {
        let entry = self
            .config
            .agents
            .get(agent_id)
            .ok_or_else(|| ResolveError::AgentNotFound {
                agent_id: agent_id.to_string(),
                root: self.root.display().to_string(),
            })?;
        Ok(build_agent_definition(entry, &self.root, &self.scope)?)
    }
```

In `mod.rs`, extend the project re-export:
```rust
#[doc(hidden)]
pub use project::{Project, ResolveError};
```

- [ ] **Step 4: Run to verify pass** — `... cargo nextest run -p tau-app resolve_unknown_agent_returns_typed_variant` → PASS.

- [ ] **Step 5: Commit** — `refactor(serve): typed AgentNotFound from Project::resolve (D3)`

---

## Task 4: D3 — dispatcher routes the single typed path

**Files:**
- Modify: `crates/tau-app/src/serve/dispatch_run.rs:62-89`

The existing `run_unknown_agent_returns_32010` and `streaming_unknown_agent_returns_32010` integration tests are the regression guard — they must stay green after deleting the pre-check.

- [ ] **Step 1: Implement** — delete the step-2 `contains_key` pre-check (lines 62-72) entirely. Replace the step-3 resolve match:

```rust
    // 2. Resolve the agent. Unknown agents and package/manifest failures
    // both surface here through one typed path (no string sniffing).
    let (agent_def, manifest) = match disp.project.resolve(&agent_id) {
        Ok(pair) => pair,
        Err(ResolveError::AgentNotFound { agent_id, .. }) => {
            disp.send_err(
                req.id,
                error_codes::UNKNOWN_AGENT,
                format!("agent_id not found: {}", agent_id),
                Some(json!({ "agent_id": agent_id })),
            )
            .await;
            return;
        }
        Err(e) => {
            disp.send_err(
                req.id,
                error_codes::RUNTIME_ERROR,
                format!("agent resolution failed: {}", e),
                Some(json!({ "agent_id": agent_id })),
            )
            .await;
            return;
        }
    };
```

Add the import near the top:
```rust
use super::project::ResolveError;
```

- [ ] **Step 2: Run the regression tests**

Run: `... cargo nextest run -p tau-app -E 'test(unknown_agent)'`
Expected: PASS — `run_unknown_agent_returns_32010`, `streaming_unknown_agent_returns_32010`, and the concurrency `slot_freed` variant all green; `resp.error.data.agent_id` still set.

- [ ] **Step 3: Commit** — `refactor(serve): drop redundant unknown-agent pre-check (D3)`

---

## Task 5: O4 — observe dropped outbound responses

**Files:**
- Modify: `crates/tau-app/src/serve/dispatch.rs` (Dispatcher field + `note_writer_gone` + 3 send sites)
- Modify: `crates/tau-app/src/serve/framing.rs` (writer logs + propagates)
- Modify: `crates/tau-app/src/serve/lifecycle.rs` (token wiring)
- Modify: `crates/tau-app/tests/common/mod.rs` (`writer_gone` + `kill_writer`)
- Test: `crates/tau-app/tests/serve_writer_gone.rs` (new)

- [ ] **Step 1: Write the failing test** — new file `crates/tau-app/tests/serve_writer_gone.rs`:

```rust
//! Layer 2 — O4: a dropped writer is observed, logged, and trips shutdown.
mod common;
use common::Harness;
use std::path::PathBuf;
use std::time::Duration;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/handshake-only")
}

/// When the writer task is gone, the next outbound send trips the
/// `writer_gone` shutdown token (and logs once). Run with `--nocapture`
/// to see the "writer task gone" warning.
#[tokio::test]
async fn writer_gone_is_logged_and_trips_shutdown() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::WARN)
        .try_init();

    let mut h = Harness::new(fixture_dir()).await;
    h.handshake().await; // drains the handshake response

    // Simulate the writer task dying: drop the out channel receiver.
    h.kill_writer();

    // meta.ping → send_ok → send fails → note_writer_gone trips the token.
    h.send_raw(r#"{"jsonrpc":"2.0","id":7,"method":"meta.ping"}"#).await;

    tokio::time::timeout(Duration::from_secs(2), h.writer_gone.cancelled())
        .await
        .expect("writer_gone token must trip after a send to a dead writer");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `... cargo nextest run -p tau-app writer_gone_is_logged_and_trips_shutdown`
Expected: FAIL to compile — `Harness::writer_gone` / `kill_writer` don't exist.

- [ ] **Step 3: Implement** — `dispatch.rs`:

Add the import: `use tokio_util::sync::CancellationToken;`

Add a field to `Dispatcher`:
```rust
    /// Tripped (once, with a log) when an outbound send fails because the
    /// writer task is gone. The dispatch loop selects on it to begin shutdown.
    pub writer_gone: CancellationToken,
```

Add a private helper inside `impl Dispatcher`:
```rust
    /// Record that the writer task is gone: log once and trip the shutdown
    /// token. Idempotent — repeated calls after the first are silent.
    fn note_writer_gone(&self) {
        if !self.writer_gone.is_cancelled() {
            warn!("writer task gone; outbound message dropped, beginning shutdown");
            self.writer_gone.cancel();
        }
    }
```

Change all three send methods from `let _ = self.out_tx.send(...).await;` to observe the error, e.g. `send_ok`:
```rust
    pub async fn send_ok(&self, id: RequestId, result: Value) {
        if self
            .out_tx
            .send(Outbound::Response(Response {
                jsonrpc: "2.0".into(),
                id,
                result,
            }))
            .await
            .is_err()
        {
            self.note_writer_gone();
        }
    }
```
Apply the same `if ... .is_err() { self.note_writer_gone(); }` wrap to `send_err` and `send_notification`.

`framing.rs` — add `use tracing::warn;` and replace the write/flush tail of `writer_task`:
```rust
        if let Err(e) = stdout.write_all(line.as_bytes()).await {
            warn!(error = %e, "stdout write failed; writer task exiting");
            return Err(e).context("stdout write failed");
        }
        if let Err(e) = stdout.flush().await {
            warn!(error = %e, "stdout flush failed; writer task exiting");
            return Err(e).context("stdout flush failed");
        }
```

`lifecycle.rs` — create the token before the Dispatcher, wire it into the struct, and add a select arm. After `let cancel_reg = CancelRegistry::default();`:
```rust
    let writer_gone = tokio_util::sync::CancellationToken::new();
```
Add `writer_gone: writer_gone.clone(),` to the `Dispatcher { ... }` literal. Clone once more for the select (before `run_until`):
```rust
    let writer_gone_signal = writer_gone.clone();
```
Inside the `tokio::select!`, add:
```rust
                _ = writer_gone_signal.cancelled() => {
                    warn!("writer task gone; shutting down dispatcher");
                    Ok(())
                }
```

`tests/common/mod.rs`:
- Add imports: `use tau_app::serve::RequestId;` is not needed; add `use tokio_util::sync::CancellationToken;`.
- Add field to `Harness`:
```rust
    /// Shutdown token tripped when an outbound send fails (O4). Exposed so
    /// tests can await it after simulating a dead writer.
    pub writer_gone: CancellationToken,
```
- In `with_options`, before building the Dispatcher:
```rust
        let writer_gone = CancellationToken::new();
```
  add `writer_gone: writer_gone.clone(),` to the `Dispatcher { ... }` literal, and `writer_gone,` to the returned `Self { ... }`.
- Add the helper method to `impl Harness`:
```rust
    /// Simulate the writer task dying: drop the out-channel receiver so the
    /// dispatcher's next outbound send fails (and trips `writer_gone`).
    pub fn kill_writer(&mut self) {
        let (tx, rx) = mpsc::channel::<Outbound>(1);
        drop(tx);
        let original = std::mem::replace(&mut self.out_rx, rx);
        drop(original);
    }
```

- [ ] **Step 4: Run to verify pass**

Run: `... cargo nextest run -p tau-app writer_gone_is_logged_and_trips_shutdown`
Expected: PASS. Then re-run with `--nocapture` (cargo test, since nextest captures differently) to show the warning:
`timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test -p tau-app --test serve_writer_gone -- --nocapture`
Expected: the line `writer task gone; outbound message dropped, beginning shutdown` appears.

- [ ] **Step 5: Commit** — `fix(serve): observe + log dropped outbound responses (O4)`

---

## Task 6: Full verification + review

- [ ] **Step 1: Full crate test run**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-app`
Expected: all serve tests green.

- [ ] **Step 2: Doctests** — `... cargo test -p tau-app --doc` (covers the new `RequestId::Null` doctest).

- [ ] **Step 3: Clippy + fmt**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo clippy -p tau-app --all-targets` → no warnings.
Run: `timeout 30 env CARGO_TARGET_DIR=target/main cargo fmt -p tau-app -- --check` → clean.

- [ ] **Step 4: requesting-code-review** — scope check (only dispatcher error contract / observability touched).

- [ ] **Step 5: Push + PR** — `gh pr create -R tau-rs/tau --base main`, cite D3, O4, O5. STOP — no merge.
```
