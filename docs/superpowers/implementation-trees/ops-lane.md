# Implementation Tree — the ops lane (record · plan · pin · apply)

> ## ⚠️ THIS IS A LIVING DOCUMENT
>
> This tree is **alive**. It is the running map of tau's operational verbs —
> the journal and its views, the pin/plan/apply cycle, the permission sheet —
> and it **must be updated after every implementation** that touches them.
>
> **Update protocol (do all four in the implementing PR):**
> 1. Flip the node's status marker and stamp its PR number.
> 2. Add a dated bullet to **Discoveries** for anything non-obvious learned.
> 3. Move any now-built item out of **Next slices**; add newly-revealed work in.
> 4. If a node's public surface changed, update its row in **Nodes**.

**Scope:** epics **E-3** (prove) + **E-4** (local ops), and the v2/v3 ops arc
(environments/promote/fleet) as backlog. Not covered: authoring
([authoring-surfaces](authoring-surfaces.md)), step kinds
([instruction-set](instruction-set.md)), exposure emitters
([exposures](exposures.md) — `tau plan`'s PR-comment rendering lives *here*,
its skill/AGENTS.md siblings live *there*).

**Sibling docs:** design
[`2026-09-01-tau-authoring-ops-and-primitives-design.md`](../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md)
§2/§5/§12 · ADRs [0074](../../decisions/0074-journal-record-substrate.md) ·
[0075](../../decisions/0075-ops-lane-local-first.md) ·
[0043](../../decisions/0043-trigger-ingress.md) ·
[0053](../../decisions/0053-turn-level-checkpoint-resume.md) · plans
[E-3](../plans/2026-09-01-epic-e3-prove.md) ·
[E-4](../plans/2026-09-01-epic-e4-local-ops.md) · backlog
[`vision-roadmap.md`](../plans/vision-roadmap.md).

---

## Legend

`✅ shipped (PR#)` · `🟡 in progress` · `⬜ not started (planned epic)` ·
`🔵 v2/v3 backlog` · `⟂ guarded by a drift/coverage/unit test`

---

## The tree

```
the ops lane — prove it, pin it, apply it (ADR-0074/0075)
│
├── the journal (the record substrate)                                E-3
│   ├── event model + append-only store (.tau/runs/<id>/journal.jsonl)   ⬜ T1  ⟂ schemas/journal drift
│   ├── interpreter records every nondeterministic crossing              ⬜ T2
│   │       keyed (instance path, per-instance seq) — never global order
│   ├── replay: interpreter = pure fn(frozen IR, journal)                ⬜ T3
│   │       ReplayDivergence named; Dynamic-with-spawns fixture = DoD
│   ├── tau record / tau replay (--live-tools; age shown; --refresh)     ⬜ T4
│   ├── HTTP-VCR cassettes retired                                       ⬜ T4
│   └── snapshots demoted to replay-shortcut optimization (ADR-0053 am.) ⬜ T3/T4
│
├── plan (the review verb)                                            E-3/E-4
│   ├── semantic diff, capability changes ALWAYS first                   ⬜ E-3 T6
│   │       precedent: crates/tau-cli/src/cmd/mcp/ pin+diff
│   ├── exit codes 0 / 2 / 3 (widens caps) / 1                           ⬜ E-3 T6
│   ├── schemas/plan/ JSON twin                                          ⬜ E-3 T6  ⟂ drift
│   ├── capability-diff-first PR comment (CI how-to)                     ⬜ E-3 T6 (DoD)
│   └── plan defaults to the local pin                                   ⬜ E-4 T2
│
├── inspect (the permission sheet)                                       ⬜ E-3 T7
│       app-store-grade card rendering; --attempt demonstrates denial
│
├── pin + apply (the ops verbs)                                       E-4
│   ├── env local + .tau/envs/local.state.toml (committed, secret-free)  ⬜ T1
│   ├── tau apply — atomic per repo; --pipeline valve; widening gate     ⬜ T3
│   ├── systemd-user timer adapters from [trigger] (ADR-0043)            ⬜ T4
│   │       retry-policy encoding deliberately absent in v1 (ADR-0075)
│   ├── [[moved]] records → plan rename-not-replace + resume remap       ⬜ T5
│   ├── lockfile v8 [synth] provenance                                   ⬜ T6
│   └── wasm run-or-refuse per environment (never silent narrowing)      ⬜ T8
│
└── the v2/v3 arc (backlog only — same pin model, no rework)
    ├── environments + promote                                           🔵 v2
    ├── serve v2 (socket, sessions, reverse dispatch)                    🔵 v2
    ├── OCI distribution + bundle gallery                                🔵 v2
    ├── k8s adapters · adapter probes · GitOps how-to                    🔵 v3
    └── fleet matrix · catalog · policy packs                            🔵 v3
```

---

## Nodes

| Node | Path | Public surface | Status |
|---|---|---|---|
| journal | `crates/tau-runtime-core/src/journal/` (new), `schemas/journal/` | `JournalEvent`, sink/replay ports, jsonl format | ⬜ E-3 |
| record/replay | `crates/tau-cli/src/cmd/{record,replay}.rs` (new) | the verbs + `--live-tools`/`--refresh` | ⬜ E-3 |
| plan | `crates/tau-cli/src/cmd/plan.rs` (new), `schemas/plan/` | verb + exit codes + JSON twin | ⬜ E-3 |
| inspect | `crates/tau-cli/src/cmd/inspect.rs` (new) | the permission sheet + `--attempt` | ⬜ E-3 |
| env pin | `crates/tau-pkg/src/envs.rs` (new) | `EnvState` + `.tau/envs/local.state.toml` | ⬜ E-4 |
| apply | `crates/tau-cli/src/cmd/apply.rs` (new) | atomic apply + adapters | ⬜ E-4 |
| lockfile v8 | `crates/tau-pkg/src/lockfile.rs` | `[synth]` provenance section | ⬜ E-4 |

**Invariants (don't break without a deliberate slice):**
- The journal is keyed `(instance path, per-instance seq)` — a global-order key anywhere is the VCR bug reborn (ADR-0074).
- A replay mismatch is a **named** `ReplayDivergence`, never a silent wrong answer.
- Snapshots are caches of journal prefixes, never an independent truth.
- Plan output: only governance deltas may be loud; capability changes render first; exit 3 is reserved for widening (design §12).
- The pin is committed and secret-free by construction; apply is atomic per repo (slicing is the labeled escape valve).
- Structural wasm capabilities are never narrowed post-build: run-or-refuse (ADR-0075).
- Every ops feature is opt-in by file presence; rung N never taxes rung N−1 (design §12).

---

## Discoveries (append-only, dated)

- **2026-09-01** (backlog session) `crates/tau-cli/src/cmd/mcp/` already implements a pin-then-diff cycle (`tau mcp pin`) — `tau plan` generalizes an in-tree pattern, not greenfield.
- **2026-09-01** (backlog session) `CheckpointGranularity::EventSourced` already exists as an enum variant (`crates/tau-ir/src/durable.rs:76`) with no substrate — the journal makes it real; no IR format change needed for E-3 T1–T3.
- **2026-09-01** (backlog session) ADR-0075 settles the state-file field list and defers adapter retry-policy encoding to the v2 Time/trigger ADR — E-4 T4 emits units without retry policy *by decision*, not omission.

---

## Next slices (ranked)

1. **E-3 T1–T3** — journal substrate + replay (everything else in the lane reads from it).
2. **E-3 T5** — pipeline RunEvents (frozen NDJSON contract; also unblocks exposures).
3. **E-3 T6** — plan + schema + exit codes (the CI gate primitive; E-4 pins plug into it).
4. **E-3 T4, T7** — record/replay CLI + inspect.
5. **E-4 T1–T4** — pin → plan-on-pin → apply → adapters.
6. **E-4 T5–T8** — moved records, lockfile v8, repairs, run-or-refuse + the epic DoD run.
