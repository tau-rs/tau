# ADR-0074: The journal — one event-sourced record substrate

**Status:** Accepted (records locked decision §10.10 of the
[2026-09-01 consolidated design](../superpowers/specs/2026-09-01-tau-authoring-ops-and-primitives-design.md);
Phase 0 ADR wave)
**Date:** 2026-09-01
**Deciders:** maintainer, via the 2026-09-01 brainstorm session
**Amends:** ADR-0053 (durability — `CheckpointGranularity::EventSourced`
becomes real; per-turn snapshots demoted to optimization)
**Retires:** HTTP-VCR cassettes as the record/replay mechanism

## Context

tau has three partial answers to "what did this run do":
per-turn/per-tool-call checkpoints (ADR-0053) for durability, HTTP-VCR
cassettes for test replay, and run events/trace for observability. They
overlap without composing: cassettes key on **global turn order**, so any
concurrency (Dynamic spawns, Parallel branches) makes replay
order-fragile; a cassette mismatch replays the *wrong* recorded response
silently (false green) instead of failing; and checkpoints capture state
without the decisions that produced it, so a resumed run cannot be
audited. `CheckpointGranularity::EventSourced` exists as an aspiration in
`crates/tau-ir/src/durable.rs:76` with no substrate behind it.

Durable-execution systems solved this shape a decade ago: record every
nondeterministic decision as a typed event; recovery and replay are
fast-forwards through the log (Temporal; formalized by Burckhardt et al.,
OOPSLA 2021). Temporal's model carries a versioning curse — user code is
replayed, so code changes break old histories — that tau structurally
avoids: runs bind to an immutable IR hash, and **no user code is ever
replayed**.

## Decision

1. **One append-only journal per run:** `.tau/runs/<id>/journal.jsonl`,
   interchange-versioned per ADR-0065 (version-gated, tolerant reader).
2. **Every nondeterministic boundary crossing is a typed, recorded
   event**, keyed **`(instance path, per-instance seq)`** — never global
   order. Event families: LLM completions (with request hash), tool
   results, event deliveries, timer firings, spawn admissions/denials,
   judge verdicts, clock/random reads, compactions, signals,
   budget-tranche transitions, cancellations.
3. **The interpreter is a pure function of (frozen IR, journal).**
   Observable scheduling is journal-derived or seeded; instance-path
   keying makes concurrent regions replay-stable.
4. **Three views over the one substrate:**
   - **Durability:** resume = fast-forward through the journal.
     `CheckpointGranularity::EventSourced` becomes the real
     implementation; per-turn snapshots remain **only** as a
     replay-shortcut optimization (a snapshot is a cache of a journal
     prefix, never an independent source of truth).
   - **Testing:** `tau record` / `tau replay`. A request-hash mismatch
     during replay is a **named `ReplayDivergence` error** — never VCR's
     silent wrong-cassette false green. `--live-tools` replays recorded
     decisions but executes tools for real.
   - **Audit:** the journal is the canonical answer to "what did this
     run do"; OTel spans map from journal events (ADR-0077 surface).
5. **HTTP-VCR cassettes are retired** once journal replay covers their
   test surface (E-3); the cassette global-turn keying defect is closed
   by construction, not patched.

## Consequences

- One recording mechanism to maintain instead of three partial ones;
  record/replay, resume, and audit stop drifting apart.
- Journal honesty is a UX requirement, not polish (design §12):
  recording age is shown at replay time and a `--refresh` flow
  re-records — stale recordings must be visible, never ambient.
- Snapshot demotion means resume correctness is testable against the
  journal alone; a snapshot-vs-journal divergence is a bug in the
  snapshot path by definition.
- Obligations: journal event schema published + drift-tested (the
  run-event schema precedent); replay conformance fixtures including a
  Dynamic run with concurrent spawns (the E-3 acceptance criterion);
  migration of existing cassette-based tests; durable-store writes go
  through the existing store abstraction (ADR-0053's `DurableStore`).
- Cost accepted: journals grow with run length; compaction events are
  themselves journaled, and per-turn snapshots bound replay time.

## Alternatives considered

- **Fix VCR keying (per-instance cassettes) and keep three systems.**
  Rejected: repairs the worst symptom but leaves resume unable to answer
  audit questions and keeps three formats drifting.
- **Temporal-style replay of user code.** Structurally unavailable and
  undesirable: tau has no user code at runtime (ADR-0071); binding runs
  to a frozen IR hash gives determinism without the versioning curse.
- **Snapshots as the primary substrate (journal derived).** Rejected:
  snapshots are lossy (no decisions, no denials, no timing); deriving a
  journal from them is impossible, while deriving snapshots from a
  journal is a fold.
- **OTel traces as the record.** Rejected: traces are best-effort
  telemetry with no replay semantics and external retention; the
  journal is local, complete, and versioned — spans are a *view* of it.

## References

- Design: [`2026-09-01-tau-authoring-ops-and-primitives-design.md`](../superpowers/specs/2026-09-01-tau-authoring-ops-and-primitives-design.md) §2 (the journal), §12
- Literature: Burckhardt et al., "Durable functions: semantics for
  stateful serverless", OOPSLA 2021
- Related: ADR-0053, ADR-0065, ADR-0049 (typed conformance observable)
- Epic: E-3 in [`vision-roadmap.md`](../superpowers/plans/vision-roadmap.md)
