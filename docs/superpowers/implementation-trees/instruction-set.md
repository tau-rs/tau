# Implementation Tree — the agentic instruction set

> ## ⚠️ THIS IS A LIVING DOCUMENT
>
> This tree is **alive**. It is the running map of tau's step-kind kernel,
> transverse planes, and primitive-pack vocabulary — what exists, what the
> redesign adds in v1, what v2 builds, and what is refused — and it **must be
> updated after every implementation** that touches the kernel or a pack.
>
> **Update protocol (do all four in the implementing PR):**
> 1. Flip the node's status marker and stamp its PR number.
> 2. Add a dated bullet to **Discoveries** for anything non-obvious learned.
> 3. Move any now-built item out of **Next slices**; add newly-revealed work in.
> 4. If a node's public surface changed, update its row in **Nodes**.

**Scope:** the step kinds, planes, predicate/verification vocabulary, and the
repairs lot. Not covered: how pipelines are *authored*
([authoring-surfaces](authoring-surfaces.md)), the record/ops verbs
([ops-lane](ops-lane.md)), consumer exposure ([exposures](exposures.md)).

**Sibling docs:** design
[`2026-09-01-tau-authoring-ops-and-primitives-design.md`](../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md)
§3 · umbrella ADR [0076](../../decisions/0076-agentic-instruction-set.md)
(kernel-closed/vocabulary-open + extension rules — **individual v2 kinds get
their own ADRs when built**) · plans
[E-2](../plans/2026-09-01-epic-e2-flow-lane.md) ·
[E-3](../plans/2026-09-01-epic-e3-prove.md) ·
[E-4](../plans/2026-09-01-epic-e4-local-ops.md) · backlog
[`vision-roadmap.md`](../plans/vision-roadmap.md) (v2 section).

---

## Legend

`✅ shipped (PR#)` · `🟡 in progress` · `⬜ not started (planned epic)` ·
`🔵 v2 backlog (no plan yet — needs its own ADR)` · `⛔ refused (design §8)` ·
`⟂ guarded by a drift/coverage/unit test`

---

## The tree

```
the instruction set — 15 kinds, 2 planes, 8 categories, 7 foundations (ADR-0076)
│
├── Flow
│   ├── sequence · Branch · Parallel · Loop                                ✅ (ADR-0058/0059; EPIC 4)
│   ├── Suspend (→ absorbed by WaitForEvent later)                         ✅ (4.3)
│   ├── Dynamic (delegation; ceiling+bounds+attenuation)                   ✅ (4.4/4.5)
│   ├── Compose (pipeline imports; unblocked by IR v3)                     ⬜ E-2 T6
│   ├── ForEach (ASL Distributed Map semantics)                            🔵 v2 (own ADR)
│   ├── Explore (Option B: budget + synthesis_reserve + judged exit)       🔵 v2 (own ADR; §3.2 pre-scope)
│   ├── per-step retry/catch — the FAULT plane                             🔵 v2 (own ADR; must settle saga-subsumption, ADR-0076)
│   ├── on_exit + cancellation                                             🔵 v2
│   └── runtime graph mutation / handoffs / ToT-LATS-MCTS / group chat     ⛔ refused (§8)
│
├── Compute
│   ├── Agent · Tool · Deterministic · Judge                               ✅ exist
│   └── user fn registry wiring (#[tau::deterministic])                    ⬜ E-1 T1–T2
│
├── Storage
│   ├── step outputs · conversation context · checkpoints                  ✅ exist
│   ├── the journal (master store; state = fold of the ledger)             ⬜ E-3 T1–T3
│   ├── variables + reducers (LangGraph channels ∘ ASL Assign)             🔵 v2 (own ADR)
│   └── blackboard / artifacts / exploration scratchpad                    🔵 v2
│
├── Messaging
│   ├── resume signals (--signal)                                          ✅ (ADR-0053)
│   ├── EmitEvent ↔ WaitForEvent (correlation keys; ⊇ Suspend)             🔵 v2 (own ADR)
│   └── typed human elicitation (resume_schema)                            🔵 v2
│
├── Time
│   ├── per-step timeout                                                   ✅ exists
│   ├── Trigger (cron/manual; declared input)                              ✅ + [trigger].input ⬜ E-4 T4 rides adapters
│   ├── Sleep (duration/until) · timeout-vs-event race                     🔵 v2 (own ADR; carries adapter retry-policy encoding, ADR-0075)
│   └── event triggers                                                     🔵 v2
│
├── Governance
│   ├── [allow] · lattice · budgets · bounds · late binding · sandbox      ✅ (EPIC 1, 4.4/4.5; extended by E-1 T7)
│   └── PARALLEL_CAP ruling: min(declared, host cap), host-config only     ⬜ E-4 T7 (ADR-0076 §8)
│
├── Verification
│   ├── Check · goals · deliverables (judged) · quality-retry rewind       ✅ (ADR-0044 deliverables-and-goals)
│   ├── predicate algebra (compare · and/or/not · two-locus)               ⬜ E-2 T8
│   ├── structured access ${steps.x.output.field} (JSON-pointer read)      ⬜ E-2 T8
│   └── goals retry authorable (OnFail no longer hardcoded Abort)          ⬜ E-4 T7
│
├── Observability
│   ├── run events + trace/OTel                                            ✅ exist
│   ├── pipeline RunEvents (StepStarted/…/CheckEvaluated/Suspended)        ⬜ E-3 T5
│   ├── tau plan / tau inspect                                             ⬜ E-3 T6–T7 (see ops-lane tree)
│   └── OTLP mapping documented as contract                                ⬜ E-3 T10
│
├── primitive packs (kernel closed, vocabulary open)
│   ├── defineFragment (scope, id, props) — the pack unit                  ⬜ E-2 T7 (SDK)
│   ├── tau new fragment scaffold                                          ⬜ E-3 T9
│   ├── best-of-N proof pack (ForEach → judge-reduce)                      🔵 v2 (needs ForEach)
│   └── rpc / broadcast / inbox packs                                      🔵 v2
│
└── repairs lot (design §3.4 — silent promises)                            ⟂ each closes with a test
    ├── any-wasi-strict empty feature set                                  🟡 PR #687 → E-2 T10
    ├── pipeline RunEvents missing                                         ⬜ E-3 T5
    ├── cassette global-turn keying                                        ⬜ E-3 T4 (journal retires VCR)
    ├── scalar-coercion of rendered args                                   ⬜ E-2 T8
    └── max_tokens · judge_model · output_schema · subflow args ·
        goals OnFail · cap-order hashes · decorative max_concurrency       ⬜ E-4 T7 (one commit each)
```

---

## Nodes

| Node | Path | Public surface | Status |
|---|---|---|---|
| control-flow kernel | `crates/tau-ir/src/{pipeline,subflow,check}.rs`, `crates/tau-runtime-core/src/interpreter/` | step kinds + typecheck + interpreter | ✅ through Dynamic |
| Compose | `crates/tau-ir/src/subflow.rs`, `crates/tau-ir-lower/` | mounted pipeline imports | ⬜ E-2 |
| predicates | `crates/tau-ir/src/check.rs`, `goal_predicates.rs` | predicate algebra + pointer access | ⬜ E-2 |
| journal | `crates/tau-runtime-core/src/journal/` (new) | `JournalEvent`, sink/replay ports | ⬜ E-3 |
| repairs | various (see anchors in plans) | promises become behavior | ⬜ E-2/E-3/E-4 |

**Invariants (don't break without a deliberate slice):**
- Kernel extends **by ADR only**; packs never add step kinds (ADR-0076).
- New step kinds = MINOR ir_format; multi-pipeline was the only MAJOR (ADR-0073).
- Fault plane (retry/catch) ≠ quality plane (Check→rewind) — never merge them.
- Every future feature must decompose into the seven foundations (Seal, Lineage, Contract, Formula, Gate, Ledger, Ceiling); an eighth foundation = foundational ADR.
- The §8 rejections ledger is binding; reopening needs a superseding ADR with new evidence.
- An accepted-but-unwired config key is a **defect** (repairs lot), never a roadmap item.

---

## Discoveries (append-only, dated)

- **2026-09-01** (backlog session) `crates/tau-native-tools` is already the one-source-of-truth for tool bodies across dev/wasm (`invoke()` match) — the E-1 unified registry generalizes an existing property rather than introducing one.
- **2026-09-01** (backlog session) `SubflowKind::Compose` has been reserved-and-rejected since v0 (`UnsupportedComposeSubflow`, `crates/tau-ir/src/error.rs:106`) — unblocking it is an error-variant retirement, not a new variant.
- **2026-09-01** (backlog session) The `any-wasi-strict` empty `supported_features` contradiction (registry comment says "cannot execute control-flow" while ADR-0068 shipped in-guest control flow) is mid-repair in PR #687 — coordinate, don't duplicate.

---

## Next slices (ranked)

1. **E-2 T6/T8** — Compose + predicate algebra (the v1 kernel additions).
2. **E-3 T1–T5** — journal + RunEvents (Storage/Observability substrate).
3. **E-4 T7** — the repairs lot (each one one commit, one test).
4. **v2 ADR queue** (in dependency order, per ADR-0076): ForEach → variables+reducers → WaitForEvent/EmitEvent (absorbs Suspend) → Sleep/Time (carries adapter retry-policy) → retry/catch (saga-subsumption analysis mandatory) → Explore (pre-scoped §3.2) → packs (best-of-N proof).
