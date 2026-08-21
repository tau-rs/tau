# EPIC 7.1 — no_std lib (Variant B) embedding worked example — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a committed, CI-compiled-and-run example that links tau as a no_std lib (Variant B), implements the ports for real, decodes `TAU_IR`, drives `run_ir`, and asserts a `RunOutcome::Completed`.

**Architecture:** Two new root-workspace-member crates under `examples/embed-native/`: the 5.1 `rust-lib` artifact (`workflow-lib`, generated + committed) and a hand-authored Variant-B host (`host`) that supplies a real `ToolDispatcher`, a real `LlmBackend` (in-process scripted, deterministic, no `test-fixtures`), and real `Clock`/`RandomSource` ports. A drift test keeps the committed lib byte-identical to a fresh `tau build`. Task 1 first closes a real API gap the example exposes: `CompletionResponse` has no public constructor, so the `LlmBackend` port is not externally implementable today.

**Tech Stack:** Rust (edition 2021), tokio, tau-runtime-core, tau-ir, tau-ports, tau-sdk-codegen (via the CLI), serde_json.

**Spec:** `docs/superpowers/specs/2026-08-21-epic-7-1-variant-b-embed-design.md`

## Global Constraints

- **Cargo discipline (CLAUDE.md):** every cargo command MUST be `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo <cmd> -p <crate>`. Timeouts: test 300, build/check 180, clippy 240, fmt 30. Never bare `cargo`, never `--workspace`.
- **Commits:** `git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "..."`. Conventional commits, imperative, scoped. End body with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Lints:** all workspace members inherit `[workspace.lints]` with `-D warnings`. New crates MUST add `[lints]\nworkspace = true` to their `Cargo.toml` and be warning-clean.
- **Determinism:** the example MUST NOT depend on `tau-ports/test-fixtures`. The asserted outcome fields (`total_turns`, final message text) are LLM-driven; never assert on clock/random-derived ids or timestamps.
- **`#[non_exhaustive]`:** `CompletionResponse`, `ToolUse`, `RunOutcome`, `RuntimeError` are `#[non_exhaustive]`. Construct via public constructors only; match with a `_ => {}` arm.
- **Naming:** source project dir `examples/embed-native/workflow/` (tau.toml `name = "embed-native-workflow"`); generated lib crate package `embed-native-workflow-lib` (crate path `embed_native_workflow_lib`); host crate package `embed-native-host` (crate path `embed_native_host`).

---

### Task 1: Public `CompletionResponse::new` constructor in tau-ports

Closes the gap that blocks any out-of-repo `LlmBackend` impl: `CompletionResponse` is `#[non_exhaustive]` and its only constructor (`fixtures::make_completion_response`) is gated behind `test-fixtures`. Mirror the existing public `ToolUse::new` / `TokenUsage::new` / `CompletionRequest::new` constructors.

**Files:**
- Modify: `crates/tau-ports/src/llm.rs` (add `impl CompletionResponse` near line 173, where `ToolUse::new` lives)
- Test: `crates/tau-ports/tests/completion_response_ctor.rs` (Create — integration test = separate crate, proves the E0639 external-construction gap is closed)

**Interfaces:**
- Produces: `CompletionResponse::new(text: String, tool_uses: Vec<ToolUse>, stop_reason: StopReason, usage: Option<TokenUsage>) -> CompletionResponse`

- [ ] **Step 1: Write the failing test**

```rust
//! EPIC 7.1: CompletionResponse must be constructible by external crates
//! (no `test-fixtures`), so `LlmBackend` plugins can build their return value.
use tau_ports::{CompletionResponse, StopReason, ToolUse};

#[test]
fn completion_response_new_builds_a_tool_use_response() {
    let tu = ToolUse::new(
        "call-1".into(),
        "echo".into(),
        serde_json::from_value(serde_json::json!({"text": "hi"})).unwrap(),
    );
    let resp = CompletionResponse::new(String::new(), vec![tu], StopReason::ToolUse, None);
    assert_eq!(resp.text, "");
    assert_eq!(resp.tool_uses.len(), 1);
    assert_eq!(resp.stop_reason, StopReason::ToolUse);
    assert!(resp.usage.is_none());
}

#[test]
fn completion_response_new_builds_a_text_response() {
    let resp = CompletionResponse::new("done".into(), Vec::new(), StopReason::EndTurn, None);
    assert_eq!(resp.text, "done");
    assert!(resp.tool_uses.is_empty());
    assert_eq!(resp.stop_reason, StopReason::EndTurn);
}
```

Ensure `crates/tau-ports/Cargo.toml` `[dev-dependencies]` has `serde_json` (it does — used by other tests; confirm, add if missing).

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo test -p tau-ports --test completion_response_ctor`
Expected: FAIL — `no function or associated item named 'new' found for struct 'CompletionResponse'`.

- [ ] **Step 3: Add the constructor**

In `crates/tau-ports/src/llm.rs`, add after the `impl ToolUse { … }` block (~line 176):

```rust
impl CompletionResponse {
    /// Construct a [`CompletionResponse`]. Provided so `LlmBackend`
    /// plugins — including out-of-repo adapters — can build their return
    /// value without struct-literal syntax (the type is
    /// `#[non_exhaustive]`). Mirrors [`ToolUse::new`] / [`TokenUsage::new`].
    pub fn new(
        text: String,
        tool_uses: Vec<ToolUse>,
        stop_reason: StopReason,
        usage: Option<TokenUsage>,
    ) -> Self {
        Self {
            text,
            tool_uses,
            stop_reason,
            usage,
        }
    }
}
```

- [ ] **Step 4: Run test + clippy to verify pass/clean**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo test -p tau-ports --test completion_response_ctor`
Expected: PASS (both tests).
Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo clippy -p tau-ports --all-targets`
Expected: no warnings.

- [ ] **Step 5: Semver check (additive method — should not require a bump)**

A new inherent method is additive (non-breaking). If the repo's `cargo-semver-checks` CI job flags a required bump, bump `crates/tau-ports/Cargo.toml` `version` minor (`0.6.0` → `0.7.0`) and update any in-workspace `version = "0.6"` requirement on tau-ports. Otherwise leave the version unchanged. Do NOT bump preemptively.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-ports/src/llm.rs crates/tau-ports/tests/completion_response_ctor.rs crates/tau-ports/Cargo.toml
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(ports): public CompletionResponse::new so LlmBackend is externally implementable

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Source project + generated `workflow-lib` + workspace wiring

Author the tau.toml source project, generate the 5.1 rust-lib crate from it, commit it verbatim, patch its Cargo.toml for in-repo path deps, and register both example crates as workspace members.

**Files:**
- Create: `examples/embed-native/workflow/tau.toml`
- Create (generated, committed): `examples/embed-native/workflow-lib/{Cargo.toml, src/lib.rs, tau.wit, README.md}`
- Modify: `Cargo.toml` (root — add two `members` entries)

**Interfaces:**
- Produces: crate `embed_native_workflow_lib` exposing `pub const TAU_IR: &[u8]`, `pub const TAU_IR_HASH: &str`, `pub use tau_runtime_core::run_ir`.

- [ ] **Step 1: Author the source project**

Create `examples/embed-native/workflow/tau.toml`:

```toml
packages = ["anthropic"]

[project]
name = "embed-native-workflow"
version = "0.1.0"

[models.claude]
backend = "anthropic"
model = "claude-sonnet-4-6"

[agents.assistant]
display_name = "Assistant"
package = "embed-native-workflow@^0.1"
model = "claude"
tool_refs = ["echo"]
max_turns = 4

[agents.assistant.prompt]
system = "Echo the user's text via the echo tool, then reply 'done'."

[tools.echo]
native = "Echo"
description = "Echo back the provided text."
capabilities = []
```

(Uses backend package `anthropic` because it resolves offline in CI, exactly like the `crates/tau-cli/tests/fixtures/wasm-build/*` fixtures. The host maps this backend name to the scripted backend at runtime — the declared name is irrelevant to determinism.)

- [ ] **Step 2: Build tau, then generate the lib crate**

Run (from repo root):
```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo build -p tau-cli
target/agent-e71/debug/tau build --target rust-lib --allow-ungoverned \
  -o examples/embed-native/workflow-lib examples/embed-native/workflow
```
Expected: writes `Cargo.toml`, `src/lib.rs`, `tau.wit`, `README.md` into `workflow-lib/`. If the CLI flag for output/project differs, run `target/agent-e71/debug/tau build --help` and adjust (`--target rust-lib`, output flag, positional project path). `--allow-ungoverned` bypasses the governance gate `dispatch_rust_lib` runs before emission; the emitted `src/lib.rs` is governance-independent.

Verify the generated `src/lib.rs` contains `#![no_std]`, `pub const TAU_IR: &[u8] = &[`, `pub use tau_runtime_core::run_ir`. **Do not hand-edit `src/lib.rs`** — the drift test (Task 6) byte-compares it.

- [ ] **Step 3: Patch the generated `Cargo.toml` for in-repo use**

The generator emits registry-style `version = "…"` deps and package name `workflow`. Rewrite `examples/embed-native/workflow-lib/Cargo.toml` to:

```toml
[package]
name = "embed-native-workflow-lib"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
tau-runtime-core = { path = "../../../crates/tau-runtime-core", default-features = false }

[lints]
workspace = true
```

(Only `Cargo.toml` is hand-edited; `src/lib.rs` stays verbatim. The drift test ignores `Cargo.toml`.)

- [ ] **Step 4: Register workspace members**

In root `Cargo.toml`, add to `[workspace].members` (keep the list tidy; place after the last `crates/…` entry or with the existing examples if any):

```toml
    "examples/embed-native/workflow-lib",
    "examples/embed-native/host",
```

(The `host` crate does not exist yet — Task 3 creates it. Add both now so the workspace resolves after Task 3; if `cargo` errors on the missing `host` here, add only `workflow-lib` in this task and `host` in Task 3.)

- [ ] **Step 5: Verify the lib compiles in-workspace**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo build -p embed-native-workflow-lib`
Expected: compiles clean.

- [ ] **Step 6: Commit**

```bash
git add examples/embed-native/workflow examples/embed-native/workflow-lib Cargo.toml
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(examples): embed-native source project + generated rust-lib artifact

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Host crate — scripted LLM backend + real Clock/Random ports

Create the host crate skeleton and its two zero-dep port impls: `ScriptedLlmBackend` (deterministic `LlmBackend`) and `SystemClock`/`HostRandom`.

**Files:**
- Create: `examples/embed-native/host/Cargo.toml`
- Create: `examples/embed-native/host/src/lib.rs`
- Create: `examples/embed-native/host/src/llm.rs`
- Create: `examples/embed-native/host/src/ports.rs`

**Interfaces:**
- Consumes: `tau_ports::{LlmBackend, CompletionRequest, CompletionResponse, CompletionStream, LlmError, StopReason, ToolUse, batch_to_stream, Clock, RandomSource}`; `CompletionResponse::new` (Task 1).
- Produces: `pub enum Turn { ToolCall { id, name, input }, Text(String) }`; `pub struct ScriptedLlmBackend` with `pub fn new(turns: Vec<Turn>) -> Self`; `pub struct SystemClock`; `pub struct HostRandom` with `pub fn new() -> Self`.

- [ ] **Step 1: Create `Cargo.toml`**

```toml
[package]
name = "embed-native-host"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
embed-native-workflow-lib = { path = "../workflow-lib" }
tau-runtime-core = { path = "../../../crates/tau-runtime-core" }
tau-ir = { path = "../../../crates/tau-ir" }
tau-ports = { path = "../../../crates/tau-ports" }
tau-domain = { path = "../../../crates/tau-domain" }
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }

[lints]
workspace = true
```

(Confirm the relative path depth `../../../crates/…` resolves from `examples/embed-native/host/`. Match the `version`/features of tau deps to whatever the workspace uses; prefer bare `{ path = … }`.)

- [ ] **Step 2: Create `src/lib.rs`**

```rust
//! Variant-B embedding host for tau (EPIC 7.1). A product links the
//! generated `embed_native_workflow_lib` (no_std) and implements the
//! runtime ports itself; see `README.md`.
pub mod dispatcher;
pub mod llm;
pub mod ports;

pub use dispatcher::HostDispatcher;
```

(`dispatcher` module lands in Task 4; if the crate must compile at the end of this task, temporarily omit the `pub mod dispatcher;` / `pub use` lines and add them in Task 4.)

- [ ] **Step 3: Write the failing port tests (`src/ports.rs` bottom + `src/llm.rs` bottom)**

In `src/ports.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tau_ports::{Clock, RandomSource};

    #[test]
    fn system_clock_is_positive() {
        assert!(SystemClock.now() > 0);
    }

    #[test]
    fn host_random_fills_bytes() {
        let r = HostRandom::new();
        let mut buf = [0u8; 16];
        r.fill(&mut buf);
        assert!(buf.iter().any(|&b| b != 0), "should write entropy");
    }
}
```

In `src/llm.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tau_ports::{LlmBackend, StopReason};

    #[tokio::test]
    async fn scripted_backend_replays_turns_in_order() {
        let b = ScriptedLlmBackend::new(vec![
            Turn::ToolCall {
                id: "c1".into(),
                name: "echo".into(),
                input: serde_json::from_value(serde_json::json!({"text": "hi"})).unwrap(),
            },
            Turn::Text("done".into()),
        ]);
        let req = tau_ports::CompletionRequest::new("m".into());
        let first = b.complete(req.clone()).await.unwrap();
        assert_eq!(first.stop_reason, StopReason::ToolUse);
        assert_eq!(first.tool_uses.len(), 1);
        let second = b.complete(req).await.unwrap();
        assert_eq!(second.stop_reason, StopReason::EndTurn);
        assert_eq!(second.text, "done");
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo test -p embed-native-host`
Expected: FAIL to compile (`SystemClock`, `HostRandom`, `ScriptedLlmBackend`, `Turn` undefined).

- [ ] **Step 5: Implement `src/ports.rs`**

```rust
//! Real host ports: wall-clock via std, entropy via a time-seeded PRNG.
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tau_ports::{Clock, RandomSource};

/// Wall-clock port backed by `std::time`.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// Non-cryptographic entropy port (time-seeded xorshift64*). A real
/// product wraps `getrandom`/OS entropy here; this example stays
/// dependency-free and its asserted outcome does not depend on entropy
/// quality.
pub struct HostRandom {
    state: AtomicU64,
}

impl HostRandom {
    pub fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
            | 1; // xorshift needs a non-zero seed
        Self {
            state: AtomicU64::new(seed),
        }
    }
}

impl Default for HostRandom {
    fn default() -> Self {
        Self::new()
    }
}

impl RandomSource for HostRandom {
    fn fill(&self, dest: &mut [u8]) {
        for chunk in dest.chunks_mut(8) {
            let mut x = self.state.load(Ordering::Relaxed);
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.state.store(x, Ordering::Relaxed);
            for (d, b) in chunk.iter_mut().zip(x.to_le_bytes()) {
                *d = b;
            }
        }
    }
}
```

- [ ] **Step 6: Implement `src/llm.rs`**

```rust
//! Deterministic in-process LLM backend. A real product swaps this for an
//! Anthropic/OpenAI-backed adapter implementing the same `LlmBackend` port.
use std::collections::VecDeque;
use std::sync::Mutex;

use tau_domain::Value;
use tau_ports::{
    batch_to_stream, CompletionRequest, CompletionResponse, CompletionStream, LlmBackend,
    LlmError, StopReason, ToolUse,
};

/// One scripted LLM turn.
pub enum Turn {
    /// Emit a tool-call response (`StopReason::ToolUse`).
    ToolCall {
        /// Provider-supplied tool-use id.
        id: String,
        /// Tool name (matches a tau tool id).
        name: String,
        /// Tool arguments.
        input: Value,
    },
    /// Emit a final text response (`StopReason::EndTurn`).
    Text(String),
}

/// Replays `turns` FIFO; an exhausted script ends the turn cleanly.
pub struct ScriptedLlmBackend {
    turns: Mutex<VecDeque<Turn>>,
}

impl ScriptedLlmBackend {
    pub fn new(turns: Vec<Turn>) -> Self {
        Self {
            turns: Mutex::new(turns.into_iter().collect()),
        }
    }

    fn next_response(&self) -> CompletionResponse {
        let turn = self.turns.lock().expect("ScriptedLlmBackend mutex poisoned").pop_front();
        match turn {
            Some(Turn::ToolCall { id, name, input }) => CompletionResponse::new(
                String::new(),
                vec![ToolUse::new(id, name, input)],
                StopReason::ToolUse,
                None,
            ),
            Some(Turn::Text(text)) => {
                CompletionResponse::new(text, Vec::new(), StopReason::EndTurn, None)
            }
            None => CompletionResponse::new(String::new(), Vec::new(), StopReason::EndTurn, None),
        }
    }
}

impl LlmBackend for ScriptedLlmBackend {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        Ok(self.next_response())
    }

    async fn stream(&self, _req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        Ok(batch_to_stream(self.next_response()))
    }
}
```

(If `CompletionRequest` does not derive `Clone`, drop `req.clone()` in the Task 3 Step 3 test and build two requests via `CompletionRequest::new("m".into())`.)

- [ ] **Step 7: Run tests to verify pass + clippy**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo test -p embed-native-host --lib`
Expected: PASS.
Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo clippy -p embed-native-host --all-targets`
Expected: clean. (Temporarily comment `pub mod dispatcher;` if Task 4 is not yet done.)

- [ ] **Step 8: Commit**

```bash
git add examples/embed-native/host
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(examples): embed-native host ports (scripted LLM backend + clock/random)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Host crate — real `ToolDispatcher`

Wire the real dispatcher: `invoke` executes the `echo` tool, `llm_backend_for` returns the scripted backend, `clock`/`random` return the real ports.

**Files:**
- Create: `examples/embed-native/host/src/dispatcher.rs`
- Modify: `examples/embed-native/host/src/lib.rs` (ensure `pub mod dispatcher; pub use dispatcher::HostDispatcher;` are present)

**Interfaces:**
- Consumes: `Turn`, `ScriptedLlmBackend` (Task 3); `SystemClock`, `HostRandom` (Task 3); `tau_runtime_core::builder::DynLlmBackend`; `tau_runtime_core::error::RuntimeError`; `tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult}`; `tau_ir::ToolId`.
- Produces: `pub struct HostDispatcher` with `pub fn new() -> Self`.

- [ ] **Step 1: Write the failing test (`src/dispatcher.rs` bottom)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tau_ir::ToolId;

    #[tokio::test]
    async fn invoke_echo_returns_echoed_text() {
        let d = HostDispatcher::new();
        let args = serde_json::json!({"text": "hello"});
        let out = d.invoke(&ToolId("echo".into()), &args).await.unwrap();
        assert_eq!(out.body.unwrap(), serde_json::json!({"echoed": "hello"}));
        assert!(out.error.is_none());
    }

    #[tokio::test]
    async fn invoke_unknown_tool_errors() {
        let d = HostDispatcher::new();
        let args = serde_json::json!({});
        let res = d.invoke(&ToolId("nope".into()), &args).await;
        assert!(res.is_err());
    }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo test -p embed-native-host --lib dispatcher`
Expected: FAIL — `HostDispatcher` undefined.

- [ ] **Step 3: Implement `src/dispatcher.rs`**

```rust
//! The Variant-B product's `ToolDispatcher`: executes tools in-process
//! and supplies real host ports (clock, random, LLM backend).
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;
use tau_ir::ToolId;
use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};

use crate::llm::{ScriptedLlmBackend, Turn};
use crate::ports::{HostRandom, SystemClock};

pub struct HostDispatcher {
    backend: Arc<dyn DynLlmBackend>,
    clock: Arc<SystemClock>,
    random: Arc<HostRandom>,
}

impl HostDispatcher {
    pub fn new() -> Self {
        // The workflow's LLM "reasoning" is scripted: call `echo`, then
        // reply "done". A real product returns its provider adapter here.
        let backend: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlmBackend::new(vec![
            Turn::ToolCall {
                id: "call-1".into(),
                name: "echo".into(),
                input: serde_json::from_value(serde_json::json!({"text": "hello"}))
                    .expect("echo input is a valid value"),
            },
            Turn::Text("done".into()),
        ]));
        Self {
            backend,
            clock: Arc::new(SystemClock),
            random: Arc::new(HostRandom::new()),
        }
    }
}

impl Default for HostDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolDispatcher for HostDispatcher {
    fn invoke<'a>(
        &'a self,
        tool_id: &'a ToolId,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        let tool = tool_id.0.clone();
        let args = args.clone();
        Box::pin(async move {
            match tool.as_str() {
                "echo" => {
                    let text = args.get("text").and_then(Value::as_str).unwrap_or("");
                    Ok(ToolInvocationResult {
                        body: Some(serde_json::json!({ "echoed": text })),
                        error: None,
                    })
                }
                other => Err(RuntimeError::Internal {
                    message: format!("host does not implement tool '{other}'"),
                }),
            }
        })
    }

    fn llm_backend_for(&self, _backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        // Single-backend host: every agent resolves to the scripted backend.
        Ok(self.backend.clone())
    }

    fn clock(&self) -> Option<Arc<dyn tau_ports::Clock>> {
        Some(self.clock.clone())
    }

    fn random(&self) -> Option<Arc<dyn tau_ports::RandomSource>> {
        Some(self.random.clone())
    }
}
```

(If `RuntimeError::Internal`'s field is not `message`, check `crates/tau-runtime-core/src/error.rs` and use the correct variant/field.)

- [ ] **Step 4: Run test + clippy to verify pass/clean**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo test -p embed-native-host --lib`
Expected: PASS (dispatcher + ports + llm).
Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo clippy -p embed-native-host --all-targets`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add examples/embed-native/host
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(examples): embed-native HostDispatcher (real tool exec + port wiring)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Host `main.rs` + end-to-end integration test (the payoff)

Decode `TAU_IR`, drive `run_ir`, print the outcome; assert `RunOutcome::Completed` with 2 turns and a `"done"` final message.

**Files:**
- Create: `examples/embed-native/host/src/main.rs`
- Create: `examples/embed-native/host/tests/runs.rs`

**Interfaces:**
- Consumes: `embed_native_host::HostDispatcher`; `embed_native_workflow_lib::{run_ir, TAU_IR}`; `tau_ir::from_canonical_bytes`; `tau_runtime_core::outcome::RunOutcome`.

- [ ] **Step 1: Write the failing integration test `tests/runs.rs`**

```rust
//! EPIC 7.1: the Variant-B embedding runs to completion in CI.
use std::sync::Arc;

use embed_native_host::HostDispatcher;
use embed_native_workflow_lib::{run_ir, TAU_IR};
use tau_ir::from_canonical_bytes;
use tau_runtime_core::outcome::RunOutcome;

#[tokio::test]
async fn embedding_runs_to_completion() {
    let module = Arc::new(from_canonical_bytes(TAU_IR).expect("TAU_IR decodes"));
    let entry = module
        .workflow
        .agents
        .keys()
        .next()
        .expect("IR module has at least one agent")
        .clone();

    let outcome = run_ir(module, &entry, Arc::new(HostDispatcher::new()), Vec::new())
        .await
        .expect("run_ir returns an outcome");

    match outcome {
        RunOutcome::Completed {
            total_turns,
            final_message,
            ..
        } => {
            assert_eq!(total_turns, 2, "tool-call turn + final text turn");
            assert!(
                format!("{final_message:?}").contains("done"),
                "final assistant message should carry 'done': {final_message:?}"
            );
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo test -p embed-native-host --test runs`
Expected: FAIL to compile (no `main.rs` yet is fine; the test may compile but fail if turn count/message differ — see Step 4 troubleshooting).

- [ ] **Step 3: Implement `src/main.rs`**

```rust
//! Runnable Variant-B embedding: `cargo run -p embed-native-host`.
use std::sync::Arc;

use embed_native_host::HostDispatcher;
use embed_native_workflow_lib::{run_ir, TAU_IR};
use tau_ir::from_canonical_bytes;
use tau_runtime_core::outcome::RunOutcome;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let module = Arc::new(from_canonical_bytes(TAU_IR)?);
    let entry = module
        .workflow
        .agents
        .keys()
        .next()
        .expect("IR module has at least one agent")
        .clone();

    let outcome = run_ir(module, &entry, Arc::new(HostDispatcher::new()), Vec::new()).await?;
    println!("{outcome:#?}");

    if matches!(outcome, RunOutcome::Failed { .. }) {
        std::process::exit(1);
    }
    Ok(())
}
```

- [ ] **Step 4: Run the test + the binary to verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo test -p embed-native-host --test runs`
Expected: PASS.

Troubleshooting if `total_turns != 2`:
- If `total_turns == 1` and outcome is `Completed`: the agent stopped after the first LLM response. Confirm the scripted turn 1 uses `StopReason::ToolUse` and the tool `echo` is declared in the agent's `tool_refs` (it is). If `max_turns` truncated it, raise `max_turns` in `workflow/tau.toml` and regenerate the lib (Task 2 Step 2), then re-commit `workflow-lib` (drift test will otherwise fail).
- If outcome is `Failed`: inspect the printed status. A capability denial means the `echo` tool needs a grant — but it declares `capabilities = []`, so this should not occur. An `AgentNotFound` is impossible (entry is derived from the module).

Run the binary as a manual smoke:
```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo run -p embed-native-host
```
Expected: prints `Completed { … total_turns: 2 … }`, exit 0.

- [ ] **Step 5: Commit**

```bash
git add examples/embed-native/host/src/main.rs examples/embed-native/host/tests/runs.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(examples): embed-native main + e2e test asserting RunOutcome::Completed

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Drift guard for the committed lib

A tau-cli integration test that regenerates the lib from source and byte-compares `src/lib.rs`, so the committed artifact cannot rot.

**Files:**
- Create: `crates/tau-cli/tests/embed_native_lib_drift.rs`

**Interfaces:**
- Consumes: `tau_cli::cmd::build::emit_rust_lib_to` (public seam, governance-free; same one `cmd_build_rust_lib.rs` uses).

- [ ] **Step 1: Write the test**

```rust
//! EPIC 7.1: the committed embed-native workflow-lib stays byte-identical
//! to a fresh `tau build --target rust-lib` of its source project.
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    // crates/tau-cli -> crates -> repo root
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn embed_native_lib_matches_fresh_render() {
    let root = repo_root();
    let project = root.join("examples/embed-native/workflow");
    let committed = std::fs::read_to_string(
        root.join("examples/embed-native/workflow-lib/src/lib.rs"),
    )
    .expect("committed workflow-lib/src/lib.rs");

    let tmp = tempfile::tempdir().unwrap();
    let gen = tmp.path().join("gen");
    tau_cli::cmd::build::emit_rust_lib_to(&project, &gen).expect("emit_rust_lib_to");
    let fresh = std::fs::read_to_string(gen.join("src/lib.rs")).expect("fresh src/lib.rs");

    assert_eq!(
        committed, fresh,
        "examples/embed-native/workflow-lib/src/lib.rs is stale; regenerate:\n  \
         tau build --target rust-lib --allow-ungoverned \
         -o examples/embed-native/workflow-lib examples/embed-native/workflow"
    );
}
```

- [ ] **Step 2: Run to verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo test -p tau-cli --test embed_native_lib_drift`
Expected: PASS. If FAIL with a diff, the committed lib is stale — regenerate it (Task 2 Step 2) and re-commit before proceeding.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-cli/tests/embed_native_lib_drift.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "test(cli): drift guard for embed-native generated rust-lib

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: README + roadmap checkbox

Document the "how a product embeds tau as a no_std lib" story and mark story 7.1.

**Files:**
- Create: `examples/embed-native/README.md`
- Modify: `docs/superpowers/plans/vision-roadmap.md` (mark 7.1)

**Interfaces:** none (docs).

- [ ] **Step 1: Write `examples/embed-native/README.md`**

Cover: (1) what Variant B is (product links the no_std lib, implements the ports); (2) the tree (`workflow/` source → `workflow-lib/` generated 5.1 artifact → `host/` product); (3) how to run (`cargo run -p embed-native-host`) and test (`cargo test -p embed-native-host`); (4) how to regenerate the lib (`tau build --target rust-lib --allow-ungoverned -o examples/embed-native/workflow-lib examples/embed-native/workflow`) and that `src/lib.rs` must not be hand-edited (drift test); (5) the two documented limitations from the spec — the workspace build proves *links+runs* not bare-metal no_std (target-isolated build is 7.3/7.4 scope), and `HostRandom` is non-crypto (real products wrap `getrandom`/OS entropy); (6) a pointer that a real product replaces `ScriptedLlmBackend` with a provider adapter and returns it from `llm_backend_for`.

- [ ] **Step 2: Mark the roadmap**

In `docs/superpowers/plans/vision-roadmap.md`, update the `- **7.1** …` line to reflect completion, matching the style used for other shipped stories in the file (e.g. a trailing status marker / PR ref once the PR number is known — leave a `[shipped #<PR>]`-style note consistent with neighbors, or check how 5.1/5.2 were marked and mirror it).

- [ ] **Step 3: Commit**

```bash
git add examples/embed-native/README.md docs/superpowers/plans/vision-roadmap.md
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "docs(examples): embed-native README + mark roadmap 7.1

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final verification (before PR)

- [ ] `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo test -p tau-ports --test completion_response_ctor` — PASS
- [ ] `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo test -p embed-native-host` — PASS (lib + runs)
- [ ] `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo test -p tau-cli --test embed_native_lib_drift` — PASS
- [ ] `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo clippy -p embed-native-host --all-targets` — clean
- [ ] `timeout 30 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo fmt -p embed-native-host -- --check` (and `-p tau-ports`) — clean
- [ ] `git diff --stat origin/main` shows only: tau-ports (ctor + test), examples/embed-native/**, root Cargo.toml, tau-cli drift test, docs.
- [ ] PR to `main`; enrol auto-merge BARE: `gh pr merge <N> --auto`; poke `gh pr update-branch <N>` on BEHIND.

## Self-review notes

- **Spec coverage:** two-crate example (Tasks 2–5) ✓; real ToolDispatcher (Task 4) ✓; scripted LLM port no test-fixtures (Task 3) ✓; real clock/random (Task 3) ✓; decode TAU_IR + run_ir + assert RunOutcome (Task 5) ✓; drift guard (Task 6) ✓; README + limitations (Task 7) ✓; workspace-member CI coverage (Task 2 Step 4) ✓. **Addition beyond spec:** Task 1 (`CompletionResponse::new`) — surfaced during planning because the port is otherwise not externally implementable; folded in as a prerequisite and flagged to the user.
- **Type consistency:** `Turn`, `ScriptedLlmBackend::new`, `HostDispatcher::new`, `SystemClock`, `HostRandom::new`, `CompletionResponse::new(text, tool_uses, stop_reason, usage)` used consistently across Tasks 1/3/4/5.
- **Known confirm-points (not placeholders — exact fallback given inline):** `RuntimeError::Internal { message }` field name (Task 4 Step 3); `CompletionRequest: Clone` for the Task 3 test (fallback given); tau dep path depth and CLI output flag (Task 2/3).
