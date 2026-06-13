# Framing: data-edges + Map/Branch/Source/Sink in the tau IR?

**Status:** Framing / scope decision. No implementation. Informs future ADRs.
**Date:** 2026-06-13.
**Audience:** ADR authors, contributors deciding what belongs in `tau-ir`,
anyone evaluating tau against dataflow/agent-graph engines.

---

## The question

Should tau's IR gain **data-edges** (typed `output-of-X → input-of-Y` wiring)
plus **`Map` / `Branch` / `Source` / `Sink`** nodes so that LLM-enrichment
pipelines (e.g. *read CSV → LLM-classify each row → route → write*) can be
expressed natively — or is that out of scope under
[ROADMAP NG5](../../../ROADMAP.md) ("tau is not a general-purpose workflow
engine")?

This came up as a **positioning question against the dataflow-engine field**
(LangGraph, n8n, Dagster), not a blocked use case.

---

## Recommendation (TL;DR)

**Abstain now; name the one exception worth revisiting later** ("verdict B").

- **`data-edges`, `Branch`, `Source`, `Sink` → out of core, indefinitely.**
  They turn tau into a second kind of engine (graph-topology-driven) on top of
  its agent-loop engine, which is exactly the breadth NG5 disclaims. Each is
  already covered by an existing primitive (see the per-node table).
- **`Map` / fan-out → out of core *now*, but it is the single primitive that
  could ever earn a place** — and only via its own scoping doc, motivated by a
  *real* downstream consumer, never speculatively and never as part of a
  four-node quartet.
- **The enrichment use case still ships — beside tau, not inside it.** It is
  built as a **separate downstream project** ("Layer B"), the way Helm sits
  beside Kubernetes. It needs **no new IR**, and it can start **today**.

The wedge tau is defending: a *minimal, capability-safe, portable* core that
others extend by lowering *down to* it — not a kitchen-sink engine.

---

## Background: what the IR is today

`tau-ir` (β.2, [ADR-0037](../../decisions/0037-workflow-ir.md)) has exactly four
node kinds and **one** edge kind:

| Element | Variants | Drives execution? |
|---|---|---|
| `Node::Agent` | LLM loop + tool refs + context + budget | **yes** — the LLM decides which tool to call, with what args, in what order |
| `Node::Tool` | `ToolImpl::{Native, Mcp, Subflow, Step}` | leaf, called by an agent |
| `Node::Deterministic` | pure Rust fn, typed in/out schemas | leaf, pure, no LLM, no I/O |
| `Node::Subflow` | `SubflowEdge::{Spawn, Compose}` | static composition edge |

The defining trait: **execution is agent-loop-driven.** The LLM is the
orchestrator. Data flows implicitly through tool-call arguments and the
conversation history. **There are no typed data-edges** wiring one node's
output to another node's input. There is no `map`/`branch` combinator.

`Map/Branch/Source/Sink` + data-edges would introduce a **second execution
driver**: one where graph *topology* drives execution deterministically and
LLM calls become leaf nodes inside it. That is a different engine.

---

## Industry survey

Three reference points frame the decision.

**LangGraph — the competitor that bakes it in.** `StateGraph` ships
`Send` (dynamic fan-out / map-reduce) and conditional edges *as core
primitives*. The engine **is** the dataflow graph. This is the model tau is
deliberately declining: it is powerful, but it makes the core the orchestrator,
which is the NG5 line.
([Control-flow primitives](https://deepwiki.com/langchain-ai/langgraph/3.5-control-flow-primitives),
[Map-reduce with Send](https://machinelearningplus.com/gen-ai/langgraph-map-reduce-parallel-execution/))

**MLIR — the compiler-world answer: extend by lowering, not by growing the
core.** MLIR's principle is *"little builtin, everything customizable."*
Domain abstractions live in **dialects** that **lower progressively** to lower
dialects; the core IR never swells.
([MLIR scaling paper](https://rcs.uwaterloo.ca/~ali/cs842-s23/papers/mlir.pdf))
A "dataflow dialect" lowers to base ops — the high-level concept exists *above*
the core, not *in* it. This is the model tau should follow.

**Kubernetes — two complementary extension layers.** K8s stays a minimal API
and is extended by:
- **Helm** — packaging/templating that **renders down to standard manifests**
  (compile-time; adds no API surface), and
- **Operators + CRDs** — new resource types + reconciling controllers
  (runtime).

They are not mutually exclusive.
([Operators vs Helm](https://www.groundcover.com/blog/kubernetes-operator-vs-helm))

The synthesis across all three: **keep the core small; let supplements compile
*down to* it (Helm / MLIR dialects) or drive it from outside (Operators).**
That is precisely tau's posture and the answer to the dataflow question.

---

## Per-node verdict

| Node | Ruling | Why |
|---|---|---|
| **data-edges** (typed output→input) | **OUT of core** | Introduces a second, topology-driven execution model. Doubles the dev/build/wasm conformance surface. This is the LangGraph model NG5 disclaims. |
| **Branch** | **OUT of core, indefinitely** | Routing is already covered twice — the LLM does it (agent loop), or a frontend lowers it to existing `SubflowEdge`s. No new primitive earns its keep. |
| **Source / Sink** | **OUT of core, indefinitely** | These *are* `Tool`s with capabilities (read-CSV = a native/MCP tool). Adding them as node types duplicates the `Tool` + capability-gate model tau already has. |
| **Map / fan-out** | **OUT now — the one exception worth revisiting** | The only primitive that cannot be cleanly expressed by lowering to today's IR, and the only one whose justification could ever be *capability-safe portable* deterministic fan-out. Gets its own scoping doc *iff* a real consumer proves it. |

---

## The architecture: where the dataflow layer lives

Authoring a pipeline `Source → Map(LLM) → Branch → Sink` has two viable homes,
mirroring K8s's two extension layers. **tau core changes in neither.**

```
   AUTHOR a dataflow pipeline (Source → Map(LLM) → Branch → Sink)
                              │
        ┌─────────────────────┴─────────────────────┐
        ▼                                            ▼
  LAYER A — FRONTEND ("tau's Helm")          LAYER B — ORCHESTRATOR ("tau's Operator")
  compile-time, tau-pure                     runtime, host-driven, full breadth
  ─────────────────────────────             ─────────────────────────────────────
  dataflow DSL  ──lowers to──▶ tau IR        host program (Rust/TS) holds the graph
    Source/Sink → Tool nodes (caps)          fans out in host code; per item calls
    Branch      → SubflowEdge / agent loop   a tau-compiled agent (the portable leaf)
    Map         → agent loop  (or core Map*) Source/Sink = tau Tools
    LLM step    → Agent node                 orchestration is NOT a portable artifact;
  `tau build` → ONE portable bundle          only the leaf agents are
        │                                            │
        ▼                                            ▼
  gets cap-gate + portability FREE           unlimited dataflow (joins, retries,
  bounded by what the IR can express         backpressure) w/o touching tau core

  * the single core Map primitive (verdict B) — petitioned only if Layer A
    hits the deterministic-fan-out wall
```

**Home, stated plainly:** a **new git-distributed project** (or a module of the
already-planned `stature` project). **Not** a tau-core change, **not** a tau
plugin (plugins are being retired for MCP), **not** an MCP server (MCP contracts
*external tools*; it is the wrong layer for orchestration topology). It sits
adjacent to tau the way Helm sits beside Kubernetes.

---

## Layer B, concretely (the recommended starting point)

Layer B is a plain program in its own repo. It owns the boring orchestration;
tau owns only the per-item LLM work. Illustrative shape (not final code):

```
   LAYER B PROGRAM  (new repo / stature)
   ─────────────────────────────────────────────
   rows = read_csv("input.csv")              ← Source  (plain code)
   for row in rows:                          ← Map     (a loop)
       result = run_tau_agent("classifier", row)   ← the ONLY tau touchpoint
       if result.label == "urgent":          ← Branch  (an if)
           urgent.append(result)
       else:
           normal.append(result)
   write_csv("urgent.csv", urgent)           ← Sink    (plain code)
   write_csv("normal.csv", normal)           ← Sink
                       │  once per row
                       ▼
   ┌──────────────────────────────────────┐
   │ tau agent bundle (built once)         │   `tau build`
   │   prompt: "classify this support row" │   portable + capability-gated
   │   model:  claude-haiku-4-5            │   THIS is the tau part
   └──────────────────────────────────────┘
```

`Source / Map / Branch / Sink` are **all ordinary code**. The only "tau" object
is the classifier **agent bundle** — a portable, sandboxed *unit of LLM work*.

### How Layer B interacts with tau

| | **Option 1: subprocess (CLI)** | **Option 2: library (in-process)** |
|---|---|---|
| How | shell out to `tau run --bundle classifier.taub` per row | depend on the tau runtime crate; call the run API directly |
| Pro | language-agnostic host, process isolation, simplest | no per-row spawn, faster at volume |
| Con | spawn cost per row (batch/pool to mitigate) | host must be Rust; couples to tau's internal API |
| Verdict | **start here** | only if spawn cost actually hurts |

### The one real integration gap

Today `tau run --bundle` runs the agent **from its prompt** — there is no
per-invocation `--input` flag, and the IR has a known v0 limitation that input
args are not forwarded into a spawned agent
([`tool_impl.rs`](../../../crates/tau-ir/src/tool_impl.rs) `ToolImpl::Subflow`).
So *"hand this specific row to the agent as input"* is the single contract
Layer B must pin down. Three options, easiest first:

1. **Template the row into the prompt** at call time (host builds the per-row
   prompt). **Works today, zero tau change. Start here.**
2. **Add a small `tau run --input <json>` (or stdin) seed** for the agent's
   first user message. A *small, in-character* core addition — it makes the
   existing "run one agent" verb composable; it does **not** make tau a workflow
   engine. This is the *only* tau-side change Layer B is likely to want.
3. Full structured input plumbing through the IR — defer to the β.7
   arg-forwarding work.

**Layer B is not blocked on tau.** Option 1 ships now; #2 is optional polish
driven by real friction.

---

## Determinism (requirement: A + B)

The pipeline's determinism splits in two; the target is **A + B**.

- **(A) Deterministic orchestration** — loop/ordering/routing is reproducible.
  Layer B is plain ordered code; the IR's existing `Deterministic` node covers
  any rule-based (non-LLM) enrichment. **Fully supported, no tau change.**
- **(B) Deterministic under replay** — a full run is bit-exact when LLM outputs
  are pinned. Supported by tau's **record/replay** (cassette transport) plus
  **reproducible builds** (`tau verify --bundle` rebuilds byte-identical). Layer
  B opts into recorded LLM outputs for tests, audits, and reruns.
- **(C) Fully deterministic *live* LLM output** is **not achievable** and not
  tau's to give — inference is delegated (NG1); models are not bit-reproducible
  even at temperature 0. The honest best is temperature 0 + pinned model + (B).

Determinism *reinforces* this framing rather than conflicting with it: the clean
design **isolates the LLM as the single source of entropy at the leaf** and
keeps everything around it pure. That is the Layer B shape exactly — and it is
an argument *against* the LangGraph "mix dataflow and LLM in one graph" model,
where non-determinism smears across the whole graph.

---

## The `Map`-only future exception (proposed shape, NOT to build)

Recorded so a future scoping doc starts from a concrete sketch — **not an
approval to implement.** If, and only if, Layer A hits a wall where it needs
*portable, deterministic, in-artifact* parallel fan-out (not host-driven), the
proposal would be a single node in the `Deterministic` family:

```text
# PROPOSED — NOT YET. One node, deterministic semantics, no general graph.
Node::Map {
    id:            StepId,
    over:          <collection-valued input>,   # the items to fan out over
    body:          AgentId | StepId,             # applied to each item
    cap_subset:    CapabilityRequirements,       # ⊆ parent (subset rule, as Spawn)
    ordering:      Ordered,                       # results in input order — determinism (A)
    concurrency:   bounded,                       # for reproducibility, not a scheduler
}
```

Guardrails that keep it from becoming a dataflow engine:

- It is **one** node, not a quartet. No `Branch`, no `Source`/`Sink`, no
  free-form data-edges accompany it.
- `body` reuses an existing `Agent`/`Deterministic` — it adds fan-out, not a new
  execution model.
- `ordering: Ordered` is mandatory: results are returned in input order, so the
  node is deterministic under (A).
- It must clear the **cross-target conformance gate** (dev ≡ build ≡ wasm)
  before shipping, like every other node.
- It ships only with a **named downstream consumer** (Layer A) that proves the
  host-driven `Map` is genuinely insufficient.

If those cannot be met, `Map` stays out too, and Layer B's host loop remains the
answer indefinitely.

---

## Execution order

| Step | What | Where | Blocked on tau? |
|---|---|---|---|
| 1 | This framing doc | **tau repo** (`docs/superpowers/specs/`) | — |
| 2 | Build **Layer B** with prompt-templating (option 1 + gap-fix #1) | **new repo / `stature`** | **No — start today** |
| 3 | File a tracking **GitHub issue on the tau repo** for Layer A (the authoring DSL) — filed as [tau-rs/tau#333](https://github.com/tau-rs/tau/issues/333) | **tau repo issue** | No |
| 4 | *Optional polish:* `tau run --input` seed (gap-fix #2) | **tau repo** | No — build on real friction |
| 5 | Build **Layer A** (DSL → IR) | **new repo / `stature`** | No — only after Layer B proves the shape |
| 6 | *Only if cornered:* core `Map` scoping doc + impl | **tau repo** | The one possible future core change |

The Layer A tracker is a **plain tau GitHub issue** — unrelated to the tau web
UI project.

---

## NG5 reconciliation

This decision *sharpens* NG5, it does not relax it. tau remains "not a
general-purpose workflow engine": it executes capability-safe portable workflow
IR, and it declines the topology-driven dataflow breadth that LangGraph / n8n /
Dagster occupy. Dataflow pipelines are served **beside** tau (Layer B/A,
lowering down to or driving the core), exactly as Helm and MLIR dialects extend
their cores without growing them. The only conceivable core growth is a single
deterministic `Map`, gated on proof — and even that stays inside the
"capability-safe portable IR" identity rather than expanding it.

---

## Open items

- Layer A tracker filed as a plain tau issue: [tau-rs/tau#333](https://github.com/tau-rs/tau/issues/333).
- Decide Layer B's host language (Rust gets option 2; any language gets
  option 1).
- Whether Layer B lives in its own repo or as a `stature` module — defer to the
  Layer B brainstorm.
```
