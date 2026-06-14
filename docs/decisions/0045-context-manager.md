# ADR-0045: Context-manager primitive (β.4)

**Status:** Accepted
**Date:** 2026-06-14
**Supersedes:** none

## Context

The runtime currently sends the **entire conversation history on every inference
turn** — no truncation, compaction, or budgeting. This produces three failure
modes at scale: hitting the context window (hard failure), paying for re-sent
stale tool dumps every turn (cost scales with tokens), and "context rot" as the
window fills. The ROADMAP β.4 milestone and `docs/explanation/tau-philosophy.md`
call for an opt-in, declarative **context manager** that is backward-compatible
by absence, lowered to IR, and portable into a wasm bundle.

β.4 is also a prerequisite for the β.6 canonical `fan-monitor` conformance
scenario (which declares a context block) and for the β.8 `contextManager()` TS
factory (currently parse-rejected pending β.4).

tau's standing principle — *any check that could run at build time must run at
build time* — applies here: unknown transformer names, a missing `fit_budget`
terminus, duplicate steps, unresolved custom-node references, and ungranted
capability declarations are all rejected at `tau build` / `tau check`, not
discovered at runtime.

Full design detail is in
`docs/superpowers/specs/2026-06-14-beta-4-context-manager-design.md`.

**ADR numbering note:** two `0044-*.md` files currently collide
(`0044-trigger-ingress-slice-1.md` and `0044-deliverables-and-goals.md`). This
ADR claims 0045 regardless; the collision is a separate cleanup.

## Decisions

### Decision 1 — Layered hybrid architecture

v1 ships a deterministic **context pipeline** (Layer 1) as the conformance-gated
core. Agent-driven memory tools (β.4.4) and retrieval via contracted MCP (γ.6)
are additive layers above it.

```
AGENT — every inference turn
  LAYER 1 — declarative context pipeline (CORE)     ← β.4 v1
    trim_old → compact_tool_outputs → fit_budget
    pure, runtime-applied, author-declared
    ★ THE CONFORMANCE-GATED PART (β.6 bit-identical)

    later transformers slot in here via the same trait:
      offload_tool_outputs  (β.4.2, pure → fs)
      summarize_oldest      (β.4.3, LLM, cassette-replayed)
      recite_goal           (β.4.4, pure)

  LAYER 2 — agent-driven memory TOOLS (capability)  ← β.4.4
  LAYER 3 — retrieval via contracted MCP             ← γ.6 (NG6)
```

The SOTA→tier roadmap, anchored to industry/research sources:

| Technique | Source | Determinism | tau tier |
|---|---|---|---|
| Truncation / sliding window | ubiquitous | pure | `trim_old` — v1 |
| Tool-result clearing | Anthropic ("safest lightest compaction") | pure | `compact_tool_outputs` — v1 |
| Token-budget fitting | ubiquitous | pure | `fit_budget` — v1 |
| Restorable externalization | Manus; Anthropic "just-in-time" | pure + fs | β.4.2 |
| LLM compaction / summary | Anthropic; LangChain; Mem0 | LLM-backed | β.4.3 |
| Structured memory tools | Anthropic memory tool; Manus `todo.md` | agent-driven | β.4.4 |
| Retrieval / vector memory | MemGPT; RAG | external store | γ.6 (NG6) |
| Hierarchical paging | MemGPT / Letta | agent + MCP | apex tier |

Sources: Anthropic — Effective context engineering for AI agents; Manus —
Context Engineering for AI Agents; MemGPT: LLMs as Operating Systems
(arXiv:2310.08560); CoALA (arXiv:2309.02427).

### Decision 2 — `DeterminismClass` is the conformance boundary, enforced structurally

The `ContextTransformer` trait declares `fn determinism() -> DeterminismClass`
(`Pure | LlmBacked | Stateful`). `TransformCx` gates what a transformer can
*access* based on its declared class:

| Class | `TransformCx` exposes | β.6 conformance |
|---|---|---|
| `Pure` | `estimate_tokens()` + config | gated (bit-identical) |
| `LlmBacked` | + `llm: &dyn LlmBackend` | gated via cassette replay (β.4.3) |
| `Stateful` | + `store: &mut dyn MemoryStore` | excluded (Layer 2) |

A `Pure` transformer has *no handle* to a model — the type system prevents it
from calling one. This is structural enforcement, not documentation. v1 ships
three `Pure` transformers: `trim_old`, `compact_tool_outputs`, `fit_budget`.

### Decision 3 — E1 heuristic token estimator behind a swappable port

`fit_budget` needs per-message token counts; the backend only provides usage
*after* a call. v1 ships `ceil(serialized_bytes / 4)` plus per-role/structural
overhead as the `TokenEstimator` implementation:

```rust
trait TokenEstimator { fn estimate(&self, msg: &Message) -> u32; }
// v1: ceil(bytes/4) + per-role overhead
```

This is pure arithmetic — deterministic, portable (works unchanged in wasm),
and requires no vocabulary blob. `fit_budget` enforces a *ceiling*, so a
conservative over-estimate is safe: at worst it drops one extra old turn. A
real per-model tokenizer or a backend-provided count can replace this later
behind the same port with no change to the transformer contract.

### Decision 4 — Public extension contract: open registry, native-first

`ContextTransformer` is a **public, stable SDK surface**. The registry is
**open** (not a closed enum). A `kind="custom"` IR node is reserved in v1.

v1 ships the **native lane** (Lane 3): users implement `ContextTransformer` in
Rust and register via the host static builder. Two further lanes are reserved
but deferred:

- **Wasm component lane** — a WIT `context-transformer` interface; rides
  β.7.5/γ.1. WIT sketch:

  ```wit
  interface context-transformer {
      record message { /* mirrors tau-domain Message shape */ }
      enum determinism { pure, llm-backed, stateful }
      transform: func(msgs: list<message>, config: string)
          -> result<list<message>, string>;
  }
  ```

- **MCP lane** — stateful/external nodes contracted over MCP; β.4.4+.

Native custom nodes run in-process; capability enforcement is build-time
(declared vs. granted). The wasm lane is the recommended path for third-party
nodes once it lands (sandboxed by construction).

### Decision 5 — Five locked contracts

Five cross-cutting contracts are locked in v1 so later tiers slot in without
an IR or trait break:

| # | Contract | Why it must be in v1 |
|---|---|---|
| 1 | `DeterminismClass` on the trait | conformance boundary + `cx` scoping (Decision 2) |
| 2 | Per-transformer capability declaration (`required_capabilities()`) | safety for all nodes including custom |
| 3 | Restorable-handle convention | β.4.2 offload + γ.6 retrieve share a stable handle shape |
| 4 | KV-cache / append-only invariant | cheap now, expensive to retrofit; prefix-busting is declared metadata |
| 5 | `ContextTransformer` = public SDK + open registry + `kind="custom"` IR node | user nodes slot in with no IR or trait break |

Contract 3 detail: v1's `compact_tool_outputs` uses a **non-restorable**
truncation marker; the handle shape for restorable externalization is defined
but the fs-write path is deferred to β.4.2.

Contract 4 detail: v1's three transformers are prefix-stable except
`fit_budget` when it must drop from the head — a declared, unavoidable cache
cost.

### Decision 6 — Transformers run on a copy; stored history is never mutated

The per-turn pipeline transforms `msgs: Vec<Message>` — a copy assembled for
the current turn. The result (budgeted view) is what gets projected to the LLM
via `agent_messages_to_provider_messages()`. Stored conversation history is
untouched. This means re-running the same turn against the same history always
produces the same budgeted view (deterministic).

### Decision 7 — Context-pipeline failures are kernel errors, not agent failures

A `ContextError` (e.g. `BudgetUnsatisfiable` — the system prompt + live turn
alone exceed `max_tokens`) routes through `RuntimeError::ContextPipeline`, not
through `RunOutcome::Failed`. Per ADR-0006's error/failure dichotomy: an error
is a kernel-level abort; a failure is an agent-level outcome the orchestrator
can handle. A budget that cannot be satisfied is structural — retrying the
agent loop cannot fix it.

### Decision 8 — Build-time IR typecheck enforces `fit_budget` terminus

The IR typecheck rejects any context pipeline that does not end with
`fit_budget`, references an unknown builtin, or contains a duplicate step. This
is the "any check that could run at build time must run at build time"
discipline applied to the context surface.

**Beneficial behavior change:** implementing this check surfaced a latent defect
— `tau build` previously swallowed all `IrError`s from lowering (warn + exit 0)
and exited 0. It now exits 2 on any lowering error, consistent with tau's
enforcement discipline. This is a breaking change for pipelines with lowering
errors that were silently passing `tau build`; those pipelines were already
broken at runtime.

### Decision 9 — Deferred scope

The following are out of scope for v1 and banked as sequenced future work:

- `offload_tool_outputs` — restorable fs externalization (β.4.2, pure, needs
  `fs.write` capability).
- `summarize_oldest` / `compact` — LLM-backed transformers (β.4.3, requires
  cassette-replayed determinism for conformance).
- Memory-tool layer + `recite_goal` (β.4.4, agent-driven, Layer 2 begins).
- Retrieval via contracted vector-store MCP (γ.6, NG6 keeps it external).
- Real per-model tokenizer (available behind the `TokenEstimator` port on demand).
- Wasm/MCP custom-node loaders (contract reserved; loaders ride β.7.5/γ.1 and
  β.4.4).
- **Runtime capability enforcement for custom nodes** — deferred to β.4.2, when
  the first capability-declaring node (fs-write offload) exists. v1 builtins
  declare no capabilities; native custom nodes run in-process and capabilities
  are checked at build time only.

## Consequences

**Positive:**

- Long-horizon runs no longer hit the context window with no recovery path —
  `fit_budget` enforces a hard ceiling before the request is sent.
- All three v1 transformers are `Pure` → β.6 conformance is bit-identical with
  zero special casing.
- Backward compatibility is absolute: an agent with no context block behaves
  byte-identically to today (no `ContextStepRan` events emitted).
- The five locked contracts ensure β.4.2/β.4.3/β.4.4/γ.6 slot in without an
  IR or trait break.
- The open registry means users can define native custom nodes today; the wasm
  sandbox lane arrives with β.7.5 without touching the contract.
- `tau build` now exits 2 on any lowering error — a latent silent-failure
  class is closed.

**Negative / obligations:**

- Authors must declare a context block explicitly — context management is
  opt-in. There is no auto-inferred context policy.
- `fit_budget` must be the last pipeline step (build-time enforced) — authors
  who omit it or place it mid-pipeline get a build error, not a runtime
  surprise.
- The E1 heuristic may over-count tokens, causing `fit_budget` to drop one
  extra old turn. This is acceptable; a real tokenizer can be swapped in.
- Native custom nodes run in-process; the wasm sandboxed lane is deferred.
  Third-party custom nodes are in-process until β.7.5.

## Alternatives considered

**Pipeline-only forever (no agent-driven layers):** the three v1 transformers
plus future offload/summarize would be the full story. Rejected because no
deterministic transformer can match MemGPT-style hierarchical paging (which
requires agent invocation and an external store). The layered architecture is
the only shape that reaches the SOTA apex while keeping tau's invariants.

**MemGPT-in-core (bake a memory store into `tau-runtime-core`):** would
collapse all tiers into one system. Rejected because: (a) it violates
constitutional non-goal NG6 ("no built-in persistent memory store"); (b) it
wrecks determinism (the core becomes stateful against an external store); (c)
wasm portability requires I/O-free core. MemGPT-style paging is achievable via
Layer 2 + Layer 3 without touching the core.

**Closed transformer enum (no open registry):** simpler dispatch. Rejected
because the public extension point (custom nodes) is load-bearing — power users
need it before wasm lands, and closing the enum would require an IR bump for
every new builtin.

**Real per-model tokenizer in v1:** exact token counts. Rejected because: a
vocabulary blob is model-specific (tight coupling to Anthropic/OpenAI internal
tokenizers), heavy (wasm size cost), and non-portable (MCU). The E1 heuristic
is sufficient for a ceiling-enforcement use case; the port accepts a drop-in
later.

**Route `BudgetUnsatisfiable` through `RunOutcome::Failed`:** allows the agent
orchestrator to handle the error. Rejected because a budget that cannot be
satisfied for the *system prompt + live turn* is a structural misconfiguration —
the agent cannot recover by retrying; a human must fix the `max_tokens` or
`keep_last_turns` settings. Routing it through `RuntimeError` is consistent
with ADR-0006's error/failure split.

## References

- Design spec:
  `docs/superpowers/specs/2026-06-14-beta-4-context-manager-design.md`
- Related ADRs: [ADR-0006](0006-tau-runtime.md) (error/failure dichotomy),
  [ADR-0037](0037-workflow-ir.md) (workflow IR + IR checks),
  [ADR-0044](0044-deliverables-and-goals.md) (build-time checks pattern),
  [ADR-0014](0014-sandboxing.md) (capability model)
- Philosophy: [`docs/explanation/tau-philosophy.md`](../explanation/tau-philosophy.md)
