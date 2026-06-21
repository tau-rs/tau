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
  writer + OTLP export. **This stack is load-bearing for every phase
  below**: spans + events are how the conformance gate (β.6) and the
  scenario suite observe behavior, and OTLP export is how production
  deployments will see what bundles do.
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

## What the philosophy pivot deliberately killed

These were considered during the brainstorm and explicitly declined. They
remain killed unless re-argued; surfaced here so the boundaries don't
get redrawn implicitly.

- **Competing with Vercel AI SDK for the easy server-agent SDK market.**
  Crowded, not differentiated, and would dilute the wedge. tau is for
  workflows that need to *be* a portable artifact.
- **Authoring agents/workflows in Python or TypeScript as canonical
  source.** Inverts the compiler thesis. TS sugar is supported (β.8 +
  δ.2), but emits the IR; it does not become the source.
- **A tau-operated package registry / marketplace.** Operationally
  expensive, cold-start hostile, and unnecessary — git URLs +
  content-hashed lockfile + capability audit (the Go-modules pattern,
  extending Skills 1–6 distribution) cover it.
- **A cross-ecosystem version solver** (unifying cargo + npm + MCP
  semantics). Years of work; no immediate user value. Delegate to each
  ecosystem's resolver. Framed in
  [framing-g-polyglot-resolver](docs/superpowers/specs/2026-05-29-framing-g-polyglot-resolver.md).
- **On-device LLM inference on MCU.** Even Espressif's ESP-Claw (the
  one named precedent for agents on ESP32) delegates inference to a
  cloud model. The harness can run on MCU; the model can't.
- **WASM Component Model on microcontrollers as a Phase 1/2
  commitment.** The runtime ecosystem isn't there (WAMR is still on
  Preview 1). MCU ships via Preview-1 wasm OR native firmware until the
  component-model runtime catches up. Framed in
  [framing-c-prime-prime-mcu-strategy](docs/superpowers/specs/2026-05-29-framing-c-prime-prime-mcu-strategy.md).
- **A bespoke wire protocol for external tools.** Replaced by MCP for
  every new external tool from β.3 onward; the existing bespoke protocol
  is preserved for in-tree plugins as legacy compat only (see migration
  strategy below).
- **A bundled vector database** for context-manager retrieval. Vector
  stores are contracted MCP servers (γ.6), never built-in.
- **Operating a tau cloud / hosted service.** `--remote` builds are
  optional convenience, not a tau-operated infrastructure commitment.

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

### Wrapper crate + lifecycle

The legacy bespoke loader lives at `crates/tau-plugin-legacy/` (renamed
from / extracted out of `tau-runtime::plugin_host`), with a documented
"this crate exists to keep in-tree plugins working during migration; do
not add new code paths here" header. Its public surface is **frozen**
at the migration start — no new features land in legacy; only bug fixes.

### Per-plugin migration triggers

A bespoke-protocol plugin migrates when its replacement is ready:

| plugin | replacement | trigger |
|---|---|---|
| `fs-read` | native tool (compiled-in) | wasm-component build target stable (β.7.5) |
| `shell` | native tool (compiled-in) | same as `fs-read` |
| `anthropic` | first-class `LlmBackend` impl in `tau-runtime-core`, OR contracted MCP server if a maintained one ships | β.5 credential chain land + in-tree LlmBackend extraction |
| `ollama` | same | same |
| `openai` | same | same |

### Exit criteria for Lane 1 (legacy bespoke compat)

Lane 1 closes when **all five** of the following hold:

1. Zero in-tree plugins remain on the bespoke protocol.
2. No external user-reported workflow requires it (6-month observation
   window from "0 in-tree plugins" milestone).
3. `tau install` of an external bespoke plugin emits a deprecation
   warning for two consecutive minor releases before refusal.
4. `tau-plugin-legacy` is feature-gated `legacy` (off by default) for
   one minor release before removal.
5. A retrospective ADR records the closure with the final user-impact
   audit.

Expected timing: end of Phase γ at the earliest; possibly into Phase δ.
**Not a Phase β concern.** Phase β only ensures the legacy lane keeps
working unchanged; it does not depend on its closure.

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

### Phase α sizing — what each framing actually spawns

The three framings are **not just three documents**; each spawns
downstream artifacts whose count is part of the framing cost.

| Framing | Spec | ADR | Other artifacts | Estimated |
|---|---|---|---|---|
| α.1 D | 1 design spec (`<date>-workflow-ir-design.md`) | 1 ADR (`0035-workflow-ir.md`) | minimal IR example in-tree (1 toml + 1 .ir snapshot); update `tau-as-language.md` "Status" section | ~2 weeks |
| α.2 G | 1 design spec (`<date>-tau-native-units-design.md`) | 1 ADR (`0036-tau-native-units.md`); possibly 1 lockfile-schema-bump ADR if v7 is needed | 1 example tau-native unit + 1 reference template in-tree | ~1.5 weeks |
| α.3 C″ | 1 design spec (`<date>-tau-runtime-core-design.md`) | 1 ADR (`0037-mcu-passthrough.md`); 1 target-registry update for the two new Reserved triples | extraction sketch (file-by-file plan for β.1); CARGO_TARGET_DIR additions if any | ~2 weeks |

**Total Phase α cost:** ~5–6 weeks of design work, 3 design specs, 3–5
ADRs, ~4 in-tree examples. This is the real entry cost before Phase β
implementation begins; the user-facing roadmap should be honest about it.

### Phase α risks

- **The IR design surfaces unknown-unknowns.** No prior art means the
  first IR will iterate. Mitigation: minimal IR (Agent + Tool nodes only,
  per D-1's recommended option); extend deliberately, not preemptively.
- **The framing docs invite scope creep.** Users will ask "can we also
  decide X in framing G?" — every "yes" delays β. Mitigation: each
  framing doc has an explicit "out of scope" section; defer with a
  pointer.
- **The MCU framing locks in a passthrough commitment.** If WAMR ships
  the component model on MCU during Phase γ, the commitment looks
  conservative in hindsight. Acceptable; the ADR is reversible.

**Until Phase α is complete, no Phase β code lands.**

---

## Phase β — Engine core (PRIORITY)

**Goal:** the portable, capability-safe agent + workflow engine. The
wedge. Even though it follows framing chronologically, it is the priority
*outcome*; framing exists to de-risk it, not to delay it. Every
sub-project below names what it builds on, preserves, adds, and
eventually supersedes.

### Phase β sequencing (the DAG)

```
β SEQUENCING — what gates what, what runs in parallel
=====================================================================

  β.1  tau-runtime-core extraction  ◀── gates everything below
       (no_std + alloc, executor-agnostic, registries moved into core)
        │
        ├──────────────┬──────────────┬──────────────┐
        ▼              ▼              ▼              ▼
      β.2            β.3            β.4            β.5
   Workflow IR    MCP facilitator  Context mgr   Credential chain
   (per α.1 D)   (Lane 2: new MCP) (opt-in;     (independent;
                                   default off) parallel-safe)
        │              │              │              │
        └──────┬───────┴──────┬───────┘              │
               ▼              ▼                      │
             β.7            β.8                      │
       dev/release      TS minimal authoring         │
       one engine       surface (sugar over IR)      │
       (`tau dev`)         │                         │
               │           │                         │
               └─────┬─────┘                         │
                     ▼                               │
                   β.6                               │
            cross-target conformance gate ◀──────────┘
                     │
                     ▼
            Phase β success criterion
            (the canonical vertical-slice scenario)
=====================================================================
  legend:  ─── strict dependency       ┄┄┄ parallel-safe
  β.1 BLOCKS all others.
  β.2/β.3/β.4/β.5 can fan out (4-way parallel) after β.1 lands.
  β.7 needs β.1 + β.2 + β.3.   β.8 needs β.2 + β.7.
  β.6 needs β.7 + β.8 (something to exercise).
```

**Critical-path serialization:** β.1 → β.2 → β.7 → β.6. Everything else
is parallel-eligible. Right-sized for ~2 implementers: one on the
critical path, one on the parallel work (β.3 then β.4 then β.5 then β.8).

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
  --no-default-features` succeeds for `no_std`. Tracing spans + events
  shipped in the logging cluster (PRs #195–#226) **continue to fire
  unchanged** under the tokio shell; core preserves the `#[instrument]`
  attribute usage on host-relevant fns and feature-gates only the
  `tracing-subscriber` integration that std requires.

This is the prerequisite for every other β sub-project and for the MCU
tiers in γ. Pure refactor; zero user-visible change. The hardest part
is the `tau-ports` `no_std` sweep — every `std::collections::HashMap` →
`hashbrown` or `alloc::BTreeMap`, every implicit `std` use audited.

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

> Implementation status (2026-06-10): The workflow IR shipped in β.2 (PRs
> #263–#271). See ADR-0037 and the design spec. v0 uses partial-interpret
> lowering; AOT (wasm component artifact) lands in β.7.5. Conformance suite
> + `tau run --bundle` interpreter dispatch shipped in β.2.6.1/β.2.6.2.

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
- **Status (2026-06-14):** Shipped. Port + `CredentialChain` in `tau-ports`;
  Env/File/Baked providers; host resolve-then-inject bridge; per-agent
  declaration + scope-level chain config; `test (credential-chain / linux)`
  CI lane green. The five plugins are **unchanged** — the bridge injects
  resolved secrets into their existing env vars; per-plugin migration stays
  coupled to in-tree `LlmBackend` extraction.

### β.6 — Cross-target conformance gate

- **Builds on:** existing test infrastructure — cassette replay
  (Phase 1 §2c), fixtures (`tau-plugin-test-support`), the
  `plugin_compat` driver (Phase 1 §12-D), Layer 4 tests, the
  `verify --bundle` reproducibility check (Phase 2 §E), and the
  tracing-layer test-recorder pattern from logging §D (PR #226).
- **Preserves:** every existing test continues to gate.
- **Adds:** a profile-agnostic scenario runner — `tau-conformance` crate
  — that exercises both the interpreted dev profile and the compiled
  wasm artifact against the same scenarios, demanding identical
  observable behavior on the trace-event stream. This is the behavioral
  sibling of `verify --bundle`'s byte-level check.
- **Supersedes:** nothing — pure addition.
- **DoD:** the **canonical scenario** (defined below) runs under both
  profiles and produces a bit-identical sequence of `RunEvent`s — same
  count, same ordering, same payload modulo timestamps + IDs. Diff a
  single event → CI fails.

- **Status — scaffolding shipped (2026-06-15).** The `tau-conformance`
  crate, the canonical fan-monitor fixture, the `DevProfile` runner, the
  channel normalizer + ordered differ, and the `conformance / linux`
  Tier-1 CI lane are all live. The dev profile produces the documented
  bit-identical `ConformanceEvent` stream (golden-checked). Design spec:
  `docs/superpowers/specs/2026-06-14-beta-6-conformance-gate-design.md`;
  ADR-0048 (the dual-channel `ConformanceEvent` contract — the ROADMAP's
  illustrative event stream is a conceptual union of the typed `RunEvent`
  enum and the tracing vocabulary, not any single channel, so the gate
  sources from both and interleaves at the engine's generator yield
  barrier).
  - **`WasmProfile` is stubbed** (`unimplemented!`) and its assertion
    (`fan_monitor_dev_matches_wasm`) is `#[ignore]`d. The full β.6 DoD
    (both profiles agree) is **NOT yet met**.
  - **β.7.5 unstub follow-up (tracked):** implement `WasmProfile::run`
    against `tau build wasm`'s artifact (run in wasmtime, harvest the
    guest's `ConformanceEvent` stream across the component boundary) and
    flip `fan_monitor_dev_matches_wasm` from `#[ignore]` to live. The
    `ConformanceEvent` contract is frozen (`CONFORMANCE_EVENT_VERSION`)
    so β.7.5 only has to *produce* the stream, not *design* it.
  - **Known minor follow-ups:** (1) the MCP weather cassette hardcodes
    `clientInfo.version` to the workspace version (`0.0.0`); a version
    bump will break the strict-match handshake with a cryptic
    `no cassette entry matches "initialize"` — make the matcher
    version-agnostic for `clientInfo.version` when convenient. (2)
    `ToolOutcome::Ok` carries an `is_error` flag (a refinement over the
    spec's original "Err→canonical marker only" framing) so semantic
    tool failures (`Ok(ToolResult{is_error:true})`) are compared across
    profiles, not silently equated with success.

#### The canonical β.6 scenario (the "fan-monitor")

The one workflow every β change must keep green. Concrete enough to be
executable; small enough to be auditable.

```
PROJECT
  one agent     "fan-monitor"
  one native    read_temp   (compiled-in mock; deterministic reading)
  one native    set_fan     (compiled-in mock; records state)
  one MCP       weather     (cassette-replayed external server)
  one context   trim_old → compact_tool_outputs → fit_budget
  one model     claude-haiku-4-5 via cassette replay (deterministic)

PROMPT
  "Read the temperature. If above 30°C, check weather; if hot outside,
   keep fan on; otherwise off."

EXPECTED EVENT STREAM  (bit-identical across both profiles)
  RunStarted{run_id}
  ToolCallStarted{name="read_temp"}
  ToolCallCompleted{name="read_temp", result=32}
  ContextStepRan{step="trim_old"}
  ContextStepRan{step="compact_tool_outputs"}
  ContextStepRan{step="fit_budget", tokens_in=…, tokens_out=…}
  InferenceCallStarted
  InferenceCallCompleted
  ToolCallStarted{name="weather"}
  ToolCallCompleted{name="weather", result=…}
  …
  ToolCallStarted{name="set_fan", args={"on": true}}
  ToolCallCompleted{name="set_fan"}
  RunCompleted{outcome=Success}
```

This scenario doubles as **the Phase β success criterion** (see below):
shipping β means this scenario runs under `tau dev` and as a wasm
component in wasmtime, and the conformance gate confirms agreement.

### β.7 — `tau dev` one-engine REPL

- **Builds on:** β.1 (`tau-runtime-core`), β.2 (workflow IR + `run_via_ir`),
  β.3 (MCP facilitator + `McpBridge`).
- **Preserves:** every existing CLI verb continues to do what it does
  today. `tau dev` is **new**; nothing existing is renamed or removed.
- **Adds:** `tau dev` — a hot-reload REPL driving the existing β.3 runtime
  path (`tau-runtime-tokio` + `McpBridge` + `run_via_ir`) with a stdin loop
  and a notify-driven file watcher. REPL semantics: explicit `:reload` by
  default, `--watch` opts into Mastra-style auto-reload, `-p "<prompt>"` for
  one-shot.
- **Supersedes:** nothing.
- **DoD:** `tau dev <project>` boots in under 1s; editing the manifest
  hot-reloads via `:reload` while preserving conversation history; the
  simplified-fan-monitor smoke runs end-to-end.

Design: `docs/superpowers/specs/2026-06-10-beta-7-tau-dev-design.md`
ADR: 0040 — records the REPL-explicit-reload over Mastra-style-auto-reload
decision + the β.7/β.7.5 split rationale.

### β.7.5 — IR-to-wasm AOT compiler (split out from β.7, 2026-06-10)

- **Builds on:** β.2 (workflow IR), β.7 (REPL gives us a working dev/host
  path to test against).
- **Preserves:** `tau dev` unchanged. `tau build wasm` is the new artifact
  path.
- **Adds:** ahead-of-time lowering of the workflow IR + `tau-runtime-core` +
  linked native tools to a runnable wasm component (WASI 0.2). The artifact
  runs in wasmtime; γ.1 extends to Spin + browser hosts.
- **DoD:** `tau build wasm <project>` produces a wasm component that executes
  the simplified-fan-monitor scenario in wasmtime and returns a
  `ConformanceReport` equal to `tau dev`'s (dev↔wasm parity via
  `assert_conform`, the D-7a multiset observable). A literal byte-identical
  `RunEvent` stream is deferred to β.6, where the cross-target conformance
  gate lives (see ADR-0048 Decision 2).
- **Sized:** ~4–8 weeks. Wasm component model integration is the hard part.

*(This sub-project was originally folded into β.7 via the β.2 footnote
"AOT lands in β.7"; split out 2026-06-10 because wasm AOT complexity
ballooned after β.3 PR-6 expanded the MCP surface — the in-wasm
MCP-facilitator path deserves its own ADR and conformance scope.)*

> Implementation status (2026-06-16): PR-1 (`any-wasi-strict` triple +
> `tau build wasm` skeleton) landed (#350); spec amended (SkillResolver port
> + single-channel observable); PR sequence A/B/C–G. ADR-0046 (Proposed);
> ADR-0049 (Accepted — single-channel typed conformance observable,
> supersedes ADR-0048 Decision 1); in-wasm MCP-facilitator ADR shifts to
> 0050 (forthcoming, PR-F).

### β.8 — TypeScript minimal authoring surface

The philosophy argues for Vercel-DX-like authoring, and deferring TS
entirely to δ.2 would contradict that. β.8 lands the **minimal** TS
surface needed for an authoring-quality experience; δ.2 polishes it
into a publishable SDK.

- **Builds on:** β.2 (the IR) + β.7 (`tau dev`).
- **Preserves:** TOML manifest authoring stays first-class. β.7's REPL
  behavior is identical regardless of project format.
- **Adds:** `@tau/sdk` package shape — `agent({...})`, `tool({...})`,
  `mcp({...})` factory functions accepting object literals matching the
  TOML schema 1:1 (snake_case fields, no name-mapping layer).
  `tau dev project.ts` parses via swc + statically analyzes the AST +
  emits the same `ProjectConfig` the TOML loader produces.
  `contextManager({...})` factory **exists** but rejects at parse time
  pending β.4. **One way to write a project** (TOML or TS, your
  choice), one IR underneath.
- **Supersedes:** nothing.
- **DoD:** the canonical β.6 scenario authored in either TOML *or* TS
  produces a byte-equal IR after canonical encoding (verified by the
  TOML↔TS conformance test).
- **Design:** `docs/superpowers/specs/2026-06-10-beta-8-ts-authoring-design.md`
- **ADR:** 0041 (records the declarations-only-no-embedded-JS decision)

Out of scope for β.8 v1:
- Inline TS tool bodies (`run: async () => ...`) — δ.2 adds runtime JS
  execution via QuickJS embed
- Multi-file TS imports (`from "./helpers"`) — v1.1
- npm publishing pipeline, TS type generation from skill schemas,
  browser-side runtime, full editor plugin polish — δ.2
- `contextManager` factory implementation — β.4 prerequisite

### Phase β success criterion

**The canonical β.6 fan-monitor scenario (above) runs end-to-end under
both profiles — `tau dev` and the wasm component in wasmtime — with
identical event streams, identical capability enforcement, and the
conformance gate green.**

**Simultaneously:** every existing fixture, plugin, and integration test
in the repository continues to pass — including all 5 plugins on the
bespoke protocol, all sandbox e2e tests, all Phase 2 bundle tests, all
Skills tests, all tau-workflow v1 tests.

### Phase β risks

- **Migration breakage.** The wrap-not-replace strategy assumes legacy
  plugins keep working through β.1's extraction. If `tau-runtime-core`
  changes a trait signature subtly, plugins built against the old
  `tau-plugin-sdk` may break at the bespoke-protocol boundary.
  Mitigation: `tau-plugin-sdk` is part of the legacy compat surface;
  freeze its v1 ABI; SDK v2 is a separate, opt-in upgrade for plugins
  that want to migrate.
- **IR debt.** The minimal IR (Agent + Tool only) almost certainly
  misses something the second-real-workflow needs. Mitigation: ship the
  minimal IR; observe what's needed for the second scenario; extend
  with an ADR rather than improvising.
- **Conformance gate flakiness.** Wasm-execution timing nondeterminism
  could make the "bit-identical event stream" claim flaky. Mitigation:
  the event stream is normalized (timestamps stripped, IDs canonicalized)
  before diff; only causal-order divergence fails.
- **Performance cliff.** The wasm path may be substantially slower than
  the dev path. Mitigation: this is acceptable at β; performance work is
  a γ concern. Document the gap; don't hide it.

---

## Phase γ — Portability targets

**Goal:** extend the engine across target triples beyond the wasm
component baseline established at the end of β.

Each target slots into the existing target-triple registry (Phase 2 §B)
as `Reserved` first, then graduates to `Available` when its CI lane is
green. The registry's stability discipline (ADR-0034) applies throughout.

| # | Target | Builds on | Preserves | Adds |
|---|---|---|---|---|
| γ.1 | wasm component on **server / edge** (Spin / wasmtime) | β.6/β.7/β.7.5 baseline; existing `tau build wasm` target slot | existing bundle format; existing `run --bundle` | hardening for Spin / wasmtime hosts; CI matrix lane |
| γ.2 | wasm component in **browser** (Angular/React via jco) | γ.1; β.5 `TokenBroker` provider | the BFF + AI Gateway credential pattern | jco / wasm-bindgen integration; browser-host scenarios |
| γ.3 | **C-ABI library** (`libtauflow.a` + cbindgen header) | β.1 core; existing `tau-app` scaffold | nothing — net new artifact shape | passthrough-only enforcement (advisory) honestly labeled |
| γ.4 | **`wasm-mcu-preview1-tau-managed`** | β.1 core; existing target registry | reserved-slot existence (per α.3) | WAMR Preview-1 host integration; ESP32-S3 reference board |
| γ.5a | **`tau-runtime-embassy`** shell | β.1 core | n/a (new crate) | embassy executor shell; the MCU-side counterpart to `tau-runtime-tokio` |
| γ.5b | **`bare-metal-xtensa-passthrough`** + siblings | γ.5a (`tau-runtime-embassy`); β.1 core | reserved-slot existence | static-tool builder; firmware-image artifact rule (a new `tau build` artifact shape); `reqwless` + `embedded-tls` `LlmBackend` impl |
| γ.6 | Context-manager v2: **`retrieve_relevant`** | β.4 pipeline | v1 transformers unchanged | new transformer backed by a contracted vector-store MCP (e.g. pgvector / qdrant) |

**γ.5 is two sub-projects, not one.** The embassy shell (γ.5a) is the
expensive part — async runtime swap, `no_std` dependency tree audit,
plumbing for `reqwless`/`embedded-tls`. It must land before any actual
firmware target (γ.5b) can produce a real artifact. Sized realistically:
γ.5a ~6–8 weeks (one implementer with embassy/HAL familiarity);
γ.5b ~3–4 weeks per CPU triple after γ.5a.

The `wasi-p2-component` target on MCU (the "wasm component on
microcontroller") is **deferred** to a future phase until the runtime
ecosystem catches up. Tracked via Framing C″.

Phase 2 §F (remote target backends) folds into Phase γ as additional
`Sandbox` + `LlmBackend` adapters as user demand surfaces (Vercel
Sandbox, Sandcastle); no commitment date.

### Phase γ risks

- **Wasm-host fragmentation.** Spin / wasmtime / wasmCloud /
  browser-jco implementations of WASI 0.2 diverge in capability-
  enforcement details. A bundle that runs in one may behave differently
  in another. Mitigation: the conformance gate (β.6) lists supported
  hosts; we test against the listed ones; others are best-effort.
- **WAMR Preview-1 obsolescence.** If WAMR ships the Component Model
  during γ, γ.4 is partially redundant — we built a Preview-1 path that
  we'd then want to graduate. Acceptable: the Component Model graduation
  is additive; Preview-1 stays available for legacy boards.
- **Embedded supply chain.** ESP32 toolchains and HAL crates change
  faster than the Rust release cadence; bit-rot risk is real.
  Mitigation: γ.5b targets pin specific HAL versions; renovate.
- **Browser wasm size budget.** wasm components carry the runtime; size
  matters for browser shipping. No commitment yet; γ.2 includes a
  measurement task and a public number.

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

### Phase δ risks

- **Adoption funnel.** The TS sugar (δ.2) and reference templates (δ.4)
  are the primary on-ramp for new users. If they ship rough, growth
  stalls regardless of engine quality. Mitigation: δ.2/δ.4 land together
  with a documentation pass; the canonical β.6 scenario is the first
  reference template.
- **Resolver scope creep.** Users will ask "can `tau add` resolve npm
  too?" and so on. Saying yes inverts Framing G. Mitigation: point at
  Framing G's explicit out-of-scope list; user-extension is "import a
  package in your native-tool Rust code via cargo, declare the
  capability in tau.toml."
- **TS-sugar drift from IR.** δ.2 polish may add ergonomic shortcuts
  that the TOML manifest can't express, fragmenting the source surface.
  Mitigation: the conformance gate (β.6) is also a TOML↔TS round-trip
  test (TS-emitted IR must equal TOML-emitted IR for shared scenarios).

---

## CI lanes added by phase

Each phase grows the required-check matrix. Sized realistically so the
checks land alongside the code, not deferred:

| Phase | New CI lanes | Rationale |
|---|---|---|
| α | none (specs only) | framing is design work |
| β.1 | `check (tau-runtime-core no-default-features / linux)` — proves `no_std` compiles | gate the extraction's contract |
| β.2 | `test (tau-workflow-ir / linux)` — IR round-trip determinism | gates the IR commitment |
| β.3 | `test (mcp-facilitator / linux)` — facilitator + capability gating | external MCP cassette tests |
| β.5 | `test (credential-chain / linux)` — chain resolution + at least one non-Env provider | proves the chain isn't `Env`-only |
| β.6 | `conformance (linux)` — the canonical β.6 scenario, both profiles, event-stream diff | THE gate that proves the philosophy holds |
| β.7 | `test (tau-dev hot-reload / linux)` | smoke for the new dev shell |
| β.8 | `test (ts-sugar emit-ir / linux)` | TS↔TOML round-trip |
| γ.1 | `conformance (wasmtime / linux)` + `conformance (spin / linux)` | host-specific divergence detection |
| γ.2 | `conformance (browser-jco / linux)` | browser host behavior |
| γ.3 | `test (c-abi-lib / linux + macos)` | cbindgen header roundtrip |
| γ.4 | `check (wasm32-wasip1 cross / linux)` (no hardware) | cross-compile drift |
| γ.5a | `check (no_std embassy / linux)` + `check (xtensa-esp32-none-elf cross / linux)` | embassy shell + MCU cross |
| γ.5b | manual hardware verification on canonical ESP32-S3 board before triple → `Available` | per Framing C″ §C″-7 |
| γ.6 | `test (retrieve-relevant / linux)` w/ pgvector cassette | retrieval transformer |
| δ.1 | `test (tau-native-units git-resolve / linux)` | resolver Phase 1 |
| δ.2 | `test (ts-sugar full / linux)` + npm publish dry-run | SDK polish |

**Phase end-state check counts (rough):**
- end of α: 14 (unchanged from today)
- end of β: ~20 required checks
- end of γ: ~28 required checks (with manual hardware sign-off for γ.5b)
- end of δ: ~30 required checks

These additions follow the discipline established in Phase 1 §12-E
(CI optimization, ADR-0018): each new lane runs from prebuilt
fixture artifacts where possible; per-target dirs prevent
lock contention; sccache caching is preserved.

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
  like Temporal/n8n on their breadth. Durability — when and whether to
  re-run — is delegated to the host orchestrator; tau guarantees the
  compiled bundle is a safe-to-retry reentrant unit. See
  [Run tau under a durable orchestrator](docs/how-to/run-tau-under-a-durable-orchestrator.md).)*
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

### One-page summary

> tau is in **Phase α** (framing the three load-bearing risks D, G, C″)
> as of 2026-05-29. The next deliverable phase is **Phase β** — the
> engine: extract `tau-runtime-core` (β.1, gates everything), then
> implement the workflow IR (β.2), MCP facilitator (β.3), context
> manager (β.4), credential chain (β.5), `tau dev` (β.7), and the TS
> authoring surface (β.8), validated by the cross-target conformance
> gate (β.6) running the canonical fan-monitor scenario under both
> profiles. **Phase γ** extends to wasm hosts (server / edge / browser),
> C-ABI, and MCU tiers (the embassy shell is its own sub-project,
> γ.5a). **Phase δ** lands the polyglot resolver, the polished TS SDK,
> and reference templates. Throughout: legacy bespoke plugins keep
> working in Lane 1, new external tools go through MCP (Lane 2), new
> local tools compile in (Lane 3); Lane 1 closes by attrition no
> earlier than Phase γ. Every shipped CLI verb, plugin, sandbox
> adapter, lockfile version, bundle artifact, and skill stays working
> throughout. **The philosophy holds only if it doesn't break what
> shipped.**
