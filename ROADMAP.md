# Tau roadmap

This document tracks current direction, prior shipped work, and the
forward phasing under the canonical philosophy
[`docs/explanation/tau-philosophy.md`](docs/explanation/tau-philosophy.md).

For per-issue tracking, see
[GitHub Issues](https://github.com/LEBOCQTitouan/tau/issues).
For the historical record of every Phase 0/1 sub-project, see
`git log` and ADRs `0001`–`0034`.

---

## Where we are (2026-05-29)

- **Phase 0 (bootstrap)** — complete.
- **Phase 1 (runnable runtime)** — complete. Sandboxing, capability
  override, transitive resolution, streaming, REPL persistence,
  multi-agent v1/v1.1, Skills 1–6, lefthook pre-push gate, macOS &
  Windows sandbox adapters.
- **Phase 2 (compiled-language foundation)** — partial.
  - **A** `tau check` — shipped 2026-05-18 (PR #161).
  - **B** Target-triple registry — shipped 2026-05-19 (PR #190);
    Bazel-inspired 3-axis struct, 5 Available + 1 Reserved; ADR-0034.
  - **C.2** `tau build` (MVP producer) — shipped 2026-05-27 (PR #242).
  - **C.2.1** `tau build` flags (`--target`/`-o`/`--json`) — shipped
    2026-05-28 (PR #251).
  - **C.3** `tau run --bundle` (MVP consumer) — shipped 2026-05-28
    (PR #247).
  - **E** `tau verify --bundle` (reproducibility) — shipped 2026-05-28
    (PR #250).
  - **D** capability forward-compatibility — shipped 2026-05-29
    (PR #254).
  - **C.1** declarative workflow IR — **not yet started; this is the
    foundational research bet of the new direction.**
  - **F** remote target backends — design-only.
  - **G** WASM target backend — design-only.
- **Logging upgrades** (Phase 2 §A precursor cluster: B, C, D, E, F) —
  complete (PRs #195, #196, #198, #221, #222, #224, #226).
- **Tau-serve mode v1** — complete (PR #143, 2026-05-18).

The 2026-05-29 strategic pivot, recorded in
[`docs/explanation/tau-philosophy.md`](docs/explanation/tau-philosophy.md),
**reframes the remaining work** around three convictions: tau is a
*compiler* not a framework; tau is a *harness everywhere* with inference
and credentials always delegated; tau is *capability-safe by
construction*, with portability as the dividend. The forward phases
(α → β → γ → δ) execute that framing.

---

## Phase α — Pre-engine framing (NOW)

**Goal:** before any engine-core implementation begins, scope the three
load-bearing risks the philosophy acknowledges. The engine cannot ship
correctly if any of these are left implicit.

**Status:** scoping documents written; downstream design specs are the
deliverable shape.

| # | Framing | Scoping doc | Required output |
|---|---|---|---|
| α.1 | **D — Workflow IR** | [framing-d-workflow-ir](docs/superpowers/specs/2026-05-29-framing-d-workflow-ir.md) | Design spec + ADR settling D-1 through D-7 (node taxonomy, message shape, capability lowering, composition, lowering strategy, determinism, conformance) |
| α.2 | **G — Polyglot resolver** | [framing-g-polyglot-resolver](docs/superpowers/specs/2026-05-29-framing-g-polyglot-resolver.md) | Design spec for tau-native units + ADR; Phase 1 scope deliberately narrow (git URLs + content-hash; delegate to host ecosystems for crates.io/npm) |
| α.3 | **C″ — MCU strategy** | [framing-c-prime-prime-mcu-strategy](docs/superpowers/specs/2026-05-29-framing-c-prime-prime-mcu-strategy.md) | Two MCU tiers added as `Reserved` in the target registry; `tau-runtime-core` extraction spec; ADR on the passthrough commitment |

**Definition of done for Phase α:** each framing produces a committed
design spec, an ADR where decisions warrant durability, and one concrete
example so the committed shape is visible (a minimal IR example, an
example tau-native unit, two `Reserved` MCU triples in the registry).

**Until Phase α is complete, no Phase β code lands.**

---

## Phase β — Engine core (PRIORITY)

**Goal:** the portable, capability-safe agent + workflow engine.
This is the project's wedge — the artifact nobody else produces.
Even though it follows the framing chronologically, it is the
*priority work*: framing exists to de-risk it, not to delay it.

### β.1 — `tau-runtime-core` extraction

Refactor the existing `tau-runtime` into:

- `tau-runtime-core` — `no_std` + `alloc`, executor-agnostic, holds the
  agent loop generic over `LlmBackend` / `Tool` / `Storage`, no tokio,
  no `std::process`. Contains the three trait-object registries.
- `tau-runtime-tokio` — the current host shell (unchanged behavior; the
  existing test suite stays green as the proof).
- Future: `tau-runtime-embassy` — the MCU shell (Phase γ).

`tau-ports::Sandbox::wrap_spawn` and `apply_post_spawn` move behind a
`std`/`process` feature so the core compiles `no_std`.

**Definition of done:** every existing `tau-runtime` test passes; no
observable host-behavior change. This is the prerequisite for everything
that follows in β and γ.

### β.2 — Workflow IR implementation (per Framing D)

Minimal IR first: Agent and Tool nodes only; one monolithic component
on lowering; AOT not partial-interpret. Extend deliberately, not
preemptively. Round-tripped through `tau build` → run → produce
identical bytes (the C3 / determinism contract).

### β.3 — MCP facilitator

Built-in handlers for the MCP interaction primitives — `tools/call`,
`resources/read`, `sampling` (routes to delegated inference), `roots`
(routes to capability gate), `elicitation`, `prompts/get`,
`notifications`, `cancellation`. Per-handler capability gating at the
contract boundary. tau packages **no MCP server code**.

Replaces the bespoke external-plugin process+stdio handshake. Native
tools (compiled in) skip the protocol entirely; external tools = MCP.

### β.4 — Context manager primitive

Stateful manager (Shape 1) containing per-turn pipeline of pure
transformers (Shape 2). v1 ships:

- `trim_old` — sliding window
- `compact_tool_outputs` — summarize large tool results in place
- `summarize_oldest` — incremental summarization via cheap model
- `fit_budget` — always-last guarantee

v2 (Phase γ) adds `retrieve_relevant` backed by a contracted vector-store
MCP. No built-in vector DB.

### β.5 — Credential provider chain

Strategy + Chain-of-Responsibility port + standard providers: `Baked`,
`Env`, `File`, `SecretManager` (Vault / AWS / GCP), `WorkloadIdentity`
(SPIFFE / IRSA), `DeviceIdentity` (per-device secure-element key,
short-lived scoped tokens), `TokenBroker` (BFF / OIDC). Deployment
configures the order. tau ships the chain; operators choose the vault.

### β.6 — Cross-target conformance gate

The C3 discipline made testable: a scenario suite that runs against both
the dev profile and each release artifact (wasm primary in Phase β),
demanding identical observable behavior. This is the behavioral sibling
of `tau verify --bundle`'s byte-level reproducibility check, and it's
what makes the "minified conserves all features" claim verifiable.

### β.7 — Dev / release one-engine discipline

`tau dev` runs `tau-runtime-core` directly (interpreted host shell, hot
reload, your tools as callbacks). `tau build --target wasm` lowers the
same IR through the workflow-IR compiler to a wasm component containing
the same core + statically-linked tools. Both must pass β.6.

### Phase β success criterion

A user can declare a workflow (one agent, one native tool, one MCP
contract, one context pipeline), run it instantly under `tau dev`, build
it to a wasm component, and run that artifact in any wasm host with the
declared capabilities enforced — and the conformance gate proves the
two profiles agree.

---

## Phase γ — Portability targets

**Goal:** extend the engine across target triples beyond the wasm
component server/edge/browser baseline established in β.

| # | Target | Notes |
|---|---|---|
| γ.1 | wasm component on **server / edge** | hardening the β baseline; Spin / wasmtime hosts |
| γ.2 | wasm component in the **browser** | jco / wasm-bindgen integration; the BFF + TokenBroker credential pattern |
| γ.3 | **C-ABI library** | passthrough; cbindgen header; embed in C / C++ / Python / Go via FFI; honest about advisory-only capability enforcement at this tier |
| γ.4 | **`wasm-mcu-preview1-tau-managed`** | WAMR runtime on ESP32-class; per Framing C″ |
| γ.5 | **`bare-metal-xtensa-passthrough`** + siblings | native firmware via the `tau-runtime-embassy` shell; per Framing C″ |
| γ.6 | Context-manager v2: **`retrieve_relevant`** | requires a contracted vector-store MCP (e.g. pgvector / qdrant) |

The `wasi-p2-component` target on MCU (the "wasm component on
microcontroller") is **deferred** to a future phase until the runtime
ecosystem catches up. Tracked via Framing C″.

---

## Phase δ — Distribution + DX

**Goal:** make tau pleasant to author against and to share work through,
once the engine is solid. Sequenced last on purpose: the package
manager is only as valuable as the engine people want to run.

### δ.1 — Polyglot resolver Phase 1 (per Framing G)

`tau add <git-url>` for tau-native units (agent templates, workflow
templates, capability profiles, context-pipeline presets). Git-pinned,
content-hashed, capability-audited at install. **No tau-operated
registry.** Cross-ecosystem solving (cargo + npm + MCP semantics) is
*explicitly out of scope* — delegate to each ecosystem's resolver.

### δ.2 — TypeScript sugar layer

`@tau/sdk` TS package that emits the IR. Type-checked authoring,
autocomplete, IDE integration. Sugar over the canonical IR, **not** a
parallel runtime. The same IR runs under `tau dev` and lowers via
`tau build`.

### δ.3 — Progressive-disclosure polish

`tau dev` is the zero-toolchain default. `tau build` defaults to
`--container` for cross-target work (the Rust `cross` model; only podman
required, no rustup-targets). `--remote` deferred to later as a hosted
convenience, **not** a Phase δ commitment.

### δ.4 — Reference templates

Three or four tau-native units (analogue to Skills-6 reference packages)
demonstrating the agent-template and workflow-template shapes,
distributable via git. Sets the pattern for community contribution
without operating infrastructure.

---

## Out of scope (forever)

The constitutional non-goals from
[`CONSTITUTION.md` §2](CONSTITUTION.md) remain in force. The philosophy
sharpens, but does not relax, any of them.

- **NG1.** Tau is not an LLM or an agent. *(Inference is always delegated.)*
- **NG2.** Tau is not a coding-specific tool.
- **NG3.** Tau is not a hosted service. *(No tau cloud. `--remote` builds
  are an option, not a dependency.)*
- **NG4.** Tau is not a package marketplace. *(Polyglot resolver +
  content-hashed lockfile; no walled registry; tau-native units via
  git URLs.)*
- **NG5.** Tau is not a general-purpose workflow engine. *(Clarification:
  tau executes workflow IR with capability-safe portability as its
  defining property; it does not compete with general orchestrators
  like Temporal/n8n on their breadth.)*
- **NG6.** Tau does not provide persistent agent memory in core.
  *(Context-manager v2 retrieval is backed by a contracted vector-store
  MCP — never built-in.)*
- **NG7.** Tau does not evaluate agent quality.
- **NG8.** Tau is not an AI safety harness.
- **NG9.** Tau does not manage identity, authentication, or credentials.
  *(The provider chain delegates to Vault / SPIFFE / cloud / device
  identity. tau resolves; operators choose the vault.)*
- **NG10.** Tau does not collect telemetry or training data.
- **NG11.** Tau is a developer tool, not an end-user tool.
- **NG12.** Tau is a runtime and a compiler, not a framework.

Adjacent ideas may belong in plugins or downstream projects (such as
`stature`, the opinionated coding pipeline planned as a separate
project).

---

## Sequencing principle

Engine (β) is the priority *outcome*. Framing (α) is the priority
*activity right now* because the engine cannot ship correctly without
it. Portability (γ) and distribution+DX (δ) ride β and are explicitly
ordered after it. The package manager is only as valuable as the engine
people want to run; the engine is only as portable as its IR allows;
the IR is only as honest as the framing makes it. Each phase exists to
enable the next, and skipping any inverts the philosophy.
