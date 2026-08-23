# EPIC 4.3 — Suspend (human-in-the-loop pause + resume)

Status: design — awaiting review
Date: 2026-08-09
Base: origin/main @ ee83f448 (EPIC 4.2b/c, #535)
Supersedes handoff decisions where noted (deliverables #1 and #4).

## Goal

Let a user **author** a `Suspend` pipeline step in `tau.toml`, have the engine
**checkpoint and pause** at it, and **resume** the run past it via an external
one-shot signal (`tau run --resume <id> --signal <name>`). This is the last
unimplemented `StepRun` variant.

## Scope

In scope (v1):
- Authoring surface, lowering, typecheck rule.
- Interpreter pause + resume (restore-and-continue).
- Durable suspension state (new port + host impl + file layout).
- CLI: `--signal` flag, suspend outcome + exit code, `--resume` path.
- One conformance fixture (dev vs bundle equivalence at the pause point).

Out of scope (deferred):
- Suspend nested inside `Parallel`/`Loop`/`Branch` (rejected at build time).
- A waiting server / daemon that delivers signals. One-shot CLI only.
- Timeouts / auto-expiry of a suspended run.

## Departures from the handoff (with rationale)

1. **Authoring: a 5th leaf `run`-kind, not a 5th structural form.** The handoff
   proposed a new `suspend`/`resume_signal` field on `UncheckedPipelineStep` and
   a "5-form" dispatch. But the leaf form is already `run = "<kind>:<id>"`, parsed
   by `split_once(':')` at `project.rs:2003`. Suspend is naturally
   `run = "suspend:<signal>"` — one new match arm, no new field, no form-count
   change, uniform with `agent:`/`tool:`/`check:`.

2. **`PipelineOutcome`, not `RunOutcome::Suspended`.** The handoff's deliverable
   #4 assumes `run_pipeline` returns `RunOutcome`. It does not — it returns
   `Result<OutputStore, RuntimeError>`, and `RunOutcome` is the *agent-loop*
   vocabulary (`final_message`, `total_turns`, `token_usage`), none of which map
   onto a pipeline pause. A dedicated 2-variant `PipelineOutcome` is cleaner than
   polluting `RunOutcome`.

3. **Restore-and-continue, not seed-and-skip.** The handoff says "mirror the
   agent-loop seed-and-skip resume (#373)." Seed-and-skip re-runs the
   deterministic prefix — but a pipeline prefix contains **agent (LLM) steps**;
   re-running them re-bills tokens and is non-deterministic. We persist the full
   `OutputStore` at the pause and, on resume, **restore it and jump to
   cursor+1** — never re-execute prior steps. The persisted store *is* the seed;
   the cursor *is* the skip.

4. **Separate `SuspensionStore` port, not an extended `TurnCheckpoint`.** Same
   `run_id` + same run dir (`.tau/runs/<id>/`), but a distinct payload
   (`PipelineSuspension`) and file (`suspend.json`), so the well-tested
   turn-checkpoint type stays pristine. Agent-durability and pipeline-HITL are
   separate concerns that share a handle, not one type.

5. **Store an IR digest in the suspension.** `--resume` re-lowers the cwd
   project; if it changed since the pause, the cursor is stale. Storing the
   canonical-IR SHA-256 lets resume reject drift loudly instead of resuming
   against a mismatched pipeline.

## Design questions — resolved

- **Q1 (checkpoint granularity):** new `SuspensionStore` port, same run dir,
  distinct file. (Departure 4.)
- **Q2 (nested suspend):** rejected at build time (typecheck). Top-level pipeline
  slice only. This keeps the recursive `run_steps` free of suspend logic.
- **Q3 (signal arrival):** one-shot `tau run --resume <id> --signal <name>`.
- **Q4 (signal semantics):** match — the resumer must pass the exact
  `resume_signal`, so a pipeline with two Suspends resumes deterministically.

## Authoring surface

```toml
[[pipeline.steps]]
id  = "await-approval"
run = "suspend:approved"      # resume_signal = "approved"
# `input` is ignored for suspend (produces no output); omit it.
```

Author-time validation (`validate_pipeline_step`): the `suspend:` arm requires a
non-empty signal — `run = "suspend:"` is rejected.

## Data model

### IR (already exists — do not touch)
`StepRun::Suspend { resume_signal: String }` (tau-ir, #444).

### tau-pkg
```rust
// PipelineRunRef — new variant
Suspend { resume_signal: String },
```
Parse: add to the leaf match at `project.rs:2003`:
```rust
Some(("suspend", sig)) if !sig.is_empty() =>
    PipelineRunRef::Suspend { resume_signal: sig.to_string() },
```

### Lowering (tau-ir-lower `parse.rs` `lower_step`)
```rust
PipelineRunRef::Suspend { resume_signal } =>
    StepRun::Suspend { resume_signal },
```
(The `match &s.run` is exhaustive — the compiler forces the arm.)

### Typecheck (tau-ir-lower `typecheck.rs`)
Currently the `Suspend` arm is a no-op. Make control-flow validation
context-aware:
- **Reject a `Suspend` that appears anywhere below the top-level pipeline
  slice** (inside `Branch.then/otherwise`, `Loop.body`, or any `Parallel`
  branch). New typecheck error, e.g. `SuspendNotTopLevel { step }`.
- **Reject `${steps.<suspend-id>.output}` references** — a Suspend step stores
  no output, so any reference to it is a build-time error.

### tau-ports (new — version bump 0.2.0 → 0.3.0)
```rust
// orchestration.rs
pub struct PipelineSuspension {
    pub run_id: RunId,
    pub resume_signal: String,
    pub step_cursor: usize,     // index into the top-level pipeline.steps slice
    pub step_id: String,        // for the "paused at <id>" human message
    pub ir_digest: String,      // canonical-IR SHA-256 of the module at pause time
    pub outputs: BTreeMap<String, serde_json::Value>, // OutputStore snapshot
}

pub trait SuspensionStore: Send + Sync {
    fn persist_suspension(&self, s: &PipelineSuspension) -> Result<(), CheckpointError>;
    fn load_suspension(&self, run_id: &RunId) -> Result<Option<PipelineSuspension>, CheckpointError>;
}
```
Reuses `CheckpointError` and `RunId`. A `MockSuspensionStore` lands in
`tau-ports::fixtures` beside `MockCheckpointStore`.

### tau-runtime-core
```rust
// interpreter/output_store.rs — add serde + snapshot/restore
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct OutputStore { map: BTreeMap<String, serde_json::Value> }
impl OutputStore {
    pub fn snapshot(&self) -> BTreeMap<String, serde_json::Value> { self.map.clone() }
    pub fn restore(map: BTreeMap<String, serde_json::Value>) -> Self { Self { map } }
}

// interpreter/pipeline.rs — new return type
pub enum PipelineOutcome {
    Completed(OutputStore),
    Suspended { run_id: String, resume_signal: String, step_id: String },
}
```
`RuntimeError::SuspendNotImplemented` (error.rs:369) is **retired**.

### tau-runtime-tokio
`FileCheckpointStore` also `impl SuspensionStore`, writing
`<scope>/.tau/runs/<run_id>/suspend.json` (atomic tmp+rename, matching the
turn-checkpoint idiom). Only one suspension per run is live at a time (a second
Suspend on resume overwrites it; on `Completed` it is removed).

## Interpreter control flow

Because Suspend is top-level-only (Q2), only `run_pipeline`'s top-level driver
handles it. The recursive `run_steps` (Branch/Loop/Parallel arms) keeps its
`Result<(), RuntimeError>` signature and never encounters a Suspend.

```
run_pipeline(module, input, suspension_sink, resume: Option<PipelineSuspension>)
  -> Result<PipelineOutcome, RuntimeError>

  if let Some(susp) = resume:
      assert susp.resume_signal == provided_signal      # Q4 (checked in CLI)
      assert susp.ir_digest == canonical_sha256(module) # departure 5
      store = OutputStore::restore(susp.outputs)
      i = susp.step_cursor + 1                           # restore-and-continue
  else:
      store = OutputStore::new(); i = 0

  while i < steps.len():
      step = steps[i]
      if let StepRun::Suspend { resume_signal } = step.run:
          suspension_sink.persist_suspension(PipelineSuspension {
              run_id, resume_signal, step_cursor: i, step_id: step.id,
              ir_digest: canonical_sha256(module), outputs: store.snapshot(),
          })?
          return Ok(PipelineOutcome::Suspended { run_id, resume_signal, step_id: step.id })
      # ... existing Check/Branch/Loop/Parallel/leaf dispatch, unchanged ...
      i += 1
  Ok(PipelineOutcome::Completed(store))
```

Note (accepted v1 wrinkle): a `Check` **after** a Suspend whose `gate` is a step
**before** the Suspend will rewind across the pause and re-run that slice
(re-billing). Gates are expected to sit within the same segment; document it,
don't guard it in v1.

### Data flow

```
tau run  (fresh)                        tau run --resume <id> --signal <name>
   │                                        │
lower → IrModule                        load_suspension(id) → PipelineSuspension
   │                                        │  verify signal (Q4) + ir_digest (dep.5)
run_pipeline(.., sink, None)            run_pipeline(.., sink, Some(susp))
   │                                        │
   ├ steps 0..k run, store fills           ├ store restored; i = cursor+1
   ├ hit Suspend@k                         ├ continue steps k+1..n
   │   persist_suspension{store,k}         │   (may hit a 2nd Suspend → persist again)
   │   → Suspended{signal,id}              └ → Completed(store) | Suspended{…}
   ▼
CLI prints run_id + resume hint, exit SUSPENDED
```

## CLI (tau-cli)

- New flag on `RunArgs`: `--signal <NAME>` (only meaningful with `--resume`).
- **Fresh run** that suspends: mint a `run_id` (reuse `mint_run_id`), construct a
  `FileCheckpointStore` as the `SuspensionStore`, thread it into the pipeline
  path (`try_run_pipeline`). On `PipelineOutcome::Suspended`, print:
  ```
  Paused at step 'await-approval' (signal: approved).
  Resume with:  tau run --resume <run_id> --signal approved
  ```
  JSON: `{"outcome":"suspended","run_id":..,"resume_signal":..,"step_id":..}`.
  Exit with a new `ExitCode::Suspended` (proposed value 3 — verify unused on the
  `run` path in `exit.rs`).
- **Resume run** (`--resume <id> --signal <name>`): re-lower the cwd project,
  `load_suspension(id)`; error if absent, if signal mismatches, or if `ir_digest`
  mismatches. Call `run_pipeline` with the rehydrated suspension. A completed
  resume renders normally (exit 0); a second suspension re-pauses (exit 3).
- `try_run_pipeline` return type widens from the store to a `PipelineOutcome`;
  `render_pipeline_result` gains a `Suspended` arm.

## Conformance (tau-ir-conformance)

tau-ir-conformance compares dev-run vs bundle-run. A Suspend fixture does not run
to `Completed`, so the report gains a **suspended terminal comparison**: dev and
bundle must agree on `resume_signal`, `step_cursor`, and the `outputs` snapshot
at the pause. (A resume-to-completion second comparison is a nice-to-have; v1
asserts pause-point equivalence.) This is the "NEW ConformanceReport shape" the
handoff flagged.

## Testing

- **tau-pkg:** `run="suspend:x"` → `PipelineRunRef::Suspend`; reject `suspend:`
  (empty signal); serialize round-trip.
- **tau-ir-lower:** lower `Suspend` arm; typecheck rejects nested Suspend
  (branch/loop/parallel); typecheck rejects `${steps.<suspend>.output}` refs.
- **tau-runtime-core:** suspend→persist→resume round trip against
  `MockSuspensionStore`; **restore-and-continue does not re-run the prefix**
  (counting mock dispatcher asserts a pre-suspend agent step is invoked exactly
  once across the pause); two sequential Suspends; signal mismatch rejected;
  ir_digest mismatch rejected. Retire `SuspendNotImplemented` and update
  `pipeline_control_flow.rs:692/699` to assert the `Suspended` outcome.
- **tau-runtime-tokio:** `FileCheckpointStore` suspension persist/load round trip
  on disk; `suspend.json` layout.
- **tau-cli:** `--signal` parsing; suspend exit code; JSON suspended outcome;
  end-to-end suspend→resume→complete.
- **tau-ir-conformance:** suspend fixture, dev vs bundle pause-point equivalence.

## Version / ABI impact

- `tau-ports` 0.2.0 → **0.3.0** (new pub trait + struct → ABI semver gate).
- `tau-pkg` (workspace-versioned): `PipelineRunRef` gains a variant — a
  workspace-internal breaking change for exhaustive matches; sweep call sites.
- `tau-runtime-core`: `OutputStore` gains serde derives; `run_pipeline` signature
  and return type change — internal, sweep callers (`tau-cli`, `ir_dispatcher`,
  conformance).

## Open items to confirm during planning

- Exact `ExitCode::Suspended` numeric value (check `exit.rs` / any `tau check`
  taxonomy collision).
- Whether the bundle path (`ir_dispatcher.rs`) needs the same suspend wiring as
  the cwd path, or v1 wires only `try_run_pipeline` (cwd) and defers bundle
  suspend.
