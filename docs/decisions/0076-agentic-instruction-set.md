# ADR-0076: The agentic instruction set — kernel, taxonomy, extension rules (umbrella)

**Status:** Accepted (records locked decision §10.11 of the
[2026-09-01 consolidated design](../superpowers/specs/2026-09-01-tau-authoring-ops-and-primitives-design.md);
Phase 0 ADR wave)
**Date:** 2026-09-01
**Deciders:** maintainer, via the 2026-09-01 brainstorm session
**Scope note:** this is the **umbrella** ADR for the instruction-set
roadmap: the taxonomy, the target kernel, and the rules by which it may
grow. Individual v2 step kinds (ForEach, Sleep, WaitForEvent/EmitEvent,
Explore, retry/catch, variables) each get their **own ADR when built**;
this ADR governs those ADRs.

## Context

tau's step vocabulary has grown construct-by-construct (ADR-0037,
0058/0059, Suspend, Dynamic regions) without a stated theory of what
belongs in the kernel, what arrives as a library, and what is refused.
Every agent framework that skipped this step accreted primitives until
semantics blurred (the design's §8/§9 survey). The 2026-09-01 design
fixes the taxonomy, audited against the Workflow Patterns Initiative
method (patterns-as-benchmark; van der Aalst et al. 2003, Russell et al.
2016) and the current agent-framework field.

## Decision

1. **Eight construction primitive categories** (each primitive lives in
   exactly ONE category — its nature; cross-category uses are
   *patterns*): **Flow · Compute · Storage · Messaging · Time ·
   Governance · Verification · Observability** (membership per design
   §3.1). Two transverse guarantees are never manipulated, always
   present: **Identity** (hierarchical lineages, content hashes,
   `[[moved]]`) and **Contracts** (versioned, drift-tested schemas).
2. **The kernel = 15 step kinds** + two planes:
   `Agent · Tool · Deterministic · Check · Branch · Parallel · Loop ·
   ForEach · Sleep · WaitForEvent (⊇ Suspend) · EmitEvent · Dynamic ·
   Explore · Compose` (+ `Suspend` retained until absorbed by
   WaitForEvent). The **fault plane** (per-step retry/catch, ASL
   Retry+Catch semantics) is distinct from the existing **quality
   plane** (Check → rewind). Copied semantics are copied, not
   reinvented: ForEach = ASL Distributed Map; WaitForEvent correlation =
   Inngest match / Restate awakeables; variables+reducers = LangGraph
   channels ∘ ASL Assign.
3. **Versioning:** new step kinds are MINOR ir_format bumps (precedent
   v2.4.0); only multi-pipeline is MAJOR (ADR-0073).
4. **Seven engine foundations** beneath the taxonomy: Seal, Lineage,
   Contract, Formula, Gate, Ledger, Ceiling. **Admission rule:** any
   future feature must be expressible as an assembly of these seven;
   needing an eighth is a major architectural event (foundational ADR,
   not a feature).
5. **Kernel closed, vocabulary open.** The kernel extends **by ADR
   only**. User-defined primitives are **primitive packs** (the Bazel
   model — IR = actions, packs = rules): one npm/git package = a typed
   fragment (`defineFragment`, `(scope, id, props)`, namespace-owned) +
   optional Rust crate (tools/fns with declared caps) + replay fixtures
   (packs arrive proven) + docs generated from the props schema.
   Capability transparency is structural: a flow-only pack has no
   powers; a pack's Rust caps are audited at install and bounded by the
   consumer's `[allow]`.
6. **Extension rule per category:** Flow / Messaging / Storage
   substrates / Identity → ADR; Compute fns, Verification predicates,
   context transformers → packs; Governance → versioned capability
   vocabulary (ADR-0036); Time / Observability → adapters / additive
   schemas.
7. **The rejections ledger is binding** (design §8): templating in TOML,
   runtime JS, runtime self-modifying workflows, ToT/LATS/MCTS
   primitives, emergent group chat, BPMN compensation as primitive,
   streaming between steps, continue-as-new, handoffs,
   hand-maintained per-language SDKs, rebuild-per-environment,
   tau-operated registry, evolutionary-search primitive. Reopening any
   requires a superseding ADR with new evidence.
8. **Residual open items settled here:**
   - **`PARALLEL_CAP` configurability shape:** the cap stays an
     engine-level ceiling (a Ceiling foundation), configurable per-host
     via the embed/engine configuration — never per-IR. A pipeline's
     declared `max_concurrency` requests concurrency *below* the host
     cap; the effective value is `min(declared, host cap)` and is
     reported in run events. No IR field can raise a host limit.
   - **Exit handlers vs. saga compensation:** the working position is
     saga = per-step catch + explicit undo steps (no compensation
     primitive). **Verification obligation:** the ADR that lands
     per-step retry/catch + `on_exit` (v2) MUST include the subsumption
     analysis; if exit handlers do not subsume the compensation
     patterns, the answer is still composition (a stdlib saga pack),
     never a BPMN-style primitive.
   - **Novelty-claim gate:** before any *published* novelty claim from
     design §9, the full texts of AgentSPEX (arXiv:2604.13346) and
     POLARIS (arXiv:2601.11816) must be reviewed and the claims
     calibrated against them. Until then, §9's claims are internal.

## Consequences

- Feature requests get a deterministic triage: which category → which
  extension rule → ADR, pack, adapter, or refusal.
- Explore is pre-scoped (design §3.2, Option B, evidence-locked:
  mandatory budget with `synthesis_reserve`, judged-deliverable exit,
  depth-1 spawns on Dynamic machinery, the compiled box on the card) —
  its build-time ADR inherits that scope rather than re-opening it.
- The repairs lot (design §3.4) is acknowledged as v1 debt: silent
  promises (`AgentBudget.max_tokens`, `judge_model`, `output_schema`,
  `any-wasi-strict` empty feature set, missing pipeline RunEvents,
  subflow args, goals `OnFail::Abort`, scalar coercion,
  capability-order-sensitive hashes, decorative `max_concurrency`) are
  scheduled in E-2/E-3/E-4 — an accepted-but-unwired config key is a
  defect, not a roadmap item.
- Proof-of-sufficiency obligation: the **best-of-N** pack
  (`ForEach(1..n) → judge-reduce`) ships with packs to demonstrate the
  vocabulary-open claim; named communication packs (rpc, broadcast,
  inbox) follow the same route.

## Alternatives considered

- **Open kernel (user-defined step kinds).** Rejected: step-kind
  semantics are what conformance, replay, and the capability card
  reason about; user kinds would make every guarantee pack-dependent.
- **Closed vocabulary (no packs).** Rejected: every reusable pattern
  would queue on the kernel ADR process; the Bazel actions/rules split
  is the proven middle.
- **Adopting a tree-search primitive (ToT/LATS/MCTS).** Rejected on
  evidence (judge noise compounds, no production wins) — recorded in
  the ledger with the Explore alternative.
- **No taxonomy (grow ad hoc).** Rejected: it is how comparable systems
  drifted; the WPI benchmark method is the prior art for doing better.

## References

- Design: [`2026-09-01-tau-authoring-ops-and-primitives-design.md`](../superpowers/specs/2026-09-01-tau-authoring-ops-and-primitives-design.md) §3 (taxonomy, kinds, packs, repairs), §8, §9
- Literature: van der Aalst et al. 2003; Russell et al., MIT Press 2016;
  Burckhardt et al. 2021; Zheng et al. NeurIPS 2023; Gu et al. 2024;
  Anthropic agent patterns 2024; AgentSPEX / POLARIS 2026 (gate above)
- Related: ADR-0036, ADR-0058/0059, ADR-0073, ADR-0074
- Epics: E-2/E-3/E-4 repairs; v2 backlog in
  [`vision-roadmap.md`](../superpowers/plans/vision-roadmap.md)
