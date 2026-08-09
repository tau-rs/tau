# Authoring a suspend step

A pipeline can pause for a human (or any external actor) to act, then
resume later from exactly where it left off. This is a **suspend** step —
a fifth leaf `run`-kind alongside `agent:`, `tool:`, `check:`, and
deterministic steps:

```toml
[[pipeline.steps]]
id  = "await-approval"
run = "suspend:approved"   # resume_signal = "approved"
```

The text after `suspend:` is the **resume signal** — the name the resumer
must supply later to unblock this exact pause point. It must be non-empty;
`run = "suspend:"` is rejected at build time.

## What happens when the engine hits a suspend step

The interpreter checkpoints the pipeline's accumulated step outputs and its
position in the step list, then the run **pauses**: `tau run` exits with a
distinct code (`3`) instead of the normal `0`, and prints a resume hint:

```
Paused at step 'await-approval' (signal: approved).
Resume with:  tau run --resume <run_id> --signal approved
```

With `--json`, the same pause emits a structured payload instead of the
usual `{"outcome":"completed", ...}` shape:

```json
{"outcome":"suspended","run_id":"...","resume_signal":"approved","step_id":"await-approval"}
```

A suspend step produces **no output**. Don't reference
`${steps.await-approval.output}` from a later step — a reference to a
suspend step's output is rejected at build time (`tau build` / `tau run`
typecheck), the same way an out-of-scope branch reference is.

## Top-level only

A suspend step is only valid directly in `[[pipeline.steps]]` — the
top-level slice. Nesting one inside a `Branch` arm (`then`/`otherwise`), a
`Loop` body, or a `Parallel` branch is rejected at build time. This keeps
the recursive step walk that executes branches/loops/parallel branches free
of suspend logic; only the top-level driver needs to know how to pause.

## Resuming

Resume a paused run with the run id from the pause message and the exact
signal it named:

```bash
tau run --resume <run_id> --signal approved
```

Both flags are required together — `--resume` without `--signal` (or vice
versa) is a usage error. The signal must match the one the run actually
paused on; a mismatch is rejected rather than silently ignored.

**Restore-and-continue, not re-run.** On resume, the engine restores the
step outputs that were captured at pause time and continues at the step
*after* the suspend — prior steps (including any agent/LLM steps) are
**not** re-executed. This matters because pipeline prefixes can contain
agent steps: re-running them would re-bill tokens and could produce
different results on a second pass.

**The project must be unchanged since the pause.** Resume re-lowers the
project in the current working directory and compares its IR digest
against the one recorded at pause time. If the project changed — even a
step reordering or a prompt edit — resume is rejected rather than
continuing against a stale plan.

A completed resume clears the checkpoint, so resuming an already-finished
run fails (there is nothing left to resume). If the resumed run hits a
*second* suspend step, the same pause/exit-3/resume-hint behavior applies
again — a pipeline may suspend more than once.

## Example: two-suspend pipeline

```toml
[[pipeline.steps]]
id  = "draft"
run = "agent:writer"

[[pipeline.steps]]
id  = "await-approval"
run = "suspend:approved"

[[pipeline.steps]]
id  = "publish"
run = "tool:publish"

[[pipeline.steps]]
id  = "await-confirmation"
run = "suspend:confirmed"

[[pipeline.steps]]
id  = "notify"
run = "tool:notify"
```

```bash
tau run                                        # runs `draft`, pauses at `await-approval`, exit 3
tau run --resume <run_id> --signal approved    # runs `publish`, pauses at `await-confirmation`, exit 3
tau run --resume <run_id> --signal confirmed   # runs `notify`, completes, exit 0
```

`draft` and `publish` each run exactly once across the whole sequence,
regardless of how many resumes it takes to reach completion.

## v1 limitations

- **One-shot CLI resume only.** There is no waiting server or daemon that
  delivers a signal automatically — a human (or a script) must invoke
  `tau run --resume ... --signal ...` explicitly. Timeouts and auto-expiry
  of a suspended run are not implemented.
- **cwd projects only.** Suspend is wired into `tau run`'s current-working-
  directory pipeline path. Suspending a pipeline run from a built bundle
  (`tau run --bundle`) is deferred; the bundle path currently errors rather
  than pausing if it reaches a suspend step.
- **`check:` gates that reference a pre-suspend step still rewind across
  the pause.** If a `check:` step *after* a suspend fails and its gate
  targets a step *before* the suspend, the interpreter rewinds and re-runs
  that earlier slice — which can re-bill an agent step. Keep gate targets
  within the same segment as the check that uses them.

## See also

- [Author a conditional branch](authoring-a-branch.md) — the `Branch`
  control-flow form suspend cannot appear inside.
- [Assert pipeline postconditions](assert-pipeline-postconditions.md) —
  `check:` steps and gate/rewind semantics.
