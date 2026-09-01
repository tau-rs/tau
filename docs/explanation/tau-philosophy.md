# The tau philosophy

**Status:** Canonical vision document. Supersedes the forward-looking parts of
[`tau-as-language.md`](tau-as-language.md), which remains in-tree as the
historical lineage (referenced by ADRs 0014–0023).

**Date:** 2026-05-29.

**Audience:** project contributors, ADR authors, users deciding whether tau
fits their problem.

---

## In one breath

> **tau is a workflow compiler for portable, capability-safe AI agents and
> workflow automation.** You declare an agent or workflow once — composing
> native tools, contracted MCP servers, and imported skills with explicit
> capabilities. `tau dev` runs it instantly against a prebuilt engine.
> `tau build` compiles the same declaration to a portable, capability-enforced
> artifact that runs where other harnesses can't — server, edge, browser
> (wasm), or embedded. Inference is always delegated to a pluggable endpoint.
> Credentials are always delegated to a provider chain. tau is the **harness
> and the compiler**; it owns no inference, no registry, no secret store, and
> packages no external tool code.

```
                          ONE DECLARATION
                                │
                ┌───────────────┴───────────────┐
                ▼                               ▼
            tau dev                         tau build --target …
       (interpreted, instant)            (IR → portable artifact)
       prebuilt engine on host           wasm primary; C-ABI / firmware
       hot reload, Vercel-DX feel        secondary; capability-enforced
                │                               │
                └───────────────┬───────────────┘
                                ▼
                       SAME engine, SAME IR
                       (no dev/prod drift)

       inference  ──────▶  pluggable endpoint (local | LAN | cloud)
       credentials ─────▶  provider chain (env | file | vault | device id | broker)
       external tools ──▶  contracted MCP servers (never packaged)
       local tools   ──▶  native, compiled in, capability-gated
```

---

## Three convictions

These are the load-bearing beliefs. Everything in tau follows from them.

### 1. tau is a *compiler*, not a framework

The agent ecosystem is saturated with frameworks: LangChain, CrewAI, Mastra,
AutoGen, Semantic Kernel, OpenAI Agents SDK, Claude Agent SDK, Vercel AI SDK.
Every one of them is an *interpreted server process*. Each ships your workflow
as a runtime call graph, lives in your application process or beside it, and
locks you to one execution environment.

tau treats an agent/workflow as a **source language** with a **canonical
intermediate representation (IR)**. `tau build` lowers the IR to a
**portable, content-hashed artifact** that runs in many environments.
The discipline is borrowed from cargo + Bazel: declare once, compile per
target, ship the artifact.

The consequence: an agent isn't *running code calling a library*. It's *a
program with a frozen capability set and a target triple*, the way a Rust
binary is.

> **Implementation status (2026-06-01):** The workflow IR shipped in β.2 (PRs #263–#271).
> See [ADR-0037](../decisions/0037-workflow-ir.md) and the
> [design spec](../superpowers/specs/2026-05-31-workflow-ir-design.md).
> v0 uses partial-interpret lowering; AOT lands in β.7. Conformance suite + `tau run --bundle`
> interpreter dispatch deferred to β.2.6.1.

### 2. tau is a *harness everywhere*; inference and credentials are *always* delegated

The harness — the loop, tool dispatch, capability enforcement, context
management — is what runs in the artifact. **Inference is always a remote
call** to a pluggable endpoint, even from a microcontroller. **Credentials
are always resolved through a provider chain**, never baked into the client.

This dissolves the conflicts that broke earlier framings:

- "Self-contained" doesn't mean "owns the model." The model is delegated.
- "Portable" doesn't mean "carries credentials." Credentials are delegated.
- The browser case isn't a special exception: a wasm-resident harness in
  Angular still delegates inference (to a gateway) and credentials (to a
  broker). Same shape as every other target.

The result is one model — *harness everywhere, inference + credentials
delegated* — that covers server, edge, browser, and embedded without forking.

### 3. tau is *capability-safe by construction*; portability is the dividend

Every tool — native or contracted — declares its capabilities **once**, in
the root `tau.toml` constitution. Capabilities are uniform in *declaration*,
not in *mechanism*: `tau build` **lowers the same declaration per target** to
whatever that target enforces with:

- **wasm (the primary target):** generated **WIT imports** + host config. A
  capability the workflow never declared produces no import; an un-imported
  host function is **unreachable by construction** — there is nothing to
  sandbox at runtime because the capability was never wired in. Enforcement is
  structural, not a runtime check.
- **host / native:** an OS sandbox — landlock / seccomp (Linux),
  sandbox-exec (macOS), AppContainer (Windows) — gates the declared
  capabilities at the process boundary.
- **bare-metal firmware:** **passthrough** — advisory only, and **honestly
  labeled** as such. The declaration is recorded and surfaced, but the chip
  provides no enforcement boundary; tau does not pretend otherwise.

`tau check` refuses to build a workflow that requires enforcement a target
can't provide. A workflow that demands `strict` cannot ship as `passthrough`
without an explicit declaration.

This is the gap that MCP itself leaves open: MCP's "capabilities" are
protocol-feature negotiation; its authorization is OAuth-scoped remote access.
**Neither sandboxes a tool's filesystem, network, or exec at runtime.** tau
fills exactly that gap, lowering one declaration to each target's native
mechanism.

Portability falls out of capability-correctness: if every tool's declared
capability shape can be lowered onto the target, the artifact runs there. The
target triple is the contract.

---

## The architecture, in one picture

```
                            tau artifact (compiled)
  ┌─────────────────────────────────────────────────────────────────┐
  │                                                                 │
  │   AGENT HARNESS — loop · dispatch · context manager · cap gate   │
  │       │                                                         │
  │       ├─ NATIVE TOOLS  (compiled in)                            │
  │       │     gpio · file · sensor · custom Rust                  │
  │       │     enforced at OS / wasm level (landlock / WASI caps)  │
  │       │     WORKS OFFLINE                                       │
  │       │                                                         │
  │       └─ MCP CONTRACTS  (typed interface, NOT packaged)         │
  │             reach external MCP servers at runtime               │
  │             capability-bounded at contract boundary             │
  │                                                                 │
  │   MCP FACILITATOR — built-in handlers for MCP primitives        │
  │     tools/call · resources/read · sampling ★ · roots ★ ·         │
  │     elicitation · prompts/get · notifications · cancellation    │
  │       ★ where capability gate + delegated inference attach      │
  │                                                                 │
  │   CONTEXT MANAGER — stateful container + per-turn pipeline       │
  │     state: history · summaries cache · memory ref (cross-turn)  │
  │     pipeline: trim → compact → summarize → retrieve → dedup →    │
  │               fit_budget   (pure, ordered, debuggable)          │
  │                                                                 │
  └─────────────────────────────────────────────────────────────────┘
              │                          │                   │
              ▼                          ▼                   ▼
       INFERENCE ENDPOINT          MCP SERVERS         CREDENTIAL CHAIN
       local / LAN / cloud         (external, ind.    Strategy +
       (pluggable via              owned, contracted) Chain-of-Responsibility
        LlmBackend port)                              baked · env · file ·
                                                      SecretMgr · WorkloadId ·
                                                      DeviceId · TokenBroker
```

The hexagonal structure is unchanged from tau's existing design. The
philosophy *narrows* what tau builds versus integrates, not the architecture.

---

## What you author: three surfaces, one IR

*(Amended 2026-09-01 per ADR-0071/0072 — the earlier "TS is sugar over
the IR" framing is dropped.)*

A tau project is authored in three complementary surfaces, each owning
one category of primitive, all lowering through one validator into one
frozen, content-hashed IR:

| Surface | Role | Contains |
|---|---|---|
| **TOML / dirs** | the vocabulary | agents, models, `[allow]` (never emittable by code), MCP contracts, triggers, context pipelines — pure data, no templating ever |
| **TypeScript** | the choreography | `pipelines/` — flow only, executed at synth time in a sandbox (never at runtime), emitting config the validator fully re-checks |
| **Rust** | the muscle | `#[tau::tool]` / `#[tau::deterministic]` — tool bodies with declared capabilities, content-hashed |

The surfaces are not interchangeable front-ends: each fact has exactly
one home, the constitution is never generated by the thing it governs,
and the engine stays 100% Rust. A typical agent in TOML:

```toml
[agent.fan-monitor]
prompt = "Watch the temperature; run the fan if it rises above 30°C."
model  = "claude-haiku-4-5"

[agent.fan-monitor.tools]
read_temp = { native = "ReadTemp" }            # local, compiled in
set_fan   = { native = "SetFan"  }              # local, compiled in
weather   = { mcp = "weather" }                 # external, contracted

[mcp.weather]
url = "https://mcp.weather.com"
capabilities = { network = ["api.weather.com"] }   # ◀ bounded

[agent.fan-monitor.context]
budget = { tokens = 16000, headroom = 0.2 }
pipeline = [ "trim_old", "compact_tool_outputs",
             "summarize_oldest", "fit_budget" ]
```

(The `native = "ReadTemp"` reference shape above is today's syntax; the
redesign's E-1 epic replaces name-string references with the Rust
`#[tau::tool]` registry and real content hashes, with a deprecate-warn
cycle — ADR-0071.)

The TypeScript surface choreographs *flow* over that vocabulary — it
never defines agents, models, tools, or capabilities. A pipeline file
(typed against the generated `tau.gen.ts` bindings):

```ts
// pipelines/fan-watch.ts — id = file path
import { pipeline } from "@tau/sdk";
import { agents } from "../tau.gen"; // generated, hash-stamped

export default pipeline((p) => {
  const reading = p.agent(agents.fanMonitor, { input: p.input });
  p.check("in-range", reading.output.field("celsius").lt(80));
});
```

**TypeScript runs only at synth time, sandboxed, and emits config the
single validator fully re-checks** (ADR-0072). The engine stays 100%
Rust; there is no JS at runtime. The IR is canonical; every surface
lowers to it.

---

## Two tool kinds, one rule

The rule for deciding between native and contracted:

> *Can I implement it directly, and does it touch local resources that need
> offline capability? → **native tool**. Is it external, owned by someone
> else, or impractical to reimplement? → **MCP contract**.*

| | native tool | MCP contract |
|---|---|---|
| code lives in | tau artifact | external MCP server |
| reached via | in-process call | MCP wire (stdio / HTTP / SSE) |
| capability gate at | OS / wasm boundary | contract boundary |
| works offline | yes | only when reachable |
| who owns the code | you | the server author |
| installed where | nowhere — it's *in* the artifact | independently distributed |

tau is the **facilitator** for the MCP side: it ships built-in handlers for
every standard MCP interaction, capability-bounds each one, and presents a
typed, ergonomic surface to the harness. tau **does not package MCP server
code**, ever. The server's distribution model stays intact; tau owns the
interface to it.

This is "contract, don't contain" applied uniformly: tau aggregates
ecosystems, it doesn't replace them.

---

## Two profiles, one engine

```
DEV — tau dev                          RELEASE — tau build --target …
====================================   ====================================
prebuilt engine on host                IR lowered to portable artifact
your tools as callbacks                 your tools linked statically
hot reload                              tree-shake unused tools (wasm-metadce)
introspection on                        stripped, debug info dropped
no toolchain required                   per-target compile (cargo / wasm tooling)
~Vercel AI SDK feel                     wasm-primary, C-ABI / firmware secondary
```

The discipline is **one engine, two modes** — the prebuilt engine driving
`tau dev` is the same Rust core that's lowered to wasm by `tau build`. Your
tools are the only thing that changes shape: callbacks in dev, statically
linked in release. There is no second runtime.

This is how tau answers "works in dev, breaks in prod": there is one
behavioral specification, exercised both ways, and a **cross-target
conformance gate** that runs the same scenarios against both profiles and
demands agreement.

---

## Context management is first-class

Context engineering is the dominant agent-quality lever in 2026.
tau treats it as a baked-in primitive, not a library you bolt on per workflow.

The shape is a **stateful manager containing a per-turn pipeline of pure
transformers** (Shape 1 ⊕ Shape 2 hybrid):

```
ContextManager  — owns long-lived state across turns
 ┌──────────────────────────────────────────────────────────────┐
 │ STATE: full history · summaries cache · memory store ref    │
 │                                                              │
 │ each turn:                                                   │
 │   PIPELINE (pure, ordered, debuggable)                       │
 │   trim_old → compact_tool_outputs → summarize_oldest →       │
 │   retrieve_relevant → dedup → fit_budget (always last)       │
 │                                                              │
 │ on response: update summaries cache, persist memory items    │
 └──────────────────────────────────────────────────────────────┘
```

Four discipline rules keep the boundary clean:

1. Pipeline steps are *pure* transforms over `(messages, ctx_state)`. They
   read state through the manager, never directly hold it.
2. State updates happen only at named hook points (`before_turn`,
   `after_response`, `on_tool_result`).
3. `fit_budget` is always the last step — the defense-in-depth guarantee that
   the produced prompt fits the model window.
4. The same hybrid runs on every target. On wasm/edge/embedded, you configure
   cheaper steps (or skip retrieval if no vector store is reachable). The
   engine doesn't fork.

v1 ships windowing + tool-output compaction (universal, no infrastructure
needed, works on every target). Retrieval / long-term memory comes as v2,
backed by a contracted vector-store MCP — no built-in vector DB.

---

## Credentials: a provider chain, never a vault

tau resolves credentials through a **provider chain** modeled on AWS / GCP /
Azure default credential chains, Dapr secret stores, and SPIFFE / Kubernetes
workload identity. Each provider is a Strategy; the chain is tried in
configured order until one resolves:

```
CredentialProvider chain
  Baked            (compile-time constant — trusted/offline)
  Env              (TAU_LLM_KEY=…)
  File             (mounted secret, k8s volume)
  SecretManager    (Vault / AWS / GCP / Azure KV)
  WorkloadIdentity (SPIFFE / IRSA / GKE Workload Identity — no static secret)
  DeviceIdentity   (per-device non-extractable key in a secure element)
  TokenBroker      (OIDC / OAuth2 short-lived token exchange — BFF)
  → first one that resolves wins; deployment configures the order.
```

tau ships the chain mechanism plus the standard providers; it does **not**
operate a secret store. For the embedded case, the recommended pattern is
**per-device identity** (non-extractable key in a secure element — ESP32-S3
eFuse + flash encryption, or ATECC608) used to mint short-lived scoped
tokens for inference and tool backends. Shared bearer keys on a fleet are
explicitly discouraged.

For the browser: the **Backend-for-Frontend (BFF) pattern** is the only
sanctioned shape — the browser never sees a provider key. Either a tiny
token broker mints short-lived scoped tokens, or a thin AI gateway proxies
calls (Cloudflare AI Gateway, Portkey, LiteLLM, Kong). tau supports both via
the same `TokenBroker` provider.

---

## What tau is NOT

The boundaries are part of the philosophy. They keep tau focused on what's
genuinely unoccupied.

- **Not an inference engine.** Inference is delegated through `LlmBackend`.
  tau will never bundle a model.
- **Not a credential vault.** tau resolves; it doesn't store. Operators choose
  Vault, SPIFFE, k8s secrets, etc.
- **Not an MCP registry or marketplace.** tau aggregates the existing MCP
  registry; it does not host servers, and packages no MCP server code.
- **Not a codegen backend.** `tau build` is a **front-end transform** —
  workflow IR → emitted code — handed off to `cargo` + the wasm component
  toolchain. tau is not reimplementing LLVM.
- **Not the easy server-agent SDK.** Vercel AI SDK wins that market. tau is
  for workflows that need to **be a portable artifact** — where ship-and-run
  somewhere unusual is the requirement.
- **Not authored in Python or TypeScript as canonical source.** A TS sugar
  layer is supported, but the IR is the source of truth. Authoring in a host
  language would invert the compiler thesis.
- **Not a walled package registry.** tau is a **polyglot resolver +
  content-hashed lockfile** over crates.io, npm, the MCP registry, Anthropic
  skills, and git URLs. tau-native units distribute via git (the Go-modules
  model), not a tau-operated store.

---

## The wedge

The 2026 field has filled in around tau — Cloudflare a portable MCP host
(vendor-locked), wasmCloud wasm sandboxing (a mesh you operate), LangGraph a
durable agent graph (Python, no artifact), BAML a prompt compiler
(host-language glue), Temporal durable execution (your infra), esp-claw an
on-MCU agent (no sandbox). Each owns one slice. Nobody ships tau's
combination:

> declare what agents are *allowed* in a root constitution, author workflows
> beautifully in any language (generated, typed), and compile to one
> hardware-agnostic, capability-bounded component proven identical across
> local / edge / browser / embedded — with build-time enforcement and no
> runtime surprises.

The moat is the **combination + conformance + vendor-independence +
root-governed capability safety** — not novelty of any one piece.

Concretely, "build-time enforcement and no runtime surprises" is three
independent gates — compile-time types, `tau check`, and the conformance gate
— each proving a property the other two cannot. See
[The three-gate guarantee](three-gate-guarantee.md).

---

## Acknowledged risks

The vision contains three pieces with no map. They are surfaced here so the
philosophy is honest about what's hard.

**(D) Declarative agent / workflow IR → portable artifact compiler.**
*Genuinely unprecedented.* LangGraph compiles to in-memory graphs; Cloudflare
Workflows persists JS step state; n8n is JSON-interpreted at runtime. Nobody
lowers an agent/workflow IR to a portable wasm component today. This is the
foundational research bet. Framed before engine implementation in a separate
scoping doc.

**(G) Polyglot resolver across crates.io / npm / MCP registry / git /
Anthropic skills.** *Hard, scope-discipline required.* No tool spans this
combination today. Phase-1 scope is deliberately narrow: tau-native units via
git + content-hash lockfile (Go-modules pattern, proven), delegate to host
ecosystems for crates.io / npm (call `cargo` / `npm`), contract MCP at
runtime. tau is not the cross-ecosystem solver. Framed in a scoping doc
before any resolver work begins.

**(C″) WASI 0.2 Component Model on microcontrollers.** *Runtime not yet
available.* WAMR on ESP32 is production but still on WASI Preview 1. The
honest framing: wasm component is the primary self-contained target for
server / edge / browser; the embedded path uses WAMR / Preview-1 wasm or
native firmware (`bare-metal-*-passthrough`) until the component-model
runtime ships on MCU. Framed in a scoping doc.

These three are first-class line items in the roadmap. Phase 0 of the
post-philosophy roadmap is framing them; no engine implementation work
begins until they are scoped.

---

## Audience and adoption posture

tau is not for everyone, on purpose. The audience self-selects to the
problem.

**For:** developers shipping agents to environments where running a tau
daemon and calling a hosted service is not an option — embedded products,
on-premises / regulated, edge / IoT, browser apps with no backend, software
that ships an agent as a library to its end users.

**Not for:** "I want a chatbot in my web app." Vercel AI SDK or calling the
API directly is lighter and better suited. tau **should not** chase that
market.

The progressive-disclosure principle keeps the on-ramp gentle: `tau dev` is
instant and toolchain-free; rigor (capabilities, target triples, hermetic
builds) turns on as you move toward release. You don't pay the build-system
ceremony until you ask for portability.

---

## Lineage

This document supersedes the forward-looking parts of
[`tau-as-language.md`](tau-as-language.md) (2026-05-02). That document
introduced the "tau as a compiled language" framing and the target-triple
discipline, which both survive. What this document adds, and where it
diverges:

- **adds** the harness-everywhere / inference-always-delegated principle.
- **adds** the two-tool-kinds rule (native vs MCP contract) and tau as MCP
  facilitator.
- **adds** the context manager as a first-class baked-in primitive.
- **adds** the credential provider chain.
- **adds** the polyglot resolver / no-walled-registry posture.
- **adds** the dev/release one-engine discipline and TS sugar layer.
- **narrows** what tau builds: the engine, the capability gate, the IR, the
  facilitator, the portability machinery. Everything else is integrated.
- **acknowledges** the three load-bearing risks (D, G, C″) that
  `tau-as-language.md` did not flag as research bets.

ADRs referencing the older vision remain valid as historical record; new
ADRs should reference this document.
