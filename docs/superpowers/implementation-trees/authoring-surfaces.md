# Implementation Tree — authoring surfaces (three-surface split)

> ## ⚠️ THIS IS A LIVING DOCUMENT
>
> This tree is **alive**. It is the running map of how a tau project gets
> authored — the TOML vocabulary, the TypeScript choreography lane (synth),
> the Rust muscle lane (macros/registry), and the IR they lower into — and it
> **must be updated after every implementation** that touches this surface.
>
> **Update protocol (do all four in the implementing PR):**
> 1. Flip the node's status marker and stamp its PR number.
> 2. Add a dated bullet to **Discoveries** for anything non-obvious learned.
> 3. Move any now-built item out of **Next slices**; add newly-revealed work in.
> 4. If a node's public surface changed, update its row in **Nodes**.

**Scope:** epics **E-0** (align & clean) + **E-1** (Rust declarations) +
**E-2** (flow lane). Deliberately not covered: the proof/ops verbs
([ops-lane](ops-lane.md)), the step-kind vocabulary
([instruction-set](instruction-set.md)), consumer surfaces
([exposures](exposures.md), [tau-sdk-consumers](tau-sdk-consumers.md)).

**Sibling docs:** design
[`2026-09-01-tau-authoring-ops-and-primitives-design.md`](../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md)
§1/§3.4/§4 · plans
[E-0](../plans/2026-09-01-epic-e0-align-and-clean.md) ·
[E-1](../plans/2026-09-01-epic-e1-rust-declarations.md) ·
[E-2](../plans/2026-09-01-epic-e2-flow-lane.md) · ADRs
[0071](../../decisions/0071-three-surface-split.md) ·
[0072](../../decisions/0072-synth-contract.md) ·
[0073](../../decisions/0073-ir-v3-multi-pipeline.md) · backlog
[`vision-roadmap.md`](../plans/vision-roadmap.md).

---

## Legend

`✅ shipped (PR#)` · `🟡 in progress` · `⬜ not started` · `⛔ blocked` ·
`⟂ guarded by a drift/coverage/unit test`

---

## The tree

```
authoring surfaces — one project → one IR, three surfaces (ADR-0071)
│
├── paper trail (Phase 0 · E-0)
│   ├── ADR wave 0071–0077 + banners (0022, 0041) + collision note        ✅ (this PR train)
│   ├── backlog + ROADMAP retirements + G6/QG12 + philosophy amendments   ✅ (this PR train)
│   ├── tau-workflow deletion (crate + CLI verbs + docs tombstone)        ⬜ E-0 T2–T3
│   └── dead weight: tau-plugin-base · landlock-exec-repro · embed_c      ⬜ E-0 T4–T6
│
├── TOML / dirs — the vocabulary  (exists; narrowed by ADR-0071)
│   ├── [dirs] agents/tools scanning (ADR-0069/0070)                      ✅ shipped pre-redesign
│   ├── [steps] / [tools] native= deprecate-warn cycle                    ⬜ E-2 T9 (removal next cycle)
│   └── [synth] table (entry/format/runner)                               ⬜ E-2 T1
│
├── Rust — the muscle  (E-1)
│   ├── tau-macros: #[tau::tool] / #[tau::deterministic]                  ⬜ E-1 T1–T2
│   ├── unified registry (native + wasm dispatchers, one source)          ⬜ E-1 T2   ⟂ dev==wasm conformance
│   │       crates/tau-native-tools/src/{lib,registry}.rs
│   ├── real content hashes (name-hash hole closed)                       ⬜ E-1 T3
│   │       crates/tau-cli/src/cmd/build.rs:533,597 (sha256_name sentinel)
│   ├── registry caps → lattice + card                                    ⬜ E-1 T7
│   ├── schemas/project-manifest/ (drift-tested)                          ⬜ E-1 T4   ⟂ schema_export
│   ├── tau.gen.ts emitter + stale-gen loud error                         ⬜ E-1 T5–T6 ⟂ gen_ts_drift
│   └── legacy lane deleted (tau-ts-extract, sdk/ts, sdk/python)          ⬜ E-1 T8 (gated on E-2 runner for the ts example)
│
├── TypeScript — the choreography  (E-2)
│   ├── synth subprocess runner + tau-sandbox-native profile              ⬜ E-2 T2
│   │       crates/tau-cli/src/cmd/project_load.rs (dispatch hook)
│   ├── merge at unchecked level; surface violations = hard errors        ⬜ E-2 T3
│   │       crates/tau-pkg/src/project/project.rs (parse_str_at)
│   ├── pipelines/ scanning (id = file path)                              ⬜ E-2 T4
│   │       crates/tau-pkg/src/project/dirs/ (reserved-kinds comment!)
│   ├── @tau/sdk — generated L1 + handle/predicate L2 (design §4)         ⬜ E-2 T7
│   ├── tau init --ts golden path                                         ⬜ E-2 T9
│   └── CI double-synth byte-identity gate                                ⬜ E-2 T11
│
└── IR v3  (E-2)
    ├── pipelines: BTreeMap<PipelineId, Pipeline> (v3.0.0, frozen v2 rdr) ⬜ E-2 T5   ⟂ schema_export + conformance
    ├── pipeline imports → SubflowKind::Compose (acyclic, namespaced)     ⬜ E-2 T6
    ├── predicate algebra + ${steps.x.output.field} JSON-pointer access   ⬜ E-2 T8
    └── any-wasi-strict feature-set repair                                🟡 PR #687 in flight (pre-redesign scope)
            crates/tau-ports/src/target/registry.rs:136-139
```

---

## Nodes

| Node | Path | Public surface | Status |
|---|---|---|---|
| ADR wave | `docs/decisions/0071..0077-*.md` | the redesign's decision record | ✅ this PR train |
| tau-workflow deletion | `crates/tau-workflow/`, `crates/tau-cli/src/cmd/workflow/` | removes `tau workflow *` | ⬜ E-0 |
| tau-macros | `crates/tau-macros/` (new) | `#[tau::tool]`, `#[tau::deterministic]`, `tau::export![]` | ⬜ E-1 |
| unified registry | `crates/tau-native-tools/src/registry.rs` (new) | `ToolRegistration`, `find()`, `invoke()` | ⬜ E-1 |
| project-manifest schema | `schemas/project-manifest/` (new) | manifest JSON schema (ADR-0072) | ⬜ E-1 |
| tau.gen.ts | `crates/tau-sdk-codegen/src/gen_ts.rs` (new) | typed bindings, hash-stamped | ⬜ E-1 |
| synth runner | `crates/tau-pkg/src/project/synth.rs` (new) | `run_synth() -> UncheckedProjectConfig` | ⬜ E-2 |
| @tau/sdk | `sdk/tau-sdk/` (new) | `pipeline()`, handles, predicates, `defineFragment` | ⬜ E-2 |
| IR v3 | `crates/tau-ir/src/{module,pipeline,subflow}.rs` | `pipelines` map; Compose | ⬜ E-2 |

**Invariants (don't break without a deliberate slice):**
- `[allow]` is never emittable by any code path — synth, macro, or SDK (ADR-0071).
- One validation path: every surface merges at the unchecked level before the single `validate()` (ADR-0072); collisions are hard errors, never overrides or auto-suffixes.
- TOML-only projects pay nothing: no `[synth]` ⇒ no subprocess, no Node, no gen files (design §12).
- The guest registry path stays `no_std`; dev and wasm dispatch from the same records (the PR-G `dev == wasm` property).
- IR versioning: schema + `REACHABLE-TYPES.md` + conformance fixtures move together; v3.0.0 is the only MAJOR (ADR-0073).

---

## Discoveries (append-only, dated)

- **2026-09-01** (backlog session) `tau-plugin-base` is a Dockerfile dir, not a crate — the `architecture_md.rs` gate ignores it (`is_crate()` false), and `tau-sandbox-container/src/runner.rs` references it in doc-comments only; but docker/CI may still *build* the image — E-0 T6 carries a stop-and-verify step instead of assuming dead weight.
- **2026-09-01** (backlog session) `landlock-exec-repro` has its own `Cargo.lock` and is not a workspace member — deletion is reference-cleanup only.
- **2026-09-01** (backlog session) The `architecture_md.rs` gate is **forward-only** (missing crates fail; stale names don't) — crate deletions must update `ARCHITECTURE.md` by discipline, not by test pressure.
- **2026-09-01** (backlog session) PR #687 (`feat/621-wasm-guest-flip`) already flips `any-wasi-strict` to Branch/Parallel/Loop — E-2 T10 must check its merge state first; the redesign's repair may reduce to the v3/Compose extension.
- **2026-09-01** (backlog session) `tau-observe` has a dev-dep cycle note on `tau-workflow` (its Cargo.toml ~line 45) — the E-0 deletion must resolve what its `workflow_run_log` layer tests need.

---

## Next slices (ranked)

1. **E-0 T1–T3** — verify the merged paper trail; delete `tau-workflow` + docs tombstone. Unblocks everything; zero behavior change.
2. **E-0 T4–T7** — dead-weight deletions + stale-reference sweep.
3. **E-1 T1–T4** — macros → registry → content hashes → manifest schema (the muscle spine; T3 closes a real integrity hole).
4. **E-1 T5–T7** — gen + stale-gen gate + lattice wiring.
5. **E-2 T1–T5** — synth spine ([synth] → runner → merge → pipelines/ → IR v3). The MAJOR bump rides here; schedule it when no other IR-touching PR is in flight.
6. **E-1 T8** — legacy-lane deletion (only after the E-2 runner covers the ts smoke example).
7. **E-2 T6–T11** — Compose, SDK L2, predicates, init --ts, wasm repair, double-synth CI.
