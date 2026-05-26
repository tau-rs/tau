# Logging Sub-project B — §3.9 Span Vocabulary Completion

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

## State addendum (2026-05-25)

Tasks 1 and 9 shipped ahead of Tasks 2-8 (PRs #198 and #196 respectively).
The remaining work is **Tasks 2-8** + Task 10 final-verify. This addendum
records every drift between the plan body below and the current `main`,
so implementer subagents can trust the task text once they've read this.

**Already in place (do not re-add):**

- `tau_observe::vocabulary::*` — all 8 span + 20 event constants exist
  with the expected names (see `crates/tau-observe/src/vocabulary.rs`).
- `tau_observe::capture::Captor` — gated behind the `test-fixtures`
  feature, exported from `lib.rs`.
- `crates/tau-runtime/Cargo.toml` — `tau-observe = { workspace = true }`
  is already a non-dev dependency. The **dev-dep with `features =
  ["test-fixtures"]` was added in this branch's first commit**; nothing
  more to do.
- `crates/tau-runtime/tests/tracing_emission.rs` — an existing
  integration test already wires a custom `Layer` (not `Captor`) and
  asserts a small set of milestone events fire. **Extend or replace it;
  do not create a parallel `tracing_vocabulary.rs`.** The fixture
  pattern uses `tau_ports::fixtures::{make_completion_response,
  make_token_usage, MockLlmBackend}` plus `common::agent_def` /
  `common::manifest_with_no_capabilities` / `common::user_message`.
- `crates/tau-runtime/tests/common/mock_llm.rs` — the real MockLlmBackend
  pattern. (The plan body mentions porting from a nonexistent
  `orchestration_patterns.rs`; ignore that — `common::` is the
  reusable home.)
- `runtime.agent_run` span exists (via `#[instrument]` on
  `Runtime::run_with_history`).
- `llm.complete` span exists (via `#[instrument(name = "llm.complete")]`
  in `stream.rs` around line 201).
- `EV_RUNTIME_RUN_STARTED`, `EV_RUNTIME_TURN_STARTED`,
  `EV_RUNTIME_LOOP_TERMINATED`, `EV_LLM_REQUEST_BUILT`,
  `EV_LLM_RESPONSE_RECEIVED` — already emitted in `stream.rs`.
- `EV_CAPABILITY_DENY` — emitted inline at `stream.rs:343` and `:973`
  (Task 6 must decide: leave inline, or move inside the new
  `capability.check` span. The latter is cleaner; do that unless it
  forces a refactor of the failure path).
- `EV_TOOL_SESSION_OPEN_FAILED`, `EV_TOOL_INVOKE_FAILED`,
  `EV_TOOL_SESSION_CLOSE_FAILED` — already emitted at the error
  branches inside `plugin_host/ipc_tool.rs`. Task 7 must verify they
  fall inside the new spans; if not, restructure.

**Known drift to fix (Task 3 / sweep):**

- `stream.rs:300` emits `name = "runtime.run_completed"` as a **literal
  string**, NOT through `EV_RUNTIME_COMPLETED` (`"runtime.completed"`).
  These are the same conceptual event. **Migrate the literal to
  `EV_RUNTIME_COMPLETED` as part of Task 3.** Two consequences:
  - The existing `tracing_emission.rs` assertion `event:runtime.run_completed`
    must update to `event:runtime.completed`.
  - Any downstream consumer (log dashboards, JSON-event parsers) sees a
    name change. None exist yet outside the kernel, so this is safe.

**Files to touch (corrected from plan body):**

- `crates/tau-runtime/src/run.rs` — `Runtime::invoke_tool` is at line
  259 (Task 5). The plan body says `crates/tau-runtime/src/dispatch.rs`;
  there is no `dispatch.rs`.
- `crates/tau-runtime/src/capability.rs:70` — `check_capabilities` lives
  here (Task 6). The plan body says `capability_check.rs`; the file is
  `capability.rs`.
- `crates/tau-runtime/src/plugin_host/ipc_tool.rs` — `IpcTool` impl
  with the `Open`/`Invoke`/`Close` paths (Task 7). The plan body says
  `plugin_host/mod.rs` or "send_open / send_invoke / send_close"; the
  real names are methods on `IpcTool`, not free functions.

**Test file convention:**

- Extend `crates/tau-runtime/tests/tracing_emission.rs` rather than
  creating `tracing_vocabulary.rs`. Optionally replace the custom Layer
  with `Captor` for richer field assertions; an incremental migration
  (keep the custom Layer for the existing milestones; use `Captor` for
  new field-level asserts) is acceptable.

The task bodies below stand as written; treat conflicts between the body
and this addendum as: **the addendum wins**.

---


**Goal:** Make the kernel emit the full ADR-0006 §3.9 vocabulary: six new spans and ~15 new events on top of the two spans and seven events that already exist.

**Architecture:** Add `#[instrument]` / `info_span!` decorations at six call sites inside the streaming pump (`stream.rs`), the dispatch path, the capability-check path, and the plugin-host session/invoke path. Every span name and event name is imported from `tau_observe::vocabulary` (introduced in Sub-project A) so the §3.9 dictionary cannot drift. Each event has unit-test coverage via a capture subscriber.

**Tech Stack:** Rust 2021, `tracing = "0.1"`, `tracing-subscriber = "0.3"` (`fmt` + `env-filter`). Test-only: `tracing-subscriber` `registry` feature for the capture subscriber.

**Cargo rules:** As in Plan A — every `cargo` invocation uses `timeout <T> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <verb> -p <crate>`.

**Depends on:** Sub-project A merged (uses `tau_observe::vocabulary::*` constants).

---

## File Structure

**Created:**
- `crates/tau-runtime/tests/tracing_vocabulary.rs` — integration test asserting each span/event fires with the expected fields when a minimal agent runs through `MockLlmBackend`.
- `crates/tau-observe/src/capture.rs` — test-only capture subscriber utility shared by tau-runtime's vocabulary tests (gated behind a `test-fixtures` feature so it doesn't appear in release builds).

**Modified:**
- `crates/tau-runtime/src/stream.rs` — add `runtime.turn` span around the turn loop; emit the missing `runtime.completed`, `runtime.failed`, `runtime.max_turns_reached`, `llm.request_built`, `llm.response_received`, `llm.token_usage`, `llm.stop_reason`, `llm.tool_use_emitted`, `message.added` events.
- `crates/tau-runtime/src/dispatch.rs` (or wherever tool dispatch lives — confirm at execution time via `grep -rn "fn dispatch_tool\|fn invoke_tool" crates/tau-runtime/src`) — add `dispatch.tool` span; emit `dispatch.tool_resolved`.
- `crates/tau-runtime/src/capability_check.rs` (confirm path via `grep -rn "fn check_capability\|EffectiveCapability::" crates/tau-runtime/src | head`) — add `capability.check` span; emit `capability.required_loaded`, `capability.granted_loaded`, `capability.satisfies_check`, `capability.allow`, `capability.deny`.
- `crates/tau-runtime/src/plugin_host/mod.rs` (or sibling — confirm at execution time) — add `tool.session_open`, `tool.invoke`, `tool.session_close` spans; emit `tool.args_received`, `tool.result_received`, `tool.invoke_failed`, `tool.session_open_failed`, `tool.session_close_failed`.
- `crates/tau-observe/Cargo.toml` — declare a `test-fixtures` feature for the capture subscriber.

---

## Task 1: Test fixture — capture subscriber in `tau-observe`

**Files:**
- Create: `crates/tau-observe/src/capture.rs`
- Modify: `crates/tau-observe/src/lib.rs`
- Modify: `crates/tau-observe/Cargo.toml`

- [ ] **Step 1: Declare the `test-fixtures` feature**

In `crates/tau-observe/Cargo.toml`, the `[features]` block becomes:

```toml
[features]
default = []
test-fixtures = []
```

- [ ] **Step 2: Write the capture subscriber**

Create `crates/tau-observe/src/capture.rs`:

```rust
//! Test-only capture subscriber.
//!
//! Each [`CapturedEvent`] records the event's `target`, level, name (the
//! event's `message` field if present, otherwise its callsite name),
//! and structured fields as a `BTreeMap<String, String>` (Display of
//! each value).
//!
//! Usage:
//! ```ignore
//! let captor = Captor::new();
//! tracing::subscriber::with_default(captor.subscriber(), || {
//!     tracing::info!(foo = 1, "my.event");
//! });
//! let events = captor.events();
//! assert_eq!(events[0].name, "my.event");
//! ```

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tracing::{field::Visit, Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::Registry;
use tracing_subscriber::Layer as _;

/// One event captured by [`Captor`].
#[derive(Debug, Clone)]
pub struct CapturedEvent {
    /// `tracing::Metadata::target()` — e.g. `"tau_runtime::stream"`.
    pub target: String,
    /// Event level as a lowercase string ("info", "debug", …).
    pub level: String,
    /// The event message (the literal in the macro) or empty.
    pub name: String,
    /// Structured fields rendered through `Display`.
    pub fields: BTreeMap<String, String>,
}

/// Shared captor handle. Cheap to clone.
#[derive(Clone, Default)]
pub struct Captor {
    inner: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl Captor {
    /// New, empty captor.
    pub fn new() -> Self {
        Self::default()
    }

    /// Wrap the captor as a `tracing` subscriber. Pass to
    /// `tracing::subscriber::with_default`.
    pub fn subscriber(&self) -> impl Subscriber + Send + Sync {
        let layer = CaptureLayer { sink: self.inner.clone() };
        Registry::default().with(layer)
    }

    /// Snapshot of all captured events so far.
    pub fn events(&self) -> Vec<CapturedEvent> {
        self.inner.lock().unwrap().clone()
    }
}

struct CaptureLayer {
    sink: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl<S> Layer<S> for CaptureLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        let meta = event.metadata();
        let name = visitor
            .fields
            .remove("message")
            .unwrap_or_else(|| meta.name().to_string());
        self.sink.lock().unwrap().push(CapturedEvent {
            target: meta.target().to_string(),
            level: meta.level().to_string().to_lowercase(),
            name,
            fields: visitor.fields,
        });
    }
}

#[derive(Default)]
struct FieldVisitor {
    fields: BTreeMap<String, String>,
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.fields.insert(field.name().to_string(), format!("{value:?}"));
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.fields.insert(field.name().to_string(), value.to_string());
    }
    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields.insert(field.name().to_string(), value.to_string());
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields.insert(field.name().to_string(), value.to_string());
    }
    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields.insert(field.name().to_string(), value.to_string());
    }
}
```

- [ ] **Step 3: Gate the module and add a smoke test**

In `crates/tau-observe/src/lib.rs`:

```rust
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! Observability primitives for tau.

#[cfg(any(feature = "test-fixtures", test))]
pub mod capture;
pub mod filter;
pub mod install;
pub mod vocabulary;
```

Add a unit test directly in `capture.rs` (at the bottom):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_message_and_fields() {
        let captor = Captor::new();
        tracing::subscriber::with_default(captor.subscriber(), || {
            tracing::info!(turn_index = 3, "runtime.turn_started");
        });
        let events = captor.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "runtime.turn_started");
        assert_eq!(events[0].level, "info");
        assert_eq!(events[0].fields.get("turn_index").map(|s| s.as_str()), Some("3"));
    }
}
```

- [ ] **Step 4: Verify**

Run: `timeout 60 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-observe --features test-fixtures capture::`
Expected: 1 test passes.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-observe/src/capture.rs crates/tau-observe/src/lib.rs crates/tau-observe/Cargo.toml
git commit -m "feat(tau-observe): test-only Captor subscriber under test-fixtures feature"
```

---

## Task 2: `runtime.turn` span around the turn loop

**Files:**
- Modify: `crates/tau-runtime/src/stream.rs` (around the `while total_turns < options.max_turns` loop body, currently line ~181)
- Modify: `crates/tau-runtime/Cargo.toml` — add `tau-observe = { workspace = true }`

- [ ] **Step 1: Add dependency**

In `crates/tau-runtime/Cargo.toml`, under `[dependencies]`:

```toml
tau-observe = { workspace = true }
```

Under `[dev-dependencies]`:

```toml
tau-observe = { workspace = true, features = ["test-fixtures"] }
```

- [ ] **Step 2: Wrap the turn-loop body in `runtime.turn`**

Locate the `while total_turns < options.max_turns {` block in `stream.rs` (approximately line 181). Inside the loop body, wrap the LLM call + dispatch sequence with an `info_span!` scope:

```rust
// Before the existing turn-loop body:
let turn_span = tracing::info_span!(
    tau_observe::vocabulary::SPAN_RUNTIME_TURN,
    turn_index = total_turns + 1,
    messages_len = messages.len(),
);
let _turn_enter = turn_span.enter();

// existing runtime.turn_started + LLM call + dispatch follow here…
```

The `total_turns + 1` reflects 1-indexed turn numbering used in user-facing output; if existing logs show 0-indexed, match that — confirm at execution time by running the binary against the `tau chat` REPL fixture and checking the log line `runtime.turn_started` against current stderr output.

- [ ] **Step 3: Write a failing assertion via the capture subscriber**

Create `crates/tau-runtime/tests/tracing_vocabulary.rs`:

```rust
//! Assertions for ADR-0006 §3.9 span + event vocabulary.

use tau_observe::capture::Captor;
use tau_observe::vocabulary::*;

#[tokio::test]
async fn turn_span_fires_with_turn_index_field() {
    let captor = Captor::new();
    tracing::subscriber::with_default(captor.subscriber(), || {
        // Drive a 2-turn agent run via the MockLlmBackend fixture
        // landed in Skills-4 (see tau-ports test-fixtures feature).
        // The fixture is at tau_ports::test_fixtures::MockLlmBackend.
        run_two_turn_mock_agent_blocking();
    });
    let events = captor.events();
    // We expect at least two enter-events for runtime.turn (one per
    // turn). The Captor records events, not spans, so use the
    // runtime.turn_started event as the proxy.
    let turn_starts = events.iter().filter(|e| e.name == EV_RUNTIME_TURN_STARTED).count();
    assert_eq!(turn_starts, 2, "expected one runtime.turn_started per turn; got {turn_starts}\n{events:?}");
}

fn run_two_turn_mock_agent_blocking() {
    // Implementation: build a Runtime from MockLlmBackend that returns
    // one tool_use on turn 1 and a final text on turn 2. Call
    // Runtime::run_with_history. Mirror the setup used in
    // crates/tau-runtime/tests/orchestration_patterns.rs (Skills-4
    // fixture pattern).
    todo!("port MockLlmBackend two-turn fixture from orchestration_patterns.rs");
}
```

> **Note for implementer:** the `todo!()` body is intentional — it must be filled with the actual fixture code from `crates/tau-runtime/tests/orchestration_patterns.rs` (or wherever the Skills-4 MockLlmBackend pattern test lives). Copy the two-turn agent setup verbatim, don't reimplement.

- [ ] **Step 4: Run the test to verify it fails meaningfully**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-runtime --test tracing_vocabulary turn_span_fires`
Expected: panic from `todo!()` until the fixture is ported. After porting, the test should pass once the span is in place.

- [ ] **Step 5: Commit when green**

```bash
git add crates/tau-runtime/Cargo.toml crates/tau-runtime/src/stream.rs crates/tau-runtime/tests/tracing_vocabulary.rs
git commit -m "feat(tau-runtime): runtime.turn span around turn-loop body"
```

---

## Task 3: Missing runtime events (`completed`, `failed`, `loop_terminated`, `max_turns_reached`)

**Files:**
- Modify: `crates/tau-runtime/src/stream.rs`
- Modify: `crates/tau-runtime/tests/tracing_vocabulary.rs`

- [ ] **Step 1: Locate the three terminal branches in `stream.rs`**

Three places in the streaming pump produce a terminal `RunCompleted`:

| Branch | Approx. line | Event to emit |
|---|---|---|
| Normal stop (no tool calls, LLM done) | look near `RunCompleted` construction with `AgentStatus::Completed` | `EV_RUNTIME_COMPLETED` at `info!` |
| Loop terminated (`runtime.loop_terminated` already exists at line 283) | line 283 | keep existing `runtime.loop_terminated` |
| Max turns reached | look near `FailureKind::OutOfResources` | `EV_RUNTIME_MAX_TURNS_REACHED` at `warn!` |
| Run-level abnormal terminate (status = Failed) | look near `AgentStatus::Failed` construction | `EV_RUNTIME_FAILED` at `warn!` |

- [ ] **Step 2: Emit each event with structured fields**

Example for the completed branch (adapt to the actual code shape):

```rust
use tau_observe::vocabulary as v;

tracing::info!(
    target: "tau_runtime::stream",
    turn_index = total_turns,
    "{}",
    v::EV_RUNTIME_COMPLETED,
);
```

> **Style note:** The `"{}"` + constant form is required because `tracing::info!` requires a `&'static str` literal message. The constant is `&'static str`, so the only way to feed it as the message is via the format placeholder. Alternatively, pass it as a field: `event_name = v::EV_RUNTIME_COMPLETED, "<empty>"` — pick one style and use it consistently throughout the kernel. The plan recommends the `"{}"` + constant form because the message text in stderr stays human-readable.

Repeat for `max_turns_reached` (level: `warn`, fields: `turn_index`, `max_turns`) and `failed` (level: `warn`, fields: `turn_index`, `failure_kind`, `detail`).

- [ ] **Step 3: Extend the integration test**

In `crates/tau-runtime/tests/tracing_vocabulary.rs`, add:

```rust
#[tokio::test]
async fn completed_event_fires_on_normal_terminate() {
    let captor = Captor::new();
    tracing::subscriber::with_default(captor.subscriber(), || {
        run_one_turn_mock_agent_blocking(); // returns final text on turn 1
    });
    let names: Vec<_> = captor.events().iter().map(|e| e.name.clone()).collect();
    assert!(names.iter().any(|n| n == EV_RUNTIME_COMPLETED), "missing runtime.completed: {names:?}");
}

#[tokio::test]
async fn max_turns_event_fires_when_loop_exhausted() {
    let captor = Captor::new();
    tracing::subscriber::with_default(captor.subscriber(), || {
        run_infinite_tool_mock_agent_blocking_with_max_turns(2);
    });
    let names: Vec<_> = captor.events().iter().map(|e| e.name.clone()).collect();
    assert!(names.iter().any(|n| n == EV_RUNTIME_MAX_TURNS_REACHED), "missing runtime.max_turns_reached: {names:?}");
}
```

The `run_*_mock_agent_blocking` helpers follow the same fixture pattern as Task 2.

- [ ] **Step 4: Run the tests, confirm pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-runtime --test tracing_vocabulary`
Expected: all asserts green.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-runtime/src/stream.rs crates/tau-runtime/tests/tracing_vocabulary.rs
git commit -m "feat(tau-runtime): emit runtime.{completed,failed,max_turns_reached}"
```

---

## Task 4: LLM-event vocabulary (`request_built`, `response_received`, `token_usage`, `stop_reason`, `tool_use_emitted`)

**Files:**
- Modify: `crates/tau-runtime/src/stream.rs` (the `llm.complete` span body, around line 196)
- Modify: `crates/tau-runtime/tests/tracing_vocabulary.rs`

- [ ] **Step 1: Emit each event at the right point in the LLM call sequence**

Inside the `info_span!("llm.complete")` body, in order:

1. Immediately before calling the backend — emit `llm.request_built` at `debug!` with fields `messages_len`, `tool_specs_len`, `max_turns_remaining`.
2. Immediately after the backend returns — emit `llm.response_received` at `debug!` with fields `text_len` (or `text_blocks_count`), `tool_uses_count`.
3. If the response carries token usage — emit `llm.token_usage` at `info!` with fields `input_tokens`, `output_tokens`, `total_tokens`.
4. If the response carries a `stop_reason` — emit `llm.stop_reason` at `debug!` with field `stop_reason` (Display of the enum).
5. For each `ToolUse` block in the response — emit `llm.tool_use_emitted` at `debug!` with fields `tool_name`, `tool_use_id`.

- [ ] **Step 2: Add tests for each event**

```rust
#[tokio::test]
async fn llm_request_and_response_events_fire() {
    let captor = Captor::new();
    tracing::subscriber::with_default(captor.subscriber(), || {
        run_one_turn_mock_agent_blocking();
    });
    let names: Vec<_> = captor.events().iter().map(|e| e.name.clone()).collect();
    for expected in [EV_LLM_REQUEST_BUILT, EV_LLM_RESPONSE_RECEIVED] {
        assert!(names.iter().any(|n| n == expected), "missing {expected}: {names:?}");
    }
}

#[tokio::test]
async fn llm_tool_use_emitted_fires_per_tool_block() {
    let captor = Captor::new();
    tracing::subscriber::with_default(captor.subscriber(), || {
        run_two_tool_use_mock_agent_blocking();
    });
    let tool_use_count = captor.events().iter().filter(|e| e.name == EV_LLM_TOOL_USE_EMITTED).count();
    assert_eq!(tool_use_count, 2, "expected 2 llm.tool_use_emitted events, got {tool_use_count}");
}
```

- [ ] **Step 3: Run + commit**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-runtime --test tracing_vocabulary`
Expected: green.

```bash
git add crates/tau-runtime/src/stream.rs crates/tau-runtime/tests/tracing_vocabulary.rs
git commit -m "feat(tau-runtime): emit llm.{request_built,response_received,token_usage,stop_reason,tool_use_emitted}"
```

---

## Task 5: `dispatch.tool` span + `dispatch.tool_resolved` event

**Files:**
- Modify: tool-dispatch code in `crates/tau-runtime/src/`. Locate via:

```bash
grep -rn "fn dispatch_tool\|dispatch_one_tool_use\|fn invoke_tool" crates/tau-runtime/src
```

- Modify: `crates/tau-runtime/tests/tracing_vocabulary.rs`

- [ ] **Step 1: Wrap the dispatch fn in `#[instrument]`**

Convert the dispatch function:

```rust
use tau_observe::vocabulary as v;

#[tracing::instrument(
    name = v::SPAN_DISPATCH_TOOL,
    skip_all,
    fields(tool_name = %tool_name, tool_use_id = %tool_use_id),
)]
async fn dispatch_one_tool_use(/* … */) -> Result<…> {
    // existing body
}
```

- [ ] **Step 2: Emit `dispatch.tool_resolved` after the registry lookup succeeds**

```rust
tracing::debug!(
    target: "tau_runtime::dispatch",
    tool_name = %tool_name,
    plugin_id = %plugin.id(),
    "{}",
    v::EV_DISPATCH_TOOL_RESOLVED,
);
```

- [ ] **Step 3: Test**

```rust
#[tokio::test]
async fn dispatch_tool_resolved_fires_for_each_tool_call() {
    let captor = Captor::new();
    tracing::subscriber::with_default(captor.subscriber(), || {
        run_one_tool_call_mock_agent_blocking();
    });
    let count = captor.events().iter().filter(|e| e.name == EV_DISPATCH_TOOL_RESOLVED).count();
    assert_eq!(count, 1, "expected 1 dispatch.tool_resolved, got {count}");
}
```

- [ ] **Step 4: Run + commit**

```bash
git add crates/tau-runtime/src/<dispatch-file>.rs crates/tau-runtime/tests/tracing_vocabulary.rs
git commit -m "feat(tau-runtime): dispatch.tool span + dispatch.tool_resolved event"
```

---

## Task 6: `capability.check` span + 5 capability events

**Files:**
- Modify: capability-check code in `crates/tau-runtime/src/`. Locate via:

```bash
grep -rn "fn check_capabilit\|capability_check\|satisfies_check" crates/tau-runtime/src
```

- Modify: `crates/tau-runtime/tests/tracing_vocabulary.rs`

- [ ] **Step 1: Wrap the check fn in `#[instrument]` named `capability.check`**

```rust
#[tracing::instrument(
    name = tau_observe::vocabulary::SPAN_CAPABILITY_CHECK,
    skip_all,
    fields(tool_name = %tool_name),
)]
fn check_capabilities(/* … */) -> Result<…> {
    /* existing body */
}
```

- [ ] **Step 2: Emit the 5 events at the matching points in the body**

| Point | Level | Event | Fields |
|---|---|---|---|
| After loading the tool's required caps | `debug!` | `EV_CAPABILITY_REQUIRED_LOADED` | `required_count` |
| After loading the agent's granted caps | `debug!` | `EV_CAPABILITY_GRANTED_LOADED` | `granted_count` |
| After computing the satisfies-check verdict | `debug!` | `EV_CAPABILITY_SATISFIES_CHECK` | `satisfied: bool` |
| On the allow branch | `info!` | `EV_CAPABILITY_ALLOW` | `tool_name` |
| On the deny branch | `warn!` | `EV_CAPABILITY_DENY` | `tool_name`, `reason` |

- [ ] **Step 3: Test the deny branch (the highest-stakes one)**

```rust
#[tokio::test]
async fn capability_deny_fires_when_check_fails() {
    let captor = Captor::new();
    tracing::subscriber::with_default(captor.subscriber(), || {
        run_one_tool_call_mock_agent_with_insufficient_caps_blocking();
    });
    let names: Vec<_> = captor.events().iter().map(|e| e.name.clone()).collect();
    assert!(names.iter().any(|n| n == EV_CAPABILITY_DENY), "missing capability.deny: {names:?}");
}
```

- [ ] **Step 4: Run + commit**

```bash
git add crates/tau-runtime/src/<capability-file>.rs crates/tau-runtime/tests/tracing_vocabulary.rs
git commit -m "feat(tau-runtime): capability.check span + 5 capability events"
```

---

## Task 7: `tool.session_open` / `tool.invoke` / `tool.session_close` spans + 5 tool events

**Files:**
- Modify: `crates/tau-runtime/src/plugin_host/mod.rs` (or the file housing the `Open`/`Invoke`/`Close` request senders — locate via `grep -rn "fn send_open\|fn send_invoke\|fn send_close" crates/tau-runtime/src`)
- Modify: `crates/tau-runtime/tests/tracing_vocabulary.rs`

- [ ] **Step 1: Add `#[instrument]` to each of the three senders**

```rust
#[tracing::instrument(
    name = tau_observe::vocabulary::SPAN_TOOL_SESSION_OPEN,
    skip_all,
    fields(tool_name = %tool_name, session_id = %session_id),
)]
async fn send_open(/* … */) -> Result<…> { /* body */ }

#[tracing::instrument(
    name = tau_observe::vocabulary::SPAN_TOOL_INVOKE,
    skip_all,
    fields(tool_name = %tool_name, session_id = %session_id, msgid = %msgid),
)]
async fn send_invoke(/* … */) -> Result<…> { /* body */ }

#[tracing::instrument(
    name = tau_observe::vocabulary::SPAN_TOOL_SESSION_CLOSE,
    skip_all,
    fields(tool_name = %tool_name, session_id = %session_id),
)]
async fn send_close(/* … */) -> Result<…> { /* body */ }
```

- [ ] **Step 2: Emit the 5 events at the matching points in the bodies**

| Where | Level | Event | Fields |
|---|---|---|---|
| `send_invoke`, before forwarding args | `debug!` | `EV_TOOL_ARGS_RECEIVED` | `args_size_bytes` (do NOT log payload — sub-project C adds preview helpers) |
| `send_invoke`, after receiving result | `debug!` | `EV_TOOL_RESULT_RECEIVED` | `result_size_bytes` |
| `send_invoke`, Err branch | `warn!` | `EV_TOOL_INVOKE_FAILED` | `tool_name`, `err = %e` |
| `send_open`, Err branch | `warn!` | `EV_TOOL_SESSION_OPEN_FAILED` | `tool_name`, `err = %e` |
| `send_close`, Err branch | `warn!` | `EV_TOOL_SESSION_CLOSE_FAILED` | `tool_name`, `err = %e` |

- [ ] **Step 3: Test the happy path (open → invoke → close)**

```rust
#[tokio::test]
async fn tool_session_events_fire_for_full_lifecycle() {
    let captor = Captor::new();
    tracing::subscriber::with_default(captor.subscriber(), || {
        run_one_tool_call_mock_agent_blocking();
    });
    let names: Vec<_> = captor.events().iter().map(|e| e.name.clone()).collect();
    for expected in [EV_TOOL_ARGS_RECEIVED, EV_TOOL_RESULT_RECEIVED] {
        assert!(names.iter().any(|n| n == expected), "missing {expected}: {names:?}");
    }
}
```

- [ ] **Step 4: Run + commit**

```bash
git add crates/tau-runtime/src/plugin_host/<file>.rs crates/tau-runtime/tests/tracing_vocabulary.rs
git commit -m "feat(tau-runtime): tool.session_* spans + 5 tool events"
```

---

## Task 8: `message.added` event

**Files:**
- Modify: `crates/tau-runtime/src/stream.rs` (every place that calls `messages.push(message)`)
- Modify: `crates/tau-runtime/tests/tracing_vocabulary.rs`

- [ ] **Step 1: Emit `message.added` at each push site**

There are typically three: assistant text append (post-LLM-call), tool-result append (post-dispatch), and the initial-message setup.

```rust
tracing::debug!(
    target: "tau_runtime::stream",
    role = %message.role,
    "{}",
    tau_observe::vocabulary::EV_MESSAGE_ADDED,
);
messages.push(message);
```

- [ ] **Step 2: Test count matches messages_pushed**

```rust
#[tokio::test]
async fn message_added_count_matches_pushed_messages() {
    let captor = Captor::new();
    tracing::subscriber::with_default(captor.subscriber(), || {
        run_one_tool_call_mock_agent_blocking();
    });
    let count = captor.events().iter().filter(|e| e.name == EV_MESSAGE_ADDED).count();
    // Expected: 1 initial user + 1 assistant + 1 tool-result + 1 final assistant = 4.
    assert_eq!(count, 4, "expected 4 message.added events, got {count}");
}
```

- [ ] **Step 3: Run + commit**

```bash
git add crates/tau-runtime/src/stream.rs crates/tau-runtime/tests/tracing_vocabulary.rs
git commit -m "feat(tau-runtime): emit message.added at every history push"
```

---

## Task 9: Sweep — migrate every remaining stringly-typed name to a `vocabulary` constant

**Files:** every `tracing::*!` call in `crates/tau-runtime/src/`. Locate via:

```bash
grep -rn 'tracing::\(info\|debug\|warn\|error\|trace\)!' crates/tau-runtime/src
```

- [ ] **Step 1: For each existing emission whose name appears in `tau_observe::vocabulary`, replace the string literal with the constant**

Example before:

```rust
tracing::info!("runtime.run_started");
```

After:

```rust
tracing::info!("{}", tau_observe::vocabulary::EV_RUNTIME_RUN_STARTED);
```

This makes a future event rename a one-line change in `vocabulary.rs` instead of a global grep.

- [ ] **Step 2: Verify nothing changed semantically**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime`
Expected: same green count as before this task.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-runtime/src
git commit -m "refactor(tau-runtime): replace literal event names with tau_observe::vocabulary constants"
```

---

## Task 10: Final verification + push

- [ ] **Step 1: clippy + nextest matrix**

```bash
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-runtime -- -D warnings
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-observe -- -D warnings
timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-observe
```

Expected: all green.

- [ ] **Step 2: Pre-push gate + push**

```bash
timeout 1800 lefthook run pre-push
scripts/agent-push.sh -u origin HEAD
```

- [ ] **Step 3: PR**

Title: `feat(tau-runtime): complete §3.9 span + event vocabulary (Sub-project B)`. Body references the design doc.

---

## Spec coverage check

- Spec sub-project B "Add the missing spans" → Tasks 2, 5, 6, 7.
- Spec sub-project B "~15 missing events" → Tasks 3, 4, 5, 6, 7, 8 cumulatively cover all 15.
- Spec sub-project B "All span names import from `tau_observe::vocabulary`" → Task 9 sweep.
- Spec testing "for every new span, an integration test … on a tracing-test-style capture subscriber" → Task 1 builds the captor, Tasks 2–8 each add asserts.
- Spec testing "for every new event, a focused unit test that asserts the event fires with the expected fields and level" → covered by the per-task tests; field-level assertions live in the field maps from `Captor::events()`.

**Out of scope, deferred to Sub-project C:** redaction of arg/payload bodies emitted by the new events. The Task 7 events deliberately log only `*_size_bytes`, not contents. C migrates those to `preview_json` and adds TRACE-level full-content counterparts.
