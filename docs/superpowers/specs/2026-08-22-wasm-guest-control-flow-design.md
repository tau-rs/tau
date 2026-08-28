# Wasm guest control-flow execution (#621) — design

**Date:** 2026-08-22
**Issue:** #621 — wasm guest cannot execute Branch/Loop; north-star (#461) wasm
execution leg blocked at feature-fit
**Decision record:** ADR-0068 (in-guest pipeline interpretation)
**Related:** ADR-0053 (guest streaming), ADR-0058 (IR control-flow blocks),
ADR-0059 (control-flow interpreter semantics + feature-fit), ADR-0048
(cross-target conformance gate), #396 (EPIC 3.4 in-guest gate)

## Problem

`any-wasi-strict` declares `supported_features: &[]`
(`crates/tau-ports/src/target/registry.rs:138`), so `feature_fit` refuses any
workflow containing Branch/Parallel/Loop/Suspend/Dynamic at
`tau build --target wasm-guest`. The refusal is honest: the guest entry
(`crates/tau-wasm-guest/src/guest.rs:139`) hard-rejects pipeline-bearing IR and
only drives a single entry agent via `run_ir_streaming`. The missing capability
is executing control-flow *inside* the guest.

## Decision (approved 2026-08-22)

**Interpret the pipeline in the guest**: the wasm component calls the same
`run_pipeline` interpreter the native path uses. No host-driven stepping, no
WIT world change. Rationale and the rejected alternative are recorded in
ADR-0068; the short version:

- `run_pipeline` is already `no_std + alloc` and executor-agnostic — no tokio,
  no spawn; Parallel is cooperative `futures_util::stream::iter(..).buffered(8)`
  (ADR-0059 Decision 2 chose this precisely so no Spawn port is needed). The
  guest already links `tau-runtime-core` (`wasm-interpreter` feature), has a
  `block_on` executor and a `ToolDispatcher` (`GuestDispatcher`).
- One interpreter ⇒ native and wasm execute the *same compiled code*; semantic
  parity is by construction, which is the spirit of ADR-0048.
- The artifact stays fully self-executing (β.7.5 fully-linked-guest contract):
  hosts keep providing only `complete`/`now-millis`/`next-u64`/`emit-event`.

## Supported feature set on wasm

`AdapterFamily::Wasi` (`any-wasi-strict`) flips from `&[]` to:

```rust
supported_features: &[IrFeature::Branch, IrFeature::Parallel, IrFeature::Loop]
```

**`Suspend` stays refused at build time.** `run_pipeline` (the
non-suspendable wrapper) errors `SuspendUnsupported` at runtime; the guest has
no `SuspensionStore` port and no durable storage channel in the WIT world.
Adding one later is a clean extension (a host store import mirroring the
ADR-0053 checkpoint pattern) — out of scope here.

**`Dynamic` stays refused at build time.** It is not executed natively either
(`RuntimeError::DynamicRegionRequiresRuntimeGate`, pending EPIC 4.5).

## Architecture

```
              tau build --target wasm-guest  (feature-fit admits Branch/Parallel/Loop)
                                 │ canonical IR baked into the component
  HOST (wasmtime)                ▼ GUEST (wasm32-wasip2, no_std + alloc)
  ┌──────────────────┐  run(prompt)  ┌─────────────────────────────────────────┐
  │ tau-wasm-host    │ ────────────► │ guest.rs:                               │
  │                  │               │   pipeline?  ── no ──► run_ir_streaming │
  │  complete()  ◄───┼───────────────┼── HostLlmBackend       (unchanged)      │
  │  now-millis() ◄──┼───────────────┼── HostClock                             │
  │  next-u64()  ◄───┼───────────────┼── HostRandom   ── yes ─► run_pipeline   │
  │  emit-event() ◄──┼───────────────┼── (agent-loop RunEvents, best-effort)   │
  └──────────────────┘               │ GuestDispatcher                         │
       ▲    payload =                │   .deterministic_registry() = Some(     │
       └─── rendered last-leaf       │       GoalPredicateRegistry)  ← NEW     │
            step output              └─────────────────────────────────────────┘
```

### Guest execution path (mirrors `ir_dispatcher::run_via_ir`)

`guest.rs` replaces the `module.workflow.pipeline.is_some()` rejection with:

1. Compute the id of the last top-level step that is neither `Check` nor
   `Suspend` (the same "last leaf" rule as
   `crates/tau-cli/src/cmd/run.rs:609` and `ir_dispatcher.rs:361`). The rule
   is extracted to one shared helper in `tau-ir` so the three call sites
   cannot drift.
2. `block_on(run_pipeline(module, prompt, dispatcher))` → `OutputStore`.
3. Render the last-leaf value exactly as `render_pipeline_result`
   (`run.rs:746`): `Value::String` renders as its inner text, any other value
   as compact JSON. Return that string as the `run` export's `Ok` payload.

The single-agent path is byte-for-byte unchanged.

### Goal-predicate registry (the one genuinely new piece)

Branch `on` / Loop `until` conditions and `Check` steps evaluate through
`ToolDispatcher::deterministic_registry()`
(`tau-runtime-core/src/interpreter/pipeline.rs:469,549,352`). The guest's
`GuestDispatcher` returns `None` today, so any control-flow step would fail
with "needs a deterministic registry".

The production registry (`crates/tau-cli/src/cmd/builtin_registry.rs`) is
std-only (`regex` for `matches`, `jsonschema` for `schema_valid`). Split:

- **New:** `tau_native_tools::goal_predicates` — no_std + alloc
  implementations of the five predicates that need no std machinery:
  `__tau::goal::{exists, non_empty, equals, matches, min_count}`. `matches`
  uses `regex-automata` (`default-features = false`, `alloc`-compatible;
  already in the workspace dependency graph as `regex`'s engine). Behind a
  `goal-predicates` cargo feature so existing `tau-native-tools` consumers
  don't pull the regex engine. Exposes `GoalPredicateRegistry` implementing
  `DeterministicRegistry`.
- **Changed:** the CLI's `BuiltinDeterministicRegistry` delegates the five to
  the shared impl and keeps only the `jsonschema`-backed `schema_valid` (and
  its `StdFsArtifactReader`). Native behavior is unchanged; the predicate
  bodies just move to the shared crate. `tau-cli` gains a direct dependency on
  `tau-native-tools` (feature `goal-predicates`).
- **Guest:** `GuestDispatcher::deterministic_registry()` returns
  `Some(Arc::new(GoalPredicateRegistry))`.

Regex dialect note: `regex-automata`'s `meta::Regex` is the engine inside the
`regex` crate — same syntax, same match semantics. The existing build-time
pattern validation (patterns must compile) stays valid for the guest.

### Build-time fn-availability gate (wasm only)

Per the project stance "build-time check possible → MUST": a wasm build must
not succeed and then die inside the guest on a predicate the guest registry
cannot answer. Next to `feature_fit` in `tau-ir-lower`, a wasm-target check
walks the pipeline and refuses, with a `feature-fit`-style diagnostic:

- any condition/check using `GoalPredicate::SchemaValid` (no no_std
  JSON-Schema validator exists);
- any `Deterministic` step whose fn name is outside the guest-answerable set
  (the five shared predicates — matching the CLI, which also only answers
  builtins today).

Native targets are untouched.

### Terminal-outcome parity, not event parity

There is no streaming variant of `run_pipeline` anywhere — the native CLI
also runs pipelines non-streaming and renders the last leaf. "Same terminal
outcome" therefore means: **the guest's returned payload equals the native
run's rendered final message.** Agent-loop `RunEvent`s inside pipeline steps
are *not* emitted in this slice (native pipeline runs emit none either).
Pipeline event streaming and the ADR-0048 `WasmProfile`
(`fan_monitor_dev_matches_wasm`, still `#[ignore]`d) remain future work and
are unaffected.

## Definition of done (north-star, per #461 "each new construct extends THIS fixture")

- `crates/tau-cli/tests/north_star_demo.rs`:
  - `north_star_wasm_guest_build_is_refused_at_feature_fit` retargets to a
    Suspend-bearing twin of the fixture, so the refusal path stays witnessed.
  - New test: `tau build --target wasm-guest` on the Branch+Loop fixture
    **succeeds**; the component runs via `tau_wasm_host::run_component` with
    cassette `CompletionResponse`s carrying the fixture's canned text; the
    returned payload contains the same terminal sentinel the dev leg asserts
    (`escalation: …URGENT… / draft: …APPROVED…`), witnessing that the branch
    then-arm and the loop body both executed in-guest.
- `crates/tau-ports/src/target/registry.rs`: Wasi `supported_features` flip;
  `any_wasi_strict_supports_no_control_flow_features` replaced by a test
  asserting exactly `{Branch, Parallel, Loop}` (Suspend/Dynamic absent).
- Determinism invariant of `tau-wasm-host` holds: same component + prompt +
  responses ⇒ same payload (Parallel's `buffered` join is order-preserving,
  ADR-0059 Decision 2, so this is by construction).

## Risks

- **Artifact size:** `regex-automata` in the guest. Mitigation: minimal
  feature set (`meta` engines needed for `is_match` only), measure the
  wasm-guest size delta in the first slice; if unacceptable, fall back to the
  `nfa-pikevm`-only configuration (slow path, fine for predicates) before
  considering a reduced matcher.
- **no_std discipline:** `wasm32-wasip2` has std, so a std dep would compile —
  the guardrail is `tau-native-tools`'s and `tau-wasm-guest`'s `no_std`
  crate-level policy plus `default-features = false` on the new dep. The
  first slice proves it with the existing guest build test.
- **Parallel determinism under cassette:** host `complete()` answers from an
  ordered cassette; cooperative `buffered(8)` polls in deterministic order,
  so cassette consumption order is stable. The north-star fixture exercises
  Branch+Loop; a Parallel-in-guest case is covered by unit tests at the
  interpreter level (already existing, target-independent) and by the
  determinism invariant above.

## Out of scope

- `Suspend`/`Dynamic` on wasm (build-time refused, as today).
- Pipeline-level event streaming; `WasmProfile` / un-ignoring
  `fan_monitor_dev_matches_wasm` (ADR-0048 lane).
- User-registered deterministic fns (not wired natively either).
- `schema_valid` in the guest (build-time refused on wasm).
