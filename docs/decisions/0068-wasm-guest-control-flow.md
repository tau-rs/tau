# ADR-0068: Wasm guest executes IR control-flow in-guest

**Status:** Accepted
**Date:** 2026-08-22
**Deciders:** Titouan Lebocq

## Context

`tau build --target wasm-guest` refuses any workflow containing
Branch/Parallel/Loop/Suspend/Dynamic because `any-wasi-strict` declares
`supported_features: &[]` (ADR-0059 feature-fit). The refusal is honest: the
guest (`tau-wasm-guest`) drives only a single entry agent via
`run_ir_streaming` (ADR-0053) and hard-rejects pipeline-bearing IR. Issue
#621 asks for the capability itself: executing control-flow inside the wasm
component, so the north-star fixture (#461) can run as a wasm bundle.

Two candidate architectures:

1. **In-guest interpretation** — the component runs the same `run_pipeline`
   interpreter the native path uses (EPIC 4.2, ADR-0059 semantics).
2. **Host-driven stepping** — the wasmtime host walks the pipeline
   (evaluating Branch/Loop conditions host-side) and calls a new fine-grained
   guest export (`run-agent(agent-id, messages-json)`) per step.

Load-bearing facts established during design:

- `run_pipeline` is `no_std + alloc` and executor-agnostic: `futures-core` /
  `futures-util` only, no tokio, no spawn. Parallel is bounded cooperative
  fork-join (`buffered(8)`, ADR-0059 Decision 2) that runs inside a single
  task — i.e. on the guest's existing `block_on` executor.
- The guest already links `tau-runtime-core` (`wasm-interpreter` feature) and
  implements `ToolDispatcher` (`GuestDispatcher`).
- The only missing dependency is a `DeterministicRegistry` for Branch/Loop
  condition evaluation: the production registry lives in `tau-cli` and is
  std-only (`regex`, `jsonschema`).

## Decision

**In-guest interpretation.** Concretely:

1. The guest, when the baked IR carries a pipeline, drives
   `tau_runtime_core::interpreter::pipeline::run_pipeline` and returns the
   rendered last non-check/non-suspend leaf output as the `run` export
   payload — mirroring the native `ir_dispatcher::run_via_ir` /
   `render_pipeline_result` contract. The WIT world is unchanged.
2. The five std-free goal predicates
   (`__tau::goal::{exists, non_empty, equals, matches, min_count}`) move to a
   shared `no_std + alloc` home, `tau_native_tools::goal_predicates`
   (feature-gated; `matches` via `regex-automata`, `default-features =
   false`). The CLI's `BuiltinDeterministicRegistry` delegates to it and
   keeps only the `jsonschema`-backed `schema_valid`. The guest's
   `GuestDispatcher` exposes the shared registry.
3. `AdapterFamily::Wasi` (`any-wasi-strict`) declares
   `supported_features: &[Branch, Parallel, Loop]`. **Suspend stays
   build-time refused** (no `SuspensionStore` channel in the WIT world; the
   non-suspendable `run_pipeline` would error at runtime). **Dynamic stays
   build-time refused** (not executed natively either, pending EPIC 4.5).
4. A wasm-only build-time fn-availability gate in `tau-ir-lower` (next to
   `feature_fit`) refuses `GoalPredicate::SchemaValid` and `Deterministic`
   steps naming fns outside the guest-answerable set — no
   "builds-fine-dies-in-guest" runtime surprises.
5. Parity contract for this ADR is **terminal outcome**, not event stream:
   no streaming `run_pipeline` variant exists on any target (the native CLI
   renders the last leaf non-streaming). Pipeline event streaming and the
   ADR-0048 `WasmProfile` conformance leg remain future work.

## Consequences

- Native and wasm execute the *same compiled interpreter* — flat-global
  scope, Parallel snapshot/merge order, Loop feedback threading (ADR-0059)
  cannot diverge between targets. Cross-target semantic parity is by
  construction, per the spirit of ADR-0048.
- The wasm artifact stays fully self-executing (β.7.5 contract): every
  embedder keeps providing only `complete`/`now-millis`/`next-u64`/
  `emit-event`.
- The guest gains a regex engine (`regex-automata`, alloc-only config).
  **Measured** (release, `wasm32-wasip2`, same fixture IR, `goal-predicates`
  on vs stubbed off): 2,558,423 B vs 2,015,233 B — **+543,190 B (~530 KiB,
  ~27%)**, paid by every component including ones whose IR uses no goal
  predicates. Accepted: the alternative (a leaner engine) costs cross-target
  parity, see below. Reducing it by cfg-gating the registry on the baked IR
  is tracked in #689.
- **The engine's feature list is a parity contract, not a size knob.** The
  predicate source is shared, but cargo feature unification decides the
  accepted regex language per graph: in the `tau-cli` graph
  `jsonschema → fancy-regex` pulls `regex-automata` up to full Unicode,
  while the guest links only what `tau-native-tools` declares. The initial
  `unicode-case`/`unicode-perl` pair was NARROWER, so `\b` and `\p{…}`
  patterns compiled natively and failed to compile in-guest — and a compile
  failure was reported as `met: false`, silently flipping a Branch to its
  `otherwise` arm on wasm only. Fixed by declaring `unicode` +
  `unicode-word-boundary` (so the guest is a superset of what authoring
  accepts) and by making an uncompilable pattern an error rather than a
  verdict. `goal_predicates::matches_parses_the_same_language_the_native_
  graph_does` fails if the list is trimmed again.
- Note that the `TAU_WASM_SIZE_BUDGET` gate does NOT bound this cost: it
  builds the guest with no `TAU_IR_BYTES`, so the empty-IR early return
  makes the interpreter and the predicate registry dead code.
- Obligation: the north-star fixture (#461) extends to a wasm execution leg
  (build succeeds; `tau_wasm_host::run_component` yields the same terminal
  sentinel as the dev leg); the build-refusal witness retargets to a
  Suspend-bearing twin. The registry emptiness test becomes an
  exact-set test (`{Branch, Parallel, Loop}`).
- Future Suspend-on-wasm has a clean path (a host store import mirroring
  ADR-0053's checkpoint pattern) without revisiting this decision.

## Alternatives considered

**Host-driven stepping** — rejected. It creates a second pipeline
interpreter whose semantics must be kept bit-identical to `run_pipeline` by
hand, permanently reintroducing the cross-target divergence risk ADR-0048
exists to eliminate. It breaks the fully-linked-guest contract: the artifact
stops being self-executing, so every host (CLI, browser EPIC 5.6, no_std
embed EPIC 7.x) must reimplement the stepping protocol; it requires a
breaking WIT world change (per-step exports, message history serialized
across the boundary both ways); and it splits execution awkwardly — the
agent-internal tool loop stays in-guest while the pipeline loop moves
host-side. Its advantages (std registry for free, host-side Suspend) are
small against that, and Suspend has a non-breaking future path anyway.

**Full feature flip including Suspend** — rejected. The guest cannot
checkpoint durably today; admitting Suspend at build time would convert an
honest build refusal into a guaranteed runtime error
(`SuspendUnsupported`), violating the feature-fit contract (ADR-0059).

**Shipping `schema_valid` in the guest** — rejected for now. No maintained
`no_std` JSON-Schema validator exists; vendoring a subset validator is
speculative scope. Build-time refusal keeps the failure honest and early.
