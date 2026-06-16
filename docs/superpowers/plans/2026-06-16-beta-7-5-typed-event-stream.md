# β.7.5 Typed RunEvent Gate Variants + Single-Channel Conformance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the β.6 conformance observable a single typed channel (`RunEvent`) so a no_std wasm guest can produce it, by promoting 4 tracing-only gate events to typed `RunEvent` variants, rewiring the dev profile to source only from `run_ir_streaming`, and deleting the std-only tracing `Captor` path.

**Architecture:** Today the dev profile interleaves two channels — `RunEvent` (tool/run lifecycle) and `tau_observe::Captor` tracing events (run/context/inference lifecycle) — at the generator yield barrier (ADR-0048). This change promotes the 4 tracing-only event kinds to first-class `RunEvent` variants emitted at the exact same code locations in `run_streaming_inner`, makes `RunEvent` serde-serializable end-to-end so a wasm guest can emit it across the component boundary, and collapses the dev profile to a single-channel consumer of `run_ir_streaming`. The tracing events stay (logging must not regress); the typed events are added alongside. ADR-0049 supersedes ADR-0048's dual-channel decision.

**Tech Stack:** Rust (no_std + alloc), `async_stream`, `serde`/`serde_json`, `futures`, `tracing`, `tokio` (test executor).

---

## Background facts (verified against the tree at `origin/main` 3d04b64)

- All gate-event emit sites live in **`crates/tau-runtime-core/src/stream.rs`** inside `run_streaming_inner` (the dev profile reaches it via `interpreter::run_ir_streaming` → `agent_loop::run_agent_streaming` → `Runtime::run_streaming_with_history` → `run_streaming_inner`).
- Emit sites & their tracing names:
  - `stream.rs:256` `info!(name = EV_RUNTIME_RUN_STARTED)` — once per run, before the `max_turns==0` guard.
  - `stream.rs:310-316` `debug!(name = EV_CONTEXT_STEP_RAN, step, tokens_in=before, tokens_out=after)` — inside the β.4 context-pipeline loop; `before`/`after` are `u32`.
  - `stream.rs:339-344` `debug!(name = EV_LLM_REQUEST_BUILT, ...)` — once per turn, before the LLM stream drain.
  - `stream.rs:409-415` `debug!(name = EV_LLM_RESPONSE_RECEIVED, stop_reason = ?turn_stop_reason, ...)` — once per turn, after the drain loop. `turn_stop_reason: Option<StopReason>` and `turn_usage: Option<TokenUsage>` are both in scope here.
  - `stream.rs:420-430` `info!(name = EV_LLM_TOKEN_USAGE, input_tokens, output_tokens, ...)` — only when `turn_usage.is_some()`.
- `RunEvent` (`stream.rs:118-201`) currently derives only `#[derive(Debug, Clone)]` and is `#[non_exhaustive]`.
- Field types already serde-capable (serde feature is unconditional in tau-runtime-core; tau-domain & tau-ports pulled with `features=["serde"]`): `tau_ports::StopReason`, `tau_ports::TokenUsage`, `tau_ports::ToolResult`, `tau_ports::ToolContent`, `tau_domain::Value`, `tau_domain::Message`, `tau_domain::AgentStatus`.
- Field types NOT yet serde-capable: **`crate::outcome::RunOutcome`** and **`crate::options::TokenUsage`** (carried by `RunCompleted` / `TurnCompleted`). These must gain serde derives for the whole `RunEvent` enum to derive serde.
- The conformance `ConformanceEvent` model (`crates/tau-conformance/src/event.rs`) is FROZEN. The golden (`crates/tau-conformance/fixtures/fan_monitor/expected_events.json`) MUST stay byte-identical.
- `normalize.rs` ordering contract (must be preserved by the single-channel emit order, per the golden):
  per turn → `RunStarted` (turn 1 only), `ContextStepRan`×N, `InferenceCallStarted`, [`ToolCallStarted` if tool turn], `InferenceCallCompleted`, [`ToolCallCompleted` if tool turn]; final turn ends with `RunCompleted`.
- `InferenceCallCompleted` in the golden always carries `tokens_in:0, tokens_out:0` for fan_monitor (mock LLM reports no usage → `turn_usage == None`). The dual-channel "patch-last" token fold becomes a direct field read from `turn_usage` in the single-channel design.
- `stop_reason` in the golden is the `StopReason` **Debug** name (`"ToolUse"`, `"EndTurn"`). `map_runevent` must format the typed `StopReason` to that exact string.

## File Structure

- **Modify** `crates/tau-runtime-core/src/options.rs` — add serde derive to `TokenUsage`.
- **Modify** `crates/tau-runtime-core/src/outcome.rs` — add serde derive to `RunOutcome`.
- **Modify** `crates/tau-runtime-core/src/stream.rs` — add 4 `RunEvent` variants; add serde derive to `RunEvent`; yield the 4 typed events at the existing tracing emit sites (tracing events stay).
- **Modify** `crates/tau-conformance/src/normalize.rs` — extend `map_runevent` to cover the 4 new variants; delete `map_tracing`, `NormState` tracing helpers no longer used, and the `tau_observe` import; convert/keep tests.
- **Modify** `crates/tau-conformance/src/profile/dev.rs` — delete `Captor` install + dual-channel interleave; consume `run_ir_streaming` single-channel.
- **Modify** `crates/tau-conformance/src/profile/mod.rs` — update module docs (no more Captor); the `?Send` bound can stay (harmless) but doc note updates.
- **Modify** `crates/tau-conformance/Cargo.toml` — drop the `tau-observe` dependency if no longer referenced anywhere in the crate.
- **Create** `docs/decisions/0049-single-channel-conformance-observable.md` — ADR superseding ADR-0048's dual-channel decision.
- **Modify** `docs/decisions/0048-cross-target-conformance-gate.md` — mark `Status: Superseded by ADR-0049`.

---

### Task 1: Make `RunOutcome` and `options::TokenUsage` serde-serializable

**Files:**
- Modify: `crates/tau-runtime-core/src/options.rs:29-30`
- Modify: `crates/tau-runtime-core/src/outcome.rs:47-49`
- Test: `crates/tau-runtime-core/src/outcome.rs` (tests module)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/tau-runtime-core/src/outcome.rs`:

```rust
    #[test]
    fn run_outcome_serde_round_trips() {
        let usage = TokenUsage {
            input_tokens: 7,
            output_tokens: 3,
            total_tokens: Some(10),
        };
        let outcome = RunOutcome::Failed {
            status: AgentStatus::Stopped,
            all_messages: alloc::vec![],
            total_turns: 2,
            token_usage: usage,
        };
        let json = serde_json::to_string(&outcome).expect("serialize");
        let back: RunOutcome = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(outcome, back);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core run_outcome_serde_round_trips`
Expected: FAIL — `RunOutcome` does not implement `Serialize`/`Deserialize`.

- [ ] **Step 3: Add the serde derives**

In `crates/tau-runtime-core/src/options.rs`, change the `TokenUsage` derive line (currently `#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]`) to add serde:

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TokenUsage {
```

In `crates/tau-runtime-core/src/outcome.rs`, change the `RunOutcome` derive line (currently `#[derive(Debug, Clone, PartialEq)]`) to add serde:

```rust
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum RunOutcome {
```

(serde is an unconditional dependency of tau-runtime-core with `alloc`+`derive`; `tau-domain`/`tau-ports` are pulled with `features=["serde"]`, so `Message`, `AgentStatus`, and `tau_ports::TokenUsage` are already serde-capable. No `cfg_attr` gate is needed.)

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core run_outcome_serde_round_trips`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-runtime-core/src/options.rs crates/tau-runtime-core/src/outcome.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(β.7.5): derive serde on RunOutcome + options::TokenUsage"
```

---

### Task 2: Add the 4 typed `RunEvent` variants + serde derive on `RunEvent`

**Files:**
- Modify: `crates/tau-runtime-core/src/stream.rs:118-201`
- Test: `crates/tau-runtime-core/src/stream.rs` (add a test in the existing `tests` module, or inline)

- [ ] **Step 1: Write the failing test**

Add a unit test (find the `#[cfg(test)] mod tests` block in `stream.rs`; if none exists in this file, add one at the end). The test asserts the new variants exist and the enum serde-round-trips:

```rust
    #[test]
    fn new_gate_variants_serde_round_trip() {
        use tau_ports::StopReason;
        let evs = alloc::vec![
            RunEvent::RunStarted,
            RunEvent::ContextStepRan {
                step: "trim_old".into(),
                tokens_in: 4,
                tokens_out: 4,
            },
            RunEvent::InferenceCallStarted,
            RunEvent::InferenceCallCompleted {
                stop_reason: StopReason::ToolUse,
                tokens_in: 0,
                tokens_out: 0,
            },
        ];
        for ev in evs {
            let json = serde_json::to_string(&ev).expect("serialize");
            let back: RunEvent = serde_json::from_str(&json).expect("deserialize");
            // RunEvent is not PartialEq; assert the Debug shape round-trips.
            assert_eq!(alloc::format!("{ev:?}"), alloc::format!("{back:?}"));
        }
    }
```

Note: `StopReason::ToolUse` must be a real variant — verify with `grep -n "ToolUse\|EndTurn" crates/tau-ports/src/llm.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core new_gate_variants_serde_round_trip`
Expected: FAIL — variants not defined / `RunEvent` not `Serialize`.

- [ ] **Step 3: Add variants + serde derive**

In `crates/tau-runtime-core/src/stream.rs`, change the `RunEvent` derive line (`stream.rs:119`, currently `#[derive(Debug, Clone)]`) to:

```rust
#[non_exhaustive]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum RunEvent {
```

Then add the 4 new variants. Place them at the TOP of the enum body (immediately after the opening brace, before `TextDelta`) so the run/context/inference lifecycle reads top-to-bottom:

```rust
    /// Run started. Emitted once, before the first turn. The typed
    /// counterpart of the `runtime.run_started` tracing event (β.7.5:
    /// promoted to a typed variant so a no_std wasm guest can emit the
    /// β.6 conformance observable over a single channel — see ADR-0049).
    RunStarted,

    /// A β.4 context-pipeline transform ran. Emitted once per transform
    /// per turn, carrying the step name and the pre/post token estimates.
    /// Typed counterpart of `runtime.context_step_ran`.
    ContextStepRan {
        /// Transform name (e.g. `"trim_old"`, `"fit_budget"`).
        step: String,
        /// Token estimate of the view entering this transform.
        tokens_in: u64,
        /// Token estimate of the view leaving this transform.
        tokens_out: u64,
    },

    /// The per-turn LLM request was built and is about to be sent.
    /// Typed counterpart of `llm.request_built`.
    InferenceCallStarted,

    /// The per-turn LLM response finished streaming. Typed counterpart of
    /// `llm.response_received` folded with `llm.token_usage`: carries the
    /// turn's `StopReason` and token usage (zero when the provider did not
    /// report usage).
    InferenceCallCompleted {
        /// Why the turn's inference stopped.
        stop_reason: StopReason,
        /// Input tokens reported for this turn (0 if unreported).
        tokens_in: u64,
        /// Output tokens reported for this turn (0 if unreported).
        tokens_out: u64,
    },
```

`StopReason` is already imported at `stream.rs:31` (`use tau_ports::{... StopReason, ...}`). Verify it is in scope (it is used by `TurnCompleted`).

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core new_gate_variants_serde_round_trip`
Expected: PASS.

- [ ] **Step 5: Confirm the crate still builds (non_exhaustive match arms)**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-core`
Expected: PASS (RunEvent is `#[non_exhaustive]`; existing `match` sites already have catch-alls).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-runtime-core/src/stream.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(β.7.5): add typed RunEvent gate variants + serde derive"
```

---

### Task 3: Emit the 4 typed events on the `run_streaming_inner` generator path

**Files:**
- Modify: `crates/tau-runtime-core/src/stream.rs` (4 emit sites inside `run_streaming_inner`)
- Test: `crates/tau-runtime-core/tests/run_ir_streaming.rs` (existing integration test file) OR a new unit test in `stream.rs`

Emit each typed event at the SAME location as its tracing sibling. KEEP every existing tracing `info!`/`debug!` call — we are ADDING typed events.

- [ ] **Step 1: Write the failing test**

Inspect `crates/tau-runtime-core/tests/run_ir_streaming.rs` first (`cat` it) to reuse its harness. Add a test that collects the stream and asserts the new variants appear in the right order for a no-tool single-turn run. If that harness is awkward, add this unit test to the `tests` mod in `stream.rs` driving `run_streaming_inner` directly with a `MockLlmBackend` (see existing stream.rs tests for the setup pattern — they already build `run_streaming_inner` with a mock backend). The assertion:

```rust
    // Drive a single-turn no-tool run; assert the typed gate events are
    // present and ordered: RunStarted, InferenceCallStarted,
    // InferenceCallCompleted, RunCompleted (no ContextStepRan because the
    // default RunOptions has an empty context_pipeline).
    #[tokio::test]
    async fn gate_events_emitted_in_order_for_simple_run() {
        let events = collect_run_events(/* mock backend yielding text+EndTurn */).await;
        let kinds: alloc::vec::Vec<&str> = events.iter().map(event_kind).collect();
        let run_started = kinds.iter().position(|k| *k == "RunStarted");
        let inf_started = kinds.iter().position(|k| *k == "InferenceCallStarted");
        let inf_done = kinds.iter().position(|k| *k == "InferenceCallCompleted");
        let run_done = kinds.iter().position(|k| *k == "RunCompleted");
        assert!(run_started < inf_started, "RunStarted before InferenceCallStarted: {kinds:?}");
        assert!(inf_started < inf_done, "InferenceCallStarted before InferenceCallCompleted: {kinds:?}");
        assert!(inf_done < run_done, "InferenceCallCompleted before RunCompleted: {kinds:?}");
    }
```

Where `event_kind` maps a `&RunEvent` to its variant name (write a small `fn event_kind(e: &RunEvent) -> &'static str` covering the variants used), and `collect_run_events` reuses the existing stream-collection helper in this file's tests (look for an existing `StreamExt`/`futures_util` collection pattern; reuse it verbatim rather than inventing a new mock).

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core gate_events_emitted_in_order_for_simple_run`
Expected: FAIL — the new variants are never yielded yet (`position` returns `None`, `None < None` is false → assert fails).

- [ ] **Step 3: Add the `RunStarted` yield**

In `crates/tau-runtime-core/src/stream.rs`, at the run-start tracing site (currently `stream.rs:256` `info!(name = EV_RUNTIME_RUN_STARTED);`), add the yield immediately after the `info!`:

```rust
        info!(name = EV_RUNTIME_RUN_STARTED);
        yield RunEvent::RunStarted;
```

- [ ] **Step 4: Add the `ContextStepRan` yield**

At the context-step tracing site (currently `stream.rs:310-316`, inside the `Ok(next) =>` arm of the context-pipeline loop), the existing code is:

```rust
                        Ok(next) => {
                            let after: u32 = next.iter().map(|m| cx.estimate_tokens(m)).sum();
                            debug!(
                                parent: &turn_span,
                                name = EV_CONTEXT_STEP_RAN,
                                step = t.name(),
                                tokens_in = before,
                                tokens_out = after,
                            );
                            view = next;
                        }
```

Add the yield right after the `debug!`, before `view = next;`:

```rust
                        Ok(next) => {
                            let after: u32 = next.iter().map(|m| cx.estimate_tokens(m)).sum();
                            debug!(
                                parent: &turn_span,
                                name = EV_CONTEXT_STEP_RAN,
                                step = t.name(),
                                tokens_in = before,
                                tokens_out = after,
                            );
                            yield RunEvent::ContextStepRan {
                                step: t.name().into(),
                                tokens_in: u64::from(before),
                                tokens_out: u64::from(after),
                            };
                            view = next;
                        }
```

(`before`/`after` are `u32`; widen to `u64` with `u64::from`. `t.name()` returns `&str`.)

- [ ] **Step 5: Add the `InferenceCallStarted` yield**

At the request-built tracing site (currently `stream.rs:339-344`), add the yield right after the `debug!`:

```rust
            debug!(
                parent: &turn_span,
                name = EV_LLM_REQUEST_BUILT,
                messages = request.messages.len(),
                tools = request.tools.len(),
            );
            yield RunEvent::InferenceCallStarted;
```

- [ ] **Step 6: Add the `InferenceCallCompleted` yield**

At the response-received tracing site (currently `stream.rs:409-415`), after the `debug!(name = EV_LLM_RESPONSE_RECEIVED, ...)`. At this point `turn_stop_reason: Option<StopReason>` and `turn_usage: Option<TokenUsage>` are in scope. Add:

```rust
            debug!(
                parent: &turn_span,
                name = EV_LLM_RESPONSE_RECEIVED,
                text_len = accumulated_text.len(),
                tool_uses = pending_tool_uses.len(),
                stop_reason = ?turn_stop_reason,
            );
            yield RunEvent::InferenceCallCompleted {
                stop_reason: turn_stop_reason.unwrap_or(StopReason::EndTurn),
                tokens_in: turn_usage.map_or(0, |u| u64::from(u.input_tokens)),
                tokens_out: turn_usage.map_or(0, |u| u64::from(u.output_tokens)),
            };
```

Rationale: the dual-channel design pushed `InferenceCallCompleted{0,0}` on `llm.response_received` then patched it on `llm.token_usage`. Reading `turn_usage` directly here yields the identical result (zero when the provider reports no usage — which is the fan_monitor case). `TokenUsage` (`tau_ports`) is `Copy`, so `turn_usage` stays usable for the existing accumulation code below.

- [ ] **Step 7: Run the new test + the full crate suite**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core`
Expected: PASS — the new ordering test passes; no existing test regresses (TurnCompleted/ToolCall ordering unchanged because the new yields are inserted at non-conflicting points).

- [ ] **Step 8: Commit**

```bash
git add crates/tau-runtime-core/src/stream.rs crates/tau-runtime-core/tests/run_ir_streaming.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(β.7.5): emit typed gate events on run_streaming_inner path"
```

---

### Task 4: Extend `map_runevent` for the 4 new variants; delete `map_tracing`

**Files:**
- Modify: `crates/tau-conformance/src/normalize.rs`
- Test: `crates/tau-conformance/src/normalize.rs` (tests module)

- [ ] **Step 1: Write the failing tests**

In the `tests` module of `normalize.rs`, ADD tests for the new `map_runevent` arms (do not delete the existing `map_runevent` tool/run tests):

```rust
    #[test]
    fn run_started_maps() {
        let mut st = NormState::default();
        assert_eq!(
            map_runevent(RunEvent::RunStarted, &mut st),
            Some(ConformanceEvent::RunStarted)
        );
    }

    #[test]
    fn context_step_ran_maps() {
        let mut st = NormState::default();
        assert_eq!(
            map_runevent(
                RunEvent::ContextStepRan {
                    step: "trim_old".into(),
                    tokens_in: 40,
                    tokens_out: 30,
                },
                &mut st
            ),
            Some(ConformanceEvent::ContextStepRan {
                step: "trim_old".into(),
                tokens_in: 40,
                tokens_out: 30,
            })
        );
    }

    #[test]
    fn inference_started_maps() {
        let mut st = NormState::default();
        assert_eq!(
            map_runevent(RunEvent::InferenceCallStarted, &mut st),
            Some(ConformanceEvent::InferenceCallStarted)
        );
    }

    #[test]
    fn inference_completed_maps_stop_reason_to_debug_name() {
        use tau_ports::StopReason;
        let mut st = NormState::default();
        assert_eq!(
            map_runevent(
                RunEvent::InferenceCallCompleted {
                    stop_reason: StopReason::ToolUse,
                    tokens_in: 12,
                    tokens_out: 5,
                },
                &mut st
            ),
            Some(ConformanceEvent::InferenceCallCompleted {
                stop_reason: "ToolUse".into(),
                tokens_in: 12,
                tokens_out: 5,
            })
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance run_started_maps context_step_ran_maps inference_started_maps inference_completed_maps_stop_reason_to_debug_name`
Expected: FAIL (the `_ => None` arm currently swallows these; build may also fail once `map_tracing` is deleted — do Step 3 then re-run).

- [ ] **Step 3: Rewrite `normalize.rs` to single-channel**

Make these edits to `crates/tau-conformance/src/normalize.rs`:

(a) Update the module doc comment (lines 1-21) to describe a single channel:

```rust
//! Channel normalizer (ADR-0049, supersedes ADR-0048's dual-channel).
//!
//! Maps the single [`tau_runtime_core::stream::RunEvent`] channel into
//! [`ConformanceEvent`]. The β.6 gate observable is now produced entirely
//! by the engine's typed event stream (`run_ir_streaming`), so a no_std
//! wasm guest can emit it across the component boundary with no tracing
//! subscriber. Non-whitelisted `RunEvent` variants (`TextDelta`,
//! `TurnCompleted`, `FatalError`, plus future `#[non_exhaustive]`
//! additions) map to `None`.
```

(b) Delete the `use tau_observe::capture::CapturedEvent;` import (line 26).

(c) Delete the helper fns only used by `map_tracing`: `field` (lines 59-61), `u64f` (66-68), `strip_quotes` (73-77), and `map_tracing` itself (98-136). KEEP `clean_stop_reason`? No — it parsed Debug-wrapped tracing strings; the typed path uses a real `StopReason`. Delete `clean_stop_reason` too (lines 82-91). Delete `use std::collections::BTreeMap;` only if `NormState` no longer needs it — `NormState` uses `BTreeMap` for `tool_ids`, so KEEP the `BTreeMap` import and `NormState`.

(d) Add a `stop_reason_name` helper near the top of the file (after the `NormState` impl):

```rust
/// Render a `StopReason` to its Debug variant name (`"ToolUse"`,
/// `"EndTurn"`, …) — the canonical string the frozen `ConformanceEvent`
/// and golden compare against.
fn stop_reason_name(sr: tau_ports::StopReason) -> String {
    format!("{sr:?}")
}
```

(e) Extend `map_runevent` (lines 142-169) to cover the 4 new variants. Add these arms BEFORE the `_ => None` catch-all:

```rust
        RunEvent::RunStarted => Some(ConformanceEvent::RunStarted),
        RunEvent::ContextStepRan {
            step,
            tokens_in,
            tokens_out,
        } => Some(ConformanceEvent::ContextStepRan {
            step,
            tokens_in,
            tokens_out,
        }),
        RunEvent::InferenceCallStarted => Some(ConformanceEvent::InferenceCallStarted),
        RunEvent::InferenceCallCompleted {
            stop_reason,
            tokens_in,
            tokens_out,
        } => Some(ConformanceEvent::InferenceCallCompleted {
            stop_reason: stop_reason_name(stop_reason),
            tokens_in,
            tokens_out,
        }),
```

(f) DELETE all `map_tracing`-based tests from the `tests` module (the tests calling `map_tracing(...)`: `whitelisted_tracing_events_map`, `token_usage_patches_last_inference_completed`, `stop_reason_unwraps_some_debug_wrapper`, `stop_reason_unwraps_some_with_end_turn`, `stop_reason_converts_none_to_lowercase`, `stop_reason_quotes_are_stripped`, `non_whitelisted_tracing_events_dropped`, `token_usage_without_prior_inference_is_noop`). Also delete the `captured(...)` test helper and `use tau_observe::capture::CapturedEvent;` from the tests module. KEEP all `map_runevent` tests and the `make_tool_started`/`make_tool_completed_ok` helpers. ADD the 4 new tests from Step 1.

- [ ] **Step 4: Run the conformance unit tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance --lib`
Expected: PASS — all `map_runevent` tests (existing + 4 new) green; no `map_tracing` references remain.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-conformance/src/normalize.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(β.7.5): single-channel normalizer — map_runevent covers gate events, drop map_tracing"
```

---

### Task 5: Rewrite the dev profile to single-channel; drop the `Captor`

**Files:**
- Modify: `crates/tau-conformance/src/profile/dev.rs`
- Modify: `crates/tau-conformance/src/profile/mod.rs` (docs)
- Modify: `crates/tau-conformance/Cargo.toml` (drop `tau-observe` if unused)

- [ ] **Step 1: Rewrite the `DevProfile::run` body**

In `crates/tau-conformance/src/profile/dev.rs`:

(a) Delete `use tau_observe::capture::Captor;` (line 14) and change the normalize import (line 18) from:

```rust
use crate::normalize::{map_runevent, map_tracing, NormState};
```
to:
```rust
use crate::normalize::{map_runevent, NormState};
```

(b) Update the module doc (lines 1-7) to:

```rust
//! Interpreted dev profile: drive `run_ir_streaming` and normalize its
//! single typed `RunEvent` stream into `ConformanceEvent`s (ADR-0049,
//! supersedes ADR-0048's dual-channel interleave). No tracing subscriber
//! is installed — the engine emits every gate event as a typed variant,
//! which is exactly what a no_std wasm guest produces across the
//! component boundary.
```

(c) In `DevProfile::run`, delete the `Captor` install (lines 46-50: the comment + `let captor = Captor::new();` + `let _guard = ...`). Replace the consume loop (lines 62-83) with the single-channel form:

```rust
        let mut out: Vec<ConformanceEvent> = Vec::new();
        let mut st = NormState::default();
        while let Some(ev) = stream.next().await {
            if let Some(ce) = map_runevent(ev, &mut st) {
                out.push(ce);
            }
        }
        Ok(out)
```

(Delete the `consumed`/`captured`/`map_tracing` machinery entirely.)

- [ ] **Step 2: Update `profile/mod.rs` docs**

In `crates/tau-conformance/src/profile/mod.rs`, update the module doc (lines 1-13) bullet for `DevProfile` to drop the Captor mention:

```rust
//! - [`DevProfile`] drives the interpreted runtime (`run_ir_streaming`)
//!   and normalizes its single typed `RunEvent` stream (ADR-0049). No
//!   tracing subscriber is installed.
```

Leave the `?Send` trait bound and its doc note as-is unless the build flags it unused — the bound is harmless. (If clippy/build complains the `?Send` is now unnecessary, that's fine to leave; do NOT chase it.)

- [ ] **Step 3: Drop the `tau-observe` dependency if unused**

Run: `grep -rn "tau_observe\|tau-observe" crates/tau-conformance/src crates/tau-conformance/tests`
If there are ZERO matches, remove the `tau-observe = { ... }` line from `crates/tau-conformance/Cargo.toml` (line 17). If there are matches, leave it.

- [ ] **Step 4: Run the dev-profile unit test**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance dev_profile_native_round_trip`
Expected: PASS — the native round-trip (RunStarted first, RunCompleted last, read_temp tool calls present) holds via the single channel.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-conformance/src/profile/dev.rs crates/tau-conformance/src/profile/mod.rs crates/tau-conformance/Cargo.toml
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(β.7.5): dev profile consumes single RunEvent channel, drop Captor"
```

---

### Task 6: Verify the golden stays byte-identical (the gate)

**Files:**
- Read-only: `crates/tau-conformance/fixtures/fan_monitor/expected_events.json`
- Test: `crates/tau-conformance/tests/conformance.rs::fan_monitor_dev_matches_golden`

- [ ] **Step 1: Run the golden gate test WITHOUT blessing**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance fan_monitor_dev_matches_golden`
Expected: PASS. The single-channel event stream must equal the golden byte-for-byte (same events, different source).

- [ ] **Step 2: If it FAILS — diagnose, do NOT re-bless**

A failure means the emitted stream diverged. Read the diff in the panic report. Likely causes and fixes (fix the EMISSION/normalizer, never the golden):
  - Wrong event ORDER → re-check the yield insertion points in Task 3 (e.g. `InferenceCallCompleted` must be yielded AFTER the drain loop, so it lands after `ToolCallStarted` but before `ToolCallCompleted`).
  - `stop_reason` string mismatch → confirm `stop_reason_name` uses `{:?}` (Debug) producing `"ToolUse"`/`"EndTurn"`, matching the golden.
  - Token values nonzero where golden has 0 → confirm the mock reports no usage; `turn_usage.map_or(0, ...)` must yield 0.
  - A missing/extra `ContextStepRan` → confirm the yield is inside the `Ok(next)` arm only (one per successful transform).
Only re-bless (`TAU_CONFORMANCE_BLESS=1`) if you can PROVE the new bytes are correct AND explain why in the commit/PR — a changed golden signals a behavioral change, which is NOT intended here.

- [ ] **Step 3: Run the full conformance + runtime-core suites**

Run:
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance
```
Expected: both green; `fan_monitor_dev_matches_golden` and `dev_profile_is_deterministic` PASS.

- [ ] **Step 4: Doctest + clippy on touched crates**

Run:
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-runtime-core --doc
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-runtime-core -p tau-conformance --all-targets
```
Expected: clean.

- [ ] **Step 5: Commit (only if any doctest/clippy fix was needed)**

```bash
git add -A
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "chore(β.7.5): clippy/doctest cleanup for typed-event conformance"
```

---

### Task 7: ADR-0049 + supersede ADR-0048

**Files:**
- Create: `docs/decisions/0049-single-channel-conformance-observable.md`
- Modify: `docs/decisions/0048-cross-target-conformance-gate.md` (status line)

- [ ] **Step 1: Write ADR-0049**

Create `docs/decisions/0049-single-channel-conformance-observable.md`. Use the repo ADR template shape (Title / Status / Date / Deciders / Context / Decision / Consequences). Content:

```markdown
# ADR-0049: Single-channel typed conformance observable

**Status:** Accepted
**Date:** 2026-06-16
**Deciders:** Titouan (architect), implementing session
**Supersedes:** ADR-0048 (dual-channel `ConformanceEvent` contract)

## Context

ADR-0048 shipped the β.6 conformance gate sourcing `ConformanceEvent`
from TWO runtime channels — the `RunEvent` enum (tool/run lifecycle) and
the `tau_observe` tracing stream (run/context/inference lifecycle) —
interleaved at the engine generator's yield barrier under a single-
threaded `Captor` subscriber.

β.7.5 must make the wasm profile reproduce that observable. A wasm guest
is no_std and has no `tracing` subscriber: it cannot install a `Captor`,
and the dual-channel interleave depended on a thread-local subscriber and
a single-threaded executor. The observable has to be something the guest
can emit directly across the component boundary.

## Decision

Promote the 4 tracing-only gate event kinds — `RunStarted`,
`ContextStepRan`, `InferenceCallStarted`, `InferenceCallCompleted` — to
first-class `tau_runtime_core::stream::RunEvent` variants, emitted on the
`run_streaming_inner` generator path at the exact points their tracing
siblings fire. `RunEvent` now derives `serde::{Serialize, Deserialize}`
(no_std + alloc) so the guest can JSON it across the boundary.

The dev profile consumes the SINGLE typed `RunEvent` channel; the
`Captor` tracing layer and the dual-channel interleave are deleted. The
`map_tracing` normalizer half is removed; `map_runevent` covers every
whitelisted kind.

The tracing events are KEPT (logging must not regress) — the typed
variants are added alongside, not moved.

The frozen `ConformanceEvent` model and the `fan_monitor` golden are
unchanged: the gate produces the identical normalized stream, now from
one source instead of two.

## Consequences

- The wasm and dev profiles share one observable shape; β.6's
  `fan_monitor_dev_matches_wasm` becomes implementable.
- `tau-conformance` no longer depends on `tau-observe`.
- `RunEvent` is now a serialization contract surface; additive variants
  remain safe under `#[non_exhaustive]`.
- Token folding is gone: `InferenceCallCompleted` reads `turn_usage`
  directly, identical to the patch-last result.
```

(Before writing, `cat docs/decisions/template.md` and match its exact section headings/format. If `docs/decisions/README.md` maintains an ADR index, add the 0049 line.)

- [ ] **Step 2: Mark ADR-0048 superseded**

In `docs/decisions/0048-cross-target-conformance-gate.md`, change the status line from `**Status:** Accepted` to:

```markdown
**Status:** Superseded by [ADR-0049](0049-single-channel-conformance-observable.md)
```

- [ ] **Step 3: Update the README index if present**

Run: `grep -n "0048" docs/decisions/README.md`. If ADR-0048 is listed, add a sibling line for ADR-0049 in the same format.

- [ ] **Step 4: Commit**

```bash
git add docs/decisions/
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "docs(β.7.5): ADR-0049 single-channel conformance observable (supersedes ADR-0048)"
```

---

### Task 8: Final verification, code review, PR

- [ ] **Step 1: Full verification of both crates**

Run:
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-runtime-core --doc
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-runtime-core -p tau-conformance --all-targets
timeout 30  env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-runtime-core -p tau-conformance -- --check
```
Expected: all green. Record the `fan_monitor_dev_matches_golden` PASS line as evidence.

- [ ] **Step 2: Use the verification-before-completion skill**

Confirm every claim with command output before asserting done.

- [ ] **Step 3: Use the requesting-code-review skill**

Request a code review of the branch diff against `origin/main`.

- [ ] **Step 4: Open the PR**

```bash
git push -u origin feat/beta-7-5-typed-event-stream
gh pr create --base main \
  --title "feat(β.7.5): typed RunEvent gate variants + single-channel conformance (supersedes ADR-0048 dual-channel)" \
  --body "<summary + test evidence + ADR-0049 note + PR-A coordination note>"
```

- [ ] **Step 5: Coordinate with PR-A if BEHIND at merge**

A parallel session (PR-A) also edits `stream.rs` (skill-from-disk path, disjoint from the `RunEvent` enum). If the PR is BEHIND main at merge time: `gh pr update-branch <PR#>` and resolve the small hunk (the enum-definition edits and the emit-site yields are disjoint from the skill-from-disk path).

---

## Self-Review

**Spec coverage:**
- Design item (1) "Add 4 RunEvent variants + serde" → Task 1 (RunOutcome/TokenUsage serde) + Task 2 (variants + RunEvent serde). ✓
- Design item (2) "Emit on run_ir_streaming path, keep tracing" → Task 3 (all 4 yields at existing tracing sites, tracing kept). ✓
- Design item (3) "Single-channel normalizer; delete map_tracing/Captor/interleave" → Task 4 (normalize) + Task 5 (dev profile + Cargo). ✓
- Design item (4) "Golden stays byte-identical" → Task 6 (gate test, no-rebless discipline). ✓
- ADR-0049 supersedes ADR-0048 → Task 7. ✓
- Verify commands + code review + PR → Task 8. ✓

**Placeholder scan:** PR body in Task 8 Step 4 is the only `<...>` — acceptable (filled at PR time from real test output). All code steps carry concrete code.

**Type consistency:** `ContextStepRan { step: String, tokens_in: u64, tokens_out: u64 }`, `InferenceCallCompleted { stop_reason: StopReason, tokens_in: u64, tokens_out: u64 }` — consistent across Task 2 (def), Task 3 (emit, widening `u32→u64`), Task 4 (`map_runevent` arms, `stop_reason_name` → String). `stop_reason_name` defined once in Task 4. `event_kind`/`collect_run_events` test helpers flagged as "reuse existing harness" in Task 3.
