# Context manager primitive (Phase β.4)

**Date:** 2026-06-14
**Status:** Design approved. v1 is deterministic-only; the SOTA end-state is
banked as sequenced deferred work (see *Future work*).
**Scope:** Add an opt-in, per-agent **context pipeline** to the canonical
`tau.toml` → IR → runtime surface: a declarative, capability-safe sequence of
**context transformers** applied to the conversation on every inference turn,
before the history is projected to the LLM. v1 ships three pure transformers
(`trim_old`, `compact_tool_outputs`, `fit_budget`) plus the public extension
contract so users can define custom nodes.

---

## 1. Why

Today the runtime sends the **entire conversation history on every turn**. The
agent loop assembles `messages: Vec<Message>`
(`crates/tau-runtime-core/src/stream.rs:~291`) and projects it wholesale through
`agent_messages_to_provider_messages()`
(`crates/tau-runtime-core/src/run.rs:645`) into `CompletionRequest.messages`.
There is no truncation, compaction, or budgeting. This:

- hits the context window on any long-horizon run (hard failure);
- pays for re-sending stale tool dumps every turn (cost scales with tokens);
- degrades recall as the window fills ("context rot" /
  "lost-in-the-middle").

The canonical philosophy (`docs/explanation/tau-philosophy.md`) and the ROADMAP
(`ROADMAP.md`, Phase β.4) call for an **opt-in context manager**:
backward-compatible by absence, declarative in the manifest, lowered into the
IR, and portable into a wasm bundle. It is also a prerequisite for the β.6
canonical "fan-monitor" conformance scenario (which declares a context block)
and for the β.8 `contextManager()` TS factory (which currently parse-rejects
pending β.4).

## 2. What the field does (SOTA survey)

The design is anchored against current industry/research practice, mapped onto
tau's hard constraints — **capability-safe · portable (wasm/MCU) · deterministic
conformance (β.6) · NG6 "no built-in persistent memory"**.

| Technique | Source | Determinism | tau-fit |
|---|---|---|---|
| Truncation / sliding window | ubiquitous | pure | `trim_old` — **v1** |
| Tool-result clearing/pruning | Anthropic ("safest lightest compaction") | pure | `compact_tool_outputs` — **v1** |
| Token-budget fitting | ubiquitous | pure | `fit_budget` — **v1** |
| File-as-context / restorable externalization | Manus; Anthropic "just-in-time" | pure (gated fs + stable handle) | `offload_tool_outputs` — β.4.2 |
| LLM compaction / recursive summary | Anthropic "compaction"; LangChain; Mem0 (−80% tokens) | LLM-backed | `summarize_oldest` / `compact` — β.4.3 (cassette-replayed) |
| Structured note-taking / memory tool / recitation | Anthropic memory tool; Manus `todo.md` | agent-driven, stateful | memory-tool layer + `recite_goal` — β.4.4 |
| Retrieval / archival vector memory | MemGPT; RAG; Mem0 | external store | `retrieve_relevant` via contracted MCP — γ.6 (NG6: never built-in) |
| Hierarchical OS-paging memory | MemGPT / Letta | agent-invoked paging | apex tier (gated tools + MCP) |
| KV-cache discipline (stable prefix, append-only, mask-don't-remove) | Manus ("single most important metric", ~10× cost) | invariant | cross-cutting contract, not a transformer |

Sources: [Anthropic — Effective context engineering for AI
agents](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents);
[Manus — Context Engineering Lessons](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus);
[MemGPT: LLMs as Operating Systems](https://arxiv.org/pdf/2310.08560);
[CoALA — Cognitive Architectures for Language Agents](https://arxiv.org/pdf/2309.02427);
[LLM Agent Memory: A Survey](https://www.preprints.org/manuscript/202603.0359/v1).

## 3. Goals / non-goals

**Goals (v1):**
- Opt-in `[agents.<id>.context]` block → IR → per-turn runtime execution.
- Three pure transformers; deterministic token budgeting; an enforced budget
  ceiling.
- Backward compatibility by absence: agents with no context block behave
  byte-identically to today.
- A **public, stable extension contract** so users define custom nodes, native
  in v1 (capability-safe by construction).
- Lock the four cross-cutting contracts (§9) so later tiers slot in without an
  IR or trait break.

**Non-goals (v1, banked as Future work):**
- LLM-backed transformers (`summarize_oldest`, `compact`) — β.4.3.
- Restorable externalization to fs — β.4.2.
- Agent-driven memory tools / recitation — β.4.4.
- Retrieval / vector memory — γ.6 (contracted MCP; NG6 keeps it external).
- Real per-model tokenizer — the `TokenEstimator` port allows it later (§7).
- Wasm/MCP custom-node loaders — the contract is reserved now; loaders land with
  the wasm artifact work (β.7.5/γ.1) and the MCP layer (β.4.4).

## 4. Architecture — layered hybrid

The conformance boundary is the dividing line. The deterministic pipeline is the
gated **core**; agent-driven memory and retrieval are additive layers above it.

```
┌─────────────────────────────────────────────────────────────────────┐
│ AGENT — every inference turn: msgs: Vec<Message>                      │
│  ┌─────────────────────────────────────────────────┐                 │
│  │ LAYER 1 — declarative context pipeline (CORE)     │  ◀ β.4 v1      │
│  │   trim_old → compact_tool_outputs → fit_budget    │                │
│  │   pure, runtime-applied, author-declared          │                │
│  │   ★ THE CONFORMANCE-GATED PART (β.6 bit-identical)│                │
│  └─────┬─────────────────────────────────────────────┘               │
│        │  later transformers slot in here via the same trait:         │
│        │    offload_tool_outputs (β.4.2, pure → fs)                   │
│        │    summarize_oldest     (β.4.3, LLM, cassette-replayed)      │
│        │    recite_goal          (β.4.4, pure)                        │
│        ▼                                                              │
│  LAYER 2 — agent-driven memory TOOLS (capability) ◀ β.4.4 (additive)  │
│  LAYER 3 — retrieval via contracted MCP            ◀ γ.6  (NG6)        │
└─────────────────────────────────────────────────────────────────────┘
```

Rejected alternatives: **pipeline-only forever** (never reaches agent-driven /
hierarchical memory) and **MemGPT-in-core** (bakes a memory store into core →
violates NG6 and wrecks determinism). The layered hybrid is the only shape that
reaches the SOTA apex while keeping tau's invariants.

## 5. The `ContextTransformer` contract

Lives in `tau-runtime-core` (`no_std` + `alloc`). It is the **public extension
point** (contract #5): re-exported as a stable SDK surface; the registry is
**open**, not a closed enum.

```rust
/// One pure-or-impure step in an agent's per-turn context pipeline.
pub trait ContextTransformer: Send + Sync {
    fn name(&self) -> &str;

    /// Gates β.6 conformance AND what `TransformCx` exposes. Locked in v1.
    fn determinism(&self) -> DeterminismClass;      // Pure | LlmBacked | Stateful

    /// Capabilities this transform needs; v1's three return &[].
    fn required_capabilities(&self) -> &[CapabilityNeed];

    /// Runs every turn, before history is projected to the LLM.
    async fn transform(
        &self,
        cx: &mut TransformCx<'_>,    // capability-scoped BY determinism class
        msgs: Vec<Message>,
    ) -> Result<Vec<Message>, ContextError>;
}

pub enum DeterminismClass { Pure, LlmBacked, Stateful }
```

Determinism is **structural, not documentary** — `TransformCx` only hands out
what the declared class permits:

| class | `TransformCx` exposes | β.6 conformance |
|---|---|---|
| `Pure` | `estimate_tokens()`, config | gated (bit-identical) |
| `LlmBacked` | + `llm: &dyn LlmBackend` | gated via cassette replay |
| `Stateful` | + `store: &mut dyn MemoryStore` | excluded (Layer 2) |

A `Pure` transformer *cannot* call a model — it has no handle. The type system
enforces the conformance boundary. The same scoping holds custom nodes (§8) to
the same discipline.

## 6. Where it runs

Per turn, at the existing projection seam — the single source of truth:

```
stream.rs:~291 (every turn)                run.rs:645
   msgs: Vec<Message>                      agent_messages_to_provider_messages()
        │                                            ▲
        ▼                                            │
   for t in pipeline:  msgs = t.transform(cx, msgs)──┘  budgeted msgs → request
```

The pipeline transforms the **conversation message list**. The system prompt
(`agent_def.system_prompt` → `request.system`) is separate and never part of the
list, but its estimated token cost is *reserved* against the budget (§10).

## 7. Token estimation (E1)

`fit_budget` needs a per-message token count; tau has none today
(`CompletionResponse.usage` reports only *after* a call). v1 ships a
**deterministic heuristic** behind a swappable port:

```rust
trait TokenEstimator { fn estimate(&self, msg: &Message) -> u32; }

// v1 impl: ceil(serialized_bytes / 4) + per-role/structural overhead.
```

- **Deterministic** — pure arithmetic, identical on every platform → β.6 holds
  for free.
- **Portable** — no vocabulary blob; works unchanged in wasm and (later) MCU.
- **Sufficient** — `fit_budget` enforces a *ceiling*; a conservative
  over-estimate at worst drops one extra old turn.

A real per-model tokenizer (`TokenizerEstimator`) or backend-provided count can
drop in later behind the same trait, with no change to the transformer contract.

## 8. Config surface

```toml
# Built-in nodes
[agents.fan-monitor.context]
pipeline = ["trim_old", "compact_tool_outputs", "fit_budget"]

[agents.fan-monitor.context.trim_old]
keep_last_turns = 4

[agents.fan-monitor.context.compact_tool_outputs]
max_bytes = 512

[agents.fan-monitor.context.fit_budget]
max_tokens = 4000
```

A **custom node** (contract #5) looks like a built-in but declares its own
determinism + capabilities:

```toml
[agents.fan-monitor.context]
pipeline = ["trim_old", "my_dedup", "fit_budget"]

[agents.fan-monitor.context.my_dedup]
kind        = "custom"        # NEW node kind, reserved in IR now
source      = "native"        # native (v1) | wasm (later) | mcp (later)
package     = "my-context-nodes@^0.1"
determinism = "pure"          # author-declared → gates conformance + cx scoping
# capabilities = [...]         # must be granted or build is rejected
```

The block parses into typed config (not the free-form `[agents.<id>.config]`
passthrough). `[agents.<id>.config]` is unaffected.

## 9. The five locked contracts

| # | Contract | Why it must be in v1 |
|---|---|---|
| 1 | Determinism class on the trait | conformance boundary + `cx` scoping |
| 2 | Per-transformer capability declaration | safety for all nodes incl. custom |
| 3 | Restorable-handle convention | β.4.2 offload + γ.6 retrieve share it |
| 4 | KV-cache / append-only invariant | cheap now, expensive to retrofit |
| 5 | `ContextTransformer` = public SDK + open registry + IR `kind="custom"` node | user nodes slot in (native now, wasm/MCP later) with no IR/trait break |

**Contract 4 detail:** transformer ordering preserves prompt-prefix stability
where possible (append-friendly); a transformer that rewrites the prefix is
flagged *cache-busting* in its metadata so authors/tools can reason about
KV-cache cost. v1's three are prefix-stable except `fit_budget` when it must drop
from the head (an unavoidable, declared cache cost).

**Contract 3 detail:** when a transformer removes content it may leave a stable
**handle** (`{ kind, ref, bytes, … }`) in place of the body, so a later tier can
restore it (β.4.2 fs path; γ.6 retrieval). v1 defines the handle shape;
`compact_tool_outputs` uses a non-restorable truncation marker (restorable
offload is β.4.2).

## 10. Runtime: budget model, protected content, errors

```
                 max_tokens ─────────────────────────────────────┐
 reserved: estimate(system_prompt) + estimate(tool_specs)  ┊  available │
 PROTECTED (never dropped): system prompt · live (current) turn         │
 if PROTECTED alone > max_tokens → ContextError::BudgetUnsatisfiable
      → fails the run with a clear message (never send an over-window request)
```

**Error tiers** (mirroring deliverables #340):

- **Build-time** (`tau-pkg` / `tau-ir` checks): unknown transformer name;
  `fit_budget` missing or not last; duplicate step; invalid per-node config; a
  declared capability not granted; unresolved `kind="custom"` reference. → reject
  at `build` / `check`.
- **Runtime** (`RuntimeError`): `BudgetUnsatisfiable`; a transformer returning
  `Err(ContextError)`.

## 11. Transformer semantics (v1)

A **turn** = the messages from one `User`-text message up to (but excluding) the
next `User`-text message. All dropping is **turn-granular** to preserve the
pairing invariant (never orphan a `ToolCall` from its `ToolResult`), and the
**live turn is never dropped**.

- **`trim_old { keep_last_turns: u32 }`** — keep the N most recent turns; drop
  older turns whole.
- **`compact_tool_outputs { max_bytes: usize }`** — for each
  `ToolResult`/`ToolError` body in *prior* turns (not the live turn) exceeding
  `max_bytes`: keep the first `max_bytes` and append `…[truncated N bytes]…`.
  Deterministic; non-restorable in v1.
- **`fit_budget { max_tokens: u32 }`** — **must be last** (build-time enforced).
  Compute `available` (§10) and drop oldest whole turns until
  `Σ estimate(msgs) ≤ available`.

## 12. IR representation + lowering

Mirrors the shipped pipeline/check machinery (`crates/tau-ir/src/pipeline.rs`,
`check.rs`):

```
tau.toml [agents.<id>.context]  ──lower──▶  tau-ir: ContextPipeline {
                                              steps: Vec<ContextStep {
                                                transformer: TransformerRef, // builtin name | custom ref
                                                determinism: DeterminismClass,
                                                config: CanonicalValue,
                                              }> }
```

Attaches per-agent in the IR module; survives canonical encoding into the bundle
(wasm-portable). Round-trip determinism: `build` → re-`build` → identical bytes
(the C3 contract). A built-in `ContextTransformerRegistry` (name → impl) follows
the `DeterministicRegistry` pattern; built-ins pre-registered; native custom
nodes registered by the host static builder.

## 13. Observability

Each transformer emits one event per turn — the β.6 scenario already names these
(`ContextStepRan`). New vocabulary constants in `tau-observe` (mirroring how
deliverables added theirs):

```
ContextStepRan { step: "trim_old",             dropped_turns: 2 }
ContextStepRan { step: "compact_tool_outputs", compacted: 1, bytes_saved: 5488 }
ContextStepRan { step: "fit_budget", tokens_in: 9012, tokens_out: 3840, dropped_turns: 1 }
```

These are the events the β.6 conformance gate diffs between profiles.

## 14. Testing / conformance

- **Unit**: table-driven per transformer (all `Pure` → trivial fixtures).
- **IR round-trip**: config → IR → canonical bytes; `build` == re-`build`.
- **Conformance fixture** (new, alongside `tau-ir-conformance` `09/10/11`): a
  context-block workflow run under **dev + bundle**, asserting an identical
  `ContextStepRan` stream.
- **Backward-compat**: an agent with no context block → zero `ContextStepRan`
  events, output byte-identical to today.
- **Custom-node**: a native custom transformer registered, referenced from
  config, run end-to-end (proves the extension point is real); plus a build-time
  rejection test for an ungranted-capability custom node.

## 15. Future work (the banked SOTA end-state)

Each tier is unblocked by the five contracts v1 locks; none requires an IR or
trait break.

```
β.4   ─ deterministic spine (trim_old + compact_tool_outputs + fit_budget)  ◀ THIS SPEC
  │      + 5 locked contracts + native custom-node path
  ▼
β.4.2 ─ offload_tool_outputs  (restorable externalization → fs-write #332)   pure
  ▼
β.4.3 ─ summarize_oldest / compact  (LLM-backed, cassette-replayed)          determinism work
  ▼
β.4.4 ─ memory-tool layer + recite_goal  (agent-driven, capability-gated)    Layer 2 begins
  ▼
γ.6   ─ retrieve_relevant  (contracted vector-store MCP; NG6)                 already on roadmap
  ▼
apex  ─ hierarchical paging (MemGPT shape) over gated tools + MCP tiers
```

**Custom-node delivery lanes** (contract #5), mapped to tau's three lanes:

- **Native (Lane 3)** — `impl ContextTransformer` in Rust, registered via the
  static builder. **Available in v1.** For power users / embedded.
- **Wasm component** — author a node → compile to a wasm component implementing a
  tau-defined WIT `context-transformer` interface; loaded by name/path.
  Capability-safe, sandboxed, portable, no recompiling tau core. **The "easy"
  path; rides β.7.5/γ.1.** WIT sketch:

  ```wit
  interface context-transformer {
      record message { /* mirrors tau-domain Message canonical shape */ }
      enum determinism { pure, llm-backed, stateful }
      transform: func(msgs: list<message>, config: string) -> result<list<message>, string>;
  }
  ```

- **MCP (Lane 2)** — a stateful/external node contracted over MCP. β.4.4+.

## 16. Risks

- **IR debt** — the minimal context-step shape may miss something β.4.2/β.4.3
  need. Mitigation: ship the minimal shape; extend with an ADR when the second
  tier proves the gap (per the β IR-debt discipline).
- **Estimator drift vs real cost** — heuristic under-counts could let a request
  exceed the real window. Mitigation: per-role overhead tuned conservatively;
  `fit_budget` leaves headroom; E2 tokenizer available behind the port for agents
  that need exactness.
- **Custom-node trust** — a native custom node runs in-process. Mitigation:
  determinism + capability declarations are enforced at build time; the wasm lane
  (sandboxed) is the recommended path for third-party nodes once it lands.
- **Conformance flakiness** — none expected for v1 (all `Pure`); the gate admits
  `LlmBacked` only via cassette replay in β.4.3.

## 17. Crates touched (sketch)

- `tau-pkg` — `[agents.<id>.context]` parse + build-time checks
  (`src/project/agent.rs`, `project.rs`).
- `tau-ir` — `ContextPipeline` / `ContextStep` IR types + lowering + canonical
  encoding + checks (`src/pipeline.rs` sibling, `lower/`, `check.rs`).
- `tau-runtime-core` — `ContextTransformer` trait, `DeterminismClass`,
  `TransformCx`, `TokenEstimator`, `ContextTransformerRegistry`, the three v1
  transformers, and the per-turn hook at the projection seam
  (`stream.rs` / `run.rs`).
- `tau-observe` — `ContextStepRan` vocabulary constants.
- `tau-ir-conformance` — new context fixture + dev/bundle assertion.
- `tau-cli` — wiring through `run` / `dev` / `build` / `check` (largely
  transparent; checks surface via existing renderers).

## 18. Definition of done

- An agent with a declared context block round-trips under `tau dev` and inside a
  wasm bundle, hitting the budget; agents without one behave byte-identically to
  today.
- `build` → re-`build` → identical IR bytes for a context-block project.
- A native custom transformer runs end-to-end; an ungranted-capability custom
  node is rejected at build time.
- The conformance fixture produces an identical `ContextStepRan` stream across
  dev and bundle profiles.

## 19. ADR

This warrants a durable ADR — **ADR-0045** (records: layered-hybrid over
pipeline-only / MemGPT-in-core; determinism-class as the conformance boundary;
E1 heuristic estimator behind a swappable port; the public extension contract +
native-first custom-node lane). Note: two `0044-*.md` files currently collide
(`trigger-ingress-slice-1`, `deliverables-and-goals`) — to renumber separately;
this spec claims 0045 regardless.
