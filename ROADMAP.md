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

### Phase 0/1 — runnable runtime (complete)

| Phase | What shipped |
|---|---|
| **0 — bootstrap** | empty workspace + governance + CI + 5 foundational crates (`tau-domain`, `tau-ports`, `tau-pkg`, `tau-runtime`, `tau-cli`). 2026-04-24 → 2026-04-28. |
| **1 — runnable runtime** | plugin loading (process + MessagePack-RPC), 5 production plugins, capability override, transitive resolution, tool-args schema, streaming, `tau update`/`verify`/`uninstall`, REPL persistence, full sandbox stack (Linux landlock + seccomp + namespaces; macOS sandbox-exec; Windows AppContainer scaffold; container adapter; passthrough), multi-agent v1/v1.1, tau-workflow v1, Skills 1–6, lefthook deep-gate. 2026-04-30 → 2026-05-16. |

### Phase 2 — compiled-language foundation (partial)

| § | Sub-project | Status |
|---|---|---|
| A | `tau check` | shipped 2026-05-18 (PR #161) |
| B | Target-triple registry (Bazel-inspired 3-axis; 5 Available + 1 Reserved) | shipped 2026-05-19 (PR #190); ADR-0034 |
| C.2 | `tau build` (MVP producer) | shipped 2026-05-27 (PR #242) |
| C.2.1 | `tau build` flags (`--target`/`-o`/`--json`) | shipped 2026-05-28 (PR #251) |
| C.3 | `tau run --bundle` (MVP consumer) | shipped 2026-05-28 (PR #247) |
| E | `tau verify --bundle` (reproducibility) | shipped 2026-05-28 (PR #250) |
| D | Capability vocabulary forward-compatibility | shipped 2026-05-29 (PR #254) |
| C.1 | **Declarative workflow IR** | **not yet started — the foundational research bet** |
| F | Remote target backends | design-only |
| G | WASM target backend | design-only — **promoted to primary** under the new philosophy (see Phase γ) |

### Adjacent clusters

- **Tau-serve mode v1** — shipped 2026-05-18 (PR #143); JSON-RPC 2.0 over
  NDJSON stdio.
- **Logging upgrades** (A precursor + B / C / D / E / F) — complete (PRs
  #195, #196, #198, #221, #222, #224, #226); tracing layers + non-blocking
  writer + OTLP export.
- **Cap-forward-compat** (Phase 2 §D) — shipped 2026-05-29; capability
  vocabulary now versioned-forward.

### Load-bearing surface that must continue to work

Every phase below is additive on top of this. **The forward plan does not
break what works today.**

| Surface | Status | What depends on it |
|---|---|---|
| 5 production plugins (anthropic, ollama, openai, fs-read, shell) on the bespoke MessagePack-RPC protocol | working | every integration test, every doctest, every fixture |
| Sandbox: landlock+seccomp (Linux), sandbox-exec (macOS), AppContainer scaffold (Windows), container adapter, passthrough | working; 4 strict-tier adapters | 10+ layer4 e2e tests; every plugin spawn |
| Lockfile v6 (flat `packages` vec with nested optional plugin/skill) | stable since 2026-05-16 | `tau install`/`resolve`/`verify`/`update`/`uninstall`, bundle producer/consumer |
| CLI verbs: `install · resolve · run · chat · list · init · verify · update · uninstall · plugin · session · sandbox · workflow · skill · serve · check · build · run --bundle · verify --bundle · target list/show` | stable | user-facing contract; all docs reference these |
| Multi-agent v1.1 (`task.*` + `run.*` virtual tools; `agent.<kind>.spawn` recursive) | working | tau-workflow v1; orchestration tests |
| Tau-workflow v1 (`workflows/*.toml` linear pipelines, JSONL + `--resume`) | working | reference workflow patterns |
| Skills (manifest extension + install pipeline + discovery + runtime invocation + Anthropic interop + 3 reference packages) | working since 2026-05-16 | the proto-distribution model Phase δ extends |
| Target-triple registry (Bazel-inspired; 5 Available + 1 Reserved) | stable since 2026-05-19 | `tau build --target`, `tau check --target` |
| Bundle format + `build` / `run --bundle` / `verify --bundle` | stable since 2026-05-28 | Phase β.2 lowers into this format |

The 2026-05-29 strategic pivot, recorded in
[`docs/explanation/tau-philosophy.md`](docs/explanation/tau-philosophy.md),
reframes the **remaining** work around three convictions — tau is a
*compiler*, a *harness everywhere*, *capability-safe by construction* —
without invalidating any of the above. The forward phases (α → β → γ → δ)
execute that reframing while preserving every shipped contract.

---

## Migration strategy: wrap, don't replace

The philosophy says external tools should go through MCP and native tools
should be in-process. Today's runtime uses a bespoke process+stdio
protocol for **all** plugins. We do not flag-day this.

```
COEXISTENCE — three lanes running simultaneously
=====================================================================
  LANE 1 — existing bespoke plugins (legacy compat layer; preserved)
    5 in-tree plugins continue to load via the existing plugin_host.
    The bespoke protocol becomes the LEGACY tier; no new plugins on it.

  LANE 2 — MCP facilitator (new; β.3)
    All NEW external tools go through MCP from day one. Capability
    gating at the contract boundary.

  LANE 3 — native tools (new; β.2 + β.3)
    Compiled-in tools registered into the trait-object registry by
    a per-target static builder. No process, no protocol, direct call.

  MIGRATION PATH — per-plugin, deliberate, NEVER a hard cut
    fs-read   → native tool                (eventually)
    shell     → native tool                (eventually)
    anthropic → first-class LlmBackend impl OR external MCP server
    ollama    → first-class LlmBackend impl OR external MCP server
    openai    → first-class LlmBackend impl OR external MCP server
    Each moves when its replacement lands; bespoke protocol shrinks
    by attrition. Deprecation notice when 0 in-tree plugins remain.
=====================================================================
```

The Phase β sub-projects below each note which lane they target and what
they preserve. The user-facing CLI verbs stay stable; what changes is
behind them.

---

## Phase α — Pre-engine framing (NOW)

**Goal:** before any engine-core implementation begins, scope the three
load-bearing risks the philosophy acknowledges. The engine cannot ship
correctly if any of these are left implicit.

**Status:** scoping documents written; downstream design specs are the
deliverable shape.

| # | Framing | Scoping doc | Required output |
|---|---|---|---|
| α.1 | **D — Workflow IR** | [framing-d-workflow-ir](docs/superpowers/specs/2026-05-29-framing-d-workflow-ir.md) | Design spec + ADR settling D-1 through D-7. Must show how the IR relates to today's `tau.toml` schema and `workflows/*.toml` v1 format (subsumes? superset? parallel?). |
| α.2 | **G — Polyglot resolver** | [framing-g-polyglot-resolver](docs/superpowers/specs/2026-05-29-framing-g-polyglot-resolver.md) | Design spec + ADR. Builds on the Skills 1–6 distribution pattern (already proven for skill bundles) and extends it to tau-native units. |
| α.3 | **C″ — MCU strategy** | [framing-c-prime-prime-mcu-strategy](docs/superpowers/specs/2026-05-29-framing-c-prime-prime-mcu-strategy.md) | Two MCU tiers added as `Reserved` in the existing target-triple registry (Phase 2 §B); `tau-runtime-core` extraction spec; ADR on the passthrough commitment. |

**Definition of done:** each framing produces a committed design spec, an
ADR where decisions warrant durability, and one concrete example
(minimal IR with its relation to today's manifest; an example tau-native
unit; two `Reserved` MCU triples in the registry).

**Until Phase α is complete, no Phase β code lands.**

---

## Phase β — Engine core (PRIORITY)

**Goal:** the portable, capability-safe agent + workflow engine. The
wedge. Even though it follows framing chronologically, it is the priority
*outcome*; framing exists to de-risk it, not to delay it. Every
sub-project below names what it builds on, preserves, adds, and
eventually supersedes.

### β.1 — `tau-runtime-core` extraction

- **Builds on:** existing `tau-runtime` (Phase 0 §4 + Phase 1 priorities
  1–12 + sandbox sub-projects 12-A through 12-J).
- **Preserves:** every existing `tau-runtime` test stays green; all 5
  plugins continue to load via the existing tokio host shell; no
  observable host-behavior change.
- **Adds:** `tau-runtime-core` (`no_std` + `alloc`, executor-agnostic,
  agent loop generic over `LlmBackend` / `Tool` / `Storage`). The three
  trait-object registries move into core. `tau-ports::Sandbox::wrap_spawn`
  and `apply_post_spawn` move behind a `std`/`process` feature so core
  compiles without `std`.
- **Supersedes:** nothing yet. The tokio shell (renamed
  `tau-runtime-tokio` if needed) continues to exist as the host driver.
  Future `tau-runtime-embassy` (Phase γ.5) is the MCU shell.
- **DoD:** existing test suite green; `cargo build -p tau-runtime-core
  --no-default-features` succeeds for `no_std`.

This is the prerequisite for every other β sub-project and for the MCU
tiers in γ. Pure refactor; zero user-visible change.

### β.2 — Workflow IR implementation (per Framing D)

- **Builds on:** the existing `tau.toml` schema (project config + agent
  declarations + capability overrides), the target-triple registry
  (Phase 2 §B), the bundle format (Phase 2 §C.2/C.3/E), and the
  cap-forward-compat machinery (Phase 2 §D).
- **Preserves:** `tau.toml` and lockfile v6 stay the source-of-truth
  for projects. The IR is what's *emitted from* the manifest by the
  compiler, not a competing format. Existing `tau install` / `resolve`
  / `run` paths unchanged. Existing bundles (Phase 2 §C.2/C.3) continue
  to load.
- **Adds:** a versioned IR (per α.1 decisions) + lowering pipeline. The
  bundle format gains an "IR payload" section; the consumer
  (`run --bundle`) gains an IR-execution path alongside the current
  manifest-execution path.
- **Supersedes:** the **tau-workflow v1** linear-pipeline format
  (`workflows/*.toml`) graduates into the IR as a degenerate case. v1
  stays available during transition; new workflows author via the IR.
- **DoD:** round-trip determinism (`tau build` → re-`build` → identical
  bytes, per the C3 contract); the existing Phase 2 §C.2/C.3 bundle
  tests still pass; one minimal IR-authored workflow runs end-to-end.

### β.3 — MCP facilitator (Lane 2)

- **Builds on:** existing `plugin_host` (Phase 1 §1), the sandbox stack
  (Phase 1 §12 + 12-A through 12-J), the capability gate (Phase 1 §4 +
  §12-B's `sandbox_check`).
- **Preserves:** all 5 in-tree plugins continue to load via the existing
  bespoke protocol (Lane 1). Their declared capabilities continue to
  flow into the sandbox unchanged. Their integration tests stay green.
- **Adds:** an MCP host runtime (built-in handlers for `tools/call`,
  `resources/read`, `sampling` → routes to delegated inference, `roots`
  → routes to capability gate, `elicitation`, `prompts/get`,
  `notifications`, `cancellation`). Per-handler capability gating at the
  contract boundary. New external tools (Lane 2) and native tools
  (Lane 3) go through the new paths.
- **Supersedes:** the bespoke protocol, **per-plugin**, only when each
  plugin's replacement is in place. No hard cut. Deprecation notice goes
  out when 0 in-tree plugins remain on the bespoke path.
- **DoD:** an external MCP server (e.g. an off-the-shelf weather server)
  is contracted by an agent and the call round-trips; its capability
  declaration is enforced; the 5 in-tree plugins still load and run
  unchanged.

### β.4 — Context manager primitive

- **Builds on:** existing message/turn types in `tau-domain` (Phase 0
  §1); current ad-hoc context handling in `tau-runtime::stream`.
- **Preserves:** existing `tau chat` / `tau run` / streaming behavior is
  unchanged for agents that **don't** declare a context block.
  Backward-compatible by absence.
- **Adds:** opt-in `[agent.<id>.context]` block in `tau.toml`. Stateful
  manager (Shape 1) + per-turn pipeline of pure transformers (Shape 2).
  v1 transformers: `trim_old`, `compact_tool_outputs`,
  `summarize_oldest` (uses any registered `LlmBackend`, typically a
  cheap model), `fit_budget` (always last).
- **Supersedes:** the implicit "throw the whole history at every turn"
  default once the new context block is opt-in across in-tree fixtures.
- **DoD:** an agent with a declared context block round-trips under
  `tau dev` and inside a wasm bundle, hitting the budget; agents
  without it behave identically to today.

### β.5 — Credential provider chain

- **Builds on:** existing env-var / config-file credential handling in
  the 5 plugins (each implements its own loader today).
- **Preserves:** today's `ANTHROPIC_API_KEY` / `OLLAMA_HOST` / `OPENAI_*`
  paths continue to work as the default `Env` provider — no breakage
  for existing users.
- **Adds:** a Strategy + Chain port (`tau-ports::CredentialProvider`)
  + standard providers (`Baked`, `Env`, `File`, `SecretManager` with
  Vault / AWS / GCP / Azure adapters, `WorkloadIdentity` for SPIFFE /
  IRSA, `DeviceIdentity` for per-device secure-element keys,
  `TokenBroker` for OIDC / OAuth2 BFF). Deployment configures the order.
- **Supersedes:** per-plugin ad-hoc credential code, eventually — each
  plugin migrates to declaring "I need credentials of kind X" and letting
  the chain resolve.
- **DoD:** at least one provider beyond `Env` ships and is exercised by
  CI (likely `File` mounted-secret); existing plugin credential paths
  unchanged.

### β.6 — Cross-target conformance gate

- **Builds on:** existing test infrastructure — cassette replay
  (Phase 1 §2c), fixtures (`tau-plugin-test-support`), the
  `plugin_compat` driver (Phase 1 §12-D), Layer 4 tests, the
  `verify --bundle` reproducibility check (Phase 2 §E).
- **Preserves:** every existing test continues to gate.
- **Adds:** a profile-agnostic scenario runner that exercises both the
  interpreted dev profile and the compiled wasm artifact against the
  same scenarios, demanding identical observable behavior. This is the
  behavioral sibling of `verify --bundle`'s byte-level check.
- **Supersedes:** nothing — pure addition.
- **DoD:** one canonical scenario (the one defined in β success
  criterion below) runs under both profiles with bit-identical event
  streams.

### β.7 — Dev / release one-engine discipline

- **Builds on:** existing `tau run` / `tau chat` / `tau serve` (dev-side
  surface) and `tau build` / `tau run --bundle` (release-side surface).
- **Preserves:** every existing CLI verb continues to do what it does
  today. `tau dev` is **new**; nothing existing is renamed or removed.
- **Adds:** `tau dev` — a hot-reload host shell driving
  `tau-runtime-core` directly, with user tools as callbacks. The new
  zero-toolchain on-ramp.
- **Supersedes:** nothing.
- **DoD:** `tau dev <project>` boots in under a second; editing a tool
  hot-reloads; the same project lowers cleanly via `tau build wasm`.

### Phase β success criterion

A user declares a workflow with one agent, one native tool, one MCP
contract, one context pipeline; runs it instantly under `tau dev`;
builds it to a wasm component via `tau build --target wasm`; runs the
component in wasmtime with the declared capabilities enforced; and the
conformance gate (β.6) proves the two profiles agree.

**Simultaneously:** every existing fixture, plugin, and integration test
in the repository continues to pass — including all 5 plugins on the
bespoke protocol, all sandbox e2e tests, all Phase 2 bundle tests, all
Skills tests, all tau-workflow v1 tests.

---

## Phase γ — Portability targets

**Goal:** extend the engine across target triples beyond the wasm
component baseline established at the end of β.

Each target slots into the existing target-triple registry (Phase 2 §B)
as `Reserved` first, then graduates to `Available` when its CI lane is
green. The registry's stability discipline (ADR-0034) applies throughout.

| # | Target | Builds on | Preserves | Adds |
|---|---|---|---|---|
| γ.1 | wasm component on **server / edge** (Spin / wasmtime) | β.6/β.7 baseline; existing `tau build wasm` target slot | existing bundle format; existing `run --bundle` | hardening for Spin / wasmtime hosts; CI matrix lane |
| γ.2 | wasm component in **browser** (Angular/React via jco) | γ.1; β.5 `TokenBroker` provider | the BFF + AI Gateway credential pattern | jco / wasm-bindgen integration; browser-host scenarios |
| γ.3 | **C-ABI library** (`libtauflow.a` + cbindgen header) | β.1 core; existing `tau-app` scaffold | nothing — net new artifact shape | passthrough-only enforcement (advisory) honestly labeled |
| γ.4 | **`wasm-mcu-preview1-tau-managed`** | β.1 core; existing target registry | reserved-slot existence (per α.3) | WAMR Preview-1 host integration; ESP32-S3 reference board |
| γ.5 | **`bare-metal-xtensa-passthrough`** + siblings | β.1 core; `tau-runtime-embassy` shell (new) | reserved-slot existence | embassy executor shell; static-tool builder; firmware-image artifact rule |
| γ.6 | Context-manager v2: **`retrieve_relevant`** | β.4 pipeline | v1 transformers unchanged | new transformer backed by a contracted vector-store MCP (e.g. pgvector / qdrant) |

The `wasi-p2-component` target on MCU (the "wasm component on
microcontroller") is **deferred** to a future phase until the runtime
ecosystem catches up. Tracked via Framing C″.

Phase 2 §F (remote target backends) folds into Phase γ as additional
`Sandbox` + `LlmBackend` adapters as user demand surfaces (Vercel
Sandbox, Sandcastle); no commitment date.

---

## Phase δ — Distribution + DX

**Goal:** make tau pleasant to author against and to share work through,
once the engine is solid. Sequenced last on purpose: the package
manager is only as valuable as the engine people want to run.

### δ.1 — Polyglot resolver Phase 1 (per Framing G)

- **Builds on:** **Skills 1–6 distribution pattern** (already proven
  for skill bundles: manifest extension → install pipeline → discovery
  → runtime invocation → interop → reference packages). Phase δ.1
  extends this proven pattern to other tau-native unit kinds.
- **Preserves:** Skills distribution unchanged; lockfile v6 → v7 is
  additive (new entry kind, not a rewrite).
- **Adds:** `tau add <git-url>` for tau-native units (agent templates,
  workflow templates, capability profiles, context-pipeline presets).
  Git-pinned, content-hashed, capability-audited at install.
- **No tau-operated registry.** Cross-ecosystem solving (cargo + npm
  + MCP semantics) is *explicitly out of scope* per Framing G —
  delegate to each ecosystem's resolver.

### δ.2 — TypeScript sugar layer

- **Builds on:** the IR shipped in β.2.
- **Preserves:** TOML manifest authoring stays first-class. TS is sugar,
  not a replacement.
- **Adds:** `@tau/sdk` TS package emitting the IR. Type-checked
  authoring, autocomplete, IDE integration. Sugar over the canonical IR,
  not a parallel runtime. Same IR runs under `tau dev` and lowers via
  `tau build`.

### δ.3 — Progressive-disclosure polish

- **Builds on:** β.7's `tau dev`; existing `tau build` (Phase 2 §C.2 +
  §C.2.1).
- **Adds:** `tau build` default for cross-target work is `--container`
  (the Rust `cross` model; only podman required, no rustup-targets).
  `--remote` deferred to later as a hosted convenience, **not** a Phase
  δ commitment.

### δ.4 — Reference templates

- **Builds on:** the Skills-6 reference-packages pattern.
- **Adds:** three to four tau-native units (analogue of the
  `critic` / `fact-checker` / `pr-reviewer` skill triad)
  demonstrating agent-template and workflow-template shapes,
  distributable via git. Sets the pattern for community contribution
  without operating infrastructure.

---

## Out of scope (forever)

The constitutional non-goals from
[`CONSTITUTION.md` §2](CONSTITUTION.md) remain in force. The philosophy
sharpens, but does not relax, any of them.

- **NG1.** Tau is not an LLM or an agent. *(Inference is always
  delegated.)*
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
ordered after it. **Every phase preserves the load-bearing surface
shipped through Phase 1 and the completed parts of Phase 2.** No
flag-days; no rewrites of working subsystems; every new mechanism enters
as a parallel lane and the old mechanism deprecates by attrition. The
package manager is only as valuable as the engine people want to run;
the engine is only as portable as its IR allows; the IR is only as
honest as the framing makes it; the whole thing only works if it doesn't
break what's already shipped.
