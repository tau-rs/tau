# ADR-0049: single-channel typed conformance observable

**Status:** Accepted
**Date:** 2026-06-16
**Deciders:** Titouan (architect), implementing session
**Supersedes:** ADR-0048 Decision 1 (dual-channel `ConformanceEvent` sourcing)

## Context

ADR-0048 defined the β.6 cross-target conformance gate around a small,
crate-owned `ConformanceEvent` model, and sourced it from **two** channels
(Decision 1 of that ADR):

- `ToolCallStarted`, `ToolCallCompleted`, `RunCompleted` ← the typed
  `RunEvent` enum yielded by `run_ir_streaming`.
- `RunStarted`, `ContextStepRan`, `InferenceCallStarted`,
  `InferenceCallCompleted` ← a std `tracing` `Captor` layer that
  string-parses log lines (`map_tracing`), interleaved with the typed
  stream at the generator's yield barrier.

That dual-channel design was sound for the **dev** profile, where a std
`tracing` subscriber is always available. β.7.5 then revealed the hole: the
gate's whole purpose is **cross-profile** parity (interpreted dev vs compiled
wasm), and a `no_std` `wasm32-wasip2` guest **cannot run the std `tracing`
subscriber**. The four tracing-sourced event kinds would therefore be
unreproducible from inside the wasm artifact — the wasm arm of the gate
(`fan_monitor_dev_matches_wasm`, `#[ignore]`'d pending β.7.5) could never go
live as designed.

The forces:

1. **The gate must be emittable from a `no_std` guest.** Whatever the wasm
   artifact exports across the WIT component boundary has to carry every
   event kind the gate compares, with no host-side subscriber in the loop.
2. **No logging regression.** The tracing events exist for real
   observability (OTLP, structured logs); they must keep firing.
3. **Don't churn the frozen contract.** ADR-0048 froze the
   `ConformanceEvent` output model + `CONFORMANCE_EVENT_VERSION` + the golden
   file. β.6 already shipped against them. Changing the *sourcing* must not
   change the *output*.

ADR-0048 anticipated this: it noted the wasm profile "must reproduce whatever
we pick" and that the chosen contract should be "an artifact we own rather
than tracing internals." Single-channel sourcing is the resolution of that
foresight, now that the wasm boundary is concrete.

## Decision

**Promote the four tracing-only gate events to typed `RunEvent` variants**,
and source `ConformanceEvent` from **`run_ir_streaming` alone** — one typed
channel, in both the dev and wasm profiles.

New `RunEvent` variants (emitted by `run_ir_streaming` directly):

```rust
RunStarted,
ContextStepRan         { step, tokens_in, tokens_out },
InferenceCallStarted,
InferenceCallCompleted { stop_reason, tokens_in, tokens_out },
```

(joining the existing `ToolCallStarted` / `ToolCallCompleted` /
`RunCompleted`.)

Consequently:

- **`tau-conformance` maps `ConformanceEvent` from the typed `RunEvent`
  stream only.** The std `Captor` tracing layer, the `map_tracing`
  translation, and the yield-barrier dual-channel **interleave are deleted**.
- **Tracing events stay for logging.** The same call sites still emit their
  `tracing` events; conformance simply stops re-parsing them. There is no
  logging or OTLP regression.
- **The frozen output contract is unchanged.** The `ConformanceEvent` model,
  its version, and the golden `expected_events.json` are byte-for-byte the
  same; only where the events come from changes. `dev == golden` keeps
  passing without a re-bless.
- **The wasm guest exports the typed stream.** β.7.5's WIT `run` export
  returns the serialized typed `RunEvent` stream; the gate's `dev == wasm`
  arm (`fan_monitor_dev_matches_wasm`) goes live in β.7.5 PR-G with no
  host-side subscriber.

## Consequences

Positive:

- The conformance stream is now fully reproducible from a `no_std` wasm
  guest — the β.6 / Phase-β capstone (`dev == wasm`) is unblocked.
- One sourcing path instead of two: no yield-barrier interleave to reason
  about, no causal-ordering argument tied to the single-threaded executor,
  no string-parsing of free-form tracing fields.
- The gate is decoupled from `tracing` entirely; incidental log churn can no
  longer perturb conformance (a goal ADR-0048 pursued via the owned model;
  this completes it at the source).

Negative / obligations:

- `RunEvent` (in `tau-runtime-core`) grows four variants — a typed-API
  surface change. `run_ir_streaming` must emit them at the same points the
  tracing events fire, with the same field payloads, or dev/wasm parity
  silently drifts. A drift test pins the two emission sites together.
- The deleted `Captor` / `map_tracing` code path removes the only consumer
  that proved the tracing-vs-typed correspondence; that correspondence is no
  longer asserted (it no longer needs to be, since there is one channel).

Neutral:

- ADR-0048's golden-file bless workflow (`TAU_CONFORMANCE_BLESS=1`) is
  retained unchanged.
- Recorded in the β.7.5 design spec (§5 WIT world, §10 conformance
  integration, §14 ADRs) and the ROADMAP β.7.5 status line.

## Alternatives considered

- **Keep dual-channel; run a `no_std` tracing subscriber in the guest.**
  Rejected: `tracing-subscriber` is std-only and string-formats records; a
  bespoke `no_std` re-implementation in the guest would be a large,
  parity-fragile surface whose entire output we'd then have to match against
  the host's — exactly the brittleness ADR-0048 wanted to avoid.

- **Emit `ConformanceEvent`s directly from the engine** (skip `RunEvent`).
  Rejected: `ConformanceEvent` is a conformance-crate concept; making
  `tau-runtime-core` depend on it inverts the dependency direction and bakes
  a test artifact into the kernel. The typed `RunEvent` enum is the kernel's
  own vocabulary and already crosses the WIT boundary.

- **Leave the four events tracing-only and exclude them from the wasm arm.**
  Rejected: `ContextStepRan` carries the β.4 context-pipeline token deltas —
  the precise behavior the canonical scenario exists to prove. Dropping it
  from the wasm comparison would make the cross-profile gate blind to the
  thing it most needs to verify (the same objection ADR-0048 raised against
  the `RunEvent`-only alternative, now in the opposite direction).
