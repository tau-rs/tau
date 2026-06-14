# β.6 Cross-Target Conformance Gate (scaffolding) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a profile-agnostic scenario runner (`tau-conformance`) that runs the canonical fan-monitor scenario under the interpreted dev profile and asserts it produces a documented, bit-identical (modulo timestamps/IDs) ordered `ConformanceEvent` stream; stub the wasm profile behind `#[ignore]` pending β.7.5.

**Architecture:** A new `tau-conformance` crate defines a `Profile` trait. `DevProfile` drives the engine generator (`run_ir_streaming`, a new thin entry in `tau-runtime-core`) with a `tau-observe::Captor` tracing layer installed, interleaving the `RunEvent` stream and tracing events at each generator yield (causal because the executor is single-threaded). Both channels normalize into a versioned `ConformanceEvent` model; a real differ compares the normalized stream against a checked-in golden file. `WasmProfile` is a compiling stub.

**Tech Stack:** Rust (workspace crates), `tracing` + `tau-observe::Captor`, `tau-runtime-core` interpreter, `tau-mcp`/`tau-mcp-tokio` cassette transport, `serde_json`, `tokio` (current-thread), `insta`-style bless via env var.

**CARGO RULES (CLAUDE.md):** every cargo command MUST be `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo <cmd> -p <crate>`. This plan uses `CARGO_TARGET_DIR=target/agent-impl` in examples; a subagent uses `target/agent-<its-role>`. Prefer `cargo nextest run -p <crate>` for tests, `cargo test --doc` for doctests.

---

## File Structure

**Modified (`tau-runtime-core`):**
- `crates/tau-runtime-core/src/interpreter/agent_loop.rs` — extract `prepare_agent_run` from `run_agent`; add `run_agent_streaming`.
- `crates/tau-runtime-core/src/interpreter/mod.rs` — add `run_ir_streaming`.

**Created (`tau-conformance`):**
- `crates/tau-conformance/Cargo.toml`
- `crates/tau-conformance/src/lib.rs` — re-exports + `Scenario` + runner glue.
- `crates/tau-conformance/src/event.rs` — `ConformanceEvent`, `CONFORMANCE_EVENT_VERSION`.
- `crates/tau-conformance/src/differ.rs` — ordered diff + report.
- `crates/tau-conformance/src/normalize.rs` — `map_tracing` / `map_runevent` + modulo rules.
- `crates/tau-conformance/src/sequenced_llm.rs` — scripted `LlmBackend`.
- `crates/tau-conformance/src/dispatcher.rs` — `ConformanceDispatcher` (`ToolDispatcher`).
- `crates/tau-conformance/src/scenario.rs` — fixture loader + lowering.
- `crates/tau-conformance/src/profile/mod.rs` — `Profile` trait.
- `crates/tau-conformance/src/profile/dev.rs` — `DevProfile`.
- `crates/tau-conformance/src/profile/wasm.rs` — `WasmProfile` stub.
- `crates/tau-conformance/fixtures/fan_monitor/{tau.toml,mock_llm.jsonl,weather.cassette.jsonl,expected_events.json}`
- `crates/tau-conformance/tests/conformance.rs`

**Modified (workspace + CI + docs):**
- `Cargo.toml` (root) — add `crates/tau-conformance` member.
- `.github/workflows/*` — add `conformance (linux)` Tier 1 lane.
- `ROADMAP.md` — β.6 status note + β.7.5 unstub follow-up.

---

## Task 1: `run_ir_streaming` — streaming interpreter entry in `tau-runtime-core`

**Files:**
- Modify: `crates/tau-runtime-core/src/interpreter/agent_loop.rs:385-537`
- Modify: `crates/tau-runtime-core/src/interpreter/mod.rs:42-64`
- Test: `crates/tau-runtime-core/tests/run_ir_streaming.rs` (create)

Rationale: `run_ir` returns `RunOutcome` (collapses the stream via `run_with_history`). The conformance dev profile needs the uncollapsed `RunEvent` stream. Extract the shared construction so `run_agent` and the new `run_agent_streaming` cannot drift.

- [ ] **Step 1: Write the failing test**

Create `crates/tau-runtime-core/tests/run_ir_streaming.rs`:

```rust
//! `run_ir_streaming` yields the same logical run as `run_ir`, but as an
//! uncollapsed RunEvent stream ending in exactly one RunCompleted.
#![cfg(feature = "test-fixtures")]

use std::sync::Arc;
use futures_util::StreamExt as _;
use tau_runtime_core::interpreter::run_ir_streaming;
use tau_runtime_core::stream::RunEvent;

mod common; // reuse the existing test MockLlmBackend + a trivial IrModule builder

#[tokio::test(flavor = "current_thread")]
async fn run_ir_streaming_yields_run_completed_last() {
    let (module, entry, dispatcher) = common::single_agent_no_tools_fixture();
    let stream = run_ir_streaming(Arc::new(module), &entry, Arc::new(dispatcher), Vec::new());
    let events: Vec<RunEvent> = Box::pin(stream).collect().await;
    assert!(matches!(events.last(), Some(RunEvent::RunCompleted { .. })),
        "stream must end with RunCompleted; got {:?}", events.last());
}
```

If no reusable `common` helper exists for a no-tools single-agent IR fixture + dispatcher, build the smallest inline fixture using the same `MockLlmBackend` pattern `tau-ir-conformance::dev_mode::SequencedLlm` uses (one scripted `end_turn` response). Add `futures-util` to `[dev-dependencies]` if absent.

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core run_ir_streaming --features test-fixtures`
Expected: FAIL to compile — `run_ir_streaming` not found.

- [ ] **Step 3: Extract `prepare_agent_run` from `run_agent`**

In `agent_loop.rs`, move steps 1–7 of `run_agent` (lines 394–532: backend, builder+tools, `builder.build()`, `AgentDefinition` synth, `RunOptions` incl. clock/random/test-fixtures injection + context pipeline, `split_history`) into:

```rust
/// Shared construction for both the batch (`run_agent`) and streaming
/// (`run_agent_streaming`) interpreter drives. Returns everything the
/// kernel agent loop needs. Mirrors the prior inline construction so the
/// two drives cannot diverge.
fn prepare_agent_run<D>(
    module: alloc::sync::Arc<IrModule>,
    agent: &Agent,
    dispatcher: Arc<D>,
    initial_messages: Vec<Message>,
) -> Result<
    (
        Runtime,
        tau_domain::AgentDefinition,
        PackageManifest,
        Vec<Message>,
        Message,
        RunOptions,
    ),
    RuntimeError,
>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    // ... body = current run_agent lines 394–532 verbatim, returning the tuple
    // instead of calling rt.run_with_history at the end ...
}
```

Rewrite `run_agent` to delegate:

```rust
pub async fn run_agent<D>(
    module: alloc::sync::Arc<IrModule>,
    agent: &Agent,
    dispatcher: Arc<D>,
    initial_messages: Vec<Message>,
) -> Result<RunOutcome, RuntimeError>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    let (rt, agent_def, manifest, history, initial_message, run_options) =
        prepare_agent_run(module, agent, dispatcher, initial_messages)?;
    rt.run_with_history(agent_def, manifest, history, initial_message, run_options)
        .await
}
```

- [ ] **Step 4: Add `run_agent_streaming`**

In `agent_loop.rs`:

```rust
/// Streaming sibling of [`run_agent`]: returns the uncollapsed RunEvent
/// stream instead of folding it into a `RunOutcome`. Used by the β.6
/// conformance dev profile, which must observe every RunEvent in order.
pub async fn run_agent_streaming<D>(
    module: alloc::sync::Arc<IrModule>,
    agent: &Agent,
    dispatcher: Arc<D>,
    initial_messages: Vec<Message>,
) -> Result<impl futures_core::Stream<Item = crate::stream::RunEvent> + 'static, RuntimeError>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    let (rt, agent_def, manifest, history, initial_message, run_options) =
        prepare_agent_run(module, agent, dispatcher, initial_messages)?;
    rt.run_streaming_with_history(agent_def, manifest, history, initial_message, run_options)
        .await
}
```

(`run_streaming_with_history` returns `impl Stream + 'static` that owns Arc-clones of the tools registry, so it does not borrow `rt`; dropping `rt` after this call is sound.)

- [ ] **Step 5: Add `run_ir_streaming` to `mod.rs`**

Mirror `run_ir` (mod.rs:42-64):

```rust
/// Streaming sibling of [`run_ir`]. Resolves the entry agent and drives
/// it via [`agent_loop::run_agent_streaming`], returning the RunEvent
/// stream. See β.6 conformance gate.
pub async fn run_ir_streaming<D>(
    module: alloc::sync::Arc<IrModule>,
    entry: &AgentId,
    dispatcher: Arc<D>,
    initial_messages: Vec<Message>,
) -> Result<impl futures_core::Stream<Item = crate::stream::RunEvent> + 'static, RuntimeError>
where
    D: tool_dispatch::ToolDispatcher + Send + Sync + 'static,
{
    let agent_node = module
        .workflow
        .agents
        .get(entry)
        .ok_or_else(|| RuntimeError::AgentNotFound { agent: entry.0.clone() })?
        .clone();
    agent_loop::run_agent_streaming(module, &agent_node, dispatcher, initial_messages).await
}
```

Note: the test in Step 1 calls `run_ir_streaming(...)` without `.await?`; update the test to `let stream = run_ir_streaming(...).await.expect("stream builds");` to match this `async fn -> Result<...>` shape.

- [ ] **Step 6: Run the new test + the full existing suite (no drift)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core --features test-fixtures`
Then: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-conformance`
Expected: PASS — `run_agent` refactor preserves behavior; `tau-ir-conformance` (which drives `run_ir`) is green.

- [ ] **Step 7: Commit**

```bash
git add crates/tau-runtime-core/src/interpreter/ crates/tau-runtime-core/tests/run_ir_streaming.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(runtime-core): run_ir_streaming — uncollapsed RunEvent stream for conformance"
```

---

## Task 2: Scaffold the `tau-conformance` crate

**Files:**
- Create: `crates/tau-conformance/Cargo.toml`
- Create: `crates/tau-conformance/src/lib.rs`
- Modify: `Cargo.toml` (root, `members` list ~line 36)

- [ ] **Step 1: Create `Cargo.toml`**

```toml
[package]
name = "tau-conformance"
description = "Cross-profile (dev vs wasm) conformance gate for the canonical fan-monitor scenario. Internal test crate; not published."
version.workspace      = true
edition.workspace      = true
rust-version.workspace = true
license.workspace      = true
publish = false

[dependencies]
tau-domain        = { workspace = true, features = ["serde", "test-fixtures"] }
tau-ir            = { workspace = true }
tau-ports         = { workspace = true, features = ["serde", "test-fixtures"] }
tau-pkg           = { workspace = true }
tau-runtime-core  = { workspace = true, features = ["test-fixtures"] }
tau-runtime-tokio = { workspace = true }
tau-observe       = { workspace = true }
tau-mcp           = { workspace = true, features = ["with-std-adapters"] }
tau-mcp-tokio     = { workspace = true }
serde             = { workspace = true }
serde_json        = { workspace = true }
tokio             = { workspace = true }
async-trait       = { workspace = true }
futures-core      = { workspace = true }
futures-util      = { workspace = true }
sha2              = { workspace = true }

[dev-dependencies]
tokio             = { workspace = true, features = ["macros", "rt"] }
tempfile          = { workspace = true }

[features]
default = []
```

If any dep is not in the root workspace `[workspace.dependencies]`, add it there (check `futures-core`/`futures-util`/`sha2` exist — they are used elsewhere).

- [ ] **Step 2: Create a minimal `src/lib.rs`**

```rust
//! β.6 cross-profile conformance gate. See
//! `docs/superpowers/specs/2026-06-14-beta-6-conformance-gate-design.md`
//! and ADR-0046.

pub mod differ;
pub mod event;
pub mod normalize;
pub mod profile;
pub mod scenario;

mod dispatcher;
mod sequenced_llm;

pub use event::{ConformanceEvent, CONFORMANCE_EVENT_VERSION};
pub use profile::{Profile, ProfileError};
pub use scenario::Scenario;
```

Create empty stub modules (`pub mod`s with a `// filled in Task N` comment) so the crate compiles after Task 2; later tasks fill them. To compile now, temporarily stub each referenced item minimally OR build incrementally — recommended: comment out the module decls not yet created and re-enable per task. Keep `cargo check` green at each task boundary.

- [ ] **Step 3: Add the workspace member**

In root `Cargo.toml`, add `"crates/tau-conformance",` to `members` (next to `"crates/tau-ir-conformance",`).

- [ ] **Step 4: Verify it builds**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-conformance`
Expected: PASS (with whatever modules are enabled).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/tau-conformance/
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "chore(conformance): scaffold tau-conformance crate"
```

---

## Task 3: `ConformanceEvent` model + version

**Files:**
- Create: `crates/tau-conformance/src/event.rs`
- Test: inline `#[cfg(test)]` in `event.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn serde_round_trips_and_carries_version() {
        let ev = ConformanceEvent::ToolCallStarted {
            name: "read_temp".into(),
            args: serde_json::json!({}),
            call: "tc#0".into(),
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: ConformanceEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(ev, back);
        assert!(CONFORMANCE_EVENT_VERSION >= 1);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance event::`
Expected: FAIL — type not defined.

- [ ] **Step 3: Implement `event.rs`**

```rust
//! The versioned, crate-owned conformance event model. The authoritative
//! comparison contract (ADR-0046). Each variant is sourced from exactly
//! one runtime channel during normalization (see `normalize.rs`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Bump when the whitelist or any field projection changes; re-bless
/// goldens in the same change. Recorded in `expected_events.json`.
pub const CONFORMANCE_EVENT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ConformanceEvent {
    /// tracing `runtime.run_started`. run_id is modulo (not present here).
    RunStarted,
    /// tracing `runtime.context_step_ran`. Token counts compared.
    ContextStepRan { step: String, tokens_in: u64, tokens_out: u64 },
    /// tracing `llm.request_built`.
    InferenceCallStarted,
    /// tracing `llm.response_received` (+ folded `llm.token_usage`).
    InferenceCallCompleted { stop_reason: String, tokens_in: u64, tokens_out: u64 },
    /// RunEvent::ToolCallStarted. `call` is the canonical first-seen
    /// ordinal (e.g. "tc#0"); the provider id is modulo.
    ToolCallStarted { name: String, args: Value, call: String },
    /// RunEvent::ToolCallCompleted. `result` is the Ok body or a canonical
    /// error marker; `call` matches the paired Started ordinal.
    ToolCallCompleted { name: String, result: ToolOutcome, call: String },
    /// RunEvent::RunCompleted. Outcome discriminant only.
    RunCompleted { outcome: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ToolOutcome {
    Ok { body: Value },
    Err, // canonical marker; error text is modulo (provider-specific)
}
```

Note `Eq` requires `serde_json::Value` to be comparable — `Value` implements `PartialEq` but not `Eq`. Drop `Eq` (keep only `PartialEq`) on both enums to avoid a compile error.

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance event::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-conformance/src/event.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(conformance): ConformanceEvent model + version"
```

---

## Task 4: The differ

**Files:**
- Create: `crates/tau-conformance/src/differ.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ConformanceEvent;

    fn run_started() -> ConformanceEvent { ConformanceEvent::RunStarted }
    fn completed(o: &str) -> ConformanceEvent { ConformanceEvent::RunCompleted { outcome: o.into() } }

    #[test]
    fn equal_streams_have_no_diff() {
        let a = vec![run_started(), completed("Success")];
        assert!(diff(&a, &a).is_none());
    }

    #[test]
    fn first_divergence_reported_with_index() {
        let a = vec![run_started(), completed("Success")];
        let b = vec![run_started(), completed("Failure")];
        let d = diff(&a, &b).expect("streams differ");
        assert_eq!(d.index, 1);
        assert!(d.report.contains("index 1"));
    }

    #[test]
    fn length_mismatch_reported() {
        let a = vec![run_started(), completed("Success")];
        let b = vec![run_started()];
        let d = diff(&a, &b).expect("length differs");
        assert_eq!(d.index, 1);
        assert!(d.report.contains("missing") || d.report.contains("extra"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance differ::`
Expected: FAIL — `diff` not defined.

- [ ] **Step 3: Implement `differ.rs`**

```rust
//! Ordered, element-by-element diff of two normalized event streams.
use crate::event::ConformanceEvent;

#[derive(Debug)]
pub struct Divergence {
    pub index: usize,
    pub report: String,
}

/// Compare `expected` vs `actual`. Returns `None` if identical, else the
/// first divergence with a readable windowed report (±2 events).
pub fn diff(expected: &[ConformanceEvent], actual: &[ConformanceEvent]) -> Option<Divergence> {
    let n = expected.len().max(actual.len());
    for i in 0..n {
        let e = expected.get(i);
        let a = actual.get(i);
        if e != a {
            let mut report = format!("event-stream divergence at index {i}\n");
            match (e, a) {
                (Some(e), Some(a)) => {
                    report.push_str(&format!("  expected: {e:?}\n  actual:   {a:?}\n"));
                }
                (Some(e), None) => report.push_str(&format!("  actual stream missing: {e:?}\n")),
                (None, Some(a)) => report.push_str(&format!("  actual stream extra:   {a:?}\n")),
                (None, None) => unreachable!(),
            }
            let lo = i.saturating_sub(2);
            report.push_str("  --- expected window ---\n");
            for (j, ev) in expected.iter().enumerate().skip(lo).take(5) {
                report.push_str(&format!("    [{j}] {ev:?}\n"));
            }
            report.push_str("  --- actual window ---\n");
            for (j, ev) in actual.iter().enumerate().skip(lo).take(5) {
                report.push_str(&format!("    [{j}] {ev:?}\n"));
            }
            return Some(Divergence { index: i, report });
        }
    }
    None
}
```

- [ ] **Step 4: Run to verify pass**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance differ::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-conformance/src/differ.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(conformance): ordered event-stream differ"
```

---

## Task 5: The normalizer

**Files:**
- Create: `crates/tau-conformance/src/normalize.rs`

Reference (verified emission sites): `runtime.run_started` (no fields), `runtime.context_step_ran` carries `step`/`tokens_in`/`tokens_out` (`stream.rs:312`), `llm.request_built` (`stream.rs:341`), `llm.response_received` carries `stop_reason` (`stream.rs:411`), `llm.token_usage` carries `input_tokens`/`output_tokens` (`stream.rs:425`). The vocabulary name lives in the `name` field of each `CapturedEvent` (see `tests/context_pipeline.rs:308` `NameVisitor`), NOT `CapturedEvent.name`. `CapturedEvent` = `{ target, level, name, fields: BTreeMap<String,String> }` (`tau-observe/src/capture.rs:32`).

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tau_observe::CapturedEvent;
    use crate::event::ConformanceEvent;

    fn captured(name: &str, kv: &[(&str, &str)]) -> CapturedEvent {
        let mut fields = BTreeMap::new();
        fields.insert("name".to_string(), name.to_string());
        for (k, v) in kv { fields.insert(k.to_string(), v.to_string()); }
        CapturedEvent { target: "tau_runtime_core::stream".into(), level: "debug".into(),
                        name: "event".into(), fields }
    }

    #[test]
    fn whitelisted_tracing_events_map() {
        let mut st = NormState::default();
        assert!(matches!(map_tracing(&captured("runtime.run_started", &[]), &mut st),
            Some(ConformanceEvent::RunStarted)));
        let ctx = map_tracing(&captured("runtime.context_step_ran",
            &[("step","trim_old"),("tokens_in","40"),("tokens_out","30")]), &mut st);
        assert_eq!(ctx, Some(ConformanceEvent::ContextStepRan {
            step: "trim_old".into(), tokens_in: 40, tokens_out: 30 }));
    }

    #[test]
    fn non_whitelisted_tracing_events_dropped() {
        let mut st = NormState::default();
        assert_eq!(map_tracing(&captured("capability.allow", &[]), &mut st), None);
        assert_eq!(map_tracing(&captured("message.added", &[("role","User")]), &mut st), None);
    }

    #[test]
    fn tool_call_ids_canonicalize_to_first_seen_ordinals() {
        use tau_runtime_core::stream::RunEvent;
        let mut st = NormState::default();
        let started = map_runevent(RunEvent::ToolCallStarted {
            id: "toolu_abc".into(), name: "read_temp".into(), args: serde_json::json!({}) }, &mut st);
        let completed = map_runevent(RunEvent::ToolCallCompleted {
            id: "toolu_abc".into(), name: "read_temp".into(),
            result: Ok(serde_json::from_value(serde_json::json!({
                "content":[{"type":"text","text":"32"}]})).unwrap()) }, &mut st);
        // both reference the same ordinal "tc#0"
        if let (Some(ConformanceEvent::ToolCallStarted{call: c1, ..}),
                Some(ConformanceEvent::ToolCallCompleted{call: c2, ..})) = (&started, &completed) {
            assert_eq!(c1, "tc#0"); assert_eq!(c2, "tc#0");
        } else { panic!("expected tool-call pair, got {started:?} {completed:?}"); }
    }
}
```

(Adjust the `ToolResult` construction in the test to match `tau_ports::ToolResult`'s actual shape; if it is `#[non_exhaustive]`, build it via `serde_json::from_value` as `McpBackedTool::schema` does at `bridge.rs:88`.)

- [ ] **Step 2: Run to verify failure**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance normalize::`
Expected: FAIL — functions not defined.

- [ ] **Step 3: Implement `normalize.rs`**

```rust
//! Map raw runtime channels (tracing CapturedEvent + RunEvent) into the
//! versioned ConformanceEvent model, applying the modulo rules (ADR-0046).

use std::collections::BTreeMap;

use tau_observe::CapturedEvent;
use tau_runtime_core::stream::RunEvent;

use crate::event::{ConformanceEvent, ToolOutcome};

/// Carries cross-event normalization state (tool-call id → ordinal map,
/// and any pending token usage folded into InferenceCallCompleted).
#[derive(Default)]
pub struct NormState {
    tool_ids: BTreeMap<String, String>,
    next_ordinal: usize,
    /// token usage captured from a preceding `llm.token_usage`, folded
    /// into the next InferenceCallCompleted (emitted just before it in the
    /// same generator step? actually after response_received — see fold).
    pending_tokens: Option<(u64, u64)>,
}

impl NormState {
    fn ordinal_for(&mut self, id: &str) -> String {
        if let Some(o) = self.tool_ids.get(id) { return o.clone(); }
        let o = format!("tc#{}", self.next_ordinal);
        self.next_ordinal += 1;
        self.tool_ids.insert(id.to_string(), o.clone());
        o
    }
}

fn field<'a>(c: &'a CapturedEvent, k: &str) -> Option<&'a str> {
    c.fields.get(k).map(|s| s.as_str())
}
fn u64f(c: &CapturedEvent, k: &str) -> u64 {
    field(c, k).and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// Map a tracing event to a ConformanceEvent, or `None` if not whitelisted.
pub fn map_tracing(c: &CapturedEvent, st: &mut NormState) -> Option<ConformanceEvent> {
    match field(c, "name")? {
        "runtime.run_started" => Some(ConformanceEvent::RunStarted),
        "runtime.context_step_ran" => Some(ConformanceEvent::ContextStepRan {
            step: field(c, "step").unwrap_or_default().to_string(),
            tokens_in: u64f(c, "tokens_in"),
            tokens_out: u64f(c, "tokens_out"),
        }),
        "llm.request_built" => Some(ConformanceEvent::InferenceCallStarted),
        "llm.token_usage" => {
            // Fold into the InferenceCallCompleted emitted from the
            // following response_received in the SAME step batch. Order in
            // the runtime is response_received → token_usage → stop_reason
            // (stream.rs:409-440), so stash and attach on a second pass.
            st.pending_tokens = Some((u64f(c, "input_tokens"), u64f(c, "output_tokens")));
            None
        }
        "llm.response_received" => {
            let (ti, to) = st.pending_tokens.take().unwrap_or((0, 0));
            Some(ConformanceEvent::InferenceCallCompleted {
                stop_reason: field(c, "stop_reason").unwrap_or("none").to_string(),
                tokens_in: ti,
                tokens_out: to,
            })
        }
        _ => None,
    }
}

/// Map a RunEvent to a ConformanceEvent, or `None` if not whitelisted.
pub fn map_runevent(ev: RunEvent, st: &mut NormState) -> Option<ConformanceEvent> {
    match ev {
        RunEvent::ToolCallStarted { id, name, args } => Some(ConformanceEvent::ToolCallStarted {
            name, args, call: st.ordinal_for(&id),
        }),
        RunEvent::ToolCallCompleted { id, name, result } => {
            let outcome = match result {
                Ok(tr) => ToolOutcome::Ok { body: tool_result_to_json(&tr) },
                Err(_) => ToolOutcome::Err,
            };
            Some(ConformanceEvent::ToolCallCompleted { name, result: outcome, call: st.ordinal_for(&id) })
        }
        RunEvent::RunCompleted { outcome } => Some(ConformanceEvent::RunCompleted {
            outcome: run_outcome_discriminant(&outcome),
        }),
        _ => None, // TextDelta, TurnCompleted, FatalError not in whitelist
    }
}
```

Implement two small helpers in the same file: `tool_result_to_json(&ToolResult) -> serde_json::Value` (join `ToolContent::Text` blocks; mirror `McpBackedTool::invoke`'s content handling at `bridge.rs:120+`) and `run_outcome_discriminant(&RunOutcome) -> String` (e.g. `"Success"`/`"Failure"` from the `RunOutcome` variant — match `RunOutcome::Completed`/`Failed` per `outcome.rs`). Verify exact `RunEvent`/`ToolResult`/`RunOutcome` field names against `crates/tau-runtime-core/src/stream.rs:118-201` and `outcome.rs` before writing — adjust pattern bindings to match.

Important ordering caveat for token-fold: in the runtime, `llm.token_usage` is emitted *after* `llm.response_received` (stream.rs:409 then 420). So `pending_tokens` stashed from `token_usage` cannot be attached to a *preceding* `response_received`. Two honest options — pick (A): **attach token usage to the InferenceCallCompleted on a post-pass** by emitting `InferenceCallCompleted{tokens 0,0}` then patching the most-recent one when `token_usage` arrives; or (B) **model token usage as a separate whitelisted `TokenUsage` event**. Implement (A): in `map_tracing`, on `llm.token_usage`, mutate the last `InferenceCallCompleted` already pushed (requires the caller to pass the accumulating `&mut Vec`). Refactor `map_tracing` signature to `map_tracing(c, st, out: &mut Vec<ConformanceEvent>)` and push/patch internally, returning `()`. Update tests accordingly. (This keeps token counts ON the inference-completed event per Decision 2.)

- [ ] **Step 4: Run to verify pass**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance normalize::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-conformance/src/normalize.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(conformance): channel normalizer + modulo rules"
```

---

## Task 6: `SequencedLlm` scripted backend

**Files:**
- Create: `crates/tau-conformance/src/sequenced_llm.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tau_ports::LlmBackend;
    #[tokio::test(flavor = "current_thread")]
    async fn pops_responses_in_order_then_errors() {
        let r0 = crate::sequenced_llm::test_text_response("hi", "end_turn");
        let llm = SequencedLlm::new("mock-llm", vec![r0.clone()]);
        let got = llm.complete(crate::sequenced_llm::test_request()).await.unwrap();
        assert_eq!(got.text(), r0.text()); // adapt to CompletionResponse accessors
        assert!(llm.complete(crate::sequenced_llm::test_request()).await.is_err());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance sequenced_llm::`
Expected: FAIL.

- [ ] **Step 3: Implement (port from `tau-ir-conformance::dev_mode::SequencedLlm`)**

Copy the `SequencedLlm` struct + `LlmBackend` impl verbatim from `crates/tau-ir-conformance/src/dev_mode.rs:46-79` (name, `Mutex<VecDeque<CompletionResponse>>`, `complete` pops front or `LlmError::Internal`, `stream` delegates via `tau_ports::batch_to_stream`). Add `pub fn new(name, Vec<CompletionResponse>)`. Add the `parse_mock_llm(jsonl: &str) -> Vec<CompletionResponse>` parser (port from `dev_mode.rs:205-236`) so the scenario loader can build the queue. Provide the test helpers `test_text_response`/`test_request` used above (smallest valid `CompletionResponse`/`CompletionRequest`).

- [ ] **Step 4: Run to verify pass**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance sequenced_llm::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-conformance/src/sequenced_llm.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(conformance): SequencedLlm scripted backend + mock_llm parser"
```

---

## Task 7: `ConformanceDispatcher`

**Files:**
- Create: `crates/tau-conformance/src/dispatcher.rs`

The dispatcher routes the three fan-monitor tools: native `read_temp` → deterministic `32`, native `set_fan` → `{"ok":true}`, MCP `weather` → real cassette replay via an opened `McpClient`. It supplies the `SequencedLlm` backend. Clock/random fall back to test-fixtures injection (returns `None`).

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tau_runtime_core::interpreter::tool_dispatch::ToolDispatcher;

    #[tokio::test(flavor = "current_thread")]
    async fn native_tools_return_deterministic_bodies() {
        let llm = Arc::new(crate::sequenced_llm::SequencedLlm::new("mock-llm", vec![]));
        let d = ConformanceDispatcher::new_native_only(llm);
        let body = d.invoke(&tau_ir::ToolId("read_temp".into()), &serde_json::json!({}))
            .await.unwrap();
        assert_eq!(body.body, Some(serde_json::json!(32)));
        let body = d.invoke(&tau_ir::ToolId("set_fan".into()), &serde_json::json!({"on":true}))
            .await.unwrap();
        assert_eq!(body.body, Some(serde_json::json!({"ok": true})));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance dispatcher::`
Expected: FAIL.

- [ ] **Step 3: Implement `dispatcher.rs`**

```rust
//! ToolDispatcher for the fan-monitor scenario: deterministic natives +
//! a cassette-replayed MCP weather server.
use std::sync::Arc;
use core::future::Future;
use core::pin::Pin;
use serde_json::Value;

use tau_ir::ToolId;
use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};
use tau_mcp_tokio::host_lifecycle::client::McpClient;

pub(crate) struct ConformanceDispatcher {
    backend: Arc<dyn DynLlmBackend>,
    /// Opened cassette-backed MCP client; None for native-only unit tests.
    weather: Option<Arc<McpClient>>,
}

impl ConformanceDispatcher {
    pub(crate) fn new(backend: Arc<dyn DynLlmBackend>, weather: Arc<McpClient>) -> Self {
        Self { backend, weather: Some(weather) }
    }
    pub(crate) fn new_native_only(backend: Arc<dyn DynLlmBackend>) -> Self {
        Self { backend, weather: None }
    }
}

impl ToolDispatcher for ConformanceDispatcher {
    fn invoke<'a>(
        &'a self,
        tool_id: &'a ToolId,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        let id = tool_id.0.clone();
        let args = args.clone();
        Box::pin(async move {
            match id.as_str() {
                "read_temp" => Ok(ToolInvocationResult { body: Some(serde_json::json!(32)), error: None }),
                "set_fan" => Ok(ToolInvocationResult { body: Some(serde_json::json!({"ok": true})), error: None }),
                "weather" => {
                    let client = self.weather.as_ref().ok_or_else(|| RuntimeError::Internal {
                        message: "weather tool invoked but no MCP client wired".into(),
                    })?;
                    let resp = client.call_tool("weather", args).await.map_err(|e| RuntimeError::Internal {
                        message: format!("MCP weather call_tool: {e}"),
                    })?;
                    Ok(ToolInvocationResult { body: Some(mcp_response_to_json(&resp)), error: None })
                }
                other => Err(RuntimeError::Internal { message: format!("unknown conformance tool {other:?}") }),
            }
        })
    }

    fn llm_backend(&self) -> Arc<dyn DynLlmBackend> { self.backend.clone() }
    // clock()/random() default to None → run_agent injects test-fixtures MockClock/DeterministicRandom.
}
```

Implement `mcp_response_to_json(&ToolsCallResponse) -> Value` (extract text content blocks deterministically; `ToolsCallResponse` shape per `client.rs`). Confirm the exact `McpClient` import path (`tau_mcp_tokio::host_lifecycle::client::McpClient`) and `call_tool` signature (`client.rs:69`: `call_tool(&self, server_tool_name: &str, args: Value) -> Result<ToolsCallResponse, McpError>`).

- [ ] **Step 4: Run to verify pass**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance dispatcher::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-conformance/src/dispatcher.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(conformance): fan-monitor ToolDispatcher (natives + MCP cassette)"
```

---

## Task 8: Scenario loader

**Files:**
- Create: `crates/tau-conformance/src/scenario.rs`

Loads a fixture dir → `(IrModule, entry agent id, Vec<CompletionResponse>, weather cassette path)`. Mirrors `tau-ir-conformance::dev_mode` lowering (`dev_mode.rs:273-318`).

- [ ] **Step 1: Write the failing test** (uses the fixture from Task 11; until then, point at a tiny temp fixture)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn loads_fan_monitor_fixture() {
        let s = Scenario::load(Scenario::fixture_dir("fan_monitor")).expect("loads");
        assert_eq!(s.entry.0, "fan-monitor");
        assert!(!s.responses.is_empty());
        assert!(s.weather_cassette.exists());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance scenario::`
Expected: FAIL (type missing, and/or fixture missing — that is fine; this task and Task 11 land together for a green run).

- [ ] **Step 3: Implement `scenario.rs`**

```rust
//! Fixture loader: tau.toml → IrModule, plus scripted LLM + cassette path.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tau_ir::{AgentId, IrModule};
use tau_pkg::project::ProjectConfig; // confirm exact path used by dev_mode
use tau_ports::CompletionResponse;

pub struct Scenario {
    pub module: Arc<IrModule>,
    pub entry: AgentId,
    pub responses: Vec<CompletionResponse>,
    pub weather_cassette: PathBuf,
    pub dir: PathBuf,
}

impl Scenario {
    pub fn fixture_dir(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures").join(name)
    }

    pub fn load(dir: impl Into<PathBuf>) -> Result<Self, String> {
        let dir = dir.into();
        let toml = std::fs::read_to_string(dir.join("tau.toml")).map_err(|e| e.to_string())?;
        let config = ProjectConfig::parse_str(&toml).map_err(|e| format!("{e}"))?;
        let target = tau_ports::target::list_available().next()
            .expect("a target triple is available").triple; // confirm registry path
        let module = {
            let caches = tau_ir::lower::Caches {
                native_tool: &|name: &str| Some(crate::scenario::sha256_name(name)),
                mcp_contract: &|_| None,
                skill: &|_| None,
            };
            tau_ir::lower::lower_project(&config, &target, &caches).map_err(|e| format!("{e}"))?
        };
        let entry = module.workflow.agents.keys().next()
            .ok_or("fixture declares no agent")?.clone();
        let jsonl = std::fs::read_to_string(dir.join("mock_llm.jsonl")).map_err(|e| e.to_string())?;
        let responses = crate::sequenced_llm::parse_mock_llm(&jsonl);
        Ok(Self { module: Arc::new(module), entry, responses,
                  weather_cassette: dir.join("weather.cassette.jsonl"), dir })
    }
}

/// SHA-256-of-name native-tool cache key, symmetric with the lowering
/// cache `tau-cli::cmd::build::lower_ir` and `tau-ir-conformance` use.
pub(crate) fn sha256_name(name: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(name.as_bytes());
    h.into()
}
```

Verify exact import paths against `tau-ir-conformance/src/dev_mode.rs` (the `Caches`/`lower_project`/`ProjectConfig`/target-registry symbols it uses) and `crate::sha256_name` shape (`tau-ir-conformance/src/lib.rs`) — copy them precisely. Adjust the `sha256_name` return type to whatever `Caches.native_tool` expects.

- [ ] **Step 4: Defer the run** — full pass arrives with Task 11's fixture. Run `cargo check -p tau-conformance` to confirm it compiles.

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-conformance`
Expected: PASS (compiles).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-conformance/src/scenario.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(conformance): scenario loader (tau.toml -> IrModule + cassettes)"
```

---

## Task 9: `Profile` trait + `DevProfile` (the yield-barrier interleave)

**Files:**
- Create: `crates/tau-conformance/src/profile/mod.rs`
- Create: `crates/tau-conformance/src/profile/dev.rs`

- [ ] **Step 1: Implement `profile/mod.rs`**

```rust
//! Execution profiles. A profile produces a normalized ConformanceEvent
//! stream from a scenario. See ADR-0046.
use crate::event::ConformanceEvent;
use crate::scenario::Scenario;

pub mod dev;
pub mod wasm;

pub use dev::DevProfile;
pub use wasm::WasmProfile;

#[derive(Debug)]
pub struct ProfileError(pub String);

#[async_trait::async_trait(?Send)]
pub trait Profile {
    fn name(&self) -> &str;
    async fn run(&self, scenario: &Scenario) -> Result<Vec<ConformanceEvent>, ProfileError>;
}
```

(`?Send` because the interpreter futures are non-`Send`; the runner is single-threaded.)

- [ ] **Step 2: Write the failing test** (lands green with Task 11; for now assert it builds + runs against the fixture)

In `profile/dev.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::Profile;
    use crate::scenario::Scenario;

    #[tokio::test(flavor = "current_thread")]
    async fn dev_profile_emits_run_started_first_and_run_completed_last() {
        let s = Scenario::load(Scenario::fixture_dir("fan_monitor")).unwrap();
        let events = DevProfile.run(&s).await.unwrap();
        assert!(matches!(events.first(), Some(crate::event::ConformanceEvent::RunStarted)));
        assert!(matches!(events.last(), Some(crate::event::ConformanceEvent::RunCompleted { .. })));
    }
}
```

- [ ] **Step 3: Implement `profile/dev.rs`**

```rust
//! Interpreted dev profile: drive run_ir_streaming with a Captor tracing
//! layer; interleave tracing + RunEvent at each generator yield.
use std::sync::Arc;
use futures_util::StreamExt as _;

use tau_observe::Captor;

use crate::dispatcher::ConformanceDispatcher;
use crate::event::ConformanceEvent;
use crate::normalize::{map_runevent, map_tracing, NormState};
use crate::profile::{Profile, ProfileError};
use crate::scenario::Scenario;
use crate::sequenced_llm::SequencedLlm;

pub struct DevProfile;

#[async_trait::async_trait(?Send)]
impl Profile for DevProfile {
    fn name(&self) -> &str { "dev" }

    async fn run(&self, scenario: &Scenario) -> Result<Vec<ConformanceEvent>, ProfileError> {
        // 1. Open the cassette-backed MCP weather client.
        let weather = open_weather_client(&scenario.weather_cassette).await
            .map_err(|e| ProfileError(format!("open weather cassette: {e}")))?;

        // 2. Build backend + dispatcher.
        let backend = Arc::new(SequencedLlm::new("mock-llm", scenario.responses.clone()));
        let dispatcher = Arc::new(ConformanceDispatcher::new(backend, weather));

        // 3. Install the Captor as the thread-local default for the whole
        //    run. set_default's guard holds across awaits on this
        //    single-threaded executor.
        let captor = Captor::new();
        let _guard = tracing::subscriber::set_default(captor.subscriber());

        // 4. Drive the generator; interleave at yield barriers.
        let stream = tau_runtime_core::interpreter::run_ir_streaming(
            scenario.module.clone(), &scenario.entry, dispatcher, Vec::new(),
        ).await.map_err(|e| ProfileError(format!("run_ir_streaming: {e}")))?;
        let mut stream = Box::pin(stream);

        let mut out: Vec<ConformanceEvent> = Vec::new();
        let mut st = NormState::default();
        let mut consumed = 0usize;
        loop {
            let next = stream.next().await;
            // tracing emitted during this resume step, in order:
            let captured = captor.events();
            for c in &captured[consumed..] {
                map_tracing(c, &mut st, &mut out); // (A)-style: pushes/patches internally
            }
            consumed = captured.len();
            match next {
                Some(ev) => { if let Some(ce) = map_runevent(ev, &mut st) { out.push(ce); } }
                None => break,
            }
        }
        Ok(out)
    }
}
```

Add `async fn open_weather_client(path: &Path) -> Result<Arc<McpClient>, String>` in this file: call `tau_mcp_tokio::host_lifecycle::open(&format!("cassette:{}", path.display()), &CapabilityPlan::new(Vec::new(), None, None), Arc::new(PassthroughSandbox::new()), McpClientOptions::default())`, wrap `Ok` in `Arc::new`. Confirm `CapabilityPlan` and `PassthroughSandbox` import paths (`PassthroughSandbox` is in `tau_runtime_tokio::process_gate`; `CapabilityPlan` per `ir_dispatcher.rs:705`). Note `map_tracing` is the `(A)` signature from Task 5 (`map_tracing(c, &mut st, &mut out)`).

- [ ] **Step 4: Build check** (full run with Task 11)

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-conformance`
Expected: PASS (compiles).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-conformance/src/profile/mod.rs crates/tau-conformance/src/profile/dev.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(conformance): Profile trait + DevProfile yield-barrier interleave"
```

---

## Task 10: `WasmProfile` stub

**Files:**
- Create: `crates/tau-conformance/src/profile/wasm.rs`

- [ ] **Step 1: Implement the compiling stub**

```rust
//! Compiled-wasm profile. STUB — unblocks at β.7.5 (`tau build wasm`).
//! TODO(β.7.5): build the fan-monitor as a wasm component, run it in
//! wasmtime, and harvest the guest's ConformanceEvent stream across the
//! component boundary. See ADR-0046 and ROADMAP §β.7.5.
use crate::event::ConformanceEvent;
use crate::profile::{Profile, ProfileError};
use crate::scenario::Scenario;

pub struct WasmProfile;

#[async_trait::async_trait(?Send)]
impl Profile for WasmProfile {
    fn name(&self) -> &str { "wasm" }
    async fn run(&self, _scenario: &Scenario) -> Result<Vec<ConformanceEvent>, ProfileError> {
        unimplemented!("TODO(β.7.5): drive tau build wasm artifact in wasmtime, harvest guest ConformanceEvents")
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-conformance`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-conformance/src/profile/wasm.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(conformance): WasmProfile stub (TODO beta-7.5)"
```

---

## Task 11: The fan-monitor fixture + bless the golden

**Files:**
- Create: `crates/tau-conformance/fixtures/fan_monitor/tau.toml`
- Create: `crates/tau-conformance/fixtures/fan_monitor/mock_llm.jsonl`
- Create: `crates/tau-conformance/fixtures/fan_monitor/weather.cassette.jsonl`
- Create: `crates/tau-conformance/fixtures/fan_monitor/expected_events.json`

- [ ] **Step 1: Author `tau.toml`** (model on fixtures 01/07/13)

```toml
[project]
name = "fan-monitor"

[agents.fan-monitor]
display_name = "Fan Monitor"
package      = "fan-monitor@^0.1"
llm_backend  = "mock-llm"
model        = "claude-haiku-4-5"
tool_refs    = ["read_temp", "weather", "set_fan"]
max_turns    = 4

[agents.fan-monitor.prompt]
system = "Read the temperature. If above 30C, check weather; if hot outside, keep fan on; otherwise off."

[[agents.fan-monitor.context.pipeline]]
transformer = "trim_old"
[agents.fan-monitor.context.steps.trim_old]
keep_last_turns = 8
[[agents.fan-monitor.context.pipeline]]
transformer = "compact_tool_outputs"
[agents.fan-monitor.context.steps.compact_tool_outputs]
max_bytes = 1024
[[agents.fan-monitor.context.pipeline]]
transformer = "fit_budget"
[agents.fan-monitor.context.steps.fit_budget]
max_tokens = 8192

[tools.read_temp]
native      = "ReadTemp"
description = "Read the current temperature."
capabilities = []

[tools.set_fan]
native      = "SetFan"
description = "Set the fan on or off."
capabilities = []

[tools.weather]
mcp         = "cassette:./weather.cassette.jsonl"
description = "Look up current weather via cassette replay."
capabilities = [{ kind = "net.http" }]
```

Confirm the exact prompt-table key and tool-body keys against the schema (memory: agent TOML uses `[agents.<id>.prompt]` with `system`/`system_file`; `native`/`mcp` tool bodies per fixtures). Adjust if the loader rejects.

- [ ] **Step 2: Author `mock_llm.jsonl`** — turn-ordered scripted responses implementing the fan-monitor logic. Match the `MockLlmLine` schema (`dev_mode.rs:181-203`: `{"response": {"text", "tool_uses":[{"id","name","input"}], "stop_reason"}}`).

```json
{"response": {"tool_uses": [{"id": "t0", "name": "read_temp", "input": {}}], "stop_reason": "tool_use"}}
{"response": {"tool_uses": [{"id": "t1", "name": "weather", "input": {"city": "here"}}], "stop_reason": "tool_use"}}
{"response": {"tool_uses": [{"id": "t2", "name": "set_fan", "input": {"on": true}}], "stop_reason": "tool_use"}}
{"response": {"text": "Fan is on.", "stop_reason": "end_turn"}}
```

- [ ] **Step 3: Author `weather.cassette.jsonl`** — copy `crates/tau-ir-conformance/fixtures/07_mcp_weather_cassette/weather_cassette.jsonl` and adapt so a `tools/call` for `weather` returns a deterministic "hot outside" result. Keep the JSONL header line (`{"version":1}`) and the recorded `initialize`/`tools/list`/`tools/call` exchange shape (`tau-mcp/src/cassette/message.rs`). The `tools/list` MUST advertise a tool named `weather` (so the contract resolves) and `tools/call` MUST return text content the dispatcher maps into the event stream.

- [ ] **Step 4: Bless the golden**

First make `expected_events.json` an empty placeholder `[]`, then run the bless path (added in Task 12) once:

Run: `timeout 300 env TAU_CONFORMANCE_BLESS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance fan_monitor_dev_matches_golden`
Expected: the test writes `expected_events.json`. Inspect it by hand: it MUST contain `RunStarted`, three `ContextStepRan` (trim_old/compact_tool_outputs/fit_budget) per turn, `InferenceCallStarted/Completed`, `ToolCallStarted/Completed` for read_temp → weather → set_fan, and a final `RunCompleted{outcome:"Success"}`. If the stream looks wrong, fix the fixture (not the golden).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-conformance/fixtures/fan_monitor/
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(conformance): canonical fan-monitor fixture + blessed golden"
```

---

## Task 12: Integration tests + bless mechanism

**Files:**
- Create: `crates/tau-conformance/tests/conformance.rs`

- [ ] **Step 1: Write the tests**

```rust
//! β.6 conformance gate tests.
//! (a) dev == golden — LIVE.
//! (b) dev == wasm — #[ignore] until β.7.5.

use std::path::PathBuf;
use tau_conformance::{differ, event::ConformanceEvent, profile::{DevProfile, WasmProfile, Profile}, scenario::Scenario};

fn golden_path(dir: &PathBuf) -> PathBuf { dir.join("expected_events.json") }

#[derive(serde::Serialize, serde::Deserialize)]
struct Golden { version: u32, events: Vec<ConformanceEvent> }

#[tokio::test(flavor = "current_thread")]
async fn fan_monitor_dev_matches_golden() {
    let s = Scenario::load(Scenario::fixture_dir("fan_monitor")).expect("load fixture");
    let actual = DevProfile.run(&s).await.expect("dev profile runs");

    if std::env::var("TAU_CONFORMANCE_BLESS").is_ok() {
        let g = Golden { version: tau_conformance::CONFORMANCE_EVENT_VERSION, events: actual.clone() };
        std::fs::write(golden_path(&s.dir), serde_json::to_string_pretty(&g).unwrap()).unwrap();
        eprintln!("blessed {} events", actual.len());
        return;
    }

    let raw = std::fs::read_to_string(golden_path(&s.dir)).expect("golden exists (bless first)");
    let golden: Golden = serde_json::from_str(&raw).expect("golden parses");
    assert_eq!(golden.version, tau_conformance::CONFORMANCE_EVENT_VERSION,
        "golden version stale — re-bless with TAU_CONFORMANCE_BLESS=1");
    if let Some(d) = differ::diff(&golden.events, &actual) {
        panic!("dev profile diverged from golden:\n{}", d.report);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dev_profile_is_deterministic() {
    let s = Scenario::load(Scenario::fixture_dir("fan_monitor")).expect("load");
    let a = DevProfile.run(&s).await.expect("run 1");
    let b = DevProfile.run(&s).await.expect("run 2");
    assert!(differ::diff(&a, &b).is_none(), "dev profile is nondeterministic");
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "TODO(β.7.5): WasmProfile needs `tau build wasm`; see ADR-0046"]
async fn fan_monitor_dev_matches_wasm() {
    let s = Scenario::load(Scenario::fixture_dir("fan_monitor")).expect("load");
    let dev = DevProfile.run(&s).await.expect("dev runs");
    let wasm = WasmProfile.run(&s).await.expect("wasm runs"); // unimplemented! until β.7.5
    if let Some(d) = differ::diff(&dev, &wasm) {
        panic!("dev vs wasm divergence:\n{}", d.report);
    }
}
```

- [ ] **Step 2: Run the live tests (after Task 11 bless)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance`
Expected: `fan_monitor_dev_matches_golden` PASS, `dev_profile_is_deterministic` PASS, `fan_monitor_dev_matches_wasm` SKIPPED (ignored).

- [ ] **Step 3: Confirm the ignored wasm test is recognized**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance --run-ignored only fan_monitor_dev_matches_wasm`
Expected: the test runs and PANICS with the `unimplemented!` `TODO(β.7.5)` message (proves the arm is wired, just not implemented). Do NOT enable it in CI.

- [ ] **Step 4: Commit**

```bash
git add crates/tau-conformance/tests/conformance.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "test(conformance): dev==golden live + dev==wasm ignored + determinism"
```

---

## Task 13: CI lane — `conformance (linux)` Tier 1

**Files:**
- Modify: the Tier 1 PR workflow under `.github/workflows/` (find the file running per-crate `cargo nextest run`; per ROADMAP CI table this is the fast loop).

- [ ] **Step 1: Inspect the existing Tier 1 matrix**

Run: `ls .github/workflows/ && grep -rln "nextest" .github/workflows/`
Identify how existing per-crate test lanes are declared (e.g. a matrix entry per crate).

- [ ] **Step 2: Add the conformance lane**

Add a job/matrix entry mirroring the existing test lanes, scoped to `tau-conformance`, named `conformance (linux)`:

```yaml
  conformance:
    name: conformance (linux)
    runs-on: ubuntu-latest
    steps:
      # ... reuse the repo's standard checkout + toolchain + sccache setup ...
      - name: Run conformance gate (dev profile)
        run: cargo nextest run -p tau-conformance
```

Match the surrounding jobs' exact setup steps (toolchain action SHA-pin, sccache env, `CARGO_INCREMENTAL=0`). The `#[ignore]`d wasm test is excluded automatically (nextest skips ignored by default). Add `conformance (linux)` to the required-checks aggregation if the repo gates on a `ci-summary` job (per memory: only `ci-summary` is required) — add it to that job's `needs`.

- [ ] **Step 3: Validate YAML locally**

Run: `python3 -c "import yaml,sys; [yaml.safe_load(open(f)) for f in sys.argv[1:]]" .github/workflows/*.yml`
Expected: no exceptions.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "ci(conformance): add conformance (linux) Tier 1 lane"
```

---

## Task 14: ROADMAP status + β.7.5 unstub follow-up

**Files:**
- Modify: `ROADMAP.md` (§β.6 ~line 408-425; CI table ~line 707)

- [ ] **Step 1: Update the β.6 entry**

Under §β.6, add a status note: scaffolding shipped (dev profile + differ + fan-monitor fixture live; wasm arm `#[ignore]`d). Add an explicit follow-up line: "β.7.5 unstub: implement `WasmProfile::run` against `tau build wasm` and flip `fan_monitor_dev_matches_wasm` from `#[ignore]` to live — the `ConformanceEvent` contract is frozen (ADR-0046)." Reference the spec + ADR paths.

- [ ] **Step 2: Confirm the CI table row** (line 707 already lists the lane) — adjust wording to note "dev profile live; wasm arm pending β.7.5" if helpful.

- [ ] **Step 3: Commit**

```bash
git add ROADMAP.md
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "docs(roadmap): beta-6 scaffolding status + beta-7.5 unstub follow-up"
```

---

## Final verification

- [ ] **Full crate test + clippy + fmt**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core --features test-fixtures
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-conformance
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-conformance -- -D warnings
timeout 30  env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-conformance -- --check
```

Expected: all green; `tau-ir-conformance` proves the `run_agent` refactor didn't regress.

- [ ] **Open the PR**

```bash
git push -u origin feat/beta-6-conformance
gh pr create --base main --title "feat(conformance): β.6 cross-target conformance gate (scaffolding)" \
  --body "Implements the β.6 scaffolding per docs/superpowers/specs/2026-06-14-beta-6-conformance-gate-design.md and ADR-0046. Dev profile + differ + fan-monitor fixture live; wasm arm #[ignore]'d pending β.7.5."
```

---

## Self-Review notes (author)

- **Spec coverage:** Profile trait (T9), dev/wasm profiles (T9/T10), ConformanceEvent + version (T3), normalization/modulo (T5), differ (T4), fan-monitor fixture w/ natives+MCP+context pipeline+cassette LLM (T11), two assertions (T12), `run_ir_streaming` (T1), CI Tier 1 (T13), β.7.5 follow-up (T14). All spec sections mapped.
- **Known verification points flagged for the implementer** (not placeholders — exact call shapes given, with the file:line to confirm field names against): `RunEvent`/`ToolResult`/`RunOutcome` exact fields (`stream.rs:118`, `outcome.rs`); `ProjectConfig`/`Caches`/`lower_project`/target-registry import paths (copy from `tau-ir-conformance/src/dev_mode.rs`); `CapabilityPlan`/`PassthroughSandbox` paths; MCP `ToolsCallResponse` content shape (`client.rs`). These are concrete lookups, each with a cited source to mirror.
- **Token-fold ordering:** `llm.token_usage` is emitted AFTER `llm.response_received` (stream.rs:409→420) — Task 5 resolves this with the patch-last-InferenceCallCompleted approach (option A), not a pre-stash.
