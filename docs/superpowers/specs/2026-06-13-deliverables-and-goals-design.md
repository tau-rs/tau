# Deliverables & Goals: build-time-checked postcondition steps

**Date:** 2026-06-13
**Status:** Design approved. **Blocked on a prerequisite** (see *Prerequisite*
below); split into two implementation plans.
**Scope:** Add two postcondition primitives — `goal` (deterministic predicate)
and `deliverable` (produced artifact + LLM-judged content) — to the canonical
`tau.toml` → IR surface, checked at both build time and runtime, with an
opt-in self-correcting retry loop.

## Prerequisite: an IR sequential pipeline executor (Plan 1)

This design assumes a **sequence of steps the engine controls** — *"the previous
pipeline produces the deliverable → the check validates → on fail rewind to the
gate and re-run forward."* The IR runtime as of 2026-06-13 **does not have this**:

- `run_ir` (`crates/tau-runtime-core/src/interpreter/mod.rs:38`) takes a **single
  `entry` agent** and runs its loop. It does not iterate a step sequence.
- `IrModule.workflow.edges` (the would-be DAG) is **reserved and unused** in v0.
- There is **no `steps.<id>.output` store** — a step's output is an ephemeral
  tool-result in the conversation, not an addressable value. So
  `evaluates = "steps.writer.output"` has nothing to bind to today.
- The only iteration that exists is an agent's own loop; multi-step pipelines are
  otherwise either the legacy `tau-workflow` runner (no IR → no build-time check)
  or LLM-driven sub-agent spawning (not engine-sequenced).

The engine-enforced gate/rewind control flow this feature needs therefore has no
substrate yet. Resolving it by handing control flow back to the LLM (a check the
agent *chooses* to retry) was rejected: it contradicts the deterministic,
build-time-checked control flow that motivated choosing IR over the legacy runner
in the first place.

**Consequence — two sub-projects, implemented in order:**

1. **Plan 1 — IR sequential pipeline executor** (foundational): declarative
   multi-step sequencing in the IR (activate/define step ordering + edges), a
   step-output store with `steps.<id>.output` resolution, and trace events for
   step entry/exit. Unblocks much more than this feature.
2. **Plan 2 — Deliverables & Goals** (this document): the two node kinds,
   build-time producer binding, the swappable judge, and rewind-to-gate retry —
   all built on Plan 1.

Everything below is the **Plan 2** design and is written as if Plan 1 exists
(engine-sequenced steps + an addressable `steps.<id>.output`).

## Problem

tau pipelines today can run a sequence of agents/tools but have no first-class
way to assert *that the pipeline produced what it was supposed to* or *that a
goal was met*. The failure mode is "ran clean, delivered nothing" — discovered
by a human reading the output, not by the system.

tau's stance (`docs/explanation/tau-philosophy.md`, and the user's standing
principle) is **Rust-class build-time enforcement: any check that could run at
build time must run at build time.** A postcondition feature must therefore do
more than a runtime assertion — it must, where structurally possible, fail the
*build* of a pipeline that cannot deliver its stated output.

## Two primitives, two verification methods

The design separates two axes that are easy to conflate:

- **What is asserted** — a *deliverable* (an artifact must be produced) vs a
  *goal* (a condition must hold).
- **How it is verified** — *deterministic* (a pure predicate) vs *LLM-judged*
  (a semantic verdict).

```
                    VERIFIED BY
                deterministic          LLM-judged
              ┌────────────────────┬────────────────────┐
  a condition │      goal          │        — —          │
  must hold   │  "cites >=2 srcs"  │   (a goal done      │
              │  predicate, no LLM │    wrong)            │
              ├────────────────────┼────────────────────┤
  an artifact │  existence floor   │   deliverable       │
  must exist  │  (the cheap gate)  │  "is report.md any  │
  & be good   │                    │   good?" content    │
              └────────────────────┴────────────────────┘
```

- **`goal`** — a *measurable* condition → a pure predicate. Crisp,
  reproducible, no model. Backed by a fixed predicate menu (easy path) or a
  registered native Rust fn (power path).
- **`deliverable`** — a *produced, qualitative* artifact → a deterministic
  existence floor, then an LLM judges the **content** against a natural-language
  criterion.

These pair each kind with the verification it is actually good at: predicates
answer "did we hit the number?"; an LLM answers "is this any good?", which a
predicate cannot.

## Surface decision: `tau.toml` → IR only

The steps are authored in `tau.toml` (and `.ts` via `tau-ts-extract`), which is
lowered to IR by `lower_project` and consumed by `tau build` / `tau check` /
`tau run` / `tau dev`. This is **forced** by the build-time requirement: only
surfaces that produce IR are seen by `tau build` / `tau check`. The legacy
standalone `tau-workflow` runner (`workflows/*.toml`) never lowers to IR and is
left **untouched**; it does not gain this feature.

Unifying the legacy runner onto the IR is noted as a possible future direction
(its own migration project) and is explicitly out of scope here.

## The `goal` primitive

A `goal` asserts a measurable condition, verified deterministically — no LLM.

```toml
# easy path — fixed predicate menu
[goals.has_sources]
evaluates = "/workspace/report.md"     # a filesystem path or a named output
check     = "matches"
pattern   = "(?m)^## Sources"

# power path — registered native Rust fn (escape hatch)
[goals.link_health]
evaluates = "steps.writer.output"
fn        = "research_checks::all_links_resolve"
```

**Predicate menu (v1):** `exists`, `non_empty`, `equals`, `matches` (regex),
`min_count`, `schema_valid`. Each operates on the `evaluates` locus (a
filesystem path or a named output reference `steps.<id>.output`).

**Native-fn escape hatch:** `fn = "<crate>::<path>"` references a function
registered in the `DeterministicRegistry`. Arbitrary power; requires shipping
and registering Rust.

`evaluates` for a goal is a *read* locus (it inspects an existing value/file);
it is distinct from a deliverable's `produces` binding.

## The `deliverable` primitive

A `deliverable` asserts an artifact is produced **and** its content is
acceptable. It has three layers — two deterministic, one LLM:

```
 deliverable "report.md is a coherent summary"
   build-time : EXISTS a producer wired to write report.md, and permitted to   (deterministic)
   runtime gate: report.md exists & non-empty                                  (deterministic, cheap)
   runtime judge: its CONTENT satisfies must_satisfy                           (LLM)  <- headline
```

```toml
[deliverables.report]
path         = "/workspace/report.md"        # locus: filesystem path OR named output
must_satisfy = "A coherent summary that accurately reflects the sources."
on_fail      = "retry"                        # default "abort"
max_attempts = 3
retry_from   = "writer"                       # the gate; default = the bound producer
# judge resolution — see below
```

### Deliverable loci (v1)

- **Filesystem path** — `path = "/workspace/report.md"`. Producer bound via
  `produces` (below). Runtime: existence/predicate on the file.
- **Named data output** — `output = "steps.writer.output"`. Producer = the
  emitting step. Runtime: the value validates / is non-empty.

Deferred loci (explicitly not in v1): external resources via capability
(HTTP/MCP side-effects), provenance/trace evidence ("tool X was invoked"),
process outcomes ("step exited 0" — belongs to `goal`).

### Producer binding (build-time contract)

A deliverable must bind to a producing step at build time. The mechanism is
**explicit `produces` declaration** (chosen over capability inference because it
captures *intent*, the Rust-signature analogue — a step declares its output):

```toml
[agents.writer]
produces  = ["/workspace/report.md"]          # declared intent
tool_refs = ["write_file"]

[tools.write_file]
native       = "WriteFile"
capabilities = ["fs-write:/workspace/**"]
```

Build-time checks:

```
error: deliverable 'report' has no producer
       no step declares  produces = ["/workspace/report.md"]

error: step 'writer' declares it produces '/workspace/report.md'
       but holds no fs-write capability covering that path
```

**Honest limits.** The build promises *"the pipeline is wired to produce X"* —
not that the LLM actually writes correct content. For an `Agent` producer the
guarantee is "capable of"; for a `Deterministic` producer it is "guaranteed".
The gap — a step declares `produces` but the LLM does not write the file — is
caught only at the runtime existence gate, never at build time. This is
inherent to LLM-produced content and is stated plainly rather than papered over.

### Judge resolution (easy path + power path)

The deliverable's content judge mirrors the goal's easy/power split. `must_satisfy`
is always required (it is the *criterion*); the judge selects *who* evaluates it.

| Effort | Author writes | Who judges |
|--------|---------------|-----------|
| Easy | just `must_satisfy` | tau's built-in minimalist judge (fixed canonical prompt, default model) |
| Tune | `must_satisfy` + `judge_model` | same minimalist judge, chosen model |
| Power | `must_satisfy` + `judge = "my_agent"` | a user `[agents.*]` — e.g. the `critic` / `pr-reviewer` reference packages |

**Verdict contract.** Every judge — built-in or custom — must return:

```jsonc
{ "met": bool, "rationale": string }
```

`rationale` is load-bearing: it is the "why" fed back into the retry loop. Build-time
checks for the judge:

```
error: deliverable 'report' sets judge = "house_critic" but no [agents.house_critic] is defined
error: deliverable 'report' sets both judge_model and judge — a custom judge brings its own model
warn:  agent 'house_critic' used as a judge declares an output_schema incompatible with
       the verdict contract { met, rationale } — its rejections won't carry a usable "why"
```

The judge reads the **actual produced artifact** (the file contents, or the
named output value), not merely an in-memory reference — it judges the real
deliverable.

## Failure handling: abort, or rewind-to-gate retry

A failed check (goal or deliverable) is either a **gate** or a **feedback loop**.

- **Default — `abort`.** The run exits non-zero; the rationale/diagnostic is the
  message. Pure assertion, zero engine cost.
- **Opt-in — `retry`.** On failure, execution **rewinds to the gate**
  (`retry_from`) and re-runs forward through the producer back to the check,
  with the failure rationale injected, bounded by `max_attempts`.

```
              ┌────────────── rewind to gate, inject "why" ──────────────┐
              │                                                           │
   ┌──────────▼──┐     ┌──────────┐     ┌─────────────┐  fail & attempt<max
   │   gather    │ ──> │  writer  │ ──> │   check     │ ──────────────────┘
   └─────────────┘     └──────────┘     └──────┬──────┘
     gate can move      (producer =      pass  │  │ fail & attempt==max
     back to here <---- default gate)          ▼  ▼
                                          continue  abort non-zero
```

### The gate (`retry_from`)

The gate is the rewind point. It **defaults to the bound producer** and may be
moved to any earlier step, but **never after the producer** — a producer
produces; everything after it merely consumes, so rewinding past it could not
change the artifact. This is a build-time check:

> **Guarantee 1 — gate position.**
> `error: deliverable 'report' has retry_from = "polish" but 'polish' runs after`
> `producer 'writer' — the gate must be at or before the producer.`

A second guarantee follows from determinism:

> **Guarantee 2 — retry must be able to change something.**
> `error: deliverable 'report' sets on_fail = "retry" but the retry span`
> `(gather -> writer) contains no non-deterministic step; retrying cannot change the result.`

(A pure-deterministic span yields identical output on every attempt; retrying it
is a guaranteed-no-op loop and is rejected at build time rather than wasted at
runtime.)

### Feedback — the "why"

On each retry, the verdict's `rationale` is injected as an extra turn into every
**agent** step inside the retry span (deterministic steps ignore it — pure
functions). This is what makes the loop *converge* rather than reroll blindly:
attempt 2 of `writer` literally sees *"previous attempt rejected: only 1 source
cited, need >=2."*

After `max_attempts`, the run aborts non-zero with the final rationale.

### Budget bounding

The retry loop is bounded by `max_attempts` **and** by the existing
`AgentBudget` — a stubborn judge or producer cannot burn unbounded tokens. The
budget cap is authoritative even below `max_attempts`.

## Placement

Checks are **checkpoints**, not only terminal steps: a check may sit anywhere
after the steps it evaluates, and multiple checks are allowed at different
points in a pipeline. A check's position determines what is in scope for its
`evaluates`/`path` and for its retry span.

## Trace / log

Every check evaluation emits a structured event — pass *or* fail — via the
existing tracing `Layer` (the beta logging work), landing in the JSONL run log
and any OTLP export:

```jsonc
{ "event": "check.evaluated", "id": "has_sources", "kind": "goal",
  "verdict": "pass", "attempt": 1 }
{ "event": "check.evaluated", "id": "report", "kind": "deliverable",
  "verdict": "fail", "attempt": 1, "locus": "/workspace/report.md",
  "rationale": "only 1 source cited; need >=2" }
{ "event": "check.retry",     "id": "report", "rewind_to": "writer", "next_attempt": 2 }
{ "event": "check.evaluated", "id": "report", "kind": "deliverable",
  "verdict": "pass", "attempt": 2 }
```

The full retry history — with the *why* at each step — is durably in the trace.

## IR representation

```
 tau.toml                          IR (lowered + checked)
 [goals.X]        ------------->   Check { target, verify: Predicate(fn_ref), retry }
                                     predicate reuses DeterministicRegistry / native fn
 [deliverables.Y] ------------->   Check { target, verify: Judge { model_or_agent, must_satisfy }, retry }
                                     existence floor (deterministic) + judge (Agent/LLM machinery)
 retry { on_fail, max_attempts, gate: StepId }   common to both
```

The single genuinely new engine capability is **bounded, budget-capped
iteration** from a check back to its gate; today's interpreter is acyclic.
Everything else reuses existing parts: capability binding, the
`DeterministicRegistry`, the agent loop, and the tracing layer. Whether the IR
carries one `Check` node or two (goal/deliverable) is a plan-phase decision; the
behavior in this document is what is fixed.

Adding a node kind is cross-cutting: it touches the node enum, lowering
(`lower/typecheck.rs`, `lower/capability_fit.rs`), the interpreter, the
canonical (hash-stable) encoder, the bundle format (version bump), and the
conformance fixtures. That ripple — not the check logic — is the bulk of the
work.

## Worked example (end to end)

```toml
[project]
name = "research"

[agents.gather]
model         = "claude-haiku-4-5"
prompt.system = "Research the question; collect sources."

[agents.writer]
model         = "claude-haiku-4-5"
prompt.system = "Write /workspace/report.md from the notes."
tool_refs     = ["write_file"]
produces      = ["/workspace/report.md"]

[tools.write_file]
native       = "WriteFile"
capabilities = ["fs-write:/workspace/**"]

[goals.has_sources]                      # deterministic, crisp, no LLM
evaluates = "/workspace/report.md"
check     = "matches"
pattern   = "(?m)^## Sources"

[deliverables.report]                    # LLM judges the artifact's content
path         = "/workspace/report.md"
must_satisfy = "A coherent summary that accurately reflects the sources."
on_fail      = "retry"
max_attempts = 3
retry_from   = "writer"
```

- `tau check` proves: `report` has a declared producer (`writer`) permitted to
  write the path; `retry_from` is `<=` the producer; the span contains an LLM
  step; `has_sources` regex compiles. No tokens spent.
- `tau run` attempt 1: `writer` writes a report citing 1 source → judge fails
  → rewind to `writer` with *"only 1 source cited; need >=2"* → attempt 2 passes
  → `has_sources` predicate passes → run succeeds. Every verdict in the trace.

## Out of scope (v1)

- Legacy `tau-workflow` runner integration (no IR, no build-time check).
- Deliverable loci beyond filesystem path and named output (external/provenance/process).
- Score-with-threshold verdicts (boolean `met` only; thresholds are a later add).
- Rewind to a step *before* the gate's own producer chain beyond a single
  `retry_from` point (no multi-gate / partial-DAG replay).
- Project-level default judge model (`judge_model` is per-deliverable; a global
  default is a trivial later add).

## Summary

A `goal` is a no-LLM predicate that gates the run (menu or native fn). A
`deliverable` is an artifact the build proves is *producible* and the runtime
proves is *good* via a swappable LLM judge (default minimalist, tunable model,
or a custom agent) returning `{ met, rationale }`. A failed check either aborts
or rewinds to a gate (`<=` its producer) and retries, feeding back **why** it
failed, bounded by `max_attempts` and `AgentBudget`. Every verdict is written to
the trace. The whole feature lives on the canonical `tau.toml` → IR path so the
structural contract is enforced at build time.
