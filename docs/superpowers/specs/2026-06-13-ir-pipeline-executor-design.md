# IR sequential pipeline executor

**Date:** 2026-06-13
**Status:** Design approved; implementation plan pending.
**Scope:** Give the canonical IR a declarative, engine-sequenced multi-step
pipeline: an ordered list of steps, a run-scoped output store with
`steps.<id>.output` resolution, input templating, and step trace events. This is
**Plan 1**, the prerequisite for the Deliverables & Goals feature
(`2026-06-13-deliverables-and-goals-design.md`).

## Problem

The IR runtime executes a **single entry agent** — `run_ir`
(`crates/tau-runtime-core/src/interpreter/mod.rs:38`) looks up one named agent
and runs its loop. There is no engine-sequenced multi-step pipeline:

- `IrModule.workflow.edges` (the would-be DAG) is reserved and **unused**.
- There is **no `steps.<id>.output` store**; a step's output is an ephemeral
  tool-result in the conversation, not an addressable value.
- The only multi-step pipeline tau has is the **legacy `tau-workflow` runner**
  (`crates/tau-workflow/`), which runs an ordered `[[steps]]` list with
  `${steps.x.output}` templating — but it never lowers to IR, so it gets no
  build-time check, no canonical form, no bundle.

Several planned features (deliverables/goals with engine-enforced gate/retry,
and eventually retiring the legacy runner) need a real sequential pipeline *in
the IR*. This plan adds it.

## Approach: sequence the primitives that already execute

The three execution primitives already work in isolation:

- **agent** — `run_agent` (`agent_loop.rs:385`) runs an LLM loop.
- **tool** — `dispatcher.invoke(tool_id, args)` invokes a native/MCP tool.
- **deterministic** — `registry.invoke(fn_name, args)` runs a pure fn
  (`deterministic.rs:14`).

The executor is a **sequencer over these**, not a new execution engine — the
lowest-risk way to add pipelines. It is **additive**: a project with no
`[[pipeline.steps]]` keeps today's exact "run the one entry agent" behavior.

```
 tau.toml                              IR runtime (NEW: run_pipeline)
 ─────────                             ─────────────────────────────
 [[pipeline.steps]]                    run_pipeline(module, input):
   id    = "gather"                      store = OutputStore::new()
   run   = "agent:gather"                for step in module.workflow.pipeline.steps:   # in order
   input = "${input}"                        args = template::render(step.input, input, &store)
                                              out  = match step.run:
 [[pipeline.steps]]                               Agent(id)         => run_agent(...)        -> text/structured
   id    = "writer"                               Tool(id)          => dispatcher.invoke(...) -> Value
   run   = "agent:writer"                         Deterministic(id) => registry.invoke(...)   -> Value
   input = "${steps.gather.output}"          store.insert(step.id, out)
                                          return store / final step output
```

## Authoring surface (`tau.toml`)

A new optional ordered table array:

```toml
[[pipeline.steps]]
id    = "gather"
run   = "agent:gather"          # "agent:<id>" | "tool:<id>" | "deterministic:<id>"
input = "${input}"

[[pipeline.steps]]
id    = "writer"
run   = "agent:writer"
input = "${steps.gather.output}"
```

- **`id`** — the step's handle, used by `steps.<id>.output` and (in Plan 2) by
  `retry_from`. Must be unique within the pipeline.
- **`run`** — a `kind:node-id` reference to an existing `[agents.*]`,
  `[tools.*]`, or `[steps.*]` (deterministic) declaration.
- **`input`** — a template string (below). Defaults to `"${input}"` (the run's
  top-level input) when omitted.

The pipeline-step `id` namespace is distinct from the IR `StepId` used for
`Deterministic` nodes — a pipeline step may *run* a deterministic node by id
(`run = "deterministic:<id>"`); the two ids are unrelated handles.

## Templating

Port the proven logic from `crates/tau-workflow/src/template.rs`:

- `${input}` → the run's top-level input value.
- `${steps.<id>.output}` → the stored output of an **earlier** step `<id>`.

Resolution rules:

- A `${steps.<id>.output}` reference must name a step that appears **earlier** in
  the ordered list (no forward/self references) — checked at build time.
- An unresolved variable is a runtime error with a precise message.

## Output model

Each step produces a JSON `Value` stored in a run-scoped `OutputStore` keyed by
the pipeline-step `id`:

- **agent step** — its final assistant message. If the agent declares structured
  output, the structured value; otherwise the final text as `Value::String`.
  (Reuse `last_assistant_text` / outcome from `outcome.rs:49`.)
- **tool step** — the tool's returned `Value`.
- **deterministic step** — the fn's returned `Value`.

The `OutputStore` is the substrate that makes `steps.<id>.output` addressable —
the thing the runtime lacks today.

## IR representation

Extend `Workflow` (`crates/tau-ir/src/module.rs:54`) with an optional pipeline:

```rust
// crates/tau-ir/src/pipeline.rs  (new)
pub struct Pipeline {
    pub steps: Vec<PipelineStep>,           // ordered; Vec preserves authoring order
}

pub struct PipelineStep {
    pub id: PipelineStepId,                 // new newtype in ids.rs
    pub run: StepRun,
    pub input: String,                      // template
}

pub enum StepRun {
    Agent(AgentId),
    Tool(ToolId),
    Deterministic(StepId),
}
```

```rust
// module.rs — Workflow gets one field
pub pipeline: Option<Pipeline>,             // None => single-entry behavior unchanged
```

- **Canonical encoder** (`canonical.rs`): `Vec<PipelineStep>` serializes in order;
  no special handling needed (serde derive). `Option<Pipeline>` with `None`
  serializes as `null` — a MINOR `ir_format` bump per D-6 (additive field).
- **Hash/bundle**: covered automatically — the bundle embeds the canonical IR
  bytes and hashes them (`tau-pkg/src/bundle/canonical.rs:58`). No bundle
  `schema_version` bump required (the IR payload already carries it).

## Lowering & build-time checks

In `lower_project` (`crates/tau-ir/src/lower/mod.rs:90`), parse the pipeline in
`parse.rs` and validate in `typecheck.rs`. New `IrError` variants
(`crates/tau-ir/src/error.rs:12`):

- `UnknownPipelineRun { step, target }` — `run = "agent:foo"` but no such node.
- `DuplicatePipelineStepId { id }` — two steps share an `id`.
- `ForwardOutputRef { step, referenced }` — `${steps.x.output}` where `x` is the
  same step or appears later.
- `UnknownOutputRef { step, referenced }` — `${steps.x.output}` where `x` is not
  any pipeline step.

These run at `tau check` / `tau build` (both invoke lowering), so a malformed
pipeline fails the build before any tokens are spent.

## Runtime (`tau-runtime-core`)

- New `crates/tau-runtime-core/src/interpreter/pipeline.rs`: `run_pipeline<D>(
  module: Arc<IrModule>, input: Value, dispatcher: Arc<D>) -> Result<OutputStore,
  RuntimeError>`.
- `run_pipeline` dispatches each step to the existing executors. Agent steps use
  `Box::pin(run_agent(...))` (the established recursion-breaking pattern,
  `agent_loop.rs:197`).
- `run_ir` is unchanged. The CLI chooses: `module.workflow.pipeline.is_some()` →
  `run_pipeline`; else → `run_ir` with the entry agent (today's path).

## Trace

Add to `crates/tau-runtime-core/src/vocabulary.rs` and emit in `run_pipeline`:

```jsonc
{ "event": "pipeline.step_started",   "id": "writer", "run": "agent:writer" }
{ "event": "pipeline.step_completed", "id": "writer", "output_bytes": 1843 }
```

Emitted via the existing `tracing` pattern (`info!(name = EV_..., ...)`,
`stream.rs:236`). A `SPAN_PIPELINE_STEP` span wraps each step so nested agent
spans nest correctly.

## CLI wiring

- `crates/tau-cli/src/cmd/run.rs`: after loading + lowering, branch on
  `pipeline.is_some()` to call `run_pipeline` vs the current single-agent path.
  Top-level `input` comes from the existing run input argument.
- `tau dev` (`cmd/dev/`) and `tau build` (`cmd/build.rs`) need no behavior change
  beyond lowering the new field (they already lower the whole project); `tau dev`
  REPL drives a pipeline if one is declared.
- `project_load.rs` is unchanged (it returns `ProjectConfig`; the new tables ride
  along).

## Project config (`tau-pkg`) & TS parity

- `crates/tau-pkg/src/project/project.rs`: add `pipeline: Option<UncheckedPipeline>`
  to `UncheckedProjectConfig` (line 14) and `Option<PipelineConfig>` to
  `ProjectConfig` (line 295), with validation in `validate()` (line 589).
- `crates/tau-ts-extract/`: add a `pipeline([...])` factory + TOML emission so the
  `.ts` surface keeps byte-equal canonical-IR parity (the conformance test at
  `tau-ts-extract/tests/fan_monitor_conformance.rs`). A TS authoring example is
  added to the parity fixture.

## Conformance

Add a fixture `crates/tau-ir-conformance/fixtures/08_pipeline_sequence/`
(`workflow.toml` + `mock_llm.jsonl`) exercising a two-step
`gather → writer` pipeline with `${steps.gather.output}` threading, plus
dev-mode + cross-mode (dev vs bundle) tests in
`tau-ir-conformance/tests/conformance.rs`.

## Out of scope (Plan 1)

- **DAG / parallel execution** — linear ordered list only; `edges` stays reserved.
  Linear is a degenerate DAG, so DAG is a clean future extension.
- **Conditionals / loops** — no branching or iteration. (Plan 2's rewind-to-gate
  retry adds the *only* iteration, scoped to a single check.)
- **Deliverables / goals** — Plan 2.
- **Retiring the legacy `tau-workflow` runner** — this plan makes retirement
  *possible* (IR now has the sequential model) but does not remove it.
- **Typed step I/O schemas** — step outputs are untyped JSON `Value` in v1;
  schema-validating the threading is a later add.

## Summary

A new optional `[[pipeline.steps]]` ordered list lowers to a `Pipeline` in the
IR; `run_pipeline` sequences the existing agent/tool/deterministic executors,
storing each step's output in a run-scoped `OutputStore` so `${steps.<id>.output}`
becomes addressable. Build-time checks reject unknown/forward references and
duplicate ids; step trace events record the run. The feature is additive — no
pipeline declared means today's single-agent behavior is byte-for-byte unchanged
— and it gives Plan 2 (deliverables & goals) the engine-sequenced substrate its
gate/rewind control flow requires.
