# β.6 — Cross-target conformance gate (design)

**Status:** Approved (scaffolding phase)
**Date:** 2026-06-14
**ROADMAP:** Phase β.6; Phase β success criterion
**ADR:** 0048
**Depends on (unstub):** β.7.5 (`tau build wasm`) for the second profile arm

## Summary

A new `tau-conformance` crate runs one scenario under multiple execution
**profiles** and demands a bit-identical, ordered stream of normalized
events. It is the behavioral sibling of `verify --bundle`'s byte-level
reproducibility check: where `verify --bundle` proves the *artifact* is
the same, the conformance gate proves the *behavior* is the same.

This phase ships everything that does not depend on a compiled wasm
artifact:

- the `tau-conformance` crate skeleton,
- the canonical **fan-monitor** scenario as a fixture,
- the `DevProfile` runner (interpreted dev profile) + the event
  normalizer + the differ,
- a live assertion that the dev profile produces the documented event
  stream (golden file),
- a `WasmProfile` arm that compiles but is `#[ignore]`d behind a clear
  `TODO(β.7.5)`.

The full β.6 DoD — both profiles agree — unblocks once β.7.5 lands
`tau build wasm`.

## Background: three event channels, none sufficient alone

The runtime emits observable data on more than one channel. This was the
central design discovery and it shapes the whole crate.

1. **The `RunEvent` enum** (`tau-runtime-core::stream`, 6 variants:
   `TextDelta`, `ToolCallStarted`, `ToolCallCompleted`, `TurnCompleted`,
   `RunCompleted`, `FatalError`). Yielded from the engine generator.
   Carries tool **name + args + result** and the final outcome. Does
   **not** carry context-pipeline steps or run/inference lifecycle marks.

2. **The tracing-event stream** (`tau-runtime-core::vocabulary`
   constants, e.g. `runtime.run_started`, `runtime.context_step_ran`,
   `llm.request_built`, `llm.response_received`), captured by a
   `Captor`-style `tracing::Layer` (logging §D / PR #226). Carries
   context steps and run/inference lifecycle marks. Does **not** carry
   tool args or success-path tool results — `dispatch.tool_resolved`
   logs `tool_name` only, and `message.added` logs `role` only.

3. **`TraceEvent`** (orchestration / multi-agent). Out of scope for the
   single-agent fan-monitor.

The ROADMAP's illustrative "EXPECTED EVENT STREAM" (with `RunStarted`,
`ContextStepRan`, `InferenceCallStarted/Completed`, `ToolCallStarted/
Completed`, `RunCompleted`) is therefore **not** any single channel — it
is a conceptual union of channels 1 and 2. The gate must source from
both.

## Architecture

```
                         Scenario (fixture dir)
                tau.toml · mock_llm.jsonl · weather.cassette.jsonl
                              · expected_events.json
                                      │
              ┌───────────────────────┴───────────────────────┐
              ▼                                                 ▼
        DevProfile                                        WasmProfile
   (interpreted, live now)                          (stub, #[ignore] β.7.5)
   run_ir_streaming + Captor                        wasmtime + guest events
   interleave at yield barrier                              │
              │                                             │
              ▼                                             ▼
        Vec<ConformanceEvent>                       Vec<ConformanceEvent>
              │                                             │
              ├──────────── normalize (modulo) ────────────┤
              ▼                                             ▼
   (a) diff vs expected_events.json            (b) diff dev vs wasm
        LIVE — proves the stream               #[ignore] until β.7.5
                                                = the real β.6 DoD
```

### Profile abstraction (Q2)

```rust
pub trait Profile {
    fn name(&self) -> &str;
    async fn run(&self, scenario: &Scenario) -> Result<Vec<ConformanceEvent>, ProfileError>;
}
```

- `DevProfile` drives the engine generator and captures both channels
  (below). Live this phase.
- `WasmProfile::run` is `unimplemented!("TODO(β.7.5): drive tau build
  wasm artifact in wasmtime, harvest guest ConformanceEvents")`. It
  compiles; only an `#[ignore]`d test calls it.

The runner makes **two distinct assertions**:

- **(a) dev == golden** — `DevProfile` output, normalized, equals the
  checked-in `expected_events.json`. Proves the documented fan-monitor
  stream. **Live this phase.**
- **(b) dev == wasm** — the cross-profile β.6 DoD. `#[ignore]` until
  β.7.5.

### Dual-channel capture, interleaved at the generator yield barrier

Tool args/results live only in `RunEvent`; context steps live only in
tracing. To merge them in true causal order **without timestamps or
heuristics**, drive the engine's generator (`run_ir_streaming`, see
below) and use each `yield` as a synchronization barrier. The executor
is single-threaded (`current_thread`, mandatory because `run_ir` is
non-`Send`), so all tracing emitted between two yields happens during the
generator's resume:

```rust
loop {
    let before = captor.len();
    let ev = stream.poll_next().await;        // resume generator to next yield
    for t in &captor.events()[before..] {     // tracing emitted during THIS step, in order
        push(map_tracing(t)?);                // RunStarted, ContextStepRan, InferenceCall*
    }
    match ev {
        Some(re) => push(map_runevent(re)?),  // ToolCall{name,args,result}, RunCompleted
        None => break,
    }
}
```

The generator's yield points *are* the ordering barrier; the
interleaving is deterministic and causal, not reconstructed.

### `run_ir_streaming` — small addition to `tau-runtime-core`

The interpreter's public entry `run_ir` returns `RunOutcome` (it drives
`run_with_history`, which already internally consumes the `RunEvent`
stream from `run_streaming_with_history` and collapses it). The
conformance runner needs the *uncollapsed* stream. Rather than have
`tau-conformance` re-implement `run_agent`'s non-trivial Runtime
construction (and drift from it), add a thin sibling entry in
`tau-runtime-core::interpreter`:

```rust
// Mirrors run_agent's construction; returns the RunEvent stream instead
// of collapsing to RunOutcome via run_with_history.
pub fn run_ir_streaming<D>(
    module: Arc<IrModule>,
    entry: &AgentId,
    dispatcher: Arc<D>,
    initial_messages: Vec<Message>,
) -> impl Stream<Item = RunEvent>
where D: ToolDispatcher + Send + Sync + 'static;
```

This is the only change outside the new crate. It is justified
independently of conformance (a streaming IR entry is generally useful)
and keeps the dev-profile capture on the exact code path dev/bundle use.

## The `ConformanceEvent` contract (Q1: the canonical comparison)

A small, versioned, crate-owned model — not the raw `RunEvent` enum and
not raw tracing. Each kind is sourced from **exactly one** channel (no
double-counting). This table is the authoritative comparison spec.

| `ConformanceEvent` | Source channel | Fields compared | Modulo (stripped/canonicalized) |
|---|---|---|---|
| `RunStarted` | tracing `runtime.run_started` | — | `run_id` |
| `ContextStepRan` | tracing `runtime.context_step_ran` | `step`, `tokens_in`, `tokens_out` | — |
| `InferenceCallStarted` | tracing `llm.request_built` | — | — |
| `InferenceCallCompleted` | tracing `llm.response_received` (+ `llm.stop_reason`, `llm.token_usage`) | `stop_reason`, `tokens_in`, `tokens_out` | — |
| `ToolCallStarted` | `RunEvent::ToolCallStarted` | `name`, `args` | `id` → first-seen ordinal |
| `ToolCallCompleted` | `RunEvent::ToolCallCompleted` | `name`, `result` (Err → canonical marker) | `id` → same ordinal |
| `RunCompleted` | `RunEvent::RunCompleted` | `outcome` discriminant | — |

**Global modulo set:** timestamps, `run_id`, `agent_id` (ULID), and
provider-supplied tool-call ids. Tool-call ids are canonicalized to
**first-seen ordinals** (`tc#0`, `tc#1`, …) so the
`ToolCallStarted`/`ToolCallCompleted` pair still correlates after
stripping. Everything not in the modulo set is compared; one divergence
fails CI.

**Token counts are compared, not modulo.** With a cassette LLM (fixed
usage), Pure context transformers, and a deterministic estimator, token
counts are reproducible across profiles — and comparing them is what
makes context-pipeline agreement meaningful rather than cosmetic.

**Versioning.** The model carries a `CONFORMANCE_EVENT_VERSION`
constant. Changing the whitelist or a field projection bumps it and
re-blesses goldens in the same change. The version is recorded in
`expected_events.json` so a stale golden is a loud mismatch, not a silent
pass.

## The differ

Ordered, element-by-element comparison of two `Vec<ConformanceEvent>`
(or one vs the golden). On first divergence it reports:

- the index,
- the expected vs actual event (pretty-printed),
- a windowed context (±2 events) so the divergence is readable in CI
  logs.

Length mismatch is reported as a divergence at the first missing/extra
index. The differ is a real, tested component this phase — not a stub —
because "the differ is real" is in the scaffolding DoD.

## The canonical fan-monitor scenario (Q3: scenario format)

Reuses the proven `tau-ir-conformance` fixture conventions (which already
ship `07_mcp_weather_cassette`, `13_context_pipeline`, and native/
deterministic tools in `01`). The fan-monitor is largely an *assembly* of
these techniques — a key risk reducer.

```
crates/tau-conformance/fixtures/fan_monitor/
  tau.toml               # agent "fan-monitor"
                         #   natives read_temp + set_fan (deterministic registry)
                         #   MCP weather (cassette transport)
                         #   context: trim_old → compact_tool_outputs → fit_budget
                         #   model: claude-haiku-4-5 (cassette)
  mock_llm.jsonl         # turn-ordered scripted LLM responses
  weather.cassette.jsonl # MCP weather cassette (tau-mcp CassetteTransport)
  expected_events.json   # golden normalized stream  (the (a) assertion)
```

Tool modeling:

- `read_temp` (deterministic reading) and `set_fan` (records state) are
  modeled as deterministic-registry-backed tools — the same technique
  fixture `01` uses. No plugin process is spawned; the run stays
  in-process. `ToolCallStarted/Completed` still fire because the LLM
  calls them by name and the dispatcher resolves them.
- `weather` is genuinely MCP, backed by `tau-mcp::CassetteTransport`
  (feature `with-std-adapters`), wired into the dispatcher's tool map —
  the same pattern as fixture `07`.

**Golden regeneration:** `TAU_CONFORMANCE_BLESS=1` rewrites
`expected_events.json` (insta-style), so updates are deliberate and
appear in review diffs.

## CI placement (Q4)

The dev arm is in-process, cassette-backed, and fully deterministic →
**Tier 1 fast loop**, as `conformance (linux)` (matches the ROADMAP CI
table). The `#[ignore]`d wasm arm runs nowhere until β.7.5 unstubs it;
its placement (Tier 1 vs nightly) is decided then, once wasmtime build
cost is measured.

## Crate shape and dependencies

New `crates/tau-conformance` (`publish = false`). **No dependency on
`tau-ir-conformance`** — different axis (cross-*profile* ordered stream
vs cross-*mode* multiset) and it needs the streaming driver the older
crate lacks. Reuse only already-shared atoms:

- `tau-plugin-test-support` (LLM cassette helpers),
- `tau-mcp` `CassetteTransport` (MCP cassette replay),
- `tau-runtime-core` (`run_ir_streaming`, the IR types, the deterministic
  registry),
- `tau-observe` (`Captor`),
- `tau-ir` / `tau-pkg` (lowering the fixture `tau.toml` → `IrModule`).

The trivial `SequencedLlm` scripted backend (~30 lines) is duplicated
from `tau-ir-conformance` rather than coupling the two crates. If a third
consumer appears, extract `tau-conformance-support` (rule of three).

### Module layout

```
crates/tau-conformance/
  Cargo.toml
  src/
    lib.rs            # Profile trait, Scenario, runner, re-exports
    event.rs          # ConformanceEvent + CONFORMANCE_EVENT_VERSION
    normalize.rs      # map_tracing / map_runevent + modulo rules
    differ.rs         # ordered diff + readable report
    scenario.rs       # fixture loading (tau.toml, mock_llm, cassettes, golden)
    profile/
      mod.rs          # Profile trait
      dev.rs          # DevProfile: run_ir_streaming + Captor interleave
      wasm.rs         # WasmProfile: unimplemented!(TODO β.7.5)
    sequenced_llm.rs  # scripted LlmBackend (duplicated)
    dispatcher.rs     # ToolDispatcher: backend + deterministic natives + MCP cassette tool
  fixtures/fan_monitor/...
  tests/
    conformance.rs    # (a) dev_vs_golden  +  (b) dev_vs_wasm #[ignore]
```

## Testing strategy

- **Unit:** `differ` (equal streams pass; injected divergence at a known
  index fails with the right report); `normalize` (modulo stripping;
  tool-id ordinal correlation; token counts preserved).
- **Integration (live):** `fan_monitor_dev_matches_golden` — runs
  `DevProfile`, normalizes, asserts equality with `expected_events.json`.
- **Integration (stubbed):** `#[ignore]` `fan_monitor_dev_matches_wasm`
  — calls `WasmProfile`, hits the `unimplemented!`; documents the β.7.5
  unstub. Carries a `TODO(β.7.5)` and is referenced from the ROADMAP
  follow-up.
- **Determinism:** the dev assertion run twice in one test yields
  byte-identical normalized streams (guards against accidental
  nondeterminism in the modulo rules).

## Scope boundaries

In scope (this phase):

- crate skeleton, fan-monitor fixture, `DevProfile`, normalizer, differ,
- live `(a) dev == golden` assertion,
- `WasmProfile` stub + `#[ignore]`d `(b)` test,
- `run_ir_streaming` in `tau-runtime-core`,
- the `conformance (linux)` Tier 1 CI lane (running only the live tests),
- ADR-0048, ROADMAP β.6 status update noting the β.7.5 unstub.

Out of scope (β.7.5 follow-up, tracked):

- `WasmProfile` real implementation (wasmtime host + guest event
  harvest),
- enabling assertion `(b)`,
- any wasm build wiring.

Out of scope (not β.6):

- multi-agent `TraceEvent` conformance,
- additional scenarios beyond fan-monitor (the canonical scenario is the
  gate; more scenarios are a later addition).

## β.7.5 dependency (tracked follow-up)

The unstub is a single, well-scoped follow-up: implement `WasmProfile::
run` against `tau build wasm`'s artifact and flip
`fan_monitor_dev_matches_wasm` from `#[ignore]` to live. The
`ConformanceEvent` contract is frozen this phase precisely so β.7.5 only
has to *produce* the stream from the guest, not *design* it. This
dependency is recorded in ADR-0048 and the ROADMAP β.6 entry.
