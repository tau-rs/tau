# ADR-0044: Deliverables & goals — build-time-checked postcondition steps

**Status:** Accepted
**Date:** 2026-06-13
**Supersedes:** none

## Context

tau pipelines could run a sequence of agents and tools but had no
first-class way to assert that the pipeline produced what it was supposed
to, or that a named goal was met. The failure mode was "ran clean, delivered
nothing" — discovered by a human reading the output, not by the system.

tau's standing principle ([`docs/explanation/tau-philosophy.md`](../explanation/tau-philosophy.md))
is **Rust-class build-time enforcement: any check that could run at build time
must run at build time.** A postcondition feature must therefore fail the
*build* of a pipeline that cannot deliver its stated output, not only the
runtime execution.

This ADR records the seven architecture decisions made during the plan phase
(see `docs/superpowers/plans/2026-06-13-deliverables-and-goals.md`, "Key
implementation decisions") and the honest limits / deferrals that were not
resolved in v1.

The prerequisite — an engine-sequenced IR pipeline executor with an
addressable `steps.<id>.output` store — was delivered as Plan 1 (workflow IR,
ADR-0037). This ADR covers Plan 2 only.

## Decisions

### Decision 1 — `Check` is NOT a `Node` enum variant

The `Node` enum (`Agent` / `Tool` / `Deterministic` / `Subflow`) is a type
abstraction that never appears in the serialized `IrModule`. Only the
`Workflow` `BTreeMap`s do. Adding a `Check` variant to `Node` would create
ripple through every exhaustive `match` on `Node` across the codebase and
tie a cross-cutting control-flow concept to an abstraction whose purpose is
typing individual steps.

**Decision:** `Check` lives in a new `workflow.checks: BTreeMap<CheckId, Check>`
map. It is positioned in the pipeline via a new `StepRun::Check(CheckId)` step
variant. `Node` is untouched.

This is exactly the same pattern used for capabilities (carried in separate
maps, not embedded in the node type).

### Decision 2 — Checks are pipeline steps, default-appended at the tail

The authoring surface (`[goals.*]` / `[deliverables.*]`) carries no position
field. Lowering auto-appends one `StepRun::Check` step per check at the end
of `pipeline.steps`, in deterministic order (goals by id, then deliverables
by id).

A check may also be placed at an explicit checkpoint by writing a pipeline
step `run = "check:<id>"`. An explicitly-placed check is not auto-appended
— it is used only at the explicit position.

**Why not a separate "tail" concept:** keeping checks as ordinary pipeline
steps means the pipeline array is the single source of execution order.
Introspection, trace spans, and step-output references all work uniformly.
The "checkpoints anywhere" door is left open with zero rework.

### Decision 3 — All build-time semantic checks live in `tau-pkg::validate()`

`tau-pkg` owns `ProjectConfig`, the pipeline order, agent `produces`,
tool capabilities, and glob subset logic, and has `std` (including regex).
`tau-ir` depends on `tau-pkg` — not the reverse — so performing the
producer/gate/judge/regex checks in `tau-pkg` needs no new cross-crate dep
and keeps `tau-ir` lowering to structural integrity only (referenced IDs
exist, locus step order is respected).

This is the existing split: `tau-pkg` owns semantics, `tau-ir` owns structure.

### Decision 4 — Goal predicates are host-registered `DeterministicRegistry` fns

`tau-runtime-core` is `#![no_std]` + `alloc` and cannot run regex or
`std::fs`. The six menu predicates (`exists` / `non_empty` / `equals` /
`matches` / `min_count` / `schema_valid`) and the native-fn escape hatch all
route through the existing `DeterministicRegistry` port.

`run_pipeline` resolves the locus to content (file via the artifact reader,
or named output from the output store), builds a JSON args object
`{ present, content, ...params }`, and calls `registry.invoke(fn_name, &args)`.
The host (`tau-cli` / conformance) registers the six menu predicates under
reserved fn names (`__tau::goal::exists`, etc.). The escape-hatch `fn` routes
to the same registry.

**Why not a separate predicate evaluator:** the deterministic-fn machinery
already handles the exact shape (named fn, JSON args, JSON result). Reusing
it means the escape hatch is one registry entry, not a new dispatch path.

### Decision 5 — The artifact reader is a defaulted `ToolDispatcher` method

Mirroring `deterministic_registry()` / `clock()` / `random()`, a new
`fn artifact_reader(&self) -> Option<Arc<dyn ArtifactReader>> { None }` is
added as a defaulted method on `ToolDispatcher`. No change to `run_pipeline`'s
signature or its callers. The tokio host returns a `std::fs` reader; tests
return an in-memory mock (`InMemoryArtifactReader`).

This keeps `tau-runtime-core` I/O-free and the host substitutable (the
conformance test harness and the production binary register different readers
via the same interface).

### Decision 6 — `judge_model` is a runtime no-op in v1 (honest limit)

The `judge_model` field is **parsed, validated at build time, stored in the
IR, and included in the bundle hash** — so it is meaningful as declarative
intent. However, at runtime in v1, it is a **no-op**.

The reason is structural: the `ToolDispatcher` exposes a single `llm_backend()`
and `Agent.model` is already ignored at runtime today (every request uses
`backend.name()`). Per-judge (and per-agent) model selection requires
multi-backend resolution — routing an inference request to a specific model
provider given a model name — which does not exist yet.

So the built-in judge and any custom judge run on the ambient backend in v1.
This is stated in the how-to guide and in this ADR, not papered over.

**Consequence:** a `tau check` that validates `judge_model = "claude-opus-4-5"`
succeeds; the runtime silently uses the ambient backend. When multi-backend
routing is implemented, the stored `judge_model` value is already in the IR and
will begin to take effect without an authoring change.

### Decision 7 — Bundle format `v1.1.0` → `v1.2.0` (additive MINOR bump)

`workflow.checks` is serialized with
`#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]` and
`StepRun::Check` never appears in a check-free pipeline. Every existing
fixture's canonical bytes are unchanged (asserted by the conformance suite).

The bump is MINOR per semver: new fields, backward-compatible read (a v1.1.0
reader ignores `workflow.checks`; a v1.2.0 reader handles both).

## Honest limits and deferrals

### `judge_model` runtime no-op (Decision 6 above)

Repeated here for emphasis: the field is fully supported at the config / IR /
bundle level; it is only the *runtime selection* that is deferred to the
multi-backend resolver.

### `output_schema` judge-compatibility warning is deferred

The design spec includes a build-time warning:

> `warn: agent 'house_critic' used as a judge declares an output_schema`
> `incompatible with the verdict contract { met, rationale }`

This warning is not implemented in v1. `AgentEntry` has no `output_schema`
field today — only `Deterministic` steps have one. When `AgentEntry` gains
an `output_schema`, checking compatibility with the `{ met, rationale }`
verdict contract is a straightforward follow-up (one `validate_postconditions`
check). Nothing prevents adding it then.

### Multi-check feedback uses single-gate semantics

When multiple checks exist in a pipeline and a check rewinds to its gate,
feedback from earlier checks that passed within the same rewind span may be
cleared when the rewind re-executes those steps. The current implementation
uses a single `feedback: Option<String>` variable — per-check feedback
isolation (each check's rationale travels independently through its own span)
is acceptable-for-v1 future work.

### `tau check --json` reports `kind = "Other"` for new `ProjectConfigError` variants

The human-readable and SARIF outputs of `tau check` carry the full error
message for all new `ProjectConfigError` variants (`DeliverableNoProducer`,
`GateAfterProducer`, etc.). The structured JSON `kind` field, however,
classifies these as `"Other"` rather than a specific kind string. Adding
precise `kind` classification is a minor follow-up that requires extending
the `kind` discriminant mapping in the check renderer.

### Loci deferred from v1

The following deliverable locus types are explicitly out of scope and not
implemented:

- External resources via capability (HTTP/MCP side-effects).
- Provenance/trace evidence ("tool X was invoked").
- Process outcome loci ("step exited 0" — belongs in `goal`).

### Legacy `tau-workflow` runner

The standalone `tau-workflow` runner (`workflows/*.toml`) never lowers to IR
and does not gain this feature. Unifying it onto the IR is a separate
migration project.

## Consequences

**Positive:**

- Pipelines that cannot produce a deliverable are rejected at `tau build` /
  `tau check` before any run — zero tokens spent on a structurally broken
  pipeline.
- The producer binding (`produces` + `fs.write` coverage) brings Rust-style
  explicit intent to artifact ownership — ambiguity (two agents claiming the
  same path) is a build error.
- The retry loop is bounded at two levels (`max_attempts` + `AgentBudget`)
  and structurally guaranteed (gate ≤ producer, span has an LLM step) at
  build time — guaranteed-no-op loops are rejected before runtime.
- The deterministic-fn reuse means the six menu predicates share the same
  registration and extension story as any native fn — no separate predicate
  evaluator to maintain.
- Every verdict (pass, fail, retry) is durable in the trace with the
  `rationale` at each attempt — the full retry history is observable.

**Negative / obligations:**

- `produces` is an explicit declaration — an agent that writes a file without
  declaring `produces` will not bind to a deliverable. This is intentional
  (intent > inference) but requires authors to be explicit.
- `judge_model` silently does nothing in v1. Authors who write it will not
  see an error; they will see no effect. The ADR and how-to guide document
  this clearly.
- The `output_schema` judge-compatibility check is absent — a custom judge
  that returns something other than `{ met, rationale }` fails at runtime
  (parse error → `met: false`), not at build time.
- Multi-backend routing (to make `judge_model` effective) is a prerequisite
  for the "tune" judge level to work end-to-end.

## Alternatives considered

**A — Make `Check` a `Node` enum variant.** Avoids the new `workflow.checks`
map. Rejected because `Node` is exhaustively matched throughout the codebase
(interpreter, typecheck, capability-fit, etc.); adding a variant creates
structural ripple and couples a cross-cutting control-flow concept to a
type-abstraction enum whose purpose is individual step typing.

**B — Infer the producer from capability paths (no `produces` field).** Build
time checks capabilities to find which agent *could* write the deliverable
path. Rejected because capability paths are glob patterns — inference produces
ambiguity (multiple agents may have covering capabilities) and loses intent
(the agent intended to write the file, not just that it was allowed to).
`produces` is the Rust-fn-signature analogue: explicit, verifiable, binding.

**C — Reuse the existing `DeterministicStep.fn_ref` mechanism vs. a new
predicate enum.** The predicate menu could be encoded as six specific
`fn_ref` values registered at host init. Chosen: the enum (`GoalPredicate`)
makes the serialized IR self-documenting and keeps common predicates readable
in bundles; the native-fn escape hatch (`NativeFn(fn_ref)`) gives the power
path. The registry remains the single execution point.

**D — Run the judge inline in `run_pipeline` without synthesizing an `Agent`.**
Simpler code path. Rejected because the `run_agent` / `AgentBudget` budget
cap then does not apply to the judge, making it possible to burn unbounded
tokens in a judge with no cap. Synthesizing an `Agent` with `budget.max_turns
= Some(1)` gives the judge the same budget machinery as every other agent.

## References

- Implementation plan: `docs/superpowers/plans/2026-06-13-deliverables-and-goals.md`
- Design spec: `docs/superpowers/specs/2026-06-13-deliverables-and-goals-design.md`
- How-to guide: [`docs/how-to/assert-pipeline-postconditions.md`](../how-to/assert-pipeline-postconditions.md)
- Related ADRs: [ADR-0037](0037-workflow-ir.md) (workflow IR, Plan 1),
  [ADR-0010](0010-tool-args-schema-validation.md) (JSON schema validation),
  [ADR-0014](0014-sandboxing.md) (capability model)
