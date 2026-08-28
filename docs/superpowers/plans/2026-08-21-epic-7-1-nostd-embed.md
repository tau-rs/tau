# EPIC 7.1 — Variant B no_std Embedding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the EPIC 5.1/5.2 rust-lib + embed-rust artifacts compile and run (filling the `TODO(7.1)` stubs), ship a curated no_std embedding API + entry-point contract, a working example host, and the Variant B docs how-to. Closes #413.

**Architecture:** Curate the existing `run_ir`/`run_ir_streaming`/`ToolDispatcher` surface into a `tau_runtime_core::embed` prelude; add `IrModule::entry_agent()` (sole-agent contract) in tau-ir and `CompletionResponse::new` in tau-ports; fix the tau-sdk-codegen templates (feature gate, `TauDep` path/version dep, runnable scaffold bodies); prove it with an e2e test that cargo-runs the generated artifact; add `crates/tau-embed-example` and a docs how-to.

**Tech Stack:** Rust workspace (see CLAUDE.md CARGO RULES — every cargo command uses `timeout NNN env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo ...` and `-p <crate>`), thiserror (no_std), futures 0.3, mdBook.

**Spec:** `docs/superpowers/specs/2026-08-21-epic-7-1-nostd-embed-design.md`

## Global Constraints

- Every cargo invocation: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo nextest run -p <crate>` (check/build: timeout 180; clippy 240; fmt 30). Subagents use `target/agent-e71-<task>`.
- No edits to tau-runtime-core interpreter internals (4.5 lane) — only additive public surface (`embed` module).
- `#![forbid(unsafe_code)]` in any new crate; thiserror at boundaries.
- Additive-only changes to tau-ir / tau-ports / tau-runtime-core public APIs (no field adds to pub structs — that is a breaking change per repo memory).
- Conventional commits, imperative, scoped. Do not push until all gates pass.
- Windows Tier-2 runs these tests nightly: normalize generated Cargo.toml paths with `.replace('\\', "/")`.

---

### Task 1: tau-ir `entry_agent()` + `EntryAgentError` (entry-point contract)

**Files:**
- Modify: `crates/tau-ir/src/module.rs` (add method + error enum after `IrModule` impl area)
- Modify: `crates/tau-ir/src/lib.rs` (re-export `EntryAgentError`)
- Modify: `crates/tau-wasm-guest/src/guest.rs:150-160` (use the helper instead of the local len==1 check)

**Interfaces:**
- Produces: `IrModule::entry_agent(&self) -> Result<&AgentId, EntryAgentError>`; `enum EntryAgentError { NoAgents, Ambiguous { available: Vec<AgentId> } }` (thiserror, no_std). Later tasks (templates, example) call `module.entry_agent()`.

- [ ] **Step 1: Write the failing tests** in `crates/tau-ir/src/module.rs` (append to the existing `#[cfg(test)]` module or add one):

```rust
#[cfg(test)]
mod entry_agent_tests {
    use super::*;
    use crate::ids::AgentId;

    fn module_with_agents(ids: &[&str]) -> IrModule {
        // Build the smallest valid IrModule: default Workflow + agents by id.
        // Reuse whatever test constructor module.rs tests already use for
        // IrModule (search this file for an existing `IrModule {` literal or
        // helper and copy its shape; only `workflow.agents` matters here).
        let mut m = existing_test_module_helper();
        m.workflow.agents.clear();
        for id in ids {
            m.workflow.agents.insert(AgentId((*id).into()), existing_test_agent_helper(id));
        }
        m
    }

    #[test]
    fn entry_agent_ok_when_exactly_one() {
        let m = module_with_agents(&["main"]);
        assert_eq!(m.entry_agent().unwrap(), &AgentId("main".into()));
    }

    #[test]
    fn entry_agent_err_when_empty() {
        let m = module_with_agents(&[]);
        assert!(matches!(m.entry_agent(), Err(EntryAgentError::NoAgents)));
    }

    #[test]
    fn entry_agent_err_lists_candidates_when_ambiguous() {
        let m = module_with_agents(&["a", "b"]);
        let err = m.entry_agent().unwrap_err();
        let msg = alloc::format!("{err}");
        assert!(msg.contains("a") && msg.contains("b"), "{msg}");
    }
}
```

(If no existing IrModule test helper exists in tau-ir, construct the literal inline: `IrModule { ir_format: <the crate's current IrFormatVersion constant>, tau_version: "0.0.0".into(), target: "any-wasi-strict".parse().unwrap(), workflow: Workflow::default(), triggers: Vec::new() }` and a minimal `Agent` — copy an `Agent` literal from an existing tau-ir unit test.)

- [ ] **Step 2: Run to verify failure**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo nextest run -p tau-ir entry_agent`
Expected: compile FAIL — `entry_agent` / `EntryAgentError` not found.

- [ ] **Step 3: Implement** in `crates/tau-ir/src/module.rs` (near `IrModule`):

```rust
use thiserror::Error;

/// Failure of the Variant B embedding entry-point contract
/// ([`IrModule::entry_agent`]).
#[derive(Debug, Error, PartialEq)]
pub enum EntryAgentError {
    /// The module contains no agents at all.
    #[error("IR module contains no agents")]
    NoAgents,
    /// More than one agent: the embedder must pick explicitly.
    #[error("IR module contains {} agents ({available:?}); pass an explicit entry AgentId to run_ir", available.len())]
    Ambiguous {
        /// Every agent id in the module, in BTreeMap (lexicographic) order.
        available: Vec<AgentId>,
    },
}

impl IrModule {
    /// Variant B embedding entry-point contract: a module is directly
    /// runnable iff it contains exactly one agent — that agent is the
    /// entry point. Multi-agent modules must select explicitly via
    /// `run_ir`'s `entry` parameter. (The wasm guest enforces the same
    /// rule at load.)
    pub fn entry_agent(&self) -> Result<&AgentId, EntryAgentError> {
        let mut keys = self.workflow.agents.keys();
        match (keys.next(), keys.next()) {
            (Some(only), None) => Ok(only),
            (None, _) => Err(EntryAgentError::NoAgents),
            (Some(_), Some(_)) => Err(EntryAgentError::Ambiguous {
                available: self.workflow.agents.keys().cloned().collect(),
            }),
        }
    }
}
```

Add to `crates/tau-ir/src/lib.rs` re-exports (next to the other `pub use module::...` / add if absent): `pub use module::EntryAgentError;` (check how `IrModule`/`Workflow` are currently re-exported and match that line's style).

- [ ] **Step 4: Run tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo nextest run -p tau-ir entry_agent`
Expected: 3 PASS.

- [ ] **Step 5: Refactor the wasm guest** (`crates/tau-wasm-guest/src/guest.rs` ~line 150-160): replace the local "exactly one agent" check + `agents.keys().next().expect(..)` with:

```rust
let entry = match module.entry_agent() {
    Ok(id) => id.clone(),
    Err(e) => return /* the function's existing error-return shape, message: format!("unsupported module: {e}") */,
};
```

Keep the surrounding error plumbing exactly as-is (guest returns its own error type/JSON — reuse the same construct the removed check used). Do NOT change any other guest behavior.

- [ ] **Step 6: Guest still compiles + tests pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo nextest run -p tau-wasm-guest`
(also: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo check -p tau-wasm-guest`)
Expected: PASS (crate is empty on non-wasm targets; check still validates cfg parsing).

- [ ] **Step 7: Commit**

```bash
git add crates/tau-ir crates/tau-wasm-guest
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(ir): IrModule::entry_agent sole-agent contract (EPIC 7.1, #413)"
```

---

### Task 2: tau-ports `CompletionResponse::new`

**Files:**
- Modify: `crates/tau-ports/src/llm.rs` (inherent impl near the struct, ~line 274)
- Modify: `crates/tau-ports/src/fixtures.rs` (`make_completion_response` delegates)

**Interfaces:**
- Produces: `CompletionResponse::new(text: String, tool_uses: Vec<ToolUse>, stop_reason: StopReason, usage: Option<TokenUsage>) -> CompletionResponse` — the only way external crates can construct the `#[non_exhaustive]` struct. Used by the embed-rust template (Task 5) and the example (Task 8).

- [ ] **Step 1: Failing test** in `crates/tau-ports/src/llm.rs` tests module:

```rust
#[test]
fn completion_response_new_constructs_all_fields() {
    let r = CompletionResponse::new("hi".into(), Vec::new(), StopReason::EndTurn, None);
    assert_eq!(r.text, "hi");
    assert!(r.tool_uses.is_empty());
    assert!(matches!(r.stop_reason, StopReason::EndTurn));
    assert!(r.usage.is_none());
}
```

- [ ] **Step 2: Verify fail**: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo nextest run -p tau-ports completion_response_new` → compile FAIL.

- [ ] **Step 3: Implement** (immediately after the `CompletionResponse` struct):

```rust
impl CompletionResponse {
    /// Construct a response. The struct is `#[non_exhaustive]`, so this
    /// is the supported way for plugins and embedders (EPIC 7.1) to
    /// build one outside this crate.
    pub fn new(
        text: String,
        tool_uses: Vec<ToolUse>,
        stop_reason: StopReason,
        usage: Option<TokenUsage>,
    ) -> Self {
        Self { text, tool_uses, stop_reason, usage }
    }
}
```

In `fixtures.rs`, change `make_completion_response`'s body to `CompletionResponse::new(text, tool_uses, stop_reason, usage)`.

- [ ] **Step 4: Verify pass**: same nextest filter → PASS; then full `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo nextest run -p tau-ports` → PASS.

- [ ] **Step 5: Commit**: `git add crates/tau-ports && git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(ports): public CompletionResponse::new constructor (EPIC 7.1, #413)"`

---

### Task 3: `tau_runtime_core::embed` prelude

**Files:**
- Create: `crates/tau-runtime-core/src/embed.rs`
- Modify: `crates/tau-runtime-core/src/lib.rs` (declare `pub mod embed` under `#[cfg(feature = "wasm-interpreter")]`, next to line 27-30)

**Interfaces:**
- Consumes: Task 1 (`EntryAgentError`), Task 2 (`CompletionResponse::new`).
- Produces: `tau_runtime_core::embed::{run_ir, run_ir_streaming, ToolDispatcher, ToolInvocationResult, RunEvent, RunOutcome, RuntimeError, DynLlmBackend, from_canonical_bytes, AgentId, IrModule, EntryAgentError, Clock, RandomSource, LlmBackend, LlmError, CompletionRequest, CompletionResponse, CompletionStream, StopReason, batch_to_stream}` — the one-import embedding surface used by the template (Task 5) and example (Task 8).

- [ ] **Step 1: Write `embed.rs`** (re-exports only — the module doc IS the embedding API doc):

```rust
//! Curated Variant B embedding surface (EPIC 7.1).
//!
//! A product embeds tau by (1) decoding baked IR bytes with
//! [`from_canonical_bytes`], (2) resolving the entry agent via
//! [`IrModule::entry_agent`] (sole-agent contract) or an explicit
//! [`AgentId`], (3) implementing [`ToolDispatcher`] — tool execution,
//! [`DynLlmBackend`] resolution, and the mandatory [`Clock`] +
//! [`RandomSource`] ports — and (4) driving [`run_ir`] (single outcome)
//! or [`run_ir_streaming`] ([`RunEvent`] stream; terminal
//! `RunEvent::RunCompleted` fires exactly once).
//!
//! Everything here is a re-export: this module pins *which* items form
//! the supported embedding API. See
//! `docs/how-to/embed-rust-native.md` for the worked example.

pub use crate::builder::DynLlmBackend;
pub use crate::error::RuntimeError;
pub use crate::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};
pub use crate::interpreter::{run_ir, run_ir_streaming};
pub use crate::outcome::RunOutcome;
pub use crate::stream::RunEvent;
pub use tau_ir::{from_canonical_bytes, AgentId, EntryAgentError, IrModule};
pub use tau_ports::{
    batch_to_stream, Clock, CompletionRequest, CompletionResponse, CompletionStream,
    LlmBackend, LlmError, RandomSource, StopReason,
};
```

Verify each source path first (e.g. `run_ir_streaming` lives at `interpreter::run_ir_streaming`; `batch_to_stream`/`StopReason` are tau-ports root re-exports — adjust paths to what `grep -n "pub use\|pub fn" ` shows if any differ; if `AgentId`/`from_canonical_bytes` are not tau-ir root exports, use their module paths `tau_ir::ids::AgentId` etc.).

In `lib.rs` add below the existing interpreter cfg block:

```rust
#[cfg(feature = "wasm-interpreter")]
pub mod embed;
```

- [ ] **Step 2: Doc-comment test** — add a unit test asserting the surface links (in `embed.rs`):

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn embed_prelude_reexports_link() {
        // Type-level smoke test: naming the items proves the re-exports resolve.
        fn _assert_surface(_: fn() -> (
            &'static dyn super::ToolDispatcher,
            super::RunOutcome,
        )) {}
        let _ = super::CompletionResponse::new(String::new(), Vec::new(), super::StopReason::EndTurn, None);
    }
}
```

(If `ToolDispatcher` is not dyn-compatible, drop it from the fn tuple and keep the `CompletionResponse::new` line — compilation of the module is the real assertion. Tests in this crate have std via `extern crate std`; use `alloc::string::String`/`alloc::vec::Vec` if the crate's other tests do.)

- [ ] **Step 3: Test with the feature on** (unit tests imply `--features` selection):

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo nextest run -p tau-runtime-core --features wasm-interpreter embed`
Expected: PASS.

- [ ] **Step 4: no_std check** (the gate the handoff requires):

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo check -p tau-runtime-core --no-default-features --features wasm-interpreter`
Expected: clean.

- [ ] **Step 5: Commit**: `git add crates/tau-runtime-core && git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(runtime-core): curated embed prelude for Variant B hosts (EPIC 7.1, #413)"`

---

### Task 4: tau-sdk-codegen `TauDep` + rust-lib template fix

**Files:**
- Modify: `crates/tau-sdk-codegen/src/emit_rust_lib.rs`
- Modify: `crates/tau-sdk-codegen/src/lib.rs:24` (`pub use emit_rust_lib::{render_rust_lib, RustLibInput, TauDep};`)

**Interfaces:**
- Produces: `pub enum TauDep<'a> { Version(&'a str), Path(&'a str) }` with `pub(crate) fn dep_table(&self, crate_name: &str, extra: &str) -> String`; `RustLibInput` field `tau_version: &'a str` REPLACED by `tau_dep: TauDep<'a>`. Tasks 5-7 consume both. `TauDep::Path` holds the tau workspace ROOT (renderer appends `/crates/<name>`), forward slashes.

- [ ] **Step 1: Update/extend the failing tests** in `emit_rust_lib.rs` tests module — replace the existing `render_rust_lib_emits_expected_files_and_bakes_ir` input with `tau_dep: TauDep::Version("0.0.0")` and change/extend assertions:

```rust
// version-dep render keeps the crates-io shape AND turns the interpreter on:
assert!(
    cargo.contains(r#"tau-runtime-core = { version = "0.0.0", default-features = false, features = ["wasm-interpreter"] }"#),
    "{cargo}"
);
assert!(lib.contains("pub use tau_runtime_core::run_ir"));
assert!(lib.contains("run_ir_streaming"), "streaming entrypoint must be re-exported: {lib}");

#[test]
fn render_rust_lib_path_dep_points_into_workspace() {
    let ir = [1u8];
    let out = render_rust_lib(RustLibInput {
        crate_name: "trivial",
        ir_bytes: &ir,
        ir_hash: "abc",
        wit: "w",
        tau_dep: TauDep::Path("/tau/checkout"),
    });
    let cargo = &out[&PathBuf::from("Cargo.toml")];
    assert!(
        cargo.contains(r#"tau-runtime-core = { path = "/tau/checkout/crates/tau-runtime-core", default-features = false, features = ["wasm-interpreter"] }"#),
        "{cargo}"
    );
}
```

- [ ] **Step 2: Verify fail**: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo nextest run -p tau-sdk-codegen render_rust_lib` → compile FAIL (no `TauDep`).

- [ ] **Step 3: Implement** in `emit_rust_lib.rs`:

```rust
/// How generated crates reference the tau runtime crates. `Version` is the
/// (future) crates.io shape; `Path` points at a tau workspace checkout ROOT
/// (the renderer appends `/crates/<name>`) — the only resolvable shape until
/// tau publishes (workspace version is 0.0.0, not on crates.io).
#[derive(Debug, Clone, Copy)]
pub enum TauDep<'a> {
    /// `{ version = "X" }` dependency.
    Version(&'a str),
    /// `{ path = "<root>/crates/<name>" }` dependency; forward slashes.
    Path(&'a str),
}

impl TauDep<'_> {
    /// Render the inline dependency table for `crate_name`, appending
    /// `extra` attributes (e.g. `default-features = false`).
    pub(crate) fn dep_table(&self, crate_name: &str, extra: &str) -> String {
        let src = match self {
            TauDep::Version(v) => format!(r#"version = "{v}""#),
            TauDep::Path(root) => format!(r#"path = "{root}/crates/{crate_name}""#),
        };
        if extra.is_empty() {
            format!("{{ {src} }}")
        } else {
            format!("{{ {src}, {extra} }}")
        }
    }
}
```

In `RustLibInput`: replace `pub tau_version: &'a str` with `pub tau_dep: TauDep<'a>` (keep doc comment: "How the generated crate depends on tau-runtime-core."). In the Cargo.toml template replace the dependency line with a pre-rendered string arg:

```rust
tau-runtime-core = {dep}
```

rendered via `dep = input.tau_dep.dep_table("tau-runtime-core", r#"default-features = false, features = ["wasm-interpreter"]"#)`.

In the lib.rs template change the re-export line to:

```rust
pub use tau_runtime_core::run_ir;
#[doc = "Streaming variant — yields RunEvent items; see tau_runtime_core::embed."]
pub use tau_runtime_core::interpreter::run_ir_streaming;
```

(plain two `pub use` lines; drop the `#[doc]` attribute if it complicates the template string — a `///` comment line is fine too).

- [ ] **Step 4: Verify pass**: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo nextest run -p tau-sdk-codegen` → the two render_rust_lib tests PASS; embed_rust tests still compile (Task 5 changes them — if this task alone breaks their compilation because `EmbedRustInput` still has `tau_version`, that field is untouched here; only `RustLibInput` changes).

- [ ] **Step 5: Commit**: `git add crates/tau-sdk-codegen && git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(sdk-codegen): TauDep path/version dep + compilable rust-lib template (EPIC 7.1, #413)"`

---

### Task 5: embed-rust template — runnable scaffold (fills TODO(7.1))

**Files:**
- Modify: `crates/tau-sdk-codegen/src/embed_rust.rs` (whole template + tests)

**Interfaces:**
- Consumes: `TauDep` (Task 4), `tau_runtime_core::embed` prelude names (Task 3), `IrModule::entry_agent` (Task 1), `CompletionResponse::new` (Task 2).
- Produces: `EmbedRustInput` field `tau_version: &'a str` REPLACED by `tau_dep: TauDep<'a>`; generated `embed-rust/` crate that compiles AND runs offline (echo backend). Task 7 cargo-runs it; Task 6 threads `TauDep` from the CLI.

- [ ] **Step 1: Rewrite the template.** `Cargo.toml` template becomes:

```toml
[package]
name = "{name}"
version = "0.1.0"
edition = "2021"
publish = false

# Generated by `tau embed --host rust` (EPIC 5.2/7.1). Native host scaffold:
# links the sibling rust-lib crate, supplies scaffold port impls, drives
# run_ir_streaming. Replace the Echo/XorShift ports with your product's.
[dependencies]
{lib} = {{ path = ".." }}
tau-runtime-core = {core_dep}
tau-ir = {ir_dep}
serde_json = {{ version = "1" }}
futures = {{ version = "0.3" }}
```

with `core_dep = input.tau_dep.dep_table("tau-runtime-core", r#"features = ["wasm-interpreter"]"#)` (NOTE: default features stay ON here — the host is std) and `ir_dep = input.tau_dep.dep_table("tau-ir", "")`. tokio is GONE.

`src/main.rs` template becomes (this is the rendered output; in the template only `{lib}` interpolates — double every literal `{`/`}` in the `format!` string):

```rust
//! Generated by `tau embed --host rust` (EPIC 5.2/7.1) — a runnable scaffold.
//!
//! Links the rust-lib crate (`{lib}`), decodes its baked `TAU_IR`, resolves
//! the entry agent (sole-agent contract — `IrModule::entry_agent`), and
//! drives `run_ir_streaming`, printing each `RunEvent` as a JSON line.
//!
//! The port impls below are scaffold-grade so `cargo run` works offline:
//! - `EchoBackend` — canned completion; REPLACE with your product's LLM client.
//! - `ScaffoldDispatcher::invoke` — rejects every tool; REPLACE with your tools.
//! - `SystemClock` / `XorShiftRandom` — std wall clock + NON-cryptographic
//!   entropy; REPLACE if your product needs real entropy.
//!
//! Capabilities the dispatcher must service are the WIT imports in `tau.wit`.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use serde_json::Value;
use tau_ir::ToolId;
use tau_runtime_core::embed::{
    batch_to_stream, run_ir_streaming, Clock, CompletionRequest, CompletionResponse,
    CompletionStream, DynLlmBackend, LlmBackend, LlmError, RandomSource, RuntimeError,
    StopReason, ToolDispatcher, ToolInvocationResult,
};
use {lib}::TAU_IR;

/// Scaffold inference: every backend name resolves to this canned echo.
struct EchoBackend;

impl LlmBackend for EchoBackend {
    fn name(&self) -> &str {
        "embed-scaffold-echo"
    }

    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        Ok(CompletionResponse::new(
            "embed-rust scaffold reply".to_string(),
            Vec::new(),
            StopReason::EndTurn,
            None,
        ))
    }

    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        Ok(batch_to_stream(self.complete(req).await?))
    }
}

/// Wall clock in ms since the Unix epoch.
struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// xorshift64* seeded from the clock — NOT cryptographic.
struct XorShiftRandom(AtomicU64);

impl RandomSource for XorShiftRandom {
    fn fill(&self, dest: &mut [u8]) {
        let mut i = 0;
        while i < dest.len() {
            let mut x = self.0.load(Ordering::Relaxed);
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0.store(x, Ordering::Relaxed);
            let bytes = x.wrapping_mul(0x2545F4914F6CDD1D).to_le_bytes();
            let take = core::cmp::min(8, dest.len() - i);
            dest[i..i + take].copy_from_slice(&bytes[..take]);
            i += take;
        }
    }
}

/// Product port surface: tools + LLM backend + mandatory clock/random.
struct ScaffoldDispatcher {
    backend: Arc<EchoBackend>,
    clock: Arc<SystemClock>,
    random: Arc<XorShiftRandom>,
}

impl ScaffoldDispatcher {
    fn new() -> Self {
        let seed = SystemClock.now() as u64 | 1;
        Self {
            backend: Arc::new(EchoBackend),
            clock: Arc::new(SystemClock),
            random: Arc::new(XorShiftRandom(AtomicU64::new(seed))),
        }
    }
}

impl ToolDispatcher for ScaffoldDispatcher {
    fn invoke<'a>(
        &'a self,
        tool_id: &'a ToolId,
        _args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            Err(RuntimeError::ToolNotRegistered {
                tool_name: tool_id.0.clone(),
                registered: Vec::new(),
            })
        })
    }

    fn llm_backend_for(&self, _backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        Ok(self.backend.clone())
    }

    fn clock(&self) -> Option<Arc<dyn Clock>> {
        Some(self.clock.clone())
    }

    fn random(&self) -> Option<Arc<dyn RandomSource>> {
        Some(self.random.clone())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    futures::executor::block_on(async {
        let module = Arc::new(tau_runtime_core::embed::from_canonical_bytes(TAU_IR)?);
        let entry = module.entry_agent()?.clone();
        let stream =
            run_ir_streaming(module, &entry, Arc::new(ScaffoldDispatcher::new()), Vec::new())
                .await?;
        futures::pin_mut!(stream);
        while let Some(event) = stream.next().await {
            println!("{{}}", serde_json::to_string(&event).expect("RunEvent serializes"));
        }
        Ok(())
    })
}
```

(README template: update wording — no more `todo!()`; say "runnable scaffold; replace Echo/XorShift ports"; keep IR-hash provenance line. `tau.wit` unchanged.)

- [ ] **Step 2: Update the string tests** in the same file: input now `tau_dep: TauDep::Version("0.0.0")`; assertions become:

```rust
assert!(main.contains("use trivial::TAU_IR"), "{main}");
assert!(main.contains("impl ToolDispatcher for ScaffoldDispatcher"), "{main}");
assert!(!main.contains("todo!("), "scaffold must be runnable, no todo! stubs: {main}");
assert!(main.contains("entry_agent()"), "must use the sole-agent contract: {main}");
assert!(main.contains("run_ir_streaming("), "{main}");
assert!(cargo.contains(r#"trivial = { path = ".." }"#), "{cargo}");
assert!(cargo.contains(r#"tau-runtime-core = { version = "0.0.0", features = ["wasm-interpreter"] }"#), "{cargo}");
assert!(cargo.contains("futures"), "{cargo}");
assert!(!cargo.contains("tokio"), "scaffold no longer needs tokio: {cargo}");
```

- [ ] **Step 3: Run**: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo nextest run -p tau-sdk-codegen` → PASS. (tau-cli won't compile until Task 6 threads the new input fields — that's expected; do NOT run -p tau-cli yet.)

- [ ] **Step 4: Commit**: `git add crates/tau-sdk-codegen && git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(sdk-codegen): embed-rust scaffold compiles and runs — fill TODO(7.1) stubs (#413)"`

---

### Task 6: tau-cli — thread `TauDep` through seams + `--tau-dep-path` flag

**Files:**
- Modify: `crates/tau-cli/src/cmd/build.rs` (`emit_rust_lib_to` signature ~line 671, `dispatch_rust_lib` ~line 738)
- Modify: `crates/tau-cli/src/cmd/embed.rs` (`emit_host_to` ~line 38, `run` ~line 105)
- Modify: `crates/tau-cli/src/cli.rs` (`BuildArgs` ~line 259, `EmbedArgs` ~line 746)
- Modify: `crates/tau-cli/tests/embed_hosts.rs`, `crates/tau-cli/tests/cmd_build_rust_lib.rs` (new param)

**Interfaces:**
- Consumes: `tau_sdk_codegen::TauDep`.
- Produces: `emit_rust_lib_to(project: &Path, out_dir: &Path, tau_dep: TauDep) -> Result<RustLibArtifact>`; `emit_host_to(host: &str, project: &Path, out_root: &Path, tau_dep: TauDep) -> Result<EmbedArtifact>`; CLI flag `--tau-dep-path <DIR>` on `tau build` and `tau embed`. Task 7 calls the seams with `TauDep::Path`.

- [ ] **Step 1: Add the flag** to both arg structs in `cli.rs`:

```rust
/// Reference tau crates by filesystem path (a tau workspace checkout
/// root) instead of a crates.io version in the generated Cargo.toml.
/// Required to build the artifact until tau is published to crates.io.
#[arg(long = "tau-dep-path", value_name = "DIR")]
pub tau_dep_path: Option<std::path::PathBuf>,
```

- [ ] **Step 2: Thread it.** In both seams add the `tau_dep: tau_sdk_codegen::TauDep` parameter and pass it as the template input's `tau_dep` (replacing `tau_version: env!("CARGO_PKG_VERSION")`). In `dispatch_rust_lib` and `embed::run` compute:

```rust
let dep_path; // owned String kept alive for the borrow
let tau_dep = match &args.tau_dep_path {
    Some(p) => {
        dep_path = p.display().to_string().replace('\\', "/");
        tau_sdk_codegen::TauDep::Path(&dep_path)
    }
    None => tau_sdk_codegen::TauDep::Version(env!("CARGO_PKG_VERSION")),
};
```

(`emit_host_to`'s `"js"` arm ignores the param — fine.) `render_embed_c` still takes no dep — untouched.

- [ ] **Step 3: Fix the two integration tests** — add `tau_sdk_codegen::TauDep::Version(env!("CARGO_PKG_VERSION"))` as the new argument; keep every existing assertion EXCEPT the ones Task 5 invalidated in generated content — update `embed_hosts.rs`'s `main.contains("impl ToolDispatcher")` to `main.contains("impl ToolDispatcher for ScaffoldDispatcher")` if it fails, keep `run_ir(`→`run_ir_streaming(`.

- [ ] **Step 4: Run**: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo nextest run -p tau-cli embed_host` and `... -p tau-cli rust_lib` → PASS.

- [ ] **Step 5: Commit**: `git add crates/tau-cli && git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(cli): --tau-dep-path for rust-lib/embed artifacts (EPIC 7.1, #413)"`

---

### Task 7: e2e — generated artifact compiles AND runs (the DoD gate)

**Files:**
- Create: `crates/tau-cli/tests/embed_rust_e2e.rs`

**Interfaces:**
- Consumes: both seams with `TauDep::Path` (Task 6), trivial fixture, generated scaffold (Task 5).

- [ ] **Step 1: Write the test:**

```rust
//! EPIC 7.1 DoD: the generated rust-lib + embed-rust crates COMPILE and RUN
//! against this workspace (no more string-only checking). Shells out to
//! cargo; slow (cold-builds futures/serde_json/tau-runtime-core once per
//! target dir) but CI-runnable.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm-build")
        .join(name)
}

/// Workspace checkout root (two levels up from crates/tau-cli).
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/tau-cli has a workspace root")
        .to_path_buf()
}

#[test]
fn generated_embed_rust_compiles_and_runs() {
    let out = tempfile::tempdir().unwrap();
    let root = workspace_root().display().to_string().replace('\\', "/");
    let dep = tau_sdk_codegen::TauDep::Path(&root);

    // rust-lib at tempdir root; embed-rust/ beside it (path dep "..").
    tau_cli::cmd::build::emit_rust_lib_to(&fixture("trivial"), out.path(), dep).unwrap();
    tau_cli::cmd::embed::emit_host_to("rust", &fixture("trivial"), out.path(), dep).unwrap();

    let target = out.path().join("e2e-target");
    let run = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .current_dir(out.path().join("embed-rust"))
        .env("CARGO_TARGET_DIR", &target)
        .env("CARGO_INCREMENTAL", "0")
        .output()
        .expect("cargo is on PATH");

    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        run.status.success(),
        "generated embed-rust failed to build/run\n--- stdout\n{stdout}\n--- stderr\n{stderr}"
    );
    assert!(
        stdout.contains("RunCompleted"),
        "expected a terminal RunCompleted event on stdout:\n{stdout}\n--- stderr\n{stderr}"
    );
    assert!(
        stdout.contains("embed-rust scaffold reply"),
        "echo backend text should appear in emitted events:\n{stdout}"
    );
}
```

NOTE for the implementer: `TauDep` derives `Copy`, so passing `dep` twice is fine. If `tau_cli::cmd::build`/`cmd::embed` are not `pub` module paths from the test (integration tests use the lib target), check how `embed_hosts.rs` imports them and copy that.

- [ ] **Step 2: Run it (first run is the honest red/green: red before Tasks 4-6 landed, green now):**

Run: `timeout 600 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo nextest run -p tau-cli --test embed_rust_e2e` (note the 600s timeout — the child cargo cold-builds; nextest's own slow-timeout config may flag it as SLOW, that's fine)
Expected: PASS. If the child cargo fails on `async-stream` resolution (root `[patch.crates-io]` doesn't reach the tempdir build): fix by appending the patch to the GENERATED rust-lib `Cargo.toml` when `TauDep::Path` is used — in `emit_rust_lib.rs`'s Cargo.toml template add, only for the `Path` variant, a trailing:

```toml
[patch.crates-io]
async-stream = { path = "{root}/vendor/async-stream" }
```

(and assert it in the path-dep unit test). Same for embed-rust's Cargo.toml if its resolution fails; document in the how-to (Task 9).

- [ ] **Step 3: Commit**: `git add crates/tau-cli/tests/embed_rust_e2e.rs && git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "test(cli): e2e — generated embed-rust artifact compiles and runs (#413)"`

---

### Task 8: example host product `tau-embed-example`

**Files:**
- Create: `crates/tau-embed-example/Cargo.toml`, `src/main.rs`, `project/tau.toml`, `fixtures/trivial.ir.json`, `README.md`
- Modify: root `Cargo.toml` members list (after `"crates/tau-sdk-codegen"`)
- Create: `crates/tau-cli/tests/embed_example_drift.rs` (fixture drift guard)

**Interfaces:**
- Consumes: `tau_runtime_core::embed` prelude (Task 3), `entry_agent` (Task 1), `CompletionResponse::new` (Task 2).
- Produces: workspace member `tau-embed-example` whose `#[test] runs_baked_workflow_to_completion` is the CI proof that a product-shaped host runs a governed workflow.

- [ ] **Step 1: Project fixture** `crates/tau-embed-example/project/tau.toml` (governed: has `[allow]`):

```toml
packages = ["anthropic"]

# Root constitution (ADR-0057): this workflow asks for no capabilities,
# and the empty [allow] says none are granted — governed-by-default.
[allow]

[project]
name = "embed-example"
version = "0.1.0"

[models.claude]
backend = "anthropic"
model = "claude-sonnet-4-6"

[agents.main]
display_name = "Main"
package = "embed-example@^0.1"
model = "claude"

[agents.main.prompt]
system = "You are the tau-embed-example agent. Reply once and stop."
```

- [ ] **Step 2: Generate the committed IR fixture** (one-off, from the workspace root):

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo run -p tau-cli --quiet -- build --target rust-lib crates/tau-embed-example/project -o /tmp/e71-fixture
cp /tmp/e71-fixture/src/lib.rs /tmp/e71-lib.rs   # sanity look only
```

The canonical bytes are what `lower_to_wasm_ir` returns — simplest reliable route: write a tiny throwaway test or use the drift test (Step 6) in reverse: run the drift test once with a `std::fs::write` line uncommented to emit `fixtures/trivial.ir.json`, then re-comment. (The fixture is canonical JSON — human-diffable.) If `tau build` fails the governance gate on this fixture, adjust `[allow]` per the diagnostic before continuing.

- [ ] **Step 3: Crate.** `crates/tau-embed-example/Cargo.toml`:

```toml
[package]
name = "tau-embed-example"
description = "EPIC 7.1 example: a product that embeds tau as a library — implements the Clock/Random/LLM/tool ports and drives a governed workflow via run_ir_streaming."
version.workspace      = true
edition.workspace      = true
rust-version.workspace = true
license.workspace      = true
repository.workspace   = true
authors.workspace      = true
publish = false

[dependencies]
tau-runtime-core = { workspace = true, features = ["wasm-interpreter"] }
tau-ir     = { workspace = true }
serde_json = { workspace = true }
futures    = { workspace = true }

[lints]
workspace = true
```

(Check the workspace `[workspace.dependencies]` alias for `futures` — if only `futures-core`/`futures-util` exist as aliases, add `futures = "0.3"` to `[workspace.dependencies]` or use `futures-util = { workspace = true, features = ["std"] }` + a manual `block_on` from `futures-executor`; prefer whichever alias already exists. `tau-runtime-core` workspace alias is `default-features = false` already — the example is a std binary but only needs the no_std surface + `wasm-interpreter`.)

`src/main.rs` — same port impls as the Task 5 scaffold, but written as a product (this is the file the docs point at). Structure:

```rust
//! tau-embed-example — EPIC 7.1 Variant B reference host.
//!
//! A "product" that links tau as a library: it bakes the IR of a governed
//! workflow (fixtures/trivial.ir.json, lowered from project/tau.toml — a
//! drift test in tau-cli keeps them in sync), implements the four ports
//! (LLM, tools, clock, entropy), and drives run_ir_streaming, printing
//! every RunEvent as a JSON line.
#![forbid(unsafe_code)]

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use serde_json::Value;
use tau_ir::ToolId;
use tau_runtime_core::embed::{
    batch_to_stream, from_canonical_bytes, run_ir_streaming, Clock, CompletionRequest,
    CompletionResponse, CompletionStream, DynLlmBackend, LlmBackend, LlmError, RandomSource,
    RunEvent, RuntimeError, StopReason, ToolDispatcher, ToolInvocationResult,
};

/// Canonical IR of `project/tau.toml`, baked at compile time exactly like a
/// `tau build --target rust-lib` artifact bakes TAU_IR.
const TAU_IR: &[u8] = include_bytes!("../fixtures/trivial.ir.json");

// <EchoBackend, SystemClock, XorShiftRandom, ProductDispatcher: same shapes
//  as the Task 5 scaffold template — copy them, rename ScaffoldDispatcher ->
//  ProductDispatcher; EchoBackend::complete returns
//  CompletionResponse::new("embed-example reply".to_string(), Vec::new(), StopReason::EndTurn, None)>

/// Run the baked workflow, calling `on_event` for every event. Returns the
/// terminal outcome debug string. Shared by main() and the test.
async fn run(on_event: &mut dyn FnMut(&RunEvent)) -> Result<(), Box<dyn std::error::Error>> {
    let module = Arc::new(from_canonical_bytes(TAU_IR)?);
    let entry = module.entry_agent()?.clone();
    let stream = run_ir_streaming(module, &entry, Arc::new(ProductDispatcher::new()), Vec::new()).await?;
    futures::pin_mut!(stream);
    while let Some(event) = stream.next().await {
        on_event(&event);
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    futures::executor::block_on(run(&mut |event| {
        println!("{}", serde_json::to_string(event).expect("RunEvent serializes"));
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_baked_workflow_to_completion() {
        let mut saw_completed = false;
        futures::executor::block_on(run(&mut |event| {
            if matches!(event, RunEvent::RunCompleted { .. }) {
                saw_completed = true;
            }
        }))
        .expect("workflow runs");
        assert!(saw_completed, "terminal RunCompleted must fire exactly once");
    }
}
```

(`RunEvent` is `#[non_exhaustive]` — matching with `{ .. }` is required. If the closure-borrow of `saw_completed` fights the `&mut dyn FnMut` signature, use a `std::cell::Cell<bool>` captured by reference.)

- [ ] **Step 4: Register member** in root `Cargo.toml` members (alphabetical-ish near tau-sdk-codegen): `"crates/tau-embed-example",`.

- [ ] **Step 5: Run the example's test**: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo nextest run -p tau-embed-example` → PASS; also `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo run -p tau-embed-example --quiet | tail -1` → last line is a RunCompleted JSON event.

- [ ] **Step 6: Drift guard** `crates/tau-cli/tests/embed_example_drift.rs`:

```rust
//! Keeps crates/tau-embed-example/fixtures/trivial.ir.json byte-equal to
//! lowering its project/tau.toml — the example's baked IR can't silently
//! drift from its source (same pattern as the SDK byte-equal tests).

use std::path::{Path, PathBuf};

fn example_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tau-embed-example")
}

#[test]
fn example_ir_fixture_matches_lowered_project() {
    let (_module, bytes) =
        tau_cli::cmd::build_wasm::lower_to_wasm_ir(&example_dir().join("project")).unwrap();
    let committed = std::fs::read(example_dir().join("fixtures/trivial.ir.json")).unwrap();
    // To REGENERATE after editing project/tau.toml:
    // std::fs::write(example_dir().join("fixtures/trivial.ir.json"), &bytes).unwrap();
    assert_eq!(
        bytes, committed,
        "example IR fixture drifted — uncomment the write line above, rerun, re-comment"
    );
}
```

(Verify `lower_to_wasm_ir` is `pub` — Task 6/7 imports settle the exact path; if it's `pub(crate)`, route through `emit_rust_lib_to` into a tempdir and read the baked const instead — simpler: make the drift test call `emit_rust_lib_to(project, tmp, TauDep::Version("0"))` and byte-compare `tmp/src/lib.rs`'s `TAU_IR` — NO: just check visibility first; `emit_host_to` already uses it cross-module, and tests can only see `pub` items. If not pub, add `pub` to `lower_to_wasm_ir` — it's already a documented seam.) Use Step 2's uncomment-once trick to mint the fixture the first time.

- [ ] **Step 7: Run**: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo nextest run -p tau-cli embed_example_drift` → PASS.

- [ ] **Step 8: README.md** (short): what it demonstrates, how to run (`cargo run -p tau-embed-example`), pointer to the how-to + spec.

- [ ] **Step 9: Commit**: `git add Cargo.toml crates/tau-embed-example crates/tau-cli/tests/embed_example_drift.rs && git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(example): tau-embed-example Variant B reference host (EPIC 7.1, #413)"`

---

### Task 9: docs — Variant B how-to

**Files:**
- Create: `docs/how-to/embed-rust-native.md`
- Modify: `docs/SUMMARY.md:31` (after "Build embedding artifacts")
- Modify: `docs/how-to/build-embedding-artifacts.md` (one cross-link sentence to the new page)

- [ ] **Step 1: Write the page.** Sections (use existing how-to pages as style guide; fenced blocks tagged with a language per the linkcheck gotcha):
  1. **What you get** — Variant B: your product links tau as a no_std library; tau compiles the workflow, your product supplies the ports.
  2. **Generate the artifacts** — `tau build --target rust-lib <project> --tau-dep-path <tau-checkout>` and `tau embed --host rust <project> --tau-dep-path <tau-checkout>`; explain `--tau-dep-path` exists because tau is not yet on crates.io (+ the async-stream patch note IF Task 7 needed it).
  3. **The entry-point contract** — sole-agent rule, `IrModule::entry_agent()`, multi-agent modules pass an explicit `AgentId` to `run_ir`.
  4. **The ports you implement** — table: mandatory `Clock`, `RandomSource`, `LlmBackend` (via `ToolDispatcher::llm_backend_for`), `ToolDispatcher::invoke` (reject-all is valid when the workflow has no tools); everything importable from `tau_runtime_core::embed`.
  5. **Drive it** — 15-line code excerpt from `crates/tau-embed-example/src/main.rs` (decode → entry_agent → run_ir_streaming → print events) + `cargo run -p tau-embed-example` output sample (2-3 JSON lines, elide with `[…]` NOT `[...]`).
  6. **Governance** — build-time gate (the artifact is emitted only from a governed project); `run_ir` itself trusts its IR bytes: provenance is the embedder's job (verify bundles with `tau verify` upstream).
  7. **Variant A** — one line: wasm-guest embedding is EPIC 7.2 (issue #414).

- [ ] **Step 2: SUMMARY entry** after line 31: `- [Embed tau in a Rust product](how-to/embed-rust-native.md)`

- [ ] **Step 3: Build the book**:

```bash
cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build && cd .. && rm -rf docs/book
```

Expected: only `[INFO]` lines.

- [ ] **Step 4: Commit**: `git add docs && git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "docs(how-to): embed tau in a Rust product — Variant B (EPIC 7.1, #413)"`

---

### Task 10: full gates, PR, merge queue, follow-up emit

- [ ] **Step 1: fmt + clippy + touched-crate tests** (per CLAUDE.md rules, sequentially):

```bash
timeout 30  env CARGO_TARGET_DIR=target/agent-e71 cargo fmt --check -p tau-ir -p tau-ports -p tau-runtime-core -p tau-sdk-codegen -p tau-cli -p tau-embed-example -p tau-wasm-guest
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo clippy -p tau-sdk-codegen -p tau-embed-example --all-targets
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo clippy -p tau-ir -p tau-ports -p tau-runtime-core -p tau-cli --all-targets
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo nextest run -p tau-ir -p tau-ports -p tau-sdk-codegen -p tau-embed-example
timeout 600 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo nextest run -p tau-runtime-core -p tau-cli
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo check -p tau-runtime-core --no-default-features --features wasm-interpreter
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e71 cargo check -p tau-ir --no-default-features
```

(remember Rule 5: `pgrep -af cargo | grep -v grep` first; wait or pick `target/agent-e71-2`). All green before proceeding. Doctests if any added: `cargo test --doc -p <crate>`.

- [ ] **Step 2: Push + PR**:

```bash
git push -u origin feat/epic-7-1-nostd-embed
gh pr create --base main --title "feat(embed): EPIC 7.1 — Variant B no_std embedding API + runnable artifacts + example (closes #413)" --body "<summary: filled TODO(7.1) stubs; entry-point contract IrModule::entry_agent (sole-agent rule); tau_runtime_core::embed prelude; TauDep --tau-dep-path; e2e compile-and-run test; tau-embed-example; docs how-to. Spec + plan under docs/superpowers/. Closes #413.>

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
gh pr merge --squash --auto   # NO --delete-branch (merge queue)
```

- [ ] **Step 3: Babysit** — `gh pr checks <N> --watch`; if BEHIND: `gh pr update-branch <N>`; if auto-merge drops after a flake, re-enroll BARE `gh pr merge <N> --auto`.

- [ ] **Step 4: On merge, emit follow-up** (from the handoff):

```bash
git fetch origin
if ! git branch -r | grep -qiE "7-2|wasm-guest-embed" && [ -z "$(gh pr list --search 'in:title 7.2 embedding' --json number -q '.[0].number')" ]; then
  echo "READY: EPIC 7.2 (issue #414) wasm-guest embedding + example ..."
else echo "no new lanes unblocked (7.2 already claimed)"; fi
```
