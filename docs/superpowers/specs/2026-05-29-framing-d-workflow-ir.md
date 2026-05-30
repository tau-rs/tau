# Framing D — declarative workflow IR

**Status:** Scoping document. Not a spec. Must land before any engine
implementation begins.

**Date:** 2026-05-29.

**Relates to:** [`docs/explanation/tau-philosophy.md`](../../explanation/tau-philosophy.md)
acknowledged risk D.

---

## Why this needs framing first

tau is a compiler. The IR is the source of truth that both surfaces (TOML
manifest, eventual TS sugar) emit and that `tau build` lowers. **If the IR is
wrong, every surface is wrong and every target is wrong.** No engine work
can begin until the IR's shape is settled enough to commit.

Industry signal: no prior art lowers a declarative agent/workflow IR to a
portable wasm component. LangGraph compiles to in-memory graphs; Cloudflare
Workflows persists JS step state; n8n is JSON-interpreted at runtime. The
closest *shape* precedent is Spin's `spin build` (TOML → component graph),
but Spin compiles HTTP handlers, not agent loops.

This is the project's foundational research bet. Framing reduces — does not
eliminate — the unknowns.

---

## Decisions this framing must reach

The framing is complete when the following are decided and recorded as ADRs
or in the IR design spec. Each is enumerated; none should be deferred.

### D-1. Node taxonomy

What are the IR's primitive nodes?

The four candidates: **Agent** (LLM loop with tools), **Tool call**
(native or MCP), **Deterministic step** (pure function, no LLM), **Subflow
edge** (compose flows). Workflow automation (n8n / Temporal class) needs at
minimum Deterministic and Subflow. The agentic case needs Agent and Tool.

**Open:** Is the IR node-typed (heterogeneous nodes with type tags) or
uniform (all nodes are "step", differentiated by their handler)? Uniform is
simpler to lower; typed is easier to optimize and validate.

### D-2. Message and event shape

The IR's wire format for messages flowing between nodes. Reuse the existing
`tau_ports::llm::Message` shape, or a thinner IR-internal type?

The pull toward reuse: the harness already uses it; serialization is solved.
The pull toward a thinner type: the IR must round-trip through bytes (wasm
component boundary, persistence, replay), and `Message` carries provider
specifics that may not lower cleanly.

### D-3. Capability lowering

Capabilities are declared per tool. When the IR lowers to a wasm component,
how are capabilities expressed? Three options:

1. WASI capability grants on the component's import surface (the most native
   shape; map fs/net/exec to WASI handles the host hands in).
2. A `tau-capabilities` interface defined in WIT, intercepted by the host.
3. Static capability metadata embedded in the component's custom section,
   enforced by the host runtime, not at the component boundary.

**Recommendation to evaluate:** (1) + (3) — WASI grants for what WASI models
directly, custom-section metadata for what WASI doesn't (exec gating, e.g.).
Decision must land before E (tree-shake) and before the wasm artifact rule.

### D-4. Composition: one component or many

Does an IR compile to a single monolithic wasm component, or to a
component graph (one per agent, one per tool, composed via WIT)?

Trade: one component is simpler to ship and tree-shake; many components
allow finer-grained capability scoping (each tool component gets only its
capability handle). The wasm component model exists *because* composition
is the right answer for capability isolation — but in the embedded /
size-constrained case, one component is cheaper.

**Open:** is the answer "one per target tier" (one monolith for embedded,
component graph for server)? That doubles design surface.

### D-5. Lowering strategy: AOT vs partial-interpret

Does `tau build` emit a wasm component that contains the harness's actual
loop logic, or a component that contains a small interpreter + the IR as
data?

The first is the genuine compiler answer (lighter artifact, no runtime
interpretation cost). The second is dramatically simpler (the IR is a config
the embedded interpreter reads), and is what nearly every existing system
does (LangGraph, n8n, Cloudflare Workflows). The size and behavior
differences must be quantified before choosing.

### D-6. Determinism contract

`tau verify --bundle` (already shipped) demands byte-identical rebuilds.
The IR lowering must be deterministic: same source, same target, same
artifact bytes. Determine which inputs are part of the hash (skill versions,
MCP contract hashes, prompt text, capability set) and which are not
(timestamps, comments, formatting).

### D-7. Cross-target conformance

The C3 discipline requires that dev (interpreted) and release (compiled)
agree on behavior. The IR must be the *only* source for both — no
shell-specific divergence. Define the minimum conformance suite: which IR
node types, in which configurations, must produce identical observable
behavior across both profiles. This is the gate that proves the philosophy
is honored.

---

## Out of scope for framing

These are explicit non-decisions: framing must not pre-empt them.

- The runtime semantics of `agent.<kind>.spawn` (multi-agent v2 work).
- Workflow durability / persistence beyond what's needed for replay.
- DX details of the eventual TS sugar layer (lands after IR is committed).
- The MCP facilitator's wire-level adapter choices.

---

## Deliverable shape

The framing is complete when:

1. A design spec lands at `docs/superpowers/specs/<date>-workflow-ir-design.md`
   answering D-1 through D-7 with chosen options and brief reasoning.
2. A consequent ADR records the IR commitment (`0035-workflow-ir.md` or
   next).
3. A minimal IR example is included in the design — one agent + one
   native tool + one MCP contract + one context pipeline — so the
   committed shape is concretely visible.

Until that exists, the engine sub-project does not start.

---

## Risk acknowledgment

This framing reduces unknowns; it does not eliminate them. The first real IR
implementation will surface lowering issues that no amount of upfront design
can predict (Spin's evolution from 1.x → 2.x → 3.x demonstrates this for the
nearest-shape precedent). The mitigation is: ship a *minimal* IR first
(D-1: only Agent and Tool node types; D-4: one component; D-5: AOT) and
extend deliberately. Resist the temptation to land a "complete" IR before
running any real workflow through it.
