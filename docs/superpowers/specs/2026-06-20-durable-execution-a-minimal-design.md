# Durable execution — Phase A-minimal: opt-in turn-level checkpoint/resume

**Status:** design, ready to implement
**Date:** 2026-06-20
**Predecessor:** Phase B (tau-as-reentrant-artifact-under-orchestrator),
shipped — see `docs/how-to/run-tau-under-a-durable-orchestrator.md`.
**Successor (gated, deferred):** Phase A-full (event-sourced replay,
exactly-once). Do NOT start until a concrete workflow needs exactly-once
that A-minimal's at-least-once cannot give, B + A-minimal are in use, and
the β.6 conformance gate is green on both profiles.

## Goal

Close the *intra-bundle* durability gap. Phase B made the whole bundle a
safe-to-retry unit, but a long multi-turn agent loop inside one bundle is
not checkpointed — a crash at turn 9 of 12 re-enters at turn 1 and
re-bills 8 completed LLM calls. A-minimal persists resumable state after
each completed turn; `tau run --resume <run_id>` re-enters at the next
turn. **No replay** — re-enter at the last committed turn. **No
engine-determinism tax.**

## Semantics

At-least-once **for the crashed turn only**. Turns 1..=N that committed a
checkpoint are never re-run. The single in-flight turn N+1 that crashed
re-runs from its start. A side-effecting tool in the crashed turn may
fire twice if the crash lands after the side effect but before the
checkpoint write. **Contract: tools used under a `durable` agent must be
idempotent.** Teams that genuinely cannot be idempotent must wait for
A-full's exactly-once — `per_tool_call` (a future granularity) only
*narrows* this window, it does not close it, so it is intentionally out
of scope here (see Decision 2).

## Key finding that shapes the design

The handoff proposed a checkpoint payload carrying β.4 context-manager
state (`context_state: ContextState`). **There is no such runtime
state.** β.4's context pipeline (`Vec<Arc<dyn ContextTransformer>>`,
`tau-runtime-core/src/context/`) is *stateless*: transformers are pure
functions re-applied to a clone of the message history each turn
(`stream.rs`), with nothing accumulated across turns. So the checkpoint
collapses to fully-serializable primitives, and rehydration is "feed the
history back in; the pipeline re-derives deterministically." The
handoff's open question #1 (serde of a manager holding an `LlmBackend`
handle) is moot.

## Locked decisions

### Decision 1 — checkpoint write lives behind a port (not feature-gated I/O)

A new `CheckpointStore` port in `tau-ports`. The agent loop in
`tau-runtime-core` (`#![no_std]`) calls `store.persist(&checkpoint)`
after `TurnCompleted` when the agent is durable; the tokio host supplies
a `FileCheckpointStore`. This keeps core I/O-free and mirrors the
existing `Clock` / `RandomSource` / `CapabilityResolver` injection
pattern. Decisive benefit: the DoD's kill-and-resume test runs in-core
against an in-memory `MockCheckpointStore`, no tokio harness needed.

```
tau-runtime-core (#![no_std])              tau-runtime-tokio (std)
  agent loop: after TurnCompleted,           FileCheckpointStore
    if durable { store.persist(&c)? } ─────▶   .tau/runs/<id>/turn-<n>.json
  store: Option<Arc<dyn CheckpointStore>>      (atomic rename)
  (NO fs, NO std)                            NoopCheckpointStore (durable off)
```

### Decision 2 — ship `PerTurn` + `File` only; enums `#[non_exhaustive]`

```rust
#[non_exhaustive] pub enum CheckpointGranularity { PerTurn }  // PerToolCall, EventSourced later
#[non_exhaustive] pub enum DurableStore { File }              // Kv later
```

`PerToolCall` requires mid-turn (partial-turn) rehydration — a materially
harder state machine that leaks turn internals into the checkpoint format
— and only narrows, not closes, the at-least-once window. Deferred to a
follow-up session. `Kv` is an MCP-contracted journal (NG6: no built-in
DB) and belongs to A-full. Both are additive minor bumps later, exactly
as `output_schema` was (IR v1.3.0).

### Decision 3 — per-turn snapshot files, keep-all, atomic-rename

`.tau/runs/<run_id>/turn-<n>.json`, each file the full history at turn
*n*. Resume reads the highest *n*. Write is `turn-<n>.json.tmp` →
atomic rename, so a crash mid-write leaves `turn-<n-1>.json` intact — the
resume point — which is exactly the promised at-least-once boundary. A
single appended JSONL was rejected: a torn final line is a parsing
problem, and because rehydration needs the *full* history, JSONL gets no
storage win without delta complexity (which A-minimal exists to avoid).
Keep all turn files for A-minimal (cheap, debuggable); a `--keep-last=k`
prune flag can come later.

## IR change (additive, MINOR `ir_format` bump)

```rust
// tau-ir/src/durable.rs (new — mirrors context.rs)
#[non_exhaustive]
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct Durability {
    pub checkpoint: CheckpointGranularity,
    pub store: DurableStore,
}
#[non_exhaustive] pub enum CheckpointGranularity { PerTurn }
#[non_exhaustive] pub enum DurableStore { File }

// tau-ir/src/node.rs — Agent gains:
#[serde(default, skip_serializing_if = "Option::is_none")]
pub durable: Option<Durability>,
```

`IrFormatVersion::CURRENT`: `v2.0.0` → `v2.1.0`. The field is invisible
when absent (`skip_serializing_if`), so a durable-absent module's
canonical bytes differ from v2.0.0 **only** in the version string — the
deliberate, honest signal that the schema grew (same treatment as
`triggers`, `checks`, `context`). All existing conformance fixtures
(none durable) stay cross-mode-equivalent; only their absolute bundle
hashes shift with the version, which is expected on any IR-format bump.

## Checkpoint payload (tau-ports, beside `RunSnapshot`)

```rust
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct TurnCheckpoint {
    pub run_id: RunId,                 // = String
    pub turn: u32,
    pub history: Vec<tau_domain::Message>,  // derives serde
    pub tokens: TokenUsage,                 // derives serde
}

pub trait CheckpointStore {
    fn persist(&self, ckpt: &TurnCheckpoint) -> Result<(), CheckpointError>;
    fn load_latest(&self, run_id: &RunId) -> Result<Option<TurnCheckpoint>, CheckpointError>;
}
```

## Author surface

```toml
[agents.fan-monitor.durable]
checkpoint = "per_turn"
store = "file"            # ./.tau/runs/<run_id>/turn-<n>.json
```

TypeScript (β.8) authoring carries the same field; TOML↔TS canonical IR
stays byte-equal (a conformance assertion).

## Flow

```
turn1✓→ckpt  turn2✓→ckpt … turn8✓→ckpt  turn9 ✗CRASH
                                   │
   tau run --resume <run_id> ──────┘→ load_latest()=turn8 → re-enter turn 9
```

## Touch points

- `tau-ir`: `durable.rs`, `Agent.durable`, version bump + tests.
- `tau-pkg` project config: `AgentEntry` gains the `durable` block.
- `tau-ir-lower`: `lower_durable` (TOML→IR); `tau-ts-extract` (TS→IR).
- `tau-ports`: `TurnCheckpoint` + `CheckpointStore` + `CheckpointError`.
- `tau-runtime-core`: `RunOptions.checkpoint_store`; persist after
  `TurnCompleted`; resume entrypoint rehydrating history+turn.
- `tau-runtime-tokio`: `FileCheckpointStore` (atomic rename, highest-n
  load) + `NoopCheckpointStore`.
- `tau-cli`: `tau run --resume <run_id>`.

## Definition of done

- An agent with `[durable] checkpoint = "per_turn"` survives a mid-run
  kill and resumes without re-billing completed turns (in-core unit test
  with `MockCheckpointStore`).
- Agents without the block behave identically to today (all existing
  conformance fixtures stay green cross-mode).
- IR round-trip: a durable-absent module emits no `durable` key
  (byte-stability test mirroring `checks`/`triggers`).
- A new conformance fixture (`16_durable_per_turn`) cross-mode conforms.
- The kill-and-resume test runs inside the normal test suite (no new
  required CI lane needed; the in-core unit test does the work the
  handoff's `durable-resume / linux` lane was for).

## Out of scope (explicit)

`PerToolCall`, `Kv`, `EventSourced`/replay, `--keep-last` prune,
checkpoint encryption, cross-host checkpoint portability. Each is an
additive follow-up.
