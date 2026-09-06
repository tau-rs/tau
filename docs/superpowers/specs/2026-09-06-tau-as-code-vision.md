# tau as code — how developers meet tau: in-code authoring, harness declensions, and the open vocabulary

**Status:** VISION. This document shapes the v2/v2.5 backlog and future ADRs.
It **builds on** the locked decisions of
[`2026-09-01-tau-authoring-ops-and-primitives-design.md`](2026-09-01-tau-authoring-ops-and-primitives-design.md)
(§10) and re-opens none of them; where it proposes anything new, the item is
marked and lands in the backlog, not in v1. Written from the maintainer's
2026-09-06 reflection: *"workflow as code is good, but I also want in-code
pipeline definitions (sugar over tau ts files), and tau as code — defining a
harness with tau and declining it in other languages; it is primordial that
tau can be augmented with custom tooling, sandboxing, and models per
project."*

**Date:** 2026-09-06.

**Set in stone by:** the worked-examples companion
[`2026-09-06-tau-as-code-worked-examples.md`](2026-09-06-tau-as-code-worked-examples.md)
— concrete end-to-end scenarios per posture, the twelve ratified invariants
(its §5), the derived requirements (B/C/X ids) that seed the
`tau-as-code` implementation tree, and the two acceptance fixtures
(`app-star`, `harness-star`). When this document and that one disagree,
that one wins.

**Relates to:** design §1 (three surfaces), §3.3 (kernel closed, vocabulary
open), §4 (TS API), §6 (integration surfaces), §10 decisions 4, 5, 7, 9, 12,
13; ADR wave 0071–0077; [`vision-roadmap.md`](../plans/vision-roadmap.md)
(v2 backlog);
[`../implementation-trees/exposures.md`](../implementation-trees/exposures.md).

---

## 0. In one breath

> The consolidated design answers *"how do I author a tau project?"* This
> vision answers the inverse question: *"how does tau show up inside a
> developer's world?"* Three postures — **tau as the project** (the workflow
> repo, v1's golden path), **tau in the project** (pipelines co-located with
> application code, harvested at build time), and **tau as the substrate**
> (your own harness, defined with tau, *declined* into your language as a
> generated projection). All three are the same product because everything
> lowers through the one validator into the same frozen IR — and because
> every per-project augmentation (tools, sandboxes, models) is declared,
> governed, and visible on the card.

"Declension" (from *décliner*): a generated, language-specific projection of
a tau-defined contract. Declensions are never hand-maintained and never the
source of truth — they are to the harness what `tau.gen.ts` is to the
registry: generated from schemas, hash-stamped, stale = loud error
(decision 13; the anti-Prisma rule).

---

## 1. The three postures

| Posture | The developer's world | What tau is to them | Status |
|---|---|---|---|
| **A. tau as the project** | a dedicated repo/dir of agents + pipelines; ops lane on top | the whole toolchain: author → prove → run → apply | v1 (E-0..E-4) — the golden path |
| **B. tau in the project** | an existing application (backend, CLI, service); agentic pipelines live *next to the code that calls them* | a build step + a typed handle; the app repo *is* a tau project | vision → v2.5 sugar over scheduled machinery |
| **C. tau as the substrate** | they are building their own agentic product (an internal copilot, a support-bot platform, a Claude-Code-like tool) in their language | the engine + governance substrate; the harness is a compiled tau artifact; their language gets a declension | vision → names and packages decisions 12/13 + serve v2 |

The postures are adoption rungs, not modes: B is A with the project rooted
in an app repo; C is A/B with the artifact's host side promoted to a
first-class, generated surface. Rung N never taxes rung N−1 (design §12).

---

## 2. The invariant spine (what makes them one product)

These invariants hold across all three postures. They are restatements of
locked decisions, listed here because every idea in this document must pass
through them:

1. **One validator, one frozen IR.** Every authoring convenience — sugar,
   frontend, declension — lowers through the synth contract
   (ProjectConfig JSON → `validate()` → sealed IR). No second path, ever
   (ADR-0069 discipline; design §1).
2. **Definition happens at build time; runtime sees only sealed artifacts.**
   "In code" never means "at runtime". Runtime graph construction stays
   refused by construction (design §7, §8).
3. **The constitution is never emitted by code.** `[allow]`, agents, models,
   capabilities stay TOML/dirs regardless of where the project roots
   (decision 5). Sugar can emit choreography only.
4. **No consumer needs a tau runtime library.** The engine is the process /
   the wasm component; everything language-facing is *generated from
   schemas* and hash-stamped (design §6; decision 13).
5. **Every extension is declared, governed, and carded.** A custom tool,
   sandbox, or model binding that does not appear on the capability card
   does not exist (design §3.3, §12 — the permission sheet).

---

## 3. Posture B — tau in the project (in-code pipelines)

**The pitch:** Inngest's co-location DX with Terraform's plan discipline.

```ts
// src/review/review.tau.ts — lives in the app repo, next to its caller
export const review = pipeline("review", (p) => {
  const triage = p.agent("triage", { /* choreography only */ });
  ...
});

// src/routes/pr.ts
import { review } from "../review/review.tau";
const verdict = await review.run({ pr: 42 }); // typed handle → frozen IR
```

One symbol, **two projections**, both derived from the same sealed artifact:

- **Synth-time projection.** The project's `[synth] entry` collects
  `*.tau.ts` modules from the app tree (collection convention TBD:
  file-suffix scan vs. explicit imports from the entry — an ADR question).
  `pipeline()` registers choreography into the emitted ProjectConfig,
  exactly the E-2 lane — same sandbox, same canonical JSON, same one
  validator. In-code definitions are *literally* sugar over
  `pipelines/*.ts`: the id grammar, collision rules, and `[[moved]]`
  discipline apply unchanged.
- **Runtime projection.** Under the app's normal runtime, the same symbol
  resolves to an invocation handle from the generated typed client
  (v2.5 `tau export --client` machinery) bound to the *pinned* bundle —
  over `tau serve` (warm daemon), the CLI NDJSON contract, or the Rust
  embed prelude. The handle carries the pipeline's input/output types from
  `tau.gen.ts`.

**Drift is loud, not possible-silent:** the gen hash-stamp (E-1.4) plus
`tau plan --check` in the app's CI mean a definition that moved ahead of the
pin fails the build, and a capability widening exits 3 and blocks the PR.
The app's deploy grows one step: `tau apply`.

**Packaging story (new, small):** `tau.toml` at the app root makes the app
repo a tau project; `agents/`, `[allow]`, and model dirs live beside
`package.json` like any other config. Nothing in the dirs-scanning design
assumes a dedicated repo; this posture only needs the golden-path docs and
`tau init --app` scaffolding to say so out loud.

**In-code in other languages** is *not* a runtime graph library in that
language (jsii-style embedding stays rejected). It is an **authoring
frontend**: the synth contract is deliberately language-agnostic
(decision 4), so a Python or Go frontend is a build-time subprocess emitting
the same ProjectConfig JSON — the "second authoring language" slot already
reserved at v2.5+. Consumer-first order still holds: a language gets a
*client* declension before it gets an authoring frontend (decision 13).

---

## 4. Posture C — tau as the substrate (the harness, declined)

Today a developer building their own agentic product either writes a harness
from scratch (loop, tool dispatch, budgets, replay, governance — years of
scar tissue) or adopts a framework that owns their process and their
language. tau's engine already *is* the hard part, and decisions 12/13 +
serve v2 already scheduled the mechanics. What this vision adds is the
product framing: **the harness itself is a declared, compiled tau
artifact.**

**The harness declaration** (TOML — it is vocabulary/governance, so the
surface split places it there) states, per project:

- **the exposed set** — which pipelines/agents form the product surface
  (everything else stays internal);
- **required host tools** — name + JSON schema + capability claims for each
  tool the embedding application must provide; enforced by the host,
  card-labeled `host-enforced` (decision 12, unchanged);
- **session policy** — what late-bound tooling (session MCP servers, host
  tools) a session may attach, under which declared ceiling;
- **approval/elicitation routes** — which checks and typed elicitations
  surface to the host (MCP-elicitation-shaped, per design §6);
- **bindings left open per environment** — model endpoints and sandbox
  profile, narrowable only (never widenable) at the host tier.

Compiled into the bundle, this yields the **harness card**: the capability
card extended with the host's *obligations* — "to run this artifact you
must provide these N tools with these schemas; you may attach at most this;
these approvals will reach you." `tau inspect` renders it; a host that
fails its obligations is refused at session start, not discovered at step 7.

**Declensions.** From the harness card, `tau export --harness <lang>`
(verb name TBD; rides the v2.5 typed-client codegen) generates the host-side
projection: a typed scaffold in TypeScript/Python/Go where the developer
implements *exactly* the declared host tools and approval callbacks — and
nothing else. The loop, journal, replay, budgets, capability enforcement,
and card stay tau's. Two transports, one contract:

| Transport | For | Mechanics |
|---|---|---|
| `tau serve` v2 | any-language processes | Unix socket JSON-RPC, `session.*` verbs, reverse dispatch for host tools/approvals |
| wasm component | web/edge/embedded hosts | the WIT world (artifact contract #2) |

The **Rust embed prelude is the reference declension** and the dogfood
(design §6: `tau dev` rebuilt on it) — proving the contract is complete
before any generated declension ships.

**Contract accounting (deliberate):** this is *not* a third contract. The
harness surface is the artifact contract seen from the host side — the WIT
world and the serve v2 protocol, both already scheduled to be frozen. The
harness declaration only selects and names obligations expressible there.
If ADR work shows it needs independent schema versioning, it graduates
explicitly then — not by drift.

---

## 5. The augmentation pillar — kernel closed, vocabulary open, extensions governed

"Primordial" is the right word, and the design already carries the
principle (§3.3); this section elevates it to a product promise: **every
project can bring its own tools, sandbox, and models — and no extension can
escape the card.**

**Tools — the four-rung ladder** (each rung declared + carded):

1. **Primitive pack** — flow-only fragments; structurally powerless
   (no caps to audit).
2. **Project Rust** — `#[tau::tool]` / `#[tau::deterministic]` in the
   project's own crate (E-1); capabilities derive from the signature,
   bounded by `[allow]`.
3. **MCP contract** — TOML-declared, double-bounded (`[allow.mcp]` hosts
   out, contract in; `tau mcp pin` freezes it).
4. **Host tool** — declared in the harness declaration, schema-validated,
   `host-enforced` label; the only rung whose enforcement is delegated,
   and the card says so.

**Sandboxing — profiles as a versioned vocabulary (new, v2-shaped).** The
sandbox crates (native/container/darwin/windows/proxy) exist; what's missing
is the *naming layer*: **sandbox profiles** — named, versioned compositions
of the capability vocabulary (ADR-0036 pattern) selectable per project and
narrowable per environment. Custom sandboxes are host-tier adapters behind
the sandbox port: a project (or org) can ship its own containment strategy,
but an adapter can only *narrow* what the artifact declared — run-or-refuse
is preserved (decision 6), and the active profile is on the card.

**Models — providers are plugins, bindings are project vocabulary.** The
provider lane exists (anthropic/ollama/openai plugin crates); a project can
already ship its own. The vision commits to the shape: model *identities*
live in the TOML dirs (project vocabulary), provider *plugins* implement
them, and environments late-bind endpoints/keys **under a declared model
allowlist ceiling** — the same late-binding-under-ceiling discipline as
session tooling. Per-project model routing is thus config, not code.

Extension governance rule, restated once: kernel (step kinds, planes,
foundations) extends by ADR only; vocabularies (tools, predicates, packs,
profiles, providers) extend per project; every extension appears on the
card or it does not run.

---

## 6. What this adds to the roadmap (the honest delta)

Nothing here moves into v1. E-0..E-4 are untouched. The vision shapes
existing v2/v2.5 backlog entries and adds a small number of new ones:

| Vision item | Rides on (already scheduled) | Genuinely new | Earliest slot |
|---|---|---|---|
| Co-located `*.tau.ts` dual projection | E-2 synth · E-1.4 `tau.gen.ts` · v2.5 typed client | collection convention · handle↔pin binding · `tau init --app` + docs | v2.5 |
| Harness declaration + harness card | decision 12 host tools · E-3.4 capability card · serve v2 | `[harness]`/exposed-set schema · card extension · refuse-at-session-start check | v2 (with serve v2) |
| Declensions (`tau export --harness <lang>`) | v2.5 typed-client codegen · WIT world · embed prelude | host-scaffold generators (TS/Python first) | v2.5 |
| Authoring frontends in other languages | decision 4 synth contract | per-language frontend | v2.5+ (already listed) |
| Sandbox profiles vocabulary | sandbox crates · env narrowing (E-4/v2 environments) | profile naming/versioning ADR · adapter port | v2 |
| Per-project model providers + env ceilings | plugin lane · E-4 pins | model-allowlist ceiling wiring · docs | rides E-4/v2 |

Each row that lands gets its own ADR when built (ADR-0076 rules); this
document is their shared "why".

---

## 7. Boundaries reaffirmed (so the vision cannot be read as re-opening them)

- **No runtime graph construction**, in any language, in any posture —
  including inside a declined harness. Dynamic/Explore remain the governed
  runtime-freedom primitives.
- **No jsii-style runtime embedding**; declensions are generated
  projections over process/wasm boundaries, not in-process tau runtimes.
- **No hand-maintained per-language SDKs** (the CDKTF scar); a language
  without codegen support gets the NDJSON/CLI contract, which needs no
  library at all.
- **`[allow]`, agents, models stay TOML/dirs** wherever the project roots;
  posture B does not soften decision 5.
- **No tau registry/cloud** (NG3/NG4); harness declensions and packs travel
  as plain npm/git/OCI artifacts like everything else.

---

## 8. Open questions (for the eventual ADRs, not for now)

1. Collection convention for co-located definitions: `*.tau.ts` suffix scan
   vs. explicit imports from the synth entry (leaning explicit — imports
   keep the entry the single root and make the sandbox's read set obvious).
2. Naming: "harness declaration" vs. "exposure" vs. "surface"; and whether
   `tau export --harness` folds into `--client` as a mode.
3. Whether declension scaffolds are generated-into-repo (committed,
   hash-stamped like `tau.gen.ts`) or build-time artifacts.
4. Monorepo shape for posture B: one `tau.toml` per app vs. workspace-level
   with per-app exposed sets.
5. Whether the harness card's obligations block at `tau check` time for
   known hosts (a host manifest?) or only at session start.
