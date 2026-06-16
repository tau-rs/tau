# ADR-0049: Single-channel typed conformance observable

**Status:** Accepted
**Date:** 2026-06-16
**Deciders:** Titouan (architect), implementing session
**Supersedes:** [ADR-0048](0048-cross-target-conformance-gate.md) (dual-channel `ConformanceEvent` contract)

## Context

ADR-0048 shipped the β.6 conformance gate sourcing `ConformanceEvent`
from **two** runtime channels — the `RunEvent` enum (tool/run lifecycle)
and the `tau_observe` tracing stream (run/context/inference lifecycle) —
interleaved at the engine generator's yield barrier under a single-
threaded `Captor` subscriber. Four event kinds (`RunStarted`,
`ContextStepRan`, `InferenceCallStarted`, `InferenceCallCompleted`) came
from tracing because the `RunEvent` enum did not carry them; the other
three (`ToolCallStarted`, `ToolCallCompleted`, `RunCompleted`) came from
`RunEvent` because tracing did not carry tool args / success-path results.

β.7.5 must make the wasm profile reproduce that observable. A wasm guest
is `no_std` and has no `tracing` subscriber: it cannot install a `Captor`,
and the dual-channel interleave depended on a thread-local subscriber plus
a single-threaded executor (`current_thread`, required because `run_ir`
is non-`Send`). The observable therefore has to be something the guest
can emit **directly** across the component boundary — a single typed
channel it owns, not a tracing side-channel.

## Decision

Promote the four tracing-only gate event kinds to first-class
`tau_runtime_core::stream::RunEvent` variants:

- `RunStarted`
- `ContextStepRan { step, tokens_in, tokens_out }`
- `InferenceCallStarted`
- `InferenceCallCompleted { stop_reason, tokens_in, tokens_out }`

They are emitted on the `run_streaming_inner` generator path at the exact
points their tracing siblings fire (run start; each β.4 context-pipeline
transform; LLM request built; LLM response received, folding token usage).
`RunEvent`, `RunOutcome`, and `options::TokenUsage` now derive
`serde::{Serialize, Deserialize}` (`no_std` + `alloc`) so the guest can
JSON the stream across the boundary.

The dev profile consumes the **single** typed `RunEvent` channel. The
`Captor` tracing layer, the `map_tracing` normalizer half, and the dual-
channel interleave are deleted; `map_runevent` covers every whitelisted
kind. `tau-conformance` no longer depends on `tau-observe`.

The tracing events are **kept** — logging must not regress. The typed
variants are added alongside the tracing emissions, not moved.

The frozen `ConformanceEvent` model and the `fan_monitor` golden are
unchanged: the gate produces the byte-identical normalized stream, now
from one source instead of two.

## Consequences

- The wasm and dev profiles can share one observable shape; β.6's
  `fan_monitor_dev_matches_wasm` becomes implementable once `tau build
  wasm` lands the wasm profile.
- `RunEvent` is now a serialization-contract surface. Additive variants
  remain safe under `#[non_exhaustive]`; the conformance whitelist's
  `_ => None` arm absorbs unknown future variants.
- The dual-channel "patch-last" token fold is gone:
  `InferenceCallCompleted` reads `turn_usage` directly at
  `llm.response_received`, which is identical to the patched result
  (zero when the provider reports no usage).
- `tau-conformance` drops its `tau-observe` (and now-unused `tracing`)
  dependency.
- Obligation: the `run_streaming_inner` typed emissions and their tracing
  siblings must stay co-located (same code point). A new gate-event
  full-sequence test in `tau-runtime-core` locks the emission order, and
  the β.6 golden gate guards the normalized result.

## Alternatives considered

- **Keep ADR-0048's dual channel; have the wasm guest synthesize tracing
  events.** Rejected: the guest is `no_std` with no subscriber; emulating
  a tracing capture buffer across the component boundary reintroduces the
  exact host-only machinery the gate is meant to outlive, and couples the
  observable to tracing field shapes that churn for unrelated reasons.
- **Make the gate observable a third, bespoke side-channel emitted by the
  interpreter.** Rejected: a second event vocabulary to keep in sync with
  `RunEvent` and the tracing stream — more drift surface, not less. The
  `RunEvent` enum is already the engine's owned, executor-agnostic
  observable; extending it is the smaller, single-source-of-truth move.
- **Drop the tracing gate events and emit only the typed variants.**
  Rejected: logging would regress for host shells that consume the
  tracing stream (CLI `--log`, OTLP export). The decision keeps both.
