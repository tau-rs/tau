# ADR-0073: IR v3 — multi-pipeline modules and pipeline imports

**Status:** Accepted (records locked decision §10.2 of the
[2026-09-01 consolidated design](../superpowers/specs/2026-09-01-tau-authoring-ops-and-primitives-design.md);
Phase 0 ADR wave)
**Date:** 2026-09-01
**Deciders:** maintainer, via the 2026-09-01 brainstorm session
**Amends:** ADR-0037 (workflow IR — module shape), ADR-0056 (contract
versioning — this is the planned single MAJOR bump)

## Context

`IrModule` today carries at most one pipeline
(`workflow.pipeline: Option<Pipeline>`,
`crates/tau-ir/src/{module,pipeline}.rs`), and module entry resolution is
agent-shaped (`IrModule::entry_agent()`, `module.rs:155`). The redesign
makes repos multi-pipeline (one file = one pipeline under `pipelines/`,
id = file path, per ADR-0069/0070 scanning and ADR-0071), and
`SubflowKind::Compose` has been a reserved-but-rejected variant since v0
(`crates/tau-ir/src/subflow.rs:27`, rejected in `error.rs:106-113`)
because composing "another full workflow" is meaningless while a module
can hold only one.

ADR-0056 requires IR format changes to be versioned; new step kinds have
shipped as MINOR bumps (precedent: v2.4.0). Removing the single-pipeline
assumption is structural — every consumer that asks "the pipeline" must
ask "which pipeline".

## Decision

1. **`pipelines: BTreeMap<PipelineId, Pipeline>`** replaces
   `workflow.pipeline: Option<Pipeline>`. This is **the one MAJOR
   ir_format bump — v3.0.0**. A frozen v2 reader is kept: v2 bundles
   load forever, mapped to the degenerate one-pipeline case.
2. **The single `[pipeline]` remains the legacy degenerate case** in
   authoring: a project with no `pipelines/` dir and one pipeline lowers
   to a one-entry map. No author is forced to restructure (no-flag-day).
3. **Entry resolution becomes pipeline-shaped:** `IrModule::entry_agent()`
   gives way to an entry-pipeline accessor; run verbs address
   `<pipeline-id>` with the obvious default when exactly one exists
   (mirroring today's exactly-one-agent rule).
4. **Pipeline imports.** Pipelines may import each other and mount the
   imported pipeline as a sub-flow step — this unblocks
   `SubflowKind::Compose`. Rules:
   - the import graph is **acyclic**; a cycle is a synth/validate error,
     never a runtime discovery;
   - mounted steps are **namespaced under the call-site id**
     (hierarchical lineage per ADR-0070);
   - **capabilities are unchanged** — same project, same `[allow]`;
     Compose grants nothing and attenuates nothing (attenuation remains
     the Dynamic-region mechanism, ADR-0059 / subflow attenuation work).
5. **Version discipline (restated from ADR-0056, binding here):** new
   step kinds are MINOR; only multi-pipeline is MAJOR. Schema files,
   `REACHABLE-TYPES.md`, and conformance fixtures move together in the
   same PR (the `UPDATE_SCHEMA=1` flow in
   `crates/tau-ir/tests/schema_export.rs`).

## Consequences

- Every ops verb gains a natural addressing unit: `tau plan`/`apply`
  diff and pin per repo but can slice per pipeline (`--pipeline`,
  ADR-0075); `tau run <pipeline-id>` disambiguates multi-pipeline repos.
- Consumers of `entry_agent()` (CLI run path, wasm guest, conformance
  fixtures) must migrate in the same change that bumps the format;
  the frozen v2 reader keeps old bundles loading.
- `UnsupportedComposeSubflow` is deleted when Compose lowers; the
  conformance suite gains multi-pipeline + compose fixtures.
- The wasm feature registry gains no new obligation from Compose itself
  (it is flow, not effect), but multi-pipeline changes the guest driving
  surface — the v3 reader must land on the guest path too
  (`run_pipeline`, ADR-0068) in the same epic (E-2).
- Risk accepted: a MAJOR bump invalidates any external tooling parsing
  v2 JSON directly; mitigated by the frozen v2 reader, the published
  schema, and the one-time nature of the bump (everything else in the
  redesign is MINOR).

## Alternatives considered

- **N single-pipeline modules per repo (one bundle per pipeline).**
  Rejected: imports across bundles would need a cross-artifact linker
  and shared-asset dedup; pins and plans fragment into N files; the
  atomic-apply unit (ADR-0075) disappears.
- **Keep `Option<Pipeline>` + a sidecar index of extra pipelines.**
  Rejected: two places to look for the same concept; the sidecar is
  exactly the kind of hidden format extension ADR-0056 bans.
- **Represent imports by inlining at lower time (no Compose at
  runtime).** Rejected as the *only* mechanism: inlining loses the
  call-site boundary that plan/trace/namespacing report on. (Lowering
  MAY inline as an optimization later; the IR keeps the boundary.)
- **MINOR-bump with `pipeline` kept alongside `pipelines`.** Rejected:
  dual-representation formats rot — every reader needs both paths
  forever; better one honest MAJOR with a frozen old reader.

## References

- Design: [`2026-09-01-tau-authoring-ops-and-primitives-design.md`](../superpowers/specs/2026-09-01-tau-authoring-ops-and-primitives-design.md) §1 (multi-pipeline & imports), §2 (IR justification)
- Related: ADR-0037, ADR-0056, ADR-0058/0059, ADR-0068, ADR-0069/0070,
  ADR-0071, ADR-0072
- Epic: E-2 in [`vision-roadmap.md`](../superpowers/plans/vision-roadmap.md)
