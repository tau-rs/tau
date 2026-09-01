# HANDOFF — redesign backlog & implementation tree

**From:** the 2026-09-01 brainstorm session (branch
`claude/pipeline-creation-patterns-dsidka`).
**To:** the session responsible for creating the backlog and the associated
implementation tree.
**Authority:** the consolidated design is
[`../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md`](../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md)
— **read it first, in full.** Its §10 decisions ledger is LOCKED: do not
re-litigate locked decisions; §10's "residual open items" are yours to settle
inside ADRs. The 2026-08-29 framing doc carries a supersession banner — trust
the banner, not its Lane-1a sections.

---

## Your mission (in order)

1. **Update the active backlog** —
   `docs/superpowers/plans/vision-roadmap.md`: add the redesign epics (below),
   mark EPIC 5.3 superseded (its acceptance criterion is invalidated by
   design §1), re-point the epic DoD ("one agent → 3 frontends" becomes "one
   project → one IR, three surfaces"), retire δ.2's QuickJS line and the β.8
   "one way to write a project (TOML or TS)" framing.
2. **Write the ADR wave** (Phase 0). Use `docs/decisions/template.md`; note
   the numbering collisions (0022×2, 0028×2, 0044×2) and record them in
   `docs/decisions/README.md`. Minimum set:
   - ADR: the three-surface split + `[steps]`/`[tools] native=` removal
     (supersedes parts of ADR-0002 vocabulary placement; amends ADR-0041).
   - ADR: the synth contract (subprocess, ProjectConfig JSON, sandbox,
     double-synth check; supersedes ADR-0041's static-extraction decision,
     preserves its one-validation-path decision — cite which sub-decisions
     carry forward).
   - ADR: multi-pipeline IR v3 + pipeline imports (unblocks
     `SubflowKind::Compose`; `IrModule::entry_agent()` → entry pipeline).
   - ADR: the journal (event-sourced record substrate; retires HTTP-VCR;
     `CheckpointGranularity::EventSourced` becomes real; snapshots =
     optimization).
   - ADR: the ops lane (env pins, `tau plan` + `schemas/plan/` + exit codes,
     atomic apply, run-or-refuse for wasm, `[[moved]]`).
   - ADR: the instruction-set roadmap (the 15 kinds + planes + extension
     rules + rejections ledger — one umbrella ADR referencing the design's
     §3; individual v2 step kinds get their own ADRs when built).
   - ADR: agent-exposure surfaces (skill/AGENTS.md emitters, MCP facade
     plan, agent-grade CLI contract).
   - Amendments: ROADMAP "killed item" (argued narrowing, never silent),
     CONSTITUTION G6 + cheatsheet + QG12, `tau-philosophy.md` §"What you
     author" (three surfaces; drop "TS is sugar"), ADR-0022 (tau-workflow)
     superseded banner.
3. **Write the per-phase implementation plans** in
   `docs/superpowers/plans/` using the house format (`YYYY-MM-DD-<slug>.md`,
   "For agentic workers" header, Goal/Architecture/Tech-Stack, Global
   Constraints incl. CARGO RULES from `CLAUDE.md`, checkbox TDD tasks with
   Files/Interfaces/Steps). One plan per Phase 0–4 epic below; keep tasks
   small and independently green.
4. **Create/extend the implementation trees** in
   `docs/superpowers/implementation-trees/` — one per epic family (authoring
   surfaces; instruction set; ops lane; exposures), linking plans → ADRs →
   design sections, per the living-documentation system described in
   `ARCHITECTURE.md`'s header.

## The epics (Phase → epic → key contents)

| Epic | Phase | Contents (design §) | Acceptance |
|---|---|---|---|
| E-0 Align & clean | 0 | deletions (tau-workflow, tau-plugin-base, landlock-exec-repro, embed_c stubs, stale examples), ADR wave, doc amendments (§11 Phase 0, §8) | repo no longer contradicts the design; CI green; `xtask/tests/architecture_md.rs` updated with the crate deletions |
| E-1 Rust declarations | 1 | proc-macro crate, unified registry, real content hashes, `tau.gen.ts`, `schemas/project-manifest/`, legacy-lane deletion (§1, §3.4, §4) | one tool authored via `#[tau::tool]` flows to gen + check + card; name-hash hole closed |
| E-2 Flow lane | 2 | synth runner, `pipelines/`, IR v3, imports, removals, predicate algebra, structured access, `init --ts`, wasm feature repair (§1, §3.4) | north-star-v2 authors + builds; TOML twin byte-equal where applicable |
| E-3 Prove | 3 | journal + record/replay, `tau plan` + plan schema + exit codes, `inspect`, pipeline RunEvents, skill/AGENTS.md emitters, authoring skill, `tau new` (§2, §5, §6) | plan renders a capability-diff-first PR comment; a journal replays a Dynamic run with concurrent spawns |
| E-4 Local ops | 4 | env `local`, pins, apply + systemd adapters, `[[moved]]`, lockfile v8, remaining repairs (§5) | north-star-v2 applied, scheduled by timer, resumed after rename via moved record |
| (v2 epics — backlog only, don't plan yet) | — | ForEach/Sleep/Wait+Emit/retry-catch/variables/Explore; serve v2; MCP facade; environments/promote; OCI + gallery; Python consumer SDK; `tau add` packs (§3.2, §6, §11) | — |

## Constraints & conventions you must honor

- `CLAUDE.md` **CARGO RULES** (CARGO_TARGET_DIR per role, `-p` crate scoping,
  timeouts, `CARGO_INCREMENTAL=0`, nextest) and PUSH/ISSUE rules.
- Branch discipline: this session's branch is
  `claude/pipeline-creation-patterns-dsidka`; your work belongs on your own
  designated branch(es). Never stack on merged history.
- No-flag-day: every removal per the design's stated deprecation path;
  ADR-0065 governs every new format (authored strict, interchange
  version-gated — the synth JSON strictness ruling is one of your ADRs).
- IR versioning: new step kinds = MINOR; multi-pipeline = the single MAJOR
  (v3.0.0) with a frozen v2 reader; schema files + `REACHABLE-TYPES.md` +
  conformance fixtures move together (`UPDATE_SCHEMA=1` flow in
  `crates/tau-ir/tests/schema_export.rs`).
- The four UX requirements (design §12) are acceptance criteria, not polish.
- Related-work citations for any published claim: design §9 (and read
  AgentSPEX arXiv:2604.13346 / POLARIS arXiv:2601.11816 full texts before
  novelty claims).

## Key code anchors (verified this session)

`crates/tau-pkg/src/project/project.rs` (schema; `parse_str_at` merge point) ·
`crates/tau-pkg/src/project/dirs/` (the `pipelines/` extension point; its
reserved-kinds comment names `steps` — update) ·
`crates/tau-ir/src/{module,pipeline,check,budget,durable,trigger}.rs` ·
`crates/tau-ir-lower/src/lower/` (7 stages; `Caches` closures) ·
`crates/tau-runtime-core/src/interpreter/` (`dynamic.rs` counters,
`output_store.rs`, `pipeline.rs`) · `crates/tau-cli/src/cmd/project_load.rs`
(the synth dispatch hook) · `cmd/build.rs:533,597` (the name-hash sentinel) ·
`crates/tau-ports/src/target/registry.rs:136-139` (the wasm feature repair) ·
`crates/tau-pkg/src/{lockfile,scope,install_sandbox}.rs` (v8 provenance;
ScopeConfig = env overlay shape; the synth sandbox port) ·
`crates/tau-cli/src/cmd/mcp/` (the pin/diff precedent for plan) ·
`tau_runtime_core::embed` (the harness surface; dogfood `tau dev` onto it).

## Session artifacts (visual references, private links)

Synth contract: <https://claude.ai/code/artifact/85b4dcdc-3383-4a8c-9aee-8d1585ba5ec3> ·
v1 scope: <https://claude.ai/code/artifact/93cb837a-a131-4977-8d28-b292bb20ab5a> ·
Integration surfaces: <https://claude.ai/code/artifact/b38d0398-a92b-4f2e-8c1b-5f030b4d5f96> ·
Instruction set: <https://claude.ai/code/artifact/f9269eaa-5744-4024-bba7-a148bc0587fe>

## Suggested first actions

1. Read the design doc end to end; skim the superseded 2026-08-29 framing
   for its still-valid §2/§3.3/§4/§5.
2. `gh issue list` / PR sweep per `CLAUDE.md` ISSUE RULES before creating
   tracking issues for the epics.
3. Draft the vision-roadmap.md backlog edit + E-0 plan first (it unblocks
   everything and touches no behavior); get the ADR wave reviewed as one PR
   train.
4. Keep each plan's tasks sized so a single agentic worker lands one green
   commit per task (superpowers:executing-plans discipline).
