# EPIC 4.2a — Branch authoring end-to-end (design)

**Date:** 2026-07-23
**Branch:** `epic-4-2a-branch-authoring` → PR to `main`
**Status:** Approved (scope + syntax), pending spec review

## Goal

Make a user able to write a conditional branch in `tau.toml` and run it. Close the
producer-without-consumer gap: the IR data model already has `StepRun::Branch` (4.1, #444),
the typecheck already validates nested Branch arms, and the interpreter already executes Branch
(#454) — but **no `tau.toml` syntax produces a Branch**. This slice adds the authoring→lowering
path so an authored branch lowers, typechecks, runs (native + wasm parity), with a conformance
fixture and a docs example.

Merge bar (slicing-policy): *a user can author and run a Branch end-to-end today.* No trailing
conformance phase; no Parallel (4.2b) or Loop (4.2c).

## Corrected scope (vs the original task DoD)

The original DoD assumed D8-B's walked feature-fit (#494) is on `main`. **It is not** — `#494`
(`9f362005`) lives only on the unmerged, blocked `feat/ir-format-acceptance-window` branch and is
not an ancestor of HEAD. Verified consequences on `main`:

| Original DoD item | Reality on `main` | This slice |
|---|---|---|
| 5. Flip Branch `IrFeature` in `tau-ir/feature.rs` | `feature.rs` / `IrFeature` / feature-fit gate **do not exist** on `main` | **Dropped.** Do not import #494 (blocked, out of scope). Nothing rejects a Branch IR on `main`. |
| 6. wasm feature-reject at load | Load gating is **MAJOR-version-based** (`from_canonical_bytes`); `ir_format` = v2.5.0; Branch is additive (v2.4.0), so a Branch IR is already accepted | **Resolves to parity.** Prove wasm runs Branch via a `tau-wasm-host` roundtrip test — same load gate + same `run_ir_streaming` interpreter as native. |
| 3. Typecheck nested `.input` reachability ("audit gap") | **Already implemented + tested** in `typecheck.rs` (tree-wide id uniqueness, `check_nested_template_refs`, condition-locus scope). Just unreachable. | Wire authoring→lowering so those arms fire; prove reachability with the conformance fixture. |
| 4. Interpreter | Present on `main` (`interpreter/pipeline.rs` `StepRun::Branch`, `eval_condition`) | Verify only. |

No `ir_format` bump is needed: Branch is representable within the current v2.5.0 contract.

## Design boundary (explicit non-goals — debt guard)

TOML is a **frontend** projecting onto the IR contract; dependencies point inward (TOML → IR).
To keep this from becoming a scripting-language-in-TOML, the following are **out of scope now and
by intent**:

- **No compound conditions** (`AND`/`OR`) and **no expression grammar** in TOML. The condition is a
  single predicate over one locus, reusing the existing `[goals.*]` vocabulary verbatim — zero new
  contract surface.
- Rich/imperative control flow is the **TypeScript authoring** surface's job (`tau-ts-extract`,
  byte-equal IR). TS-authored Branch parity is a noted **follow-up**, not part of this slice.
- No Parallel / Loop / Suspend authoring (4.2b / 4.2c / 4.3).

If TOML control flow ever proves too clumsy, it is a *frontend removal*, not a contract migration.

## TOML syntax (Option A)

A `[[pipeline.steps]]` entry is either a **leaf** (`run = "kind:id"`, as today) or a **branch**
(`branch = { <condition> }` + nested `then` / `otherwise` step arrays). The two forms are mutually
exclusive.

```toml
[[pipeline.steps]]
id    = "triage"
run   = "agent:triage"
input = "${input}"

[[pipeline.steps]]
id     = "route"
branch = { evaluates = "steps.triage.output", check = "matches", pattern = "(?i)urgent" }

  [[pipeline.steps.then]]
  id    = "escalate"
  run   = "agent:oncall"
  input = "${steps.triage.output}"

  [[pipeline.steps.otherwise]]      # optional; omit for a one-armed branch
  id    = "ack"
  run   = "agent:writer"
  input = "${steps.triage.output}"
```

The `branch` inline table is a **byte-for-byte match of the `[goals.*]` field-set** (minus the
table-key id): `evaluates` (a locus string — `steps.<id>.output` or a path) plus one predicate
selector — `check = "exists"|"non_empty"|"equals"|"matches"|"min_count"|"schema_valid"` with its
companion field (`equals` / `pattern` / `min_count` / `schema`), **or** `fn = "<crate>::<path>"`
(native-fn escape hatch). `then` / `otherwise` are ordinary `[[pipeline.steps.*]]` arrays and
recurse through the same validator, so nested branches and `${steps.<id>.output}` templating in
arms work exactly like top-level steps. `otherwise` defaults to `[]` (matches the IR's "may be
empty").

## Authoring model (tau-pkg `project/project.rs`)

```rust
// Raw TOML (deny_unknown_fields). `run` becomes Option; add branch fields.
struct UncheckedPipelineStep {
    id: String,
    run: Option<String>,                       // leaf: "agent:x" | "tool:x" | "deterministic:x" | "check:x"
    input: Option<String>,
    branch: Option<UncheckedCondition>,        // branch form
    #[serde(default)] then: Vec<UncheckedPipelineStep>,       // recursive
    #[serde(default)] otherwise: Vec<UncheckedPipelineStep>,  // recursive; may be empty
}

// Exact field-set of UncheckedGoal, minus the table-key id.
struct UncheckedCondition {
    evaluates: String,
    check: Option<String>, pattern: Option<String>, equals: Option<String>,
    min_count: Option<u64>, schema: Option<serde_json::Value>,
    #[serde(rename = "fn")] r#fn: Option<String>,
}

// Validated
enum PipelineRunRef {
    Agent(String), Tool(String), Deterministic(String), Check(String),
    Branch { on: ConditionConfig, then: Vec<PipelineStepConfig>, otherwise: Vec<PipelineStepConfig> },
}
struct ConditionConfig { evaluates: LocusConfig, predicate: GoalPredicateConfig }   // mirrors IR Condition
```

**Validation** (`validate_pipeline`, made recursive):
- Exactly one of `{run, branch}` must be present; `then`/`otherwise` are only allowed when `branch`
  is present. Violations → `ProjectConfigError::PipelineValidation { id, message }`.
- Leaf `run` parsing unchanged (`run.split_once(':')`).
- Branch condition parsed by a **shared predicate parser** extracted from `validate_goal` —
  `parse_predicate(check, pattern, equals, min_count, schema, fn) -> Result<GoalPredicateConfig, _>`
  — reused by both `[goals.*]` and branch conditions (in-scope refactor, no behavior change; regex
  compiled at build time as today). `evaluates` parsed via the existing `parse_locus`.
- `then`/`otherwise` steps validated by recursing `validate_pipeline`'s per-step logic. Duplicate-id
  and forward-ref checks stay at the IR typecheck layer (tree-wide), which already handles nesting.

## Lowering (tau-ir-lower `lower/parse.rs`)

The current flat, infallible `.map` over pipeline steps becomes a **recursive, fallible** helper so
Branch arms lower their nested steps:

```rust
fn lower_step(s: &PipelineStepConfig) -> PipelineStep {
    PipelineStep {
        id: PipelineStepId(s.id.clone()),
        run: match &s.run {
            PipelineRunRef::Agent(id) => StepRun::Agent(AgentId(id.clone())),
            PipelineRunRef::Tool(id) => StepRun::Tool(ToolId(id.clone())),
            PipelineRunRef::Deterministic(id) => StepRun::Deterministic(StepId(id.clone())),
            PipelineRunRef::Check(id) => StepRun::Check(CheckId(id.clone())),
            PipelineRunRef::Branch { on, then, otherwise } => StepRun::Branch {
                on: lower_condition(on),                        // ConditionConfig -> tau_ir::check::Condition
                then: then.iter().map(lower_step).collect(),
                otherwise: otherwise.iter().map(lower_step).collect(),
            },
        },
        input: s.input.clone(),
    }
}
```

`lower_condition` maps `LocusConfig` → `tau_ir::check::Locus` and `GoalPredicateConfig` →
`tau_ir::check::GoalPredicate` (the same mapping already used when lowering `[goals.*]` into
`CheckVerify::Goal`; reuse that helper). Lowering stays infallible here — all condition/predicate
validity is decided in tau-pkg validation, and all reference/scope integrity in the existing IR
typecheck (`check_pipeline` / `validate_step_run`), which now becomes reachable.

## Typecheck (tau-ir-lower `lower/typecheck.rs`)

**No code change.** The Branch arms (tree-wide id uniqueness via `collect_all_ids`, per-arm scope in
`validate_step_run`, nested template refs via `check_nested_template_refs`, condition-locus
`ConditionUnknownOutput`) already exist and are unit-tested. This slice makes them **reachable** from
authored input; the conformance fixture + tau-pkg tests are the reachability proof.

## Interpreter

**No code change** (#454). `interpreter/pipeline.rs` early-dispatches `StepRun::Branch`, evaluates
`on` via `eval_condition`, and runs the chosen arm through recursive `run_steps` on the shared flat
store. Verified by the new conformance fixture (DevMode + BundleMode).

## wasm: explicit feature-reject at load (as-built correction)

The original plan assumed the guest could *run* a Branch via the shared interpreter. Implementation
uncovered a deeper reality: `tau-wasm-guest/src/guest.rs` drives `run_ir_streaming` — the **single
entry agent's loop** — not the pipeline executor (`run_pipeline`). The guest never executes *any*
pipeline (linear or Branch); on native it is the *host* (tau-cli / conformance) that dispatches
`run_pipeline` vs `run_ir`. The guest also hard-rejects ≠1 agent. So "run a Branch in-wasm" would
require teaching the guest to drive `run_pipeline` (+ multi-agent + deterministic registry +
artifact reader) — a substantial, orthogonal "guest drives pipelines" slice.

Worse, a *single-agent* workflow carrying a pipeline would pass the one-agent check and then run the
entry agent while **silently skipping the pipeline** — a latent correctness hole.

**Decision (spec's "OR explicit feature-reject at load" arm):** the guest rejects any
pipeline-bearing IR at load:

```rust
if module.workflow.pipeline.is_some() {
    return Err("tau-wasm-guest: pipelines (incl. Branch) are not yet executed in-wasm".to_string());
}
```

This is honest (the guest genuinely cannot run pipelines yet), closes the silent-skip bug, and keeps
the slice small. Native `tau run` — the reference host — already executes authored branches with all
predicates, so "author and run a Branch end-to-end" holds. A `tau-wasm-host` roundtrip test bakes a
Branch IR and asserts the guest **rejects** it cleanly.

**Follow-up slice:** *guest drives `run_pipeline` in-wasm* (multi-agent + no_std goal-predicate
registry + artifact reader), which lifts this reject into true execution parity.

## Conformance fixture (CI)

Add `crates/tau-ir-conformance/fixtures/20_branch_route/` (triple: `workflow.toml` + `mock_llm.jsonl`
+ `expected_report.json`), modeled on `08_pipeline_sequence`. The workflow authors a `triage` agent
step, then a `route` Branch whose condition tests `steps.triage.output` (e.g. `matches`), with an
agent in each arm. Wire it into `crates/tau-ir-conformance/tests/conformance.rs` (it runs under both
DevMode and BundleMode, and BundleMode exercises serialize→`from_canonical_bytes`→interpret — the
same path wasm uses). The existing IR-schema sample `schemas/ir/conformance/valid/control_flow_branch.json`
already covers the schema kit; no change there.

## Docs example

Add a short reference/how-to page under `docs/` (e.g. `docs/how-to/authoring-a-branch.md`) showing
the `route` example above, the condition vocabulary, and the one-armed (`otherwise` omitted) form.
Register it in `docs/SUMMARY.md`. Build locally with `mdbook build` + linkcheck before the PR.

## Testing (TDD)

Write the failing e2e test first: a `workflow.toml` with a branch → build IR → run interpreter →
assert the correct arm ran (the `20_branch_route` fixture, plus a direct tau-ir-lower e2e test in
`lower_e2e.rs`). Then:
- **tau-pkg** (`project.rs` unit tests): parses a branch step; rejects `run`+`branch` both present;
  rejects `then`/`otherwise` without `branch`; parses a one-armed branch (`otherwise` defaults empty);
  parses a nested branch.
- **tau-ir-lower** (`lower_e2e.rs`): authored branch lowers to `StepRun::Branch`; a branch arm
  referencing a ghost agent is rejected by typecheck (`UnknownPipelineRun`); a condition reading an
  out-of-scope output is rejected (`ConditionUnknownOutput`).
- **tau-ir-conformance**: `20_branch_route` green (DevMode + BundleMode).
- **tau-wasm-host**: roundtrip test — Branch IR runs in-guest, correct arm executes.

## Files touched

- `crates/tau-pkg/src/project/project.rs` — authoring model + recursive `validate_pipeline` +
  shared `parse_predicate` refactor + tests.
- `crates/tau-ir-lower/src/lower/parse.rs` — recursive `lower_step` + `lower_condition`.
- `crates/tau-ir-lower/tests/lower_e2e.rs` — e2e lowering/typecheck tests.
- `crates/tau-ir-conformance/fixtures/20_branch_route/*` + `tests/conformance.rs` wiring.
- `crates/tau-wasm-host/tests/roundtrip.rs` — Branch parity test.
- `docs/how-to/authoring-a-branch.md` + `docs/SUMMARY.md`.

**Conflict note:** this slice owns the tau-pkg pipeline authoring model + lowering match. 4.2b
(Parallel) and 4.2c (Loop) share these files and must serialize *after* this lands.
