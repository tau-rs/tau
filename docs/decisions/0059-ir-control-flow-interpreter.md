# ADR-0059: IR control-flow interpreter semantics

**Status:** Accepted (Decision 4 superseded in part — see note below)
**Date:** 2026-07-18
**Deciders:** tau core

> **Update (2026-08-09, EPIC 4.3):** Decision 4's "loud named error" behavior
> for `Suspend` has been superseded. The interpreter now checkpoints the
> pipeline's outputs and step position and pauses, returning
> `PipelineOutcome::Suspended { run_id, resume_signal, step_id }` instead of
> erroring; `tau run --resume <id> --signal <name>` restores the checkpoint
> and continues past the pause. `RuntimeError::SuspendNotImplemented` is
> retired; the remaining error case (a caller that runs a pipeline without
> wiring a `SuspensionStore`, e.g. the bundle-run path) returns
> `RuntimeError::SuspendUnsupported` instead. See
> [Author a suspend step](../how-to/authoring-suspend.md) for the shipped
> authoring/resume surface.

## Context

ADR-0058 (EPIC 4.1) added four recursive structured-block variants to `tau-ir`'s `StepRun`
(`Branch`/`Parallel`/`Loop`/`Suspend`) plus a reusable `Condition`, bumped `ir_format` to
v2.4.0, and typechecked the blocks' *shape*. 4.1 shipped the blocks as **inert data**: the
interpreter (`tau-runtime-core`) returns a generic `RuntimeError::Internal("… lands in EPIC
4.2")` stub whenever the pipeline walk reaches any control-flow block.

ADR-0058 §Consequences also **explicitly deferred** one design hole to this story: how
*nested* steps inside a block scope their ids and outputs. 4.1's typecheck validates that
nested steps reference real nodes, but it *rejects* a `Loop.until` that reads its own body's
output, because nested-scope visibility was not yet modeled.

EPIC 4.2 (this ADR) resolves that deferral and specifies the execution semantics for
`Branch`, `Parallel`, and `Loop`, plus the interim behavior for `Suspend`. `Suspend`'s full
HITL checkpoint/resume round-trip remains out of scope — that is EPIC 4.3.

## Decision

Four decisions, taken together, define the interpreter walk added in EPIC 4.2.

### Decision 1 — nested scope: flat global namespace + execution-order visibility

**This resolves ADR-0058's deferred nested-scope note.**

Every `PipelineStepId` in the whole pipeline tree (top-level and nested) is **globally
unique**. There is **one flat `OutputStore`** keyed by bare id. The template grammar is
**unchanged** (`${steps.<id>.output}`; bare id everywhere). A reference (a `Condition`'s
`Locus::Output`, or a `${steps.<id>.output}` template ref) resolves iff the referenced step
**executed before** the reference point in the structured walk.

"Executed before" is *execution-order*, not pure lexical order, and differs per block:

| Reference site | In-scope steps |
|---|---|
| `Branch.on` | outer-earlier only (condition runs *before* `then`/`otherwise`) |
| `Branch.then`/`otherwise` step | outer-earlier + prior siblings in that arm |
| `Loop.until` | outer-earlier **+ the loop body's ids** (until runs *after* each body pass) |
| `Loop.body` step | outer-earlier + prior body ids (+ previous iteration's outputs at runtime) |
| `Parallel` branch step | outer-earlier + that branch's own prior steps **only** (never sibling branches — they are concurrent) |

A downstream step reads a block's result by **bare id** (`${steps.<inner>.output}`), because
the store is flat and block-transparent. In a `Loop`, a body step's output key is
**overwritten each iteration** (last-iteration-wins).

### Decision 2 — `Parallel`: bounded cooperative fork-join (nginx model)

`tau-runtime-core` is `no_std` + `alloc` and **executor-agnostic** (only `futures-core`
traits; it cannot spawn tasks — the same code runs in the wasm guest and on embassy). So
`Parallel` concurrency is **cooperative multiplexing inside the single interpreter task**:
each branch fires its LLM/tool request and parks on `.await`; the driver polls the next
branch, firing *its* request. All requests are in flight at once, on one thread — the nginx
event-loop model, which is exactly right for τ's I/O-bound agent/tool branches.

- Each branch forks a **read-only `store.clone()`** (no `&mut` aliasing), runs via `run_steps`,
  and returns `(branch_index, produced_store)`.
- Branches are driven with `futures_util::stream::iter(futs).buffered(PARALLEL_CAP)` — a
  **bounded** number in flight (`PARALLEL_CAP = 8`, a hardcoded const for 4.2; "make it
  configurable" is a later story), and `buffered` **preserves result order** so the join is
  deterministic.
- At join, each branch's produced keys are **merged back into the shared store in
  branch-index order**. Because branches only *add* keys (Decision 1 isolates their reads to
  their own branch), the merge is a straight key insertion.
- Branch futures are polled in the same task → **no `Send` bound** is required between them,
  and no `Spawn` port is introduced. The interpreter stays executor-agnostic.

### Decision 3 — `Loop`: own bounded walk, hard exhaustion, feedback threading

**3a. Exhaustion is a hard error.** When `body` has run `max_iters` times and `until` never
held, the interpreter returns `RuntimeError::LoopExhausted { step, max_iters }` and the run
aborts. A loop that cannot reach its goal within its mandatory bound is a failure, not a
silent success — consistent with ADR-0058's "no runaway agents" and the no-silent-wrong-result
stance.

**3b. `Loop` gets its own small bounded walk** and shares only the *primitives* with the
existing `Check` gate-rewind — condition evaluation (`eval_condition`, the existing
`evaluate_goal`, already reused by `Branch.on`) and the feedback-injection idiom. It does
**not** literally reuse the flat top-level gate-rewind index loop, whose flat-index
arithmetic (`gate_index`, `i = gate_idx`) would require projecting nested ids into the flat
pipeline index — fighting Decision 1's tree walk. A `Check` *inside* a loop body still
rewinds within its own body via the per-slice check logic in `run_steps`, so check-retry
composes without literal reuse.

**3c. Feedback threads across iterations.** When iteration *N*'s `until` fails, its rationale
is carried into iteration *N+1*'s body as a prior agent turn (`"Previous attempt rejected:
<rationale>"`), reusing the existing `feedback: Option<String>` idiom (`pipeline.rs` +
`split_history` in `agent_loop`). A looping agent that is told *why* it is being asked again
converges faster; a blind re-roll is much weaker and more likely to exhaust.

### Decision 4 — `Suspend`: loud named error (round-trip deferred to 4.3)

The full HITL round-trip (checkpoint run state → emit event → wait for `resume_signal` →
seed-and-skip resume) is EPIC 4.3. In 4.2 the recursive walk reaching a `Suspend` (top-level
or nested) returns `RuntimeError::SuspendNotImplemented { step, resume_signal }` and aborts.
This keeps 4.1's never-silently-skip discipline while not shipping a half-built pause that
looks resumable but cannot resume. The only change vs today's generic `Internal` stub is a
**named** variant, giving 4.3 a clear seam.

## Consequences

**Positive:**

- The interpreter (`tau-runtime-core`) executes `Branch`, `Parallel`, and `Loop` for real;
  the recursive walk (`run_steps`) is shared by the top level and every nested body.
- No `ir_format` bump. 4.2 changes only the interpreter and the `tau-ir-lower` typecheck; the
  IR wire format (v2.4.0, from ADR-0058) is unchanged, and existing serialised workflows stay
  byte-stable.
- The nested-scope deferral from ADR-0058 is closed: `Loop.until` reading its own body's
  output now typechecks and executes; nested `PipelineStepId`s are uniqueness-checked across
  the whole tree; `Parallel` branch isolation and nested-template ref-checking are enforced at
  typecheck (tau build time), not runtime — consistent with the Rust-like build-time
  enforcement principle.
- `Parallel`'s bounded cooperative fork-join gets real wall-clock concurrency for I/O-bound
  branches without adding an executor dependency, so the interpreter continues to run
  unmodified in the wasm guest and on embassy.
- A `Spawn` port is left as the deliberate future seam for compute-heavy `StepRun::Dynamic`
  regions (EPIC 4.4+), rather than being introduced now for a workload (I/O-bound agent/tool
  branches) that does not need it.

**Negative / obligations:**

- Two new named `RuntimeError` variants (`LoopExhausted`, `SuspendNotImplemented`) replace the
  4.1 generic `Internal(...)` stubs; any code matching exhaustively on `RuntimeError` must
  handle them.
- `Suspend` still aborts the run in 4.2 — no workflow using `Suspend` can complete until EPIC
  4.3 ships the checkpoint/resume round-trip.
- **Known limitation surfaced during implementation:** under the flat-global model, a
  top-level step that references a `Branch` arm step which did **not** execute (because only
  one of `then`/`otherwise` runs per invocation) yields a runtime unresolved-template error at
  the point of reference, not a typecheck-time diagnostic — the reference is only invalid
  *conditionally*, depending on which arm executes. `Loop` bodies do not have this hazard:
  they always run at least once, so a downstream reference to a loop-body output is always
  safe once the loop step itself resolves. This is treated as an honest hard-error rather than
  silently resolving to an empty/default value; a typecheck-time warning on such
  conditionally-populated refs is a possible later story, not required for 4.2's DoD.
- `Parallel` concurrency is capped at `PARALLEL_CAP = 8` (hardcoded); making it configurable
  is deferred to a later story.

## Alternatives considered

**Decision 1 — hierarchical/pathed scope.** A namespace keyed by path
(`${steps.<block>.<child>.output}`, ids unique only per-block) was rejected: it changes the
template grammar, the `Locus::Output` shape (an IR schema bump), and the `OutputStore`
keying, buying cross-block id reuse that hand-authored τ workflows do not need.

**Decision 1 — block-encapsulated outputs.** Making nested outputs invisible outside their
enclosing block is a dead-simple lexical rule, but it makes loops/branches output-opaque, so
downstream steps could not consume their results without inventing a new "block export" IR
feature. Flat-global is the only option where "read a loop/branch result downstream" works
with zero contract change.

**Decision 2 — sequential `Parallel` (concurrency = 1).** Semantically correct but makes
`Parallel` pointless: two 5s LLM waits take 10s instead of ~5s.

**Decision 2 — true multi-thread parallelism via a new `Spawn` port** (apache prefork model).
Delivers simultaneous *CPU* across branches, but forces a new port plus three host
implementations (tokio real, wasm/embassy no-op — which just re-does the cooperative path
anyway) and a `Send + 'static` sweep through the whole interpreter (`agent_loop` /
`tool_dispatch` / `check`), for capacity that I/O-bound agent workflows do not use. If a
future compute-heavy `Dynamic` region (EPIC 4.4+) needs it, that is the right place to
introduce a `Spawn` port deliberately.

**Decision 3 — literal reuse of the flat top-level gate-rewind loop for `Loop`.** Rejected:
the flat-index arithmetic (`gate_index`, `i = gate_idx`) would require projecting nested ids
into the flat pipeline index, fighting Decision 1's tree walk. `Loop` gets its own small
bounded walk and shares only the condition-eval and feedback primitives with `Check`
gate-rewind.

**Decision 3 — stop-and-continue on loop exhaustion.** Rejected: shipping the last
un-converged output and exiting 0 silently hides a loop that never reached its goal,
violating the no-silent-wrong-result stance. Exhaustion is a hard error
(`LoopExhausted`).

**Decision 4 — emit a `RunOutcome::Suspended` / `RunEvent::Suspended` terminal now.**
Rejected: this introduces a public contract that looks resumable but is a dead end until
EPIC 4.3, which would then have to reshape it. A named hard-error (`SuspendNotImplemented`)
keeps the seam honest without committing to an unfinished public shape.
