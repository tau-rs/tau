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
- The guest gains a regex engine (`regex-automata`, alloc-only config), paid
  by every component including ones whose IR uses no goal predicates.
  Two separate measurements, different methods — do NOT add them into a
  single headline:

  | What | Method | Result |
  |---|---|---|
  | The registry itself | `goal_predicates::matches_` body stubbed out, narrow (`unicode-case`+`unicode-perl`) feature set | 2,558,423 B vs 2,015,233 B → **+543,190 B (~530 KiB)** |
  | Widening the features to restore parity | `tau build wasm` on `fixtures/wasm-build/pipeline`, narrow vs `unicode`+`unicode-word-boundary` | 2,550,340 B vs 2,786,007 B → **+235,667 B (~230 KiB, +9.2%)** |

  The two used different fixtures and different toggles, so the honest
  statement is "roughly 0.5 MiB for the registry, plus ~230 KiB to make its
  accepted language match the native graph" — not a single summed figure.
  Accepted: the alternative (a leaner engine) costs cross-target parity, see
  below.

  **Resolved by #689** — a component now pays only for the predicates its
  baked IR can actually reach. `tau-wasm-guest/build.rs` decodes the IR it
  bakes and emits `tau_goal_predicates` / `tau_goal_matches`; the guest then
  either omits `goal_registry` entirely, routes through
  `goal_predicates::invoke_alloc_only` (the four allocation-only
  predicates), or links the full table. Nothing about a linked predicate's
  MEANING changes — a build that can reach `matches` links the identical
  engine the native registry uses, so the parity contract below is
  untouched.

  A Cargo feature could not express this: features resolve before build
  scripts run, so nothing at feature-resolution time knows which IR is being
  baked. The lever is reachability — the guest stops *referencing*
  `matches_`, and wasm-ld garbage-collects the engine, the same mechanism
  the `tau_cap_net_http` / `tau_cap_fs_*` arms use.

  | Fixture | Reaches | Component |
  |---|---|--:|
  | `wasm-build/trivial` (one agent) | nothing | 1,988,124 B |
  | `wasm-build/pipeline` (two agents) | nothing | 1,988,452 B |
  | `north-star` (`matches` in Branch + Loop) | full table | 2,799,802 B |

  Measured with `tau build wasm`, `wasm32-wasip2` release, at the commit
  that landed #689. The `pipeline` fixture was 2,777,914 B before the gate,
  so the saving on it is **789,462 B (~771 KiB, 28.4%)** — one end-to-end
  differential on one fixture, NOT the sum of the two rows above. It exceeds
  the 543,190 B row because that one stubbed only the `matches_` body,
  pre-widening; unlinking the registry outright also drops `regex_syntax`
  (the parser — the larger half of the regex code section, per #679) and
  regex's Unicode tables in the data section.
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
  makes the interpreter and the predicate registry dead code. #689's gate is
  therefore guarded by its own tests, not by that budget:
  `goal_free_component_links_no_regex_engine` and
  `predicate_without_matches_runs_in_guest_without_the_regex_engine`
  (`tau-cli/tests/build_wasm_e2e.rs`). Both assert the ABSENCE of regex
  symbols, which a build that stopped emitting a name section would satisfy
  vacuously, so the north-star wasm leg carries the paired positive
  assertion that a `matches`-using component still links the engine.
  Absent `TAU_IR_BYTES`, the scan links everything — which is what keeps
  CI's standalone link gate covering the full predicate path.
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
