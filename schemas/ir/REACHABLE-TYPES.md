# IR schema — reachable type inventory (from IrModule)

This inventory was produced by walking `tau_ir::module::IrModule` fields
transitively through `crates/tau-ir/src/*.rs` and the foreign types it reaches
in `tau-domain` and `tau-ports`. Every type that appears in the serialized form
of an `IrModule` is listed below.

The `schema strategy` column records how the `JsonSchema` impl will be provided:
- **cfg_attr derive**: the type already derives `Serialize`/`Deserialize` via
  `#[derive(…)]`; adding `#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]`
  is sufficient.
- **hand impl**: the type has a hand-written `impl Serialize` (custom wire format);
  a matching hand-written `impl JsonSchema` is required so the schema reflects the
  actual JSON shape (not the struct/enum Rust shape).

## Table

| type | crate | serde | schema strategy |
|---|---|---|---|
| IrModule | tau-ir | derive | cfg_attr derive |
| IrFormatVersion | tau-ir | derive (newtype String) | cfg_attr derive |
| Workflow | tau-ir | derive | cfg_attr derive |
| TargetTriple | tau-ports | **custom** (parse-from-string round-trip) | hand impl |
| TriggerBinding | tau-ir | derive | cfg_attr derive |
| TriggerKind | tau-ir | derive | cfg_attr derive |
| RetryPolicy (trigger) | tau-ir | derive | cfg_attr derive |
| Backoff | tau-ir | derive | cfg_attr derive |
| BackoffStrategy | tau-ir | derive | cfg_attr derive |
| Agent | tau-ir | derive | cfg_attr derive |
| AgentId (tau-ir) | tau-ir | derive (newtype String) | cfg_attr derive |
| ModelRef | tau-ir | derive | cfg_attr derive |
| ContextConfig | tau-ir | derive | cfg_attr derive |
| ContextStep | tau-ir | derive | cfg_attr derive |
| DeterminismClass | tau-ir | derive | cfg_attr derive |
| ContextNodeKind | tau-ir | derive | cfg_attr derive |
| AgentBudget | tau-ir | derive | cfg_attr derive |
| Durability | tau-ir | derive | cfg_attr derive |
| CheckpointGranularity | tau-ir | derive | cfg_attr derive |
| DurableStore | tau-ir | derive | cfg_attr derive |
| Tool | tau-ir | derive | cfg_attr derive |
| ToolId | tau-ir | derive (newtype String) | cfg_attr derive |
| ToolImpl | tau-ir | derive | cfg_attr derive |
| NativeFnRef | tau-ir | derive | cfg_attr derive |
| Hash256 | tau-ir | type alias `[u8; 32]` — serialized as serde bytes array | cfg_attr derive (on NativeFnRef/ToolImpl fields) |
| ToolSpec | tau-ir | derive | cfg_attr derive |
| CapabilityRequirements | tau-ir | derive | cfg_attr derive |
| Capability | tau-domain | **custom** (oneOf by "kind" map) | hand impl |
| CapabilityTable | tau-ir | derive | cfg_attr derive |
| Deterministic | tau-ir | derive | cfg_attr derive |
| StepId | tau-ir | derive (newtype String) | cfg_attr derive |
| SubflowEdge | tau-ir | derive | cfg_attr derive |
| SubflowId | tau-ir | derive (newtype String) | cfg_attr derive |
| SubflowKind | tau-ir | derive | cfg_attr derive |
| Pipeline | tau-ir | derive | cfg_attr derive |
| PipelineStep | tau-ir | derive | cfg_attr derive |
| PipelineStepId | tau-ir | derive (newtype String) | cfg_attr derive |
| StepRun | tau-ir | derive | cfg_attr derive |
| Check | tau-ir | derive | cfg_attr derive |
| CheckId | tau-ir | derive (newtype String) | cfg_attr derive |
| CheckVerify | tau-ir | derive | cfg_attr derive |
| Locus | tau-ir | derive | cfg_attr derive |
| GoalPredicate | tau-ir | derive | cfg_attr derive |
| JudgeRef | tau-ir | derive | cfg_attr derive |
| RetryPolicy (check) | tau-ir | derive | cfg_attr derive |
| OnFail | tau-ir | derive | cfg_attr derive |
| serde_json::Value | serde_json | — (foreign crate) | schemars ships built-in impl |
| Node | tau-ir | derive | cfg_attr derive (not in IrModule directly, but exported) |

## Notes

1. **Two `RetryPolicy` types**: `tau_ir::trigger::RetryPolicy` (trigger re-invocation)
   and `tau_ir::check::RetryPolicy` (check failure handling) are distinct types with
   the same name in different modules. Both need derives; the crate root does NOT
   re-export `check::RetryPolicy` to avoid the name clash.

2. **`Hash256 = [u8; 32]`**: A type alias, not a newtype. serde serializes `[u8; 32]`
   as a JSON array of integers. schemars has a built-in `JsonSchema` impl for fixed-size
   byte arrays; no extra work needed.

3. **`serde_json::Value`**: Used directly in `Agent.output_schema`, `ToolSpec.input_schema`,
   `Deterministic.input_schema/output_schema`, `GoalPredicate::SchemaValid`, and
   `ContextStep.config`. schemars 1.x ships `impl JsonSchema for serde_json::Value`
   (produces `{}` — accepts any JSON) behind the `impl-serde-json` feature which is
   on by default. No custom impl needed.

4. **`tau-domain::Capability`** is the only tau-domain type reachable from `IrModule`
   (via `CapabilityRequirements.declared: Vec<Capability>`). All other tau-domain id
   types (`MessageId`, `AgentInstanceId`, `AgentId`, `PackageName`) are NOT reachable
   from the serialized `IrModule` — they are used only in `tau_ir::message::Message`
   (runtime wire, not IR storage) or in tau-domain internals. So only `Capability`
   needs a hand `JsonSchema` impl in tau-domain.

5. **`TargetTriple`** uses a custom `impl Serialize` that serializes to/from its
   string representation (e.g. `"linux-native-strict"`). The hand `JsonSchema` impl
   must declare `{ "type": "string" }` (plus optionally an `enum` of known values).

## Summary

- **Total types**: 46
- **hand impl** (custom JsonSchema required): 2 — `TargetTriple` (tau-ports), `Capability` (tau-domain)
- **cfg_attr derive** (standard derive sufficient): 44

Beyond the three originally known hand types, investigation found that `MessageId`
(tau-domain) is **NOT** reachable from `IrModule`'s serialized form; it lives only in
the runtime wire message. Similarly `AgentInstanceId`, `PackageName`, and `AgentId`
(tau-domain) are not reachable. No new hand-impl types were found beyond the expected
`TargetTriple` and `Capability` (the brief listed `MessageId` as a known hand type
but it is not reachable from `IrModule` — it can be dropped from the schema task's
scope).
