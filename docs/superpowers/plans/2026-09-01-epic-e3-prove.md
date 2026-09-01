# E-3 — Prove Implementation Plan (Phase 3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The proof verbs exist: an append-only **journal** records every nondeterministic crossing and `tau record`/`tau replay` are views over it (a Dynamic run with concurrent spawns replays deterministically); **`tau plan`** renders a capability-diff-first semantic diff with the 0/2/3/1 exit-code contract and a versioned JSON twin; **`tau inspect`** renders the permission sheet; pipelines emit **RunEvents**; the **skill + AGENTS.md emitters**, the authoring skill, and `tau new` make tau projects agent-discoverable.

**Architecture:** The journal lands in `tau-runtime-core` beneath the interpreter (`crates/tau-runtime-core/src/interpreter/` — `dynamic.rs` spawn counters, `output_store.rs`, `pipeline.rs` are the recording points), keyed `(instance path, per-instance seq)`, written through the ADR-0053 `DurableStore` to `.tau/runs/<id>/journal.jsonl`. Replay wraps the interpreter's effect ports with journal-fed fakes; `ReplayDivergence` is a named error on request-hash mismatch. `tau plan` diffs pinned-vs-current lowered IR semantically (the `tau mcp pin`/diff code in `crates/tau-cli/src/cmd/mcp/` is the in-tree precedent to generalize); its JSON twin is schema-frozen in `schemas/plan/`. RunEvents extend the frozen run-event schema additively (`crates/tau-runtime-core/src/stream.rs` + `schemas/run-event/`). Emitters live in `tau-sdk-codegen` (the `embed_js.rs` emitter + drift-test pattern).

**Tech Stack:** Rust; serde/schemars schema freezes; `cargo nextest`; NDJSON contract tests.

**Design:** [`../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md`](../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md) §2 (journal), §5 (plan/inspect), §6 (emitters), §3.4 (RunEvents repair), §12 (UX requirements).
**ADRs:** [0074](../../decisions/0074-journal-record-substrate.md) · [0075](../../decisions/0075-ops-lane-local-first.md) · [0077](../../decisions/0077-agent-exposure-surfaces.md) · [0053](../../decisions/0053-turn-level-checkpoint-resume.md) (amended).
**Trees:** [`../implementation-trees/ops-lane.md`](../implementation-trees/ops-lane.md) · [`../implementation-trees/exposures.md`](../implementation-trees/exposures.md)

## Global Constraints

- Every cargo command: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p <crate> <filter>` (repo CARGO RULES; never bare cargo; never workspace-wide).
- Commit with explicit identity: `git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "..."`.
- `tau-runtime-core` keeps its `no_std` boundary: journal *types* + fold logic are alloc-only; file IO stays behind the host-side store port (ADR-0053 pattern). Verify with the isolated `cargo check -p tau-runtime-core --no-default-features` guard after every core task.
- Schema discipline: journal + plan + run-event schemas are interchange (ADR-0065: version-gated), frozen with drift tests; additive changes only after freeze.
- UX requirements are acceptance (design §12): recording age shown at replay; only governance deltas may be loud in plan output.
- ISSUE RULES sweep before each task.

---

### Task 1: Journal event model + append-only writer

**Files:**
- Create: `crates/tau-runtime-core/src/journal/mod.rs` (`JournalEvent` enum — the design-§2 families: LlmCompletion{request_hash,..}, ToolResult, EventDelivery, TimerFired, SpawnAdmission/Denial, JudgeVerdict, ClockRead, RandomRead, Compaction, Signal, BudgetTranche, Cancellation; envelope `{instance_path, seq, at, event}`)
- Create: `schemas/journal/journal-event.v1.schema.json` + drift test (schemars freeze, run-event precedent)
- Modify: host-side store (the ADR-0053 `DurableStore` file adapter) for `.tau/runs/<id>/journal.jsonl` append

**Steps:**
- [ ] **Step 1 (red):** Type round-trip tests (serde, every variant); envelope ordering test: events for one instance path are seq-dense; schema drift test red until the schema is committed.
- [ ] **Step 2:** Implement types + writer; `UPDATE_SCHEMA=1` freeze; green incl. the no_std check.
- [ ] **Step 3:** Commit `feat(runtime-core): journal event model + append-only store (ADR-0074)`.

### Task 2: Interpreter records — every nondeterministic crossing

**Files:**
- Modify: `crates/tau-runtime-core/src/interpreter/` — the LLM call boundary (request hash), tool dispatch, `dynamic.rs` (spawn admissions/denials + pooled-counter transitions), timer/clock/random reads, judge verdicts, `output_store.rs` (compactions), signal/cancellation paths
- Test: a recorded north-star run's journal snapshot (stable modulo timestamps); a Dynamic-region run records admissions AND denials with instance paths

**Steps:**
- [ ] **Step 1 (red):** Fixture assertions on journal contents per boundary (drive with the cassette-less fake ports the conformance suite already uses).
- [ ] **Step 2:** Implement recording behind a `JournalSink` port (no_std-clean; None = zero-cost today's behavior — zero mandatory concepts, design §12).
- [ ] **Step 3:** Commit `feat(runtime-core): interpreter journals every nondeterministic crossing`.

### Task 3: Replay — the interpreter as a pure function of (IR, journal)

**Files:**
- Create: `crates/tau-runtime-core/src/journal/replay.rs` (journal-fed effect ports; scheduling derived from recorded order per instance path)
- Test: conformance — record a run, replay it, event streams identical; **the epic's DoD fixture: a Dynamic run with concurrent spawns replays deterministically**; a doctored request hash → named `ReplayDivergence { instance_path, seq, expected, found }` (never a silent wrong answer)

**Steps:**
- [ ] **Step 1 (red):** The three fixtures above.
- [ ] **Step 2:** Implement; green; `-p tau-ir-conformance` full pass.
- [ ] **Step 3:** Commit `feat(runtime-core): journal replay — ReplayDivergence, never false green`.

### Task 4: `tau record` / `tau replay` CLI + cassette retirement

**Files:**
- Create: `crates/tau-cli/src/cmd/{record,replay}.rs` (`record` = run with sink on; `replay <run-id>` (+`--live-tools`: decisions from journal, tools executed) ; recording age displayed; `--refresh` re-records)
- Modify: migrate cassette-based tests to journal fixtures; delete the HTTP-VCR machinery once the last consumer moves (`rg -ln "cassette" crates/ --type rust` is the inventory)
- Test: CLI integration on the north-star fixture

**Steps:**
- [ ] **Step 1 (red):** CLI tests: record→replay round-trip; age line present (`recorded 3 days ago` style); `--live-tools` executes the fake tool port while decisions come from the journal.
- [ ] **Step 2:** Implement; migrate cassette consumers crate-by-crate (each its own commit, suites green).
- [ ] **Step 3:** Final commit `refactor(runtime)!: retire HTTP-VCR cassettes (journal is the record substrate)`.

### Task 5: Pipeline RunEvents (the NDJSON contract repair)

**Files:**
- Modify: `crates/tau-runtime-core/src/stream.rs` + `schemas/run-event/` (additive variants: `StepStarted`, `StepCompleted`, `CheckEvaluated`, `Suspended` — design §3.4), `interpreter/pipeline.rs` (emission points)
- Modify: freeze note in the contract-compatibility doc (the NDJSON stdout stream is a frozen integration surface per ADR-0077)
- Test: schema drift + an event-stream conformance fixture (pipeline run → exact event sequence)

**Steps:**
- [ ] **Step 1 (red):** Fixture asserting the full expected sequence for a 3-step pipeline with one Check.
- [ ] **Step 2:** Implement; additive schema bump; green incl. `sdk/embed-js` normalize tests (`RunEvent` union grows — update `RunEvent.ts` + coverage test per the tree's invariants).
- [ ] **Step 3:** Commit `feat(runtime-core): pipeline RunEvents — the NDJSON contract is complete (design §3.4)`.

### Task 6: `tau plan` — semantic diff, exit codes, JSON twin

**Files:**
- Create: `crates/tau-cli/src/cmd/plan.rs` (source-vs-pin, pin-vs-pin, `--check`; generalize the diff approach in `crates/tau-cli/src/cmd/mcp/` pin/diff), diff core in a lib module (`tau-pkg` or a `tau-plan` module in `tau-cli` — keep it lib-testable)
- Create: `schemas/plan/plan.v1.schema.json` + drift test
- Test: fixtures — no-change (exit 0), non-capability change (exit 2), capability widening (exit 3), parse error (exit 1); rendering order: capability changes FIRST, always

**Interfaces:**
- Produces: `PlanReport { capability_changes: Vec<CapDelta>, changes: Vec<SemanticDelta>, .. }` (IR vocabulary, stable field names — the JSON twin IS the report serialized).
- Consumes: pinned IR (E-4 pins; until then, `--against <bundle|ir.json>` gives plan a target so E-3 ships testable without E-4).

**Steps:**
- [ ] **Step 1 (red):** The four exit-code fixtures + the ordering test + schema drift test.
- [ ] **Step 2:** Implement; green; commit `feat(cli): tau plan — capability-diff-first semantic diff, exit codes 0/2/3/1`.
- [ ] **Step 3:** **DoD demo:** wire a CI example rendering the plan as a PR comment (docs/how-to page + a workflow snippet — the capability-diff-first PR comment is the epic's acceptance).

### Task 7: `tau inspect` — the permission sheet

**Files:**
- Create: `crates/tau-cli/src/cmd/inspect.rs` (render the capability card app-store-grade: per-agent/tool/region grants, ceilings, budgets, targets; `--attempt <cap>` demonstrates denial by dry-running the gate)
- Test: golden-file rendering test on the north-star bundle; `--attempt` on an ungranted cap exits non-zero with the denial line

**Steps:**
- [ ] **Step 1 (red):** Golden file + attempt tests.
- [ ] **Step 2:** Implement over the existing card data (effective-capabilities machinery from `tau verify --bundle`); green.
- [ ] **Step 3:** Commit `feat(cli): tau inspect — the permission sheet (+ --attempt)`.

### Task 8: SKILL.md + AGENTS.md emitters

**Files:**
- Create: `crates/tau-sdk-codegen/src/{skill_md,agents_md}.rs` + drift tests (committed export == fresh emit); `crates/tau-cli/src/cmd/export.rs` (`--skill`, `--agents-md`)
- Test: emitter goldens generated from the north-star bundle (content: how to run each pipeline via CLI, capability card summary, exit codes, NDJSON pointers — generated from IR + card, never hand-written)

**Steps:**
- [ ] **Step 1 (red):** Golden tests; a bundle with two pipelines lists both with their typed inputs.
- [ ] **Step 2:** Implement; green; commit `feat(export): generated SKILL.md + AGENTS.md (ADR-0077)`.

### Task 9: Authoring skill + `tau new` + agent-grade CLI contract

**Files:**
- Create: `crates/tau-cli/src/cmd/new.rs` (`tau new fragment|pipeline|project` scaffolds per design §3.3), the official authoring-skill package (docs + skill dir per the two-layer-skills convention)
- Test: CLI help-budget test (`--help` trees ≤ 1,500 tokens — count with the repo tokenizer convention or bytes/4 heuristic, threshold asserted), deterministic exit-code table test, scaffold-builds test

**Steps:**
- [ ] **Step 1 (red):** The help-budget + scaffold tests.
- [ ] **Step 2:** Implement; green; commit `feat(cli): tau new scaffolds + agent-grade CLI contract (help budget, exits)`.

### Task 10: Epic close-out

**Steps:**
- [ ] **Step 1:** DoD: (a) plan renders a capability-diff-first PR comment (Task 6 Step 3 demo); (b) a journal replays a Dynamic run with concurrent spawns (Task 3 fixture) — both linked from the PR.
- [ ] **Step 2:** OTLP span mapping documented as a contract (docs page mapping journal events → spans; ADR-0077 obligation — docs-only).
- [ ] **Step 3:** Update [ops-lane](../implementation-trees/ops-lane.md) + [exposures](../implementation-trees/exposures.md) trees + `vision-roadmap.md` E-3 stories.
