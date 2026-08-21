# Execution Trace TUI (M1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a live/post-mortem terminal waterfall (`tau run --tui`, `tau trace <run_id>`) that renders a run's agent turns, tool calls, and capability verdicts from `.tau/runs/<id>.jsonl`.

**Architecture:** A new pure `tau-trace` crate turns a stream of `TraceEvent`s into a renderable `TraceModel` (span tree + time-axis math), with zero runtime/TUI deps. A thin ratatui frontend in `tau-cli` tail-follows the jsonl, feeds the model, and draws it — so live and post-mortem share one code path. One instrumentation change adds the missing `ToolCall` trace-event producer + capability verdict.

**Tech Stack:** Rust, `ratatui` + `crossterm` (new workspace deps), `serde_json`, `chrono` (`DateTime<Utc>`), `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-08-21-execution-trace-tui-design.md`

## Global Constraints

- **Cargo discipline (from CLAUDE.md):** every cargo command uses
  `timeout <n> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <cmd> -p <crate>`.
  Never bare `cargo`, never `--workspace`, always `-p`. Prefer `cargo nextest run`; `cargo test --doc` for doctests.
- **`forbid(unsafe_code)`** in the new `tau-trace` crate.
- **Errors:** `thiserror` at the `tau-trace` public boundary; `anyhow` inside `tau-cli`.
- **`tau-ports` is a public-API crate:** adding a field to `TraceEventKind::ToolCall` is a breaking change → bump `tau-ports` version by a **minor** (0.x semver) in its `Cargo.toml`.
- **Default output is unchanged:** the existing npm-style line printer stays the default for `tau run`. The TUI is strictly opt-in (`--tui`). `--tui` is rejected together with `--json` and when stdout is not a TTY.
- **MSRV 1.91**, edition per workspace. `chrono` with `DateTime<Utc>` (already a `tau-ports` dep via `TraceEvent.ts`).
- **Commits (from CLAUDE.md — overrides the `git commit` lines in each task):** agent commits must NEVER trigger the lefthook pre-commit gate (it is slow and corrupts git identity to `Test User`). Always commit with:
  `git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "<message>"`
  Stage only the task's own files; the controller owns the `docs/` spec+plan and `.gitignore`. CI + the opt-in deep gate are the real gates.

---

### Task 1: `CapabilityVerdict` + enrich `TraceEventKind::ToolCall`

**Files:**
- Modify: `crates/tau-ports/src/orchestration.rs` (enum near line 145 `ToolCall`, and add `CapabilityVerdict` nearby)
- Modify: `crates/tau-ports/Cargo.toml` (version minor bump)
- Test: inline `#[cfg(test)]` in `crates/tau-ports/src/orchestration.rs`

**Interfaces:**
- Produces:
  ```rust
  pub enum CapabilityVerdict {
      Allow,
      Clamp { to: String },
      Drop { reason: String },
  }
  // TraceEventKind::ToolCall now:
  ToolCall { tool_name: String, duration_ms: u64, status: String, capability: Option<CapabilityVerdict> }
  ```

- [ ] **Step 1: Write the failing test** (append to the existing `#[cfg(test)] mod tests` in `orchestration.rs`)

```rust
#[cfg(feature = "serde")]
#[test]
fn tool_call_serdes_capability_verdict() {
    let evt = TraceEventKind::ToolCall {
        tool_name: "net.http".into(),
        duration_ms: 380,
        status: "ok".into(),
        capability: Some(CapabilityVerdict::Clamp { to: "api.example.com".into() }),
    };
    let json = serde_json::to_string(&evt).unwrap();
    assert!(json.contains(r#""kind":"tool_call""#));
    assert!(json.contains(r#""capability""#));
    let back: TraceEventKind = serde_json::from_str(&json).unwrap();
    assert_eq!(back, evt);
}

#[cfg(feature = "serde")]
#[test]
fn tool_call_capability_absent_deserializes_none() {
    // Forward-compat: an older run without the field parses as None.
    let json = r#"{"kind":"tool_call","tool_name":"fs.read","duration_ms":2,"status":"ok"}"#;
    let back: TraceEventKind = serde_json::from_str(json).unwrap();
    match back {
        TraceEventKind::ToolCall { capability, .. } => assert!(capability.is_none()),
        _ => panic!("wrong variant"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ports capability`
Expected: FAIL — `capability` field / `CapabilityVerdict` unknown.

- [ ] **Step 3: Add the type and field**

Add above `TraceEventKind`:
```rust
/// Governance decision recorded for a capability-gated tool call.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "verdict", rename_all = "snake_case"))]
pub enum CapabilityVerdict {
    /// Call allowed as requested.
    Allow,
    /// Call allowed after meet-clamping to a narrower authority.
    Clamp {
        /// Human-readable clamped target (e.g. the allowed host).
        to: String,
    },
    /// Call denied fail-closed.
    Drop {
        /// Why it was dropped.
        reason: String,
    },
}
```
Change the `ToolCall` variant to add, with a default on missing:
```rust
    ToolCall {
        /// Tool name.
        tool_name: String,
        /// Duration in ms.
        duration_ms: u64,
        /// Status (`"ok"`, `"error"`).
        status: String,
        /// Capability decision, if this tool was capability-gated.
        /// `None` for un-gated tools or traces predating this field.
        #[cfg_attr(feature = "serde", serde(default))]
        capability: Option<CapabilityVerdict>,
    },
```
Then bump `version` in `crates/tau-ports/Cargo.toml` by one minor.

- [ ] **Step 4: Run tests to verify they pass**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ports capability`
Expected: PASS (both tests).

- [ ] **Step 5: Fix the one existing consumer that constructs/matches `ToolCall`**

`crates/tau-cli/src/cmd/output_orchestration.rs:87` matches `ToolCall { .. }`. If it binds fields explicitly, add `capability: _`. Build to confirm:
Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-cli`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-ports/src/orchestration.rs crates/tau-ports/Cargo.toml crates/tau-cli/src/cmd/output_orchestration.rs
git commit -m "feat(tau-ports): add CapabilityVerdict to TraceEventKind::ToolCall"
```

---

### Task 2: Emit `ToolCall` trace events at the dispatch site

**Files:**
- Modify: the tool-dispatch code path in `crates/tau-runtime-core/src/` wrapped by the `tool.invoke` / `dispatch.tool` spans (locate — see Step 1)
- Test: an integration test under `crates/tau-runtime-core/tests/` (or extend an existing orchestration test) asserting a `ToolCall` event reaches the trace stream

**Interfaces:**
- Consumes: `CapabilityVerdict`, enriched `TraceEventKind::ToolCall` (Task 1).
- Produces: a `TraceEvent { kind: TraceEventKind::ToolCall { .. } }` emitted via the same `s.trace.emit(...)` channel used for `Turn` in `run.rs:474`.

- [ ] **Step 1: Locate the dispatch + verdict site**

Run: `grep -rn "tool.invoke\|dispatch.tool\|EV_CAPABILITY_ALLOW\|capability.allow" crates/tau-runtime-core/src crates/tau-observe/src`
Identify (a) where a tool call completes with a duration and ok/error status, and (b) where the capability verdict is computed for the `capability.*` tracing event. These are the values to thread into the new emission.

- [ ] **Step 2: Write the failing test**

Add to `crates/tau-runtime-core/tests/` a test that runs a minimal single-tool pipeline through the existing test harness (mirror an existing orchestration/stream test for setup) and collects emitted `TraceEvent`s via a capturing trace subscriber:
```rust
#[test]
fn dispatch_emits_toolcall_trace_event_with_verdict() {
    let events = run_fixture_and_collect_trace("fixtures/single_tool.<ext>");
    let tool = events.iter().find_map(|e| match &e.kind {
        TraceEventKind::ToolCall { tool_name, duration_ms, capability, .. } =>
            Some((tool_name.clone(), *duration_ms, capability.clone())),
        _ => None,
    }).expect("a ToolCall trace event must be emitted");
    assert_eq!(tool.0, "echo");            // fixture's tool
    assert!(tool.2.is_some());             // verdict populated for a gated tool
}
```
(Use whatever capture helper the existing trace tests use; if none, add a `Vec`-backed `TraceSubscriber` in the test.)

- [ ] **Step 3: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core dispatch_emits_toolcall`
Expected: FAIL — no `ToolCall` event found.

- [ ] **Step 4: Emit the event at dispatch completion**

At the located site, after the tool result and duration are known, alongside the existing span/`capability.*` emission:
```rust
s.trace.emit(tau_ports::TraceEvent {
    id: /* same id scheme as Turn emission */,
    ts: clock.now(),
    run_id: run_id.clone(),
    agent_id: Some(agent_id.clone()),
    kind: tau_ports::TraceEventKind::ToolCall {
        tool_name: tool_name.to_string(),
        duration_ms: elapsed_ms,
        status: if ok { "ok".into() } else { "error".into() },
        capability: Some(verdict), // the CapabilityVerdict computed for capability.*
    },
});
```
Map the existing verdict representation to `CapabilityVerdict::{Allow,Clamp,Drop}`. If a tool is not capability-gated, pass `capability: None`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core dispatch_emits_toolcall`
Expected: PASS. Then run the crate's full suite to catch regressions:
Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-runtime-core
git commit -m "feat(runtime): emit ToolCall trace events with capability verdict"
```

---

### Task 3: `tau-trace` crate — `Span`, `SpanKind`, `TraceModel`, time-axis

**Files:**
- Create: `crates/tau-trace/Cargo.toml`, `crates/tau-trace/src/lib.rs`, `crates/tau-trace/src/model.rs`
- Modify: root `Cargo.toml` (add `"crates/tau-trace"` to `members`)
- Test: inline `#[cfg(test)]` in `model.rs`

**Interfaces:**
- Consumes: `tau_ports::{TraceEvent, TraceEventKind, CapabilityVerdict}`.
- Produces:
  ```rust
  pub enum SpanKind { Agent, Tool, Reasoning, Branch, Parallel, Loop, Suspend }
  pub enum SpanStatus { Running, Ok, Failed }
  pub struct Span {
      pub id: usize, pub kind: SpanKind, pub label: String,
      pub start: DateTime<Utc>, pub end: Option<DateTime<Utc>>,
      pub tokens: Option<u64>, pub capability: Option<CapabilityVerdict>,
      pub parent: Option<usize>, pub status: SpanStatus,
  }
  pub struct TraceModel { /* private */ }
  impl TraceModel {
      pub fn new() -> Self;
      pub fn apply(&mut self, event: &TraceEvent);
      pub fn spans(&self) -> &[Span];
      pub fn window(&self) -> Option<(DateTime<Utc>, DateTime<Utc>)>; // min start .. max end|now
      pub fn bar(&self, span: &Span, width_cols: u16) -> (u16, u16);  // (offset_cols, len_cols)
  }
  ```

- [ ] **Step 1: Scaffold the crate**

`crates/tau-trace/Cargo.toml`:
```toml
[package]
name = "tau-trace"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true

[dependencies]
tau-ports = { path = "../tau-ports", features = ["serde"] }
chrono = { workspace = true }
thiserror = { workspace = true }

[lints]
workspace = true
```
`crates/tau-trace/src/lib.rs`:
```rust
#![forbid(unsafe_code)]
//! Renderable trace model over `tau_ports::TraceEvent`. No runtime or TUI deps.
mod model;
pub use model::{Span, SpanKind, SpanStatus, TraceModel};
```
Add `"crates/tau-trace",` to root `Cargo.toml` `members`.

- [ ] **Step 2: Write the failing tests** (`model.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use tau_ports::{TraceEvent, TraceEventKind, CapabilityVerdict};

    fn ev(secs: i64, kind: TraceEventKind) -> TraceEvent {
        TraceEvent { id: "x".into(), ts: Utc.timestamp_opt(secs, 0).unwrap(),
                     run_id: Default::default(), agent_id: None, kind }
    }

    #[test]
    fn tool_call_becomes_span_with_derived_start_and_verdict() {
        let mut m = TraceModel::new();
        m.apply(&ev(100, TraceEventKind::ToolCall {
            tool_name: "net.http".into(), duration_ms: 2000, status: "ok".into(),
            capability: Some(CapabilityVerdict::Drop { reason: "egress".into() }) }));
        let s = &m.spans()[0];
        assert!(matches!(s.kind, SpanKind::Tool));
        assert_eq!(s.end.unwrap(), Utc.timestamp_opt(100, 0).unwrap());
        // start = ts - duration
        assert_eq!(s.start, Utc.timestamp_opt(98, 0).unwrap());
        assert!(matches!(s.status, SpanStatus::Ok));
        assert!(s.capability.is_some());
    }

    #[test]
    fn window_spans_min_start_to_max_end() {
        let mut m = TraceModel::new();
        m.apply(&ev(100, TraceEventKind::Turn { agent_id: Default::default(),
            turn_index: 0, duration_ms: 1000, tokens: 10 }));
        m.apply(&ev(105, TraceEventKind::ToolCall { tool_name: "t".into(),
            duration_ms: 500, status: "ok".into(), capability: None }));
        let (lo, hi) = m.window().unwrap();
        assert_eq!(lo, Utc.timestamp_opt(99, 0).unwrap());   // 100-1s
        assert_eq!(hi, Utc.timestamp_opt(105, 0).unwrap());  // 105 end
    }

    #[test]
    fn bar_maps_span_onto_column_width() {
        let mut m = TraceModel::new();
        m.apply(&ev(100, TraceEventKind::Turn { agent_id: Default::default(),
            turn_index: 0, duration_ms: 0, tokens: 0 }));          // point at t=100
        m.apply(&ev(110, TraceEventKind::Turn { agent_id: Default::default(),
            turn_index: 1, duration_ms: 0, tokens: 0 }));          // point at t=110
        let last = m.spans()[1].clone();
        let (off, _len) = m.bar(&last, 100);
        assert_eq!(off, 100); // t=110 is the right edge of a 100-col window [100,110]
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-trace`
Expected: FAIL — types not defined.

- [ ] **Step 4: Implement `model.rs`**

Implement `SpanKind`/`SpanStatus`/`Span`, and `TraceModel` holding `Vec<Span>` + agent-id→span-index map for parent resolution. `apply`:
- `Turn` → `Agent` span, `end = ts`, `start = ts - duration_ms`, tokens set.
- `ToolCall` → `Tool` span, same start/end derivation, `status` string → `SpanStatus::{Ok,Failed}`, carry `capability`.
- `Spawn` → record child agent-id → parent mapping (no span, or a zero-width Agent marker).
- `Completion`/`Abort`/budget/orphan → status/label only (no bar) for M1.
- Unknown/other → ignored (forward-compat).
`window()` = (min `start`, max(`end`.unwrap_or(now-of-last-event))). For a purely point-in-time model, use the max `ts` seen as the upper bound when a span is still running.
`bar(span, width)` = linear map of `[start,end]` onto `[0,width]` using `window()`; clamp to `[0,width]`; a zero-length span renders `len = 1`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-trace`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-trace Cargo.toml
git commit -m "feat(tau-trace): renderable TraceModel with span tree + time-axis"
```

---

### Task 4: jsonl ingestion (line → `TraceEvent`, malformed-tolerant)

**Files:**
- Create: `crates/tau-trace/src/ingest.rs`
- Modify: `crates/tau-trace/src/lib.rs` (`mod ingest; pub use ingest::parse_line;`)
- Test: inline `#[cfg(test)]` in `ingest.rs`

**Interfaces:**
- Produces:
  ```rust
  /// Parse one `.tau/runs/<id>.jsonl` line into a TraceEvent.
  /// Returns Ok(None) for a blank/partial line (tail may read mid-write).
  pub fn parse_line(line: &str) -> Result<Option<TraceEvent>, IngestError>;
  #[derive(thiserror::Error, Debug)] pub enum IngestError { #[error("bad trace line: {0}")] Json(String) }
  ```

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_a_turn_line() {
        let line = r#"{"id":"1","ts":"2026-08-21T00:00:00Z","run_id":"r","agent_id":null,"kind":"turn","agent_id_x":null,"turn_index":0,"duration_ms":100,"tokens":5}"#;
        // NOTE: adjust field names to the real serialized TraceEvent shape captured in Step 0.
        let evt = parse_line(line).unwrap().unwrap();
        assert!(matches!(evt.kind, tau_ports::TraceEventKind::Turn { .. }));
    }
    #[test]
    fn blank_line_is_none() { assert!(parse_line("   ").unwrap().is_none()); }
    #[test]
    fn garbage_is_err_not_panic() { assert!(parse_line("{not json").is_err()); }
}
```
> Step 0 (do first): capture a real line — run any example with the existing runner and `cat` one line of `.tau/runs/<id>.jsonl` to get the exact serialized shape; paste it verbatim into `parses_a_turn_line`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-trace ingest`
Expected: FAIL.

- [ ] **Step 3: Implement `parse_line`**

```rust
use tau_ports::TraceEvent;
pub fn parse_line(line: &str) -> Result<Option<TraceEvent>, IngestError> {
    let t = line.trim();
    if t.is_empty() { return Ok(None); }
    serde_json::from_str::<TraceEvent>(t).map(Some).map_err(|e| IngestError::Json(e.to_string()))
}
```
Add `serde_json = { workspace = true }` to `tau-trace/Cargo.toml`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-trace ingest`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-trace
git commit -m "feat(tau-trace): malformed-tolerant jsonl line ingestion"
```

---

### Task 5: TUI renderer (ratatui, `TestBackend` golden)

**Files:**
- Create: `crates/tau-cli/src/tui/mod.rs`, `crates/tau-cli/src/tui/render.rs`
- Modify: `crates/tau-cli/Cargo.toml` (add `ratatui`, `crossterm` to workspace + here), `crates/tau-cli/src/lib.rs` (`mod tui;`)
- Test: inline `#[cfg(test)]` in `render.rs` using `ratatui::backend::TestBackend`

**Interfaces:**
- Consumes: `tau_trace::TraceModel`.
- Produces: `pub fn draw(frame: &mut Frame, model: &TraceModel, ui: &UiState)` where `UiState { selected: usize, filter: Filter, search: String, scroll: u16 }` and `pub enum Filter { All, Errors, Tools, Reasoning }`.

- [ ] **Step 1: Add deps**

In root `Cargo.toml` `[workspace.dependencies]`: `ratatui = "0.29"`, `crossterm = "0.28"` (pin to current). In `tau-cli/Cargo.toml`: `ratatui = { workspace = true }`, `crossterm = { workspace = true }`.

- [ ] **Step 2: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};
    #[test]
    fn renders_tool_row_with_capability_badge() {
        let mut model = tau_trace::TraceModel::new();
        // feed one Tool span with a Drop verdict (build a TraceEvent inline)
        // ... apply event ...
        let mut term = Terminal::new(TestBackend::new(120, 10)).unwrap();
        let ui = UiState { selected: 0, filter: Filter::All, search: String::new(), scroll: 0 };
        term.draw(|f| draw(f, &model, &ui)).unwrap();
        let buf = term.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("drop"));      // capability badge rendered
        assert!(text.contains("net.http"));  // tool label rendered
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli renders_tool_row`
Expected: FAIL.

- [ ] **Step 4: Implement `draw`**

Lay out three regions: toolbar (run id + filter chips + search), waterfall table (columns `Name | Tokens | Dur | Cap` + a bar cell built from `model.bar(span, bar_width)` using block glyphs `▐▊░`), detail pane for `ui.selected`. Color the `Cap` cell by verdict (green/amber/red). Apply `ui.filter` to the visible span set. No terminal I/O in `draw` — pure frame rendering (keeps it `TestBackend`-testable).

- [ ] **Step 5: Run test to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli renders_tool_row`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-cli/src/tui Cargo.toml crates/tau-cli/Cargo.toml
git commit -m "feat(tau-cli): ratatui waterfall renderer"
```

---

### Task 6: Event loop — keys, filter, search, tail-follow

**Files:**
- Create: `crates/tau-cli/src/tui/app.rs` (terminal setup/teardown, input + file-tail loop)
- Modify: `crates/tau-cli/src/tui/mod.rs`
- Test: inline unit test for the pure input-reducer (`apply_key`), no real terminal

**Interfaces:**
- Produces:
  ```rust
  pub struct App { model: TraceModel, ui: UiState }
  impl App {
      pub fn apply_key(&mut self, key: KeyCode) -> Loop; // Loop::{Continue, Quit}
      pub fn ingest_line(&mut self, line: &str);          // parse + model.apply
  }
  pub fn run_tui(source: TraceSource) -> anyhow::Result<()>; // owns raw-mode + tail loop
  pub enum TraceSource { File(PathBuf), Live(mpsc::Receiver<TraceEvent>) }
  ```

- [ ] **Step 1: Write the failing test** (pure reducer)

```rust
#[test]
fn keys_navigate_and_quit() {
    let mut app = App::with_model(three_span_model());
    app.apply_key(KeyCode::Down); assert_eq!(app.selected(), 1);
    app.apply_key(KeyCode::Char('f')); // cycle filter All->Errors
    assert!(matches!(app.filter(), Filter::Errors));
    assert!(matches!(app.apply_key(KeyCode::Char('q')), Loop::Quit));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli keys_navigate`
Expected: FAIL.

- [ ] **Step 3: Implement**

`apply_key`: `Down/Up` move `selected` (clamped to filtered len), `f` cycles `Filter`, `/` enters search-input mode appending to `ui.search`, `Enter` toggles detail/expand, `q`/`Esc` → `Loop::Quit`. `ingest_line` = `parse_line` → `model.apply`. `run_tui`: enable raw mode + alternate screen via crossterm; for `TraceSource::File`, open, read to EOF, then poll for appended lines (200ms) while also polling `crossterm::event`; for `Live`, `try_recv` the channel. Auto-scroll to newest unless the user scrolled up. Always restore the terminal on exit (RAII guard).

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli keys_navigate`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-cli/src/tui
git commit -m "feat(tau-cli): TUI event loop with tail-follow, filter, search"
```

---

### Task 7: CLI wiring — `tau run --tui` and `tau trace <run_id> | --last`

**Files:**
- Modify: `crates/tau-cli/src/cli.rs` (`RunArgs` add `--tui`; add `Command::Trace(TraceArgs)`)
- Modify: `crates/tau-cli/src/lib.rs` (dispatch `Command::Trace`; branch `run` to TUI when `--tui`)
- Modify: `crates/tau-cli/src/cmd/run.rs` (when `--tui`, feed the live MPSC into `run_tui(TraceSource::Live(rx))` instead of `run_printer`)
- Create: `crates/tau-cli/src/cmd/trace.rs` (resolve run id / `--last` under `./.tau/runs`, call `run_tui(TraceSource::File(path))`)
- Test: `crates/tau-cli/tests/` — arg-guard test (`--tui` + `--json` rejected); `--last` resolution unit test

**Interfaces:**
- Consumes: `tui::run_tui`, `tui::TraceSource` (Task 6); existing `drive_with_live_trace` MPSC `rx` (`run.rs:344`).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn tui_conflicts_with_json() {
    let err = try_parse(&["tau","run","app.tau","--tui","--json"]).unwrap_err();
    assert!(err.to_string().contains("--tui"));
}
#[test]
fn last_resolves_newest_run_file() {
    let dir = tempdir_with_runs(&["01A.jsonl","01B.jsonl"]); // 01B newer
    assert_eq!(resolve_run(&dir, None /*--last*/).unwrap().file_name().unwrap(), "01B.jsonl");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli tui_conflicts last_resolves`
Expected: FAIL.

- [ ] **Step 3: Implement**

Add `#[arg(long)] pub tui: bool` to `RunArgs`; in `run.rs`, after building `(rx, run_fut)` from `drive_with_live_trace`, if `args.tui` run `tokio::join!(run_fut, async { run_tui(TraceSource::Live(rx)) })` else the existing `run_printer`. Guard `args.tui && args.json` and `args.tui && !stdout.is_terminal()` → clap/`anyhow` error with a hint. Add `Command::Trace(TraceArgs { run_id: Option<String>, #[arg(long)] last: bool })`; `cmd::trace::run` resolves the path (explicit id → `./.tau/runs/<id>.jsonl`; `--last` → newest by mtime; error listing available ids if missing) and calls `run_tui(TraceSource::File(path))`.

- [ ] **Step 4: Run tests + full crate suite**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli`
Expected: PASS.

- [ ] **Step 5: Manual smoke (document result in the PR)**

```bash
timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo run -p tau-cli -- run <example>.tau --tui
timeout 60  env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo run -p tau-cli -- trace --last
```
Expected: live waterfall during the run; `trace --last` re-opens the finished run.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-cli
git commit -m "feat(tau-cli): tau run --tui and tau trace <run_id>|--last"
```

---

### Task 8: Docs + fmt/clippy gate

**Files:**
- Modify: `docs/` reference page for `tau trace` (add to `docs/SUMMARY.md` if it's a book page), and `--tui` in the `tau run` reference
- Modify: `ARCHITECTURE.md` code-map (new `tau-trace` crate) if the freshness gate requires it

- [ ] **Step 1: Write the docs** — a short `tau trace` reference (subcommand, `--last`, keybindings) and the `--tui` flag note under `tau run`. If added as book pages, add them to `docs/SUMMARY.md`.

- [ ] **Step 2: Build the book**

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build` then `rm -rf docs/book`
Expected: only `[INFO]` lines.

- [ ] **Step 3: fmt + clippy the touched crates**

Run: `timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-ports -p tau-trace -p tau-cli -p tau-runtime-core -- --check`
Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-trace -p tau-cli --all-targets`
Expected: clean (workspace treats warnings as deny).

- [ ] **Step 4: Commit**

```bash
git add docs ARCHITECTURE.md
git commit -m "docs(trace): document tau trace and tau run --tui"
```

---

## Self-Review

**Spec coverage:**
- §4.1 `tau-trace` model → Tasks 3, 4. §4.2 TUI frontend → Tasks 5, 6. §4.3 CLI surface → Task 7. §5 data mapping → Task 3. §6 tool-call instrumentation gap → Tasks 1, 2. §8 error handling → Task 4 (malformed line), Task 3 (unknown kind ignored), Task 7 (no-TTY/`--json` guard, missing run id). §10 testing → each task's TDD + Task 5 `TestBackend`. Reasoning/💭 (§7 M2) → **deliberately out of this plan** (separate plan once the backend adapter is chosen).
- Gap accepted: control-flow spans (branch/loop/parallel) and suspend markers (spec §5) may not reach the jsonl today — Task 3 handles present variants and ignores absent ones; a follow-up (or Task 2 sibling) adds their producers if the M1 review wants those rows. Flagged in spec §11 open question 3.

**Placeholder scan:** Task 2 and Task 4 Step 0 require the executor to capture a real value from the running system (dispatch site; serialized line shape) before writing code — these are *locate-then-fill* steps with the surrounding code shown, not open-ended TODOs. All other steps carry concrete code.

**Type consistency:** `CapabilityVerdict` (Task 1) is consumed unchanged in Tasks 2, 3, 5. `TraceModel::{new,apply,spans,window,bar}` (Task 3) used verbatim in Tasks 5–7. `UiState`/`Filter` (Task 5) reused in Task 6. `run_tui`/`TraceSource` (Task 6) used in Task 7.

## Notes for M2 (separate plan — not now)
Resolve first: which backend adapter is the reasoning source. Candidates in-tree: `crates/tau-plugins/anthropic` (extended thinking), `ollama`, `openai` (o-series summaries). Then: `ContentBlock::Reasoning` + `CompletionChunk::Reasoning` (tau-ports), `RunEvent::ReasoningDelta` + a `TraceEvent` reasoning projection, and a `SpanKind::Reasoning` drill-down (already scaffolded in Task 3's enum) with raw|summarized flag.
