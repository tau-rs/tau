# ADR-0053: Opt-in turn-level checkpoint/resume (durable execution A-minimal)

**Status:** Accepted
**Date:** 2026-06-20
**Deciders:** tau core

## Context

tau's only durability surface was run-end-only: `RunSnapshot`
(`tau-ports/src/orchestration.rs`) captures aggregate accounting when a
run finishes. A crash at turn 9 of a 12-turn agent loop restarts at turn
1 and re-bills 8 completed LLM calls.

Phase B (shipped — `docs/how-to/run-tau-under-a-durable-orchestrator.md`,
ROADMAP NG5) made the *whole bundle* a safe-to-retry reentrant unit, so a
host orchestrator (Temporal / Inngest / Cloudflare Workflows) owns
durability across bundle invocations. But durability under Phase B is
whole-bundle: an expensive multi-turn loop *inside* one bundle is opaque
to the orchestrator and re-runs from the top on retry.

A-minimal closes that intra-bundle gap with opt-in per-turn
checkpoint/resume, while staying inside NG5 (tau is not a general-purpose
workflow engine) and NG6 (no built-in DB). It deliberately avoids
event-sourced replay (Phase A-full), which carries a permanent
engine-determinism tax and is gated until a concrete workflow needs
exactly-once.

The substrate already supports this cheaply: β.1 put every
nondeterminism source behind a port, and β.4's context pipeline is
*stateless* (transformers re-apply to the message history each turn,
accumulating nothing). So the resumable state is just the message
history + token accounting + turn number — all already serde-derived.

## Decision

A durable agent persists a `TurnCheckpoint` after each completed turn;
`tau run --resume <run_id>` rehydrates the last committed checkpoint and
re-enters at the next turn. **No replay** — re-enter at the last
committed turn boundary.

**Semantics: at-least-once for the crashed turn only.** Committed turns
never re-run; the single in-flight turn re-runs from its start. Tools
under a durable agent **must be idempotent** — a side effect in the
crashed turn may fire twice if the crash lands after it but before the
checkpoint write. Exactly-once is A-full's job, not A-minimal's.

Four sub-decisions:

- **D1 — The checkpoint write lives behind a `CheckpointStore` port.** A
  new trait in `tau-ports` (`persist` + `load_latest`). The agent loop in
  `tau-runtime-core` (`#![no_std]`) calls `store.persist(&ckpt)` after
  `TurnCompleted` when the agent is durable; the tokio host supplies a
  `FileCheckpointStore`. Mirrors the existing `Clock` / `RandomSource` /
  `CapabilityResolver` injection. Keeps core I/O-free *and* lets the
  kill-and-resume DoD test run in-core against an in-memory
  `MockCheckpointStore` with no tokio harness.

- **D2 — Ship `PerTurn` + `File` only; enums `#[non_exhaustive]`.**
  `CheckpointGranularity::PerToolCall` requires mid-turn (partial-turn)
  rehydration — a harder state machine that leaks turn internals into the
  checkpoint format — and only *narrows*, never closes, the at-least-once
  window, so it is deferred. `DurableStore::Kv` is an MCP-contracted
  journal (NG6) and belongs to A-full. Both land later as additive minor
  bumps, exactly as `output_schema` did (IR v1.3.0).

- **D3 — Per-turn snapshot files, keep-all, atomic-rename.**
  `.tau/runs/<run_id>/turn-<n>.json`, each the full history at turn *n*;
  resume reads the highest *n*. Write is `*.tmp` → atomic rename, so a
  crash mid-write leaves `turn-<n-1>.json` intact — the resume point,
  which is the promised at-least-once boundary. A single appended JSONL
  was rejected: a torn final line is a parsing problem, and full-history
  rehydration gets no storage win from JSONL without delta complexity
  A-minimal exists to avoid.

- **D4 — Drop the handoff's `context_state` field.** β.4's context
  pipeline carries no cross-turn runtime state, so `TurnCheckpoint` is
  `{ run_id, turn, history: Vec<Message>, tokens: TokenUsage }`.
  Rehydration feeds `history` back in and the pipeline re-derives
  deterministically.

## Consequences

- Per-agent durable execution is real on the IR-interpreter path
  (`tau run`, `tau run --bundle`, `tau dev`). It composes with Phase B:
  the orchestrator still owns *when* to retry; A-minimal narrows *how
  much* re-runs per retry.
- **IR format version: MINOR bump v2.0.0 → v2.1.0.** `Agent` gains
  `durable: Option<Durability>` with `#[serde(default,
  skip_serializing_if = "Option::is_none")]`. The field is invisible when
  absent, so a durable-absent module's canonical bytes differ from v2.0.0
  *only* in the version string — the honest signal that the schema grew
  (same treatment as `triggers` / `checks` / `context`). Drift tests in
  `tau-ir` and the `tau-runtime-tokio` mirror assert v2.1.0.
- New port surface in `tau-ports`: `TurnCheckpoint`, `CheckpointStore`,
  `CheckpointError`. New `RunOptions.checkpoint_store:
  Option<Arc<dyn CheckpointStore>>` in `tau-runtime-core`.
- New host types in `tau-runtime-tokio`: `FileCheckpointStore`,
  `NoopCheckpointStore`. New CLI flag: `tau run --resume <run_id>`.
- TypeScript authoring parity: `tau-ts-extract` carries the `durable`
  field; TOML↔TS canonical IR stays byte-equal (conformance fixture).
- Conformance: a new `16_durable_per_turn` fixture cross-mode conforms;
  all existing fixtures stay green (none are durable). Their absolute
  bundle hashes shift with the version bump — expected on any IR-format
  change.
- **Honest limit.** At-least-once, not exactly-once. Documented as the
  idempotency contract; teams needing exactly-once wait for A-full.

## Alternatives considered

- **Feature-gated I/O inside core** (`#[cfg(feature = "host-fs")]` write
  after `TurnCompleted`) instead of a port. Rejected: it sprinkles `cfg`
  branches through the no_std hot path and forces the kill-and-resume
  test onto a real filesystem + tokio host, where a port lets it run
  in-core with a mock. The port also matches the established
  side-effect-injection discipline.
- **Ship `PerToolCall` now.** Rejected for A-minimal: mid-turn
  rehydration is materially more complex and buys only a narrower (still
  unsafe) window, not exactly-once — a half-measure paid for in
  complexity. Deferred to a follow-up; the `#[non_exhaustive]` enum makes
  it additive.
- **Single appended JSONL run log** (reusing workflow v1's shape).
  Rejected: torn-line fragility and no storage benefit under
  full-history rehydration (see D3).
- **Carry β.4 `context_state` in the checkpoint** (per the original
  handoff). Rejected as unnecessary: the context pipeline is stateless,
  so there is nothing to serialize beyond the message history (D4).
- **Event-sourced replay now** (A-full). Rejected/deferred: it imposes a
  permanent replay-determinism tax on every future engine change; gated
  until a concrete exactly-once need exists and β.6 is green on both
  profiles.
