# ADR-0058: IR structured control-flow blocks

**Status:** Accepted
**Date:** 2026-06-22
**Deciders:** tau core

## Context

The workflow IR (`tau-ir`) represents a pipeline as a flat `Vec<PipelineStep>`, where each
step holds a single `StepRun` leaf. This is sufficient for linear, unconditional workflows
but cannot express the control-flow patterns real workflows require: conditional branching,
parallel fan-out, bounded loops, and human-in-the-loop suspend/resume.

EPIC 4 (structured control flow) extends the IR to support these patterns. The IR is a
versioned public contract (ADR-0056), so any extension must preserve byte-stability for
existing serialised workflows and must bump the minor version when new variants are used.

Two design constraints follow from the existing IR shape:

1. `PipelineStep.run` is the single execution point. Any extension must keep it that way so
   the interpreter walk stays uniform — one match arm, one dispatch path.
2. Conditions must reuse existing primitives. The IR already carries `Locus` (where to read
   a value) and `GoalPredicate` (how to test it) for verify steps. Introducing a parallel
   condition language would duplicate semantics and split the type-checker.

## Decision

`StepRun` gains four recursive structured-block variants, keeping `PipelineStep.run` as
the single execution point:

```
StepRun::Branch  { on: Condition, then: Vec<PipelineStep>, otherwise: Vec<PipelineStep> }
StepRun::Parallel { branches: Vec<Vec<PipelineStep>> }
StepRun::Loop    { body: Vec<PipelineStep>, until: Condition, max_iters: u64 }
StepRun::Suspend { resume_signal: String }
```

A new `Condition` type reuses the existing locus/predicate vocabulary:

```
Condition { evaluates: Locus, predicate: GoalPredicate }
```

This keeps the branch/loop condition language identical to the verify-step condition
language — one type-checker, one schema definition.

`Loop` reuses the `OnFail::Retry` rewind machinery (the interpreter already knows how to
re-enter a step sequence from a saved snapshot) and carries a mandatory `max_iters: u32`
bound. Unbounded loops are rejected at typecheck; `max_iters = 0` is a typecheck error (`u64`).

`Suspend` reuses the ADR-0053 `per_tool_call` checkpoint mechanism. The HITL round-trip
is: checkpoint the run state, emit the `Suspend` event, wait for an external
`resume_signal`, then seed-and-skip resume from the checkpoint. The full round-trip
implementation is EPIC 4.3.

`Eq` is dropped from `StepRun`, `PipelineStep`, and `Pipeline`. The subtree now
transitively holds `GoalPredicate::SchemaValid(serde_json::Value)`, which does not
implement `Eq`. This mirrors the existing `CheckVerify` arm; equality on pipeline
structures has no defined semantics in the IR contract.

`ir_format` bumps from `v2.3.0` to `v2.4.0` (additive minor). Serialised workflows that
use none of the new variants are byte-stable: the new enum arms are never written, and
existing deserialisation round-trips are unaffected.

The published IR JSON Schema is regenerated at `v2.4.0` with `Branch`, `Parallel`,
`Loop`, `Suspend`, and `Condition` definitions added. A conformance sample for `Branch`
is included in the conformance kit.

## Consequences

**Positive:**

- Real workflows with conditional logic, parallel fan-out, bounded iteration, and
  human-in-the-loop pauses can now be expressed and type-checked at build time.
- The `PipelineStep.run` single-dispatch invariant is preserved: the interpreter gains
  four new match arms, but no structural change to the walk.
- `Condition` reuse eliminates a parallel predicate sublanguage; the type-checker and
  JSON Schema share one definition.
- Additive versioning (`v2.4.0`) means no breaking change for existing serialised
  workflows.

**Negative / obligations:**

- `Eq` is dropped from the pipeline subtree; any downstream code that compared pipelines
  structurally will not compile (intentional: equality was not a defined contract).
- The interpreter (EPIC 4.2), Suspend round-trip (EPIC 4.3), `StepRun::Dynamic`
  capability-bounded regions (EPICs 4.4–4.5), and extended conformance kit (EPIC 4.6)
  are follow-on obligations. This ADR covers only the data model, typecheck, and schema.
- Nested control-flow refs and bounds (`max_iters > 0`, branch/parallel non-empty) are
  enforced at typecheck (tau build time), not at runtime — consistent with the
  Rust-like build-time enforcement principle.
- Nested-scope resolution — a `Loop`'s `until` referencing its own body's output,
  uniqueness of nested `PipelineStepId`s, and nested `${steps.<id>.output}` template
  visibility — is **deferred to EPIC 4.2** (which adds execution); 4.1's typecheck
  validates nested node references but does not yet model nested scope, so a condition
  that reads a *nested* step's output is currently rejected.

## Alternatives considered

**A. Separate `ControlFlow` node type instead of extending `StepRun`.**
Rejected. `PipelineStep.run` is the single execution point; splitting it into a union of
`StepRun | ControlFlow` forces every interpreter match to handle two top-level types.
One recursive enum keeps the walk uniform.

**B. Flat goto/label steps.**
Rejected. Goto-style control flow is not statically structured, so it cannot be
type-checked for termination or well-nestedness at build time. `max_iters`-bounded loops
and explicit `Branch`/`Parallel` blocks give build-time verifiability; goto labels do not.

**C. Unbounded loops (no `max_iters`).**
Rejected. An unbounded loop is a potential runaway agent. `max_iters` is mandatory and
enforced at typecheck — no workflow reaches the interpreter with an uncapped loop.
