# Subflow runtime capability attenuation (D5-C runtime half / D1-C amendment)

**Status:** approved (brainstorm), pending implementation plan
**Date:** 2026-07-19
**Decision refs:** D5-C (runtime half), D1-C attenuation family
**Related landed work:** EPIC 1.5 relative capability lattice (PR #445, `d678802d`)

## Problem

The IR interpreter path (`tau-runtime-core/src/interpreter/`) performs **zero
capability enforcement**. `agent_loop.rs:347-351` states it outright, and the
subflow dispatch arm (`agent_loop.rs:190-206`) re-invokes `run_ir` for the child
agent with the **same** dispatcher and **no capability narrowing**. A subflow tool
declares a capability envelope (`[tools.X] subflow = "child"; capabilities = […]`)
intended to bound what the spawned child may do, but at runtime that envelope is
ignored — the child runs with whatever its own tools declare.

The static half (EPIC 1.5 lattice link L2) already checks
`subflow-tool.capabilities ⊆ agent-effective-caps` for **tau-cli-authored**
workflows (`tau-cli/src/cmd/check/categories/governance.rs:275-290`). But:

1. Hand-crafted or externally-produced IR (bundles, non-tau-cli authoring paths)
   bypasses tau-cli governance entirely.
2. Static L2 checks the *authored* envelope against the *manifest* effective caps;
   it cannot see the *runtime* narrowing a parent was itself spawned under. A child
   running under an attenuated grant `C1` may hold a subflow tool whose cap_subset
   `C2 ⊄ C1` — a case only a runtime check catches.

This spec covers the **runtime half**: enforce, in `tau-runtime-core`, that a
subflow child (and every descendant) is clamped to the meet of its ancestors'
declared cap_subsets.

## Non-goals

- **No change to the root agent's gating.** The top-level agent's own tool calls
  remain ungated at runtime (build-time governance covers them). Only subflow
  descendants are attenuated. `meet(⊤, C1) = C1`.
- **No IR data-model change.** No new `ToolImpl::Subflow` field, no IR format bump.
- **No new static check.** The static half is EPIC 1.5 L2 (already landed).
- **No materialized-meet / `granted_capabilities_override` reuse.** Rejected — see
  Appendix A.

## Design

### 1. Data model & lowering — unchanged

A subflow tool's `cap_subset` **is** its existing IR capability envelope:
`workflow.tools[tool_id].capabilities` (`CapabilityRequirements`), already populated
by lowering from `[tools.X].capabilities` and reachable at the dispatch site via the
`DispatcherTool`'s `tool_id` + `module`.

Consequences:

- EPIC 1.5 L2 already statically guarantees `cap_subset ⊆ agent-effective` for
  tau-cli workflows, so the static and runtime halves are consistent **by
  construction** — the runtime meet is pure defense-in-depth.
- No IR format bump, no lowering change, no new lowering error variant.

### 2. Enforcement — `AttenuatedDispatcher` (no_std, `tau-runtime-core`)

A dispatcher decorator that gates each tool invocation against one frame's
cap_subset and delegates everything else:

```rust
pub(crate) struct AttenuatedDispatcher {
    /// This frame's cap_subset (the invoking subflow tool's capabilities).
    grant: CapabilityRequirements,
    /// The subflow tool id that imposed this frame — provenance for denials.
    frame: ToolId,
    /// Source of a called tool's declared required caps
    /// (`module.workflow.tools[id].capabilities`).
    module: Arc<IrModule>,
    /// dyn ⇒ recursive nesting does not produce unbounded monomorphized types.
    inner: Arc<dyn ToolDispatcher>,
}

impl ToolDispatcher for AttenuatedDispatcher {
    fn invoke(&self, tool_id, args) -> …future… {
        // A called tool's declared caps live on its Tool node; absent ⇒ no caps ⇒ allowed.
        let required = self.module.workflow.tools.get(tool_id).map(|t| &t.capabilities);
        if let Some(missing) = first_unsatisfied(required, &self.grant) {
            return Ok(deny(tool_id, missing, &self.frame)); // is_error tool result
        }
        self.inner.invoke(tool_id, args)                    // may re-check parent frame
    }
    // llm_backend_for / clock / random / deterministic_registry /
    // artifact_reader / context_transformer_registry / checkpointing
    //   → delegate verbatim to `inner`.
}
```

`first_unsatisfied` reuses the existing `capability::capability_satisfies`
per-capability subsumption (the same predicate the kernel path uses). Both the frame
`grant` and a called tool's required caps come from the **same** source — the Tool
node's `capabilities` field — so there is no ambiguity vs. the redundant
`workflow.capability_table`.

**Wiring.** The subflow arm (`agent_loop.rs:190`) wraps the child's dispatcher in
**one** `AttenuatedDispatcher` layer carrying this tool's capabilities, then spawns
the child through it:

```rust
ToolImpl::Subflow { target } => {
    let cap_subset = self.module.workflow.tools[&self.tool_id].capabilities.clone();
    let child_dispatcher: Arc<dyn ToolDispatcher> = Arc::new(AttenuatedDispatcher {
        grant: cap_subset,
        frame: self.tool_id.clone(),
        module: self.module.clone(),      // Arc clone, cheap
        inner: self.dispatcher.clone(),   // Arc<D> → Arc<dyn ToolDispatcher> coercion
    });
    run_ir(self.module.clone(), &target, child_dispatcher, Vec::new()).await
}
```

**Composition = exact meet, lazily.** Nesting yields
`Attenuated{C2, inner: Attenuated{C1, inner: RAW}}`. A grandchild tool call is
checked `⊆ C2` (outer) then, on delegation, `⊆ C1` (inner). By the defining lattice
identity `r ⊑ meet(C1,C2) ⟺ r ⊑ C1 ∧ r ⊑ C2`, this is the **exact** meet decision —
with no glob-set intersection and no loss of precision. Grants narrow monotonically
with depth.

**Monomorphization note.** `inner` must be `Arc<dyn ToolDispatcher>` (not a generic
`D`); otherwise recursive nesting produces `Attenuated<Attenuated<…>>` of unbounded
depth at compile time. This may require `run_ir` (and the subflow call path) to
accept `Arc<dyn ToolDispatcher>` at the subflow boundary, or an internal object-safe
entry. To confirm during planning: whether `run_ir`'s current `D: ToolDispatcher`
generic bound admits `D = AttenuatedDispatcher` with a `dyn` inner cleanly, or a thin
`run_ir_dyn` shim is warranted.

### 3. Denial semantics + observability

**Soft-deny.** On a denied call the decorator returns
`Ok(ToolInvocationResult { body: None, error: Some(msg) })`. `DispatcherTool`'s
existing Native/Mcp arm (`agent_loop.rs:172-177`) converts a tool-side `error` into
an `is_error: true` `ToolResult`, so the child LLM sees the denial and can adapt. The
tool **never executes** — the security boundary holds; only the run-continues-vs-abort
policy differs from the kernel's hard `PolicyDenied`. Soft-deny is chosen because the
gate lives in the dispatcher (not the kernel loop), and because "child is told and
reacts" is the better agent-model behavior. (Hard-fail would require relocating the
gate into the kernel `stream.rs` loop — larger, and rejected here.)

**Structured error.** Reuse `error::CapabilityDenial` (`error.rs:130`,
`#[non_exhaustive]`), extended non-breakingly:

- add `narrowing_frame: Option<String>` (defaults `None`; set via a
  `with_narrowing_frame(tool_id)` builder method so the existing `new()` signature
  and all current callers are untouched);
- `Display` appends `" (narrowed by subflow \`<frame>\`)"` when present.

The denial `msg` names the tool, the missing capability kind/detail, and the
narrowing frame — satisfying the amendment's "name tool, missing caps, and the
narrowing frame that removed them."

**Trace event.** Emit from core on every denial:

```rust
warn!(name = "runtime.subflow.attenuation_denied",
      tool = %tool_id.0, missing = %missing_kind, frame = %self.frame.0);
```

`tracing` is no_std/wasm-portable and is the established convention
(`stream.rs:487`, `pipeline.rs:159`). Because the decorator lives in
`tau-runtime-core`, the event and the `is_error` tool result are produced
**identically** under `tau dev`, `tau run --bundle`, and the wasm guest.

### 4. Testing

Unit (tau-runtime-core):

- `AttenuatedDispatcher` denies a call whose required cap ⊄ grant; allows one ⊆ grant;
  passes through a tool with no declared caps.
- Denial `CapabilityDenial` carries tool, missing cap, and `narrowing_frame`.
- Nested `Attenuated{C2, Attenuated{C1}}`: a cap in `C1\C2` and one in `C2\C1` are
  both denied; a cap in `C1∩C2` is allowed (composition = meet).

e2e / conformance (tau-ir-conformance + host):

- Fixture: parent holds tool `T` (cap `fs.write /proj/**`); subflow `handoff`
  cap_subset excludes `fs.write`; child calls `T` → denied with structured error
  naming `T`, `filesystem.write`, frame `handoff`.
- Fixture: proper narrowing (child's calls ⊆ cap_subset) → runs to completion.
- Multiset-conformance on the narrowing fixture: identical observable event multiset
  across `dev`, `bundle`, and `wasm`.

## Appendix A — why not materialized meet / `granted_capabilities_override`

Enforcement precision is identical: `r ⊑ meet(P,C) ⟺ r ⊑ P ∧ r ⊑ C` (defining
property of the lattice meet), so composition computes the same decision a
materialized `child_effective` would. Materializing costs a **glob path-set
intersection** that (a) does not exist in the codebase (only a *subset* predicate
does), and (b) is not closed under the glob syntax — forcing an unsound
over-approximation, a spuriously-denying under-approximation, or net-new glob algebra.

The only real pull toward materializing is reusing the kernel's
`granted_capabilities_override: Vec<Capability>` (the mechanism multi-agent
agent-spawn attenuation uses at `stream.rs:956,1213`). But that plumbing is not free
on the interpreter path: `prepare_agent_run` builds the child with
`RunOptions::default()` (no override) and a stub manifest (`capabilities: []`,
`agent_loop.rs:373`), and registers each `DispatcherTool` with **no** declared caps
(`:448-455`). To use the override one would also have to wire every IR tool's caps
into kernel tool registration — strictly more new code than a decorator — **and**
still solve the glob meet. Passing `cap_subset` (not the meet) as the override would
drop the ancestor bound and be **unsound**. Composition preserves the ancestor bound
structurally, with better denial provenance (which frame narrowed the cap).

## Open items to resolve in the plan

- Exact `run_ir` signature accommodation for a `dyn`-inner decorator (generic vs
  `run_ir_dyn` shim).
- Confirm the Tool node's `capabilities` field (not `workflow.capability_table`) is
  the authoritative, populated source for a tool's required caps at interpret time;
  the decorator holds `Arc<IrModule>` and reads `workflow.tools[id].capabilities`.
- Conformance-fixture placement + the wasm lane that exercises it.
