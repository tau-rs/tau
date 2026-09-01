# ADR-0077: Agent-exposure surfaces — skill/AGENTS.md emitters, MCP facade plan, agent-grade CLI

**Status:** Accepted (records locked decisions §10.9/§10.12/§10.13 of the
[2026-09-01 consolidated design](../superpowers/specs/2026-09-01-tau-authoring-ops-and-primitives-design.md);
Phase 0 ADR wave)
**Date:** 2026-09-01
**Deciders:** maintainer, via the 2026-09-01 brainstorm session
**Amends:** CONSTITUTION G6 + QG12 (public-surface wording; see
Consequences), `GUIDELINES_CHEATSHEET.md` accordingly

## Context

The design's integration principle: **no consumer ever needs a tau
library** (design §6). Two consumer classes need first-class treatment
now: *other agents* (harnesses that should discover and drive a tau
project as a tool) and *programs* (shell/CI today, backend services and
code-mode harnesses later). Meanwhile G6/QG12 still name "the
`tau-runtime` crate + serve-mode IPC" as the two public surfaces —
wording ADR-0055/0056 already reframed (the two contracts) and the
design's §10.13 vision reframe finishes: "the tau SDK" is one
language-plural product generated from schemas, consumer-first in
non-flagship languages.

## Decision

**v1 exposure surfaces (E-3):**

1. **Generated SKILL.md** — `tau export --skill` emits a skill package
   (AAIF open standard; ~40 harnesses consume it) describing how to
   drive this project's pipelines via the CLI, generated from the IR +
   capability card, never hand-written.
2. **AGENTS.md emitter** — same generation, agent-harness convention.
3. **Agent-grade CLI contract:** `--help` output ≤ 1,500 tokens,
   deterministic exit codes, frozen NDJSON stdout (needs the pipeline
   RunEvents repair, design §3.4) — the CLI is itself an agent-usable
   tool and is tested against that budget.
4. **Official authoring skill + `tau new` scaffolder** — agents author
   workflows safely by construction (choreography, never the
   constitution — ADR-0071), gated in CI by `tau plan --check` exit 3
   (ADR-0075).
5. **OTLP span mapping documented as a contract** (journal-derived,
   ADR-0074) + the plan JSON twin (ADR-0075) for observability/policy
   consumers.

**v2 (planned here, built later — backlog only):**

6. **MCP facade** — `tau serve --mcp`: one pipeline = one typed MCP
   tool; the capability card travels in `_meta`; 2026-07-28 stateless
   spec; tasks extension; OAuth for remote. Also the ChatGPT/MCP-Apps
   door. Cross-org: MCP both ways, double-bounded (`[allow.mcp]` hosts
   out, bundle card in, `tau mcp pin` freezing the contract).
7. **serve v2** — Unix socket, warm daemon, concurrent JSON-RPC,
   `session.*` verbs + reverse dispatch (host tools / approvals,
   MCP-elicitation-shaped). Host tools are declared in `[allow]`,
   schema-validated, card-labeled `host-enforced`; session MCP servers
   = late binding under a declared ceiling (§10.12).
8. **Typed generated clients** — `tau export --client ts|py` (v2.5),
   generated over CLI/serve from schemas, never hand-maintained.

**The SDK vision reframe (§10.13):** "the tau SDK" = **one
language-plural product** — authoring bindings (flagship TS, via the
synth contract) + engine/consumer API — **generated from the published
schemas** (`schemas/ir/`, `schemas/project-manifest/`, `schemas/plan/`,
run-event, journal). Non-flagship languages are **consumer-first**
(drive artifacts before authoring them). CONSTITUTION G6/QG12 are
amended to name the schema-defined contracts — not one crate + one IPC
protocol — as the public surface set.

## Consequences

- A tau project becomes discoverable and drivable by any agent harness
  with zero tau-specific code on the consumer side; the emitters are
  generated, so they cannot drift from the artifact.
- G6/QG12 amendment (applied with this wave, 24h-window process per
  `docs/decisions/README.md`): public surfaces = the versioned contracts
  — authoring contract (ProjectConfig + synth JSON, ADR-0072), artifact
  contract (IR + WIT, ADR-0055/0056), and the operational interchange
  schemas (plan, run events, journal). The `tau-runtime` crate API and
  serve-mode IPC remain versioned surfaces *within* that set, not the
  definition of it.
- Obligations: emitter drift tests (committed export == fresh emit, the
  `embed_js_drift` precedent); a CLI help-budget test; NDJSON contract
  freeze rides the RunEvents repair (E-3); MCP facade and serve v2 get
  their own ADRs when built (per ADR-0076's process rule).
- Rejected surfaces recorded: A2A as invocation protocol (wrong
  abstraction, thin production reality; card projection possible later),
  OpenAPI facade, jsii-style runtime embedding. Watch item: Wassette
  (wasm components as MCP tools) — positioned, not actionable.

## Alternatives considered

- **Hand-written per-harness integration docs.** Rejected: N harnesses
  × M projects of drift; generation from the IR is the only scale-free
  path, and the capability card must never be hand-transcribed.
- **MCP facade in v1.** Rejected for sequencing: the facade needs
  multi-pipeline (ADR-0073), typed pipeline IO, and the stateless serve
  substrate; v1 ships the CLI + skill surface that agents can use today.
- **Hand-maintained Python/TS client SDKs.** Rejected (CDKTF scar):
  every hand-maintained SDK lags its source; clients are generated from
  schemas or not shipped.
- **Keeping G6/QG12 as-is.** Rejected: the constitution would name a
  crate as the public surface while the artifact contract (ADR-0056)
  already governs stability; a constitution that contradicts the ADR
  layer trains readers to ignore one of them.

## References

- Design: [`2026-09-01-tau-authoring-ops-and-primitives-design.md`](../superpowers/specs/2026-09-01-tau-authoring-ops-and-primitives-design.md) §6 (integration surfaces), §7, §10.9/12/13
- Related: ADR-0033 (serve v1), ADR-0038/0054 (MCP), ADR-0055/0056,
  ADR-0071/0072/0074/0075
- Epics: E-3 (emitters, CLI contract); v2 backlog in
  [`vision-roadmap.md`](../superpowers/plans/vision-roadmap.md)
