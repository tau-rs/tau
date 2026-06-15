# ADR-0048: β.6 cross-target conformance gate — dual-channel `ConformanceEvent` contract

**Status:** Accepted
**Date:** 2026-06-14
**Deciders:** Titouan (architect), implementing session

## Context

β.6 adds the conformance gate: the canonical fan-monitor scenario must
produce a bit-identical, ordered event stream across execution profiles
(interpreted dev vs compiled wasm), the behavioral sibling of
`verify --bundle`'s byte-level check. This phase ships the scaffolding
(dev profile live; wasm profile stubbed pending β.7.5).

Three forces shaped the contract:

1. **The ROADMAP's "EXPECTED EVENT STREAM" is not a real channel.** Its
   names (`RunStarted`, `ContextStepRan`, `InferenceCall*`,
   `ToolCall*`, `RunCompleted`) are a conceptual union. The actual
   `RunEvent` enum has 6 variants and lacks context steps and run/
   inference lifecycle marks; the tracing-event stream has those but
   lacks tool args and success-path tool results
   (`dispatch.tool_resolved` logs `tool_name` only; `message.added`
   logs `role` only).

2. **The gate must survive incidental observability churn.** Binding the
   diff directly to raw tracing fields would break the gate whenever an
   unrelated log line or field is added.

3. **The wasm profile (β.7.5) must reproduce whatever we pick.** The
   contract has to be something a wasm guest can emit across the
   component boundary, ideally an artifact we own rather than tracing
   internals.

## Decision

**Define a small, versioned, crate-owned `ConformanceEvent` model**, and
source it from **both** the `RunEvent` enum and the tracing stream — each
event kind from exactly one channel.

- `RunStarted`, `ContextStepRan`, `InferenceCallStarted`,
  `InferenceCallCompleted` ← tracing vocabulary events.
- `ToolCallStarted`, `ToolCallCompleted`, `RunCompleted` ← `RunEvent`
  enum (the only channel carrying tool args + results).

**Interleave the two channels at the engine generator's yield barrier.**
The dev profile drives the engine generator (`run_ir_streaming`, a new
thin entry in `tau-runtime-core::interpreter`) with a `Captor` tracing
layer installed. Because the executor is single-threaded (`current_
thread`, required since `run_ir` is non-`Send`), all tracing emitted
between two `yield`s belongs to that step; the interleaving is causal by
construction, with no timestamps or heuristics.

**Normalization (the canonical comparison).** Modulo (stripped/
canonicalized): timestamps, `run_id`, `agent_id` (ULID), provider
tool-call ids (→ first-seen ordinals so the started/completed pair
correlates). Compared: event kind + ordering, tool name + args + result,
context step name + `tokens_in`/`tokens_out`, inference `stop_reason` +
token usage, run outcome. **Token counts are compared, not modulo** —
deterministic under a cassette LLM + Pure transformers + a deterministic
estimator, and comparing them is what makes context-pipeline agreement
meaningful. The model carries `CONFORMANCE_EVENT_VERSION`, recorded in
the golden file.

**Crate shape.** New `tau-conformance` crate (`publish = false`), no
dependency on `tau-ir-conformance` (different axis: cross-*profile*
ordered stream vs cross-*mode* multiset; and it needs a streaming driver
the older crate lacks). Two assertions: `(a) dev == golden` (live this
phase), `(b) dev == wasm` (`#[ignore]` until β.7.5). The dev arm runs in
Tier 1 CI as `conformance (linux)`.

## Consequences

Positive:

- A stable, owned comparison contract decoupled from incidental tracing
  fields; β.7.5 only has to *produce* the stream from the wasm guest, not
  *design* it.
- The yield-barrier interleave is provably causal — no flaky
  timestamp-based ordering, directly mitigating the ROADMAP's
  "conformance gate flakiness" risk.
- Context-pipeline behavior (the β.4 transformers) is a first-class part
  of the gate, not proven separately.

Negative / obligations:

- Adds `run_ir_streaming` to `tau-runtime-core` (justified independently;
  mirrors `run_agent` construction to avoid drift).
- `SequencedLlm` is duplicated from `tau-ir-conformance` (~30 lines)
  rather than coupling the crates; a third consumer triggers extraction
  of `tau-conformance-support` (rule of three).
- Two conformance crates now exist on different axes; the distinction
  must stay documented to avoid confusion.
- The wasm arm is dead-stubbed (`unimplemented!`) and `#[ignore]`d until
  β.7.5; the full β.6 DoD is not met until that unstub. Tracked here and
  in the ROADMAP β.6 entry.

Neutral:

- A golden file (`expected_events.json`) with a bless workflow
  (`TAU_CONFORMANCE_BLESS=1`) becomes part of the review surface.

## Alternatives considered

- **Bind the gate to the `RunEvent` enum only.** Rejected: the enum
  cannot see `ContextStepRan`, so the fan-monitor's context pipeline
  (the β.4 deliverable the scenario is built to exercise) would be
  invisible to the gate. Trade-off: simpler single-channel capture, but
  the gate would fail to prove the thing it most needs to prove.

- **Bind the gate to the raw tracing stream only.** Rejected on two
  counts: tracing carries neither tool args nor success-path tool
  results (so `set_fan{on:true}` / `read_temp→32` — the scenario's
  decision logic — would be unverifiable), and coupling to free-form
  tracing fields makes the gate brittle to unrelated log changes.

- **Enrich tracing so a single channel suffices** (add args/result
  fields to dispatch events). Rejected for this phase: a broader change
  to the hot run path for a conformance-only need, when the yield-barrier
  interleave gets causal ordering from two channels at zero run-path cost.

- **Merge the two channels by timestamp / sequence number.** Rejected:
  reintroduces exactly the timing-nondeterminism flakiness the ROADMAP
  warns about; the generator yield barrier gives deterministic causal
  order for free.

- **Extend `tau-ir-conformance` instead of a new crate.** Rejected: it
  asserts cross-*mode* multiset equivalence via the batch `run_ir` path;
  β.6 needs cross-*profile* *ordered* streams via a streaming driver. The
  axes and drivers differ; a new crate keeps each focused.
