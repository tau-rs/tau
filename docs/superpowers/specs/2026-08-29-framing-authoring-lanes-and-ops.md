# Framing — authoring lanes and the ops model (config / code / IaC)

**Status:** Brainstorm / scoping document. Not a spec, not an ADR. Decisions
enumerated in §6 graduate to ADRs individually.

**Date:** 2026-08-29.

**Relates to:**
[`docs/explanation/tau-philosophy.md`](../../explanation/tau-philosophy.md),
ADR-0055/0056 (the two contracts), ADR-0057 (root `[allow]` governance),
ADR-0041 (TS declarations-only authoring), ADR-0043 (compile the trigger,
delegate the substrate), ADR-0065 (unknown-input policy), EPIC 5.3
(authoring-SDK codegen).

---

## The question being framed

Today a tau project is created through `tau.toml` (plus `[dirs]`, plus the
early TS/Python front-ends), and settings live in TOML (`.tau/config.toml`
scope config, agent definitions, engine parameters). That split is right and
stays. The question is what the **other two lanes** for interacting with the
engine should look like:

1. **Code integration** — programmatic authoring and programmatic *driving*
   of the engine.
2. **Infrastructure-as-code** — defining, reviewing, promoting, and operating
   pipelines *at scale* (many pipelines × many environments × many operators)
   with an ops-grade workflow, without degrading the solo-developer UX/DX.

This document audits what exists in-tree, audits how comparable systems
(Terraform/OpenTofu, Pulumi, CDK, Crossplane, Argo CD, Dagger, Temporal,
Airflow/Dagster/Prefect, BuildKit, CUE/KCL, and the agent-framework field)
split config vs code vs IaC, and proposes a three-lane model with an
enumerated decision list.

---

## 1. Audit: how things are created today

### 1.1 One convergence point (this is the asset)

Every authoring surface converges on a single validated in-memory model and a
single lowering pass:

```
tau.toml ─────────────┐
[dirs] agents/tools ──┤
project.ts (swc AST) ─┼─▶ ProjectConfig ─▶ lower_project() ─▶ IrModule ─▶ bundle (.tau)
sdk/python (emits ────┤    (tau-pkg,        (tau-ir-lower,     (tau-ir,      (self-hashed,
 TOML on stdout)      │     validated,       7 pure stages)     frozen JSON   reproducible,
sdk/ts factories ─────┘     one validate())                     schema        governance
                                                                v2.7.0)       verdict)
```

- The IR JSON schema is frozen, versioned, drift-tested
  (`schemas/ir/tau-ir.v2.7.0.schema.json`, `schema_export.rs` byte-equality).
- The TS extractor deliberately emits TOML internally and re-enters
  `ProjectConfig::parse_str` — **one validation path** (ADR-0041).
- `[dirs]` merges at the *unchecked* level before the single `validate()`
  (ADR-0069) — same discipline.
- Byte-equal-IR-across-surfaces is already an enforced conformance property
  (TOML ↔ TS fixtures; TOML/TS/Python `byte_equal.rs`).
- Bundles are content-hashed, reference-only, reproducible
  (`tau verify --bundle`), and carry a hashed governance verdict.
- `tau mcp pin / diff / refresh` already implements a pin-a-resolution /
  diff-against-live loop for MCP contracts.

This is, structurally, the architecture every mature system in the external
audit either started with or was forced to retrofit (§2). The foundation for
both new lanes already exists; neither lane requires a new runtime concept.

### 1.2 What exists per lane, honestly

| Lane | What exists | Completeness |
|---|---|---|
| Config (TOML) | Full project schema; `[allow]` lattice; `[dirs]`; scope config; lockfile v7; bundle format | The reference surface. Complete. |
| Code: authoring | `tau-ts-extract` (8 factories, static-only); generated `@tau/sdk` + `tau-sdk` Python (EPIC 5.3) | **Thin.** Generated factories cover `models`/`tool`/`agent` only — no pipeline, no goals/deliverables, no `[allow]`, no `mcp`/`subflow` bodies, no durability. TS extractor lacks `[allow]`, `[trigger]`, `[steps]`, `[agent.kinds]`, `[dirs]`, credentials. Not published to npm/PyPI. |
| Code: driving the engine | `tau-runtime-tokio` (library embedding API), `tau_runtime_core::embed` prelude, `tau-embed-example`, serve mode (JSON-RPC/NDJSON), `@tau/embed-js` + React/Angular glue | Real but framed as *embedding a built workflow*, not as *driving the toolchain* (no programmatic build/check/plan). |
| IaC / ops | Nothing. No plan, no diff, no apply, no environments, no promotion, no recorded deploy state, no fleet | The nearest primitives: `tau check` (would it build), `tau verify` (drift of installs/bundles), `tau mcp diff` (pinned vs live), committed `tau-lock.toml`. |

### 1.3 Frictions and drift noticed during the audit

- **Constitution vs shipped reality:** CONSTITUTION §1 still says the two
  public surfaces are the `tau-runtime` Rust API + serve-mode IPC and that
  SDKs "wrap the serve-mode protocol"; ADR-0055/0056 reframed the contracts
  (authoring/IR schema + WIT world) and EPIC 5.3 shipped SDKs as *authoring
  front-ends*. Both SDK framings are actually correct — they are two
  different lanes (§3.2) — but the Constitution text predates the
  distinction and should be reconciled when the lane naming lands (S-1).
- **Stale examples:** `examples/dev-smoke-fan-monitor/tau.toml` (and its TS
  twin) use `llm_backend = …`, which `deny_unknown_fields` rejects against
  the current `UncheckedAgent`.
- **`[allow]` is TOML-only.** No non-TOML surface can author the
  constitution today; a code-authored project is forced through
  `--allow-ungoverned` or a TOML side-file. Any new lane must treat
  governance as first-class, not a side door (ADR-0069's phrasing).

---

## 2. External audit: what comparable systems teach

Full survey compressed to the patterns that transfer. (Systems audited:
Terraform/OpenTofu incl. Stacks GA, Pulumi incl. Automation API/ESC/IDP, AWS
CDK + the CDKTF sunset (archived 2025-12), Crossplane v2, Argo CD/Flux +
ApplicationSets + the rendered-manifests pattern, Argo Workflows/Tekton,
GitHub Actions, Dagger, Temporal, Airflow 3/Prefect 3/Dagster Components,
BuildKit LLB, CUE/KCL/Jsonnet/Starlark/Dhall/Nickel, LangGraph Platform,
CrewAI, OpenAI Agents SDK + the Agent Builder sunset, Claude Agent SDK,
Microsoft Agent Framework, A2A, Bazel/Buck2, Nix, cargo/npm, Helm OCI,
Backstage.)

1. **Two artifacts: intent + frozen resolution; the frozen one is the unit
   of review, promotion, and audit.** (cargo lock, Nix derivations, CDK
   synth, BuildKit LLB, Terraform plan files, Airflow 3 versioned DAG
   snapshots.) tau already has both (`tau.toml` + bundle/IR). The pattern
   degrades to rubber-stamping unless a **semantic diff** renders the frozen
   artifact for humans (`terraform plan`, `cdk diff`).
2. **Plan/diff against recorded state is *the* ops primitive, and state must
   be first-class and tiny.** Terraform's decade of state pain says: never
   put secrets in state, keep it declarative, make it survive manual
   surgery. Terraform's *machine-readable plan JSON schema* is what enabled
   its whole policy/cost/review ecosystem — the schema is the product.
3. **Push (plan/apply) and reconcile (GitOps) are complements.** Reviewed
   change → frozen artifact → a reconciler converges the fleet to the pinned
   artifact. Reconcile-only (Crossplane) loses the human approval gate and
   is opaque to debug; plan-only loses drift-freedom.
4. **The IR is the stable contract; surfaces multiply above it — and every
   surface must be *generated* from the schema.** BuildKit frontends → LLB;
   Dagger's GraphQL schema → codegen'd SDKs; CDK via jsii. The fatal
   counterexample: **CDK for Terraform was archived in Dec 2025** — hand-fed
   per-language bindings over a fast-moving schema are a maintenance death
   sentence, and a code lane that adds no abstraction over the config lane
   finds no adoption.
5. **The config-language trap.** Pure-data surfaces accrete logic until they
   are bad programming languages: Helm text-templating, GitHub Actions
   `${{ }}` + bolted-on reuse, HCL `count`/`dynamic`, Crossplane's
   patch-and-transform DSL (deprecated in v2 for real-code functions).
   The stable equilibrium has exactly two poles: **pure data validated by
   schemas/constraints**, or **a real language that hermetically synthesizes
   data**. Everything in between gets replaced.
6. **The config/code split that scales: code defines *types*, data declares
   *instances*.** Dagster Components (Python component types +
   schema-validated YAML instances + a scaffolding CLI) is the
   best-designed current example; CrewAI's `agents.yaml`-over-code and
   Microsoft Agent Framework's declarative agents rhyme with it.
7. **No frozen artifact ⇒ versioning moves into the runtime and never
   leaves.** Temporal (determinism constraints, patch APIs, versioning
   redesigned twice) is the proof by counterexample. tau's
   run-bound-to-IR-hash property deletes that entire problem class — keep it
   sacred.
8. **Fleet = generator × template over a parameter matrix, plus a catalog;
   promotion = re-pinning an immutable artifact, never rebuilding per
   env.** (Argo CD ApplicationSets, Terraform Stacks' components ×
   deployments with per-deployment plans, Backstage's catalog-first
   framing.) Per-instance plan isolation is the guard against "one template
   change plans against 200 targets."
9. **Distribution is solved: signed OCI artifacts.** Helm, OpenTofu 1.10
   providers/modules, CUE/KCL modules all converged on OCI + cosign. On the
   agent side, A2A's signed Agent Cards extend the same idea to declarative
   agent capability manifests.
10. **Visual builders converge on export-to-artifact or die** (OpenAI Agent
    Builder: launched 2025-10, shutdown 2026-11, migration path = exported
    SDK code). Any future GUI must read/write the config or the IR, never
    hold private state.
11. **The open lane:** every agent-framework "ops story" is a hosted control
    plane with deploy + observe + rollback. **None has plan/diff, drift, or
    promotion semantics.** An IaC-grade ops model over a governed agent
    artifact does not exist anywhere yet. Combined with tau's `[allow]`
    lattice, this is differentiation nobody else can copy cheaply (§3.3.6).
12. **Pin the compiler from the source** (BuildKit's
    `# syntax=docker/dockerfile:1.7`): the artifact being compiled selects
    the versioned frontend, so old sources build identically forever and new
    frontends ship without engine changes.

---

## 3. Proposal: three lanes, one IR

```
 LANE 0 — CONFIG (TOML)          LANE 1 — CODE                 LANE 2 — IaC / OPS
 settings + definitions          1a AUTHORING (synth)          intent:   tau.toml + fleet manifest
 tau.toml · [allow] · [dirs]     TS/Python factories ─┐        frozen:   bundle (content-hashed)
 .tau/config.toml (scope)        (declarations only)  │        recorded: env pins + deploy state (git)
 pure data, forever              1b ENGINE API        │        verbs:    plan · apply · promote ·
        │                        Rust lib + serve-mode│                  fleet · drift
        │                        (build/check/plan/   │        substrate: DELEGATED (adapters),
        │                         run as functions)   │                   optional GitOps reconcile
        ▼                                 │           ▼                        │
        └────────────────┬────────────────┴───────────┘                        │
                         ▼                                                     │
                  ProjectConfig ──▶ IrModule (frozen schema) ──▶ bundle ◀──────┘
                                    ONE validation path · ONE lowering pass
                                    byte-equal IR across all surfaces (enforced)
```

The invariant that makes all three lanes safe: **every lane produces or pins
the same frozen artifact, and governance flows through the same lattice.**
No lane is a side door.

### 3.1 Lane 0 — TOML stays pure data, and gets *stronger*, not smarter

The premise of this framing — settings stay in TOML — is affirmed and
sharpened into a rule:

> **`tau.toml` and `.tau/config.toml` never grow templating, expressions,
> conditionals, or includes.** Reuse and parameterization live in Lane 1
> (code synthesizes data) and Lane 2 (fleet stamps instances). The moment an
> expression enters the TOML, the Helm cycle starts (§2.5).

What Lane 0 *should* gain instead:

- **A published JSON Schema for the project manifest itself** (not just the
  IR). `schemas/project-manifest/` generated from the `Unchecked*` structs
  the same way `schemas/ir/` is generated, drift-tested the same way.
  Editors and CI get validation/autocomplete for free; the SDK codegen
  gains a machine-readable source of truth (S-7). This is the single
  cheapest DX win available.
- **A constraint/policy layer, not a language.** The CUE lesson: the
  durable value is schema + constraints *vetting* plain data. tau already
  has the strongest constraint layer in the field (`[allow]`, the five
  lattice links, `tau check`'s 9 categories); extending `tau check` with
  org-supplied policy (e.g. "no `net.http` host outside `*.corp`", "judge
  models must be from `[allow.models]` tier X") covers the "governance at
  scale" need without a config language. Policy input can itself be plain
  TOML.

### 3.2 Lane 1 — code integration is *two* products; name them separately

The audit surfaced a real ambiguity (Constitution G6 vs EPIC 5.3): "SDK"
currently means two unrelated things. Both are wanted; conflating them is
what hurt other ecosystems.

**1a. Authoring SDKs — the synth pattern (CDK/Dagster shape).**
Code that *defines* a project and synthesizes the same `ProjectConfig`/IR.
Already the shipped direction (ADR-0041, EPIC 5.3). What changes:

- **Close the coverage gap.** The generated factories must span the full
  authoring schema — pipeline (incl. branch/parallel/loop/suspend/dynamic),
  goals/deliverables, triggers, `mcp`/`subflow` tool bodies, durability,
  context pipelines, `agent.kinds`, credentials, and **`[allow]`**. A
  governed project must be fully authorable in TS/Python. Until then the
  code lane fails pattern §2.4's adoption test (CDKTF: a code lane that
  can't do what the config lane does adds nothing).
- **Generate, never hand-feed.** `authoring.rs`'s hand-owned field table is
  the CDKTF trap in miniature. Once `schemas/project-manifest/` exists
  (S-7), the factory surface derives from it; the hand-owned part shrinks
  to naming/ergonomics.
- **Keep declarations-only.** Static extraction (no JS execution) is the
  Bazel/Buck2 phase-separation lesson (§2, Starlark): authoring code runs
  hermetically at build time and its sole output is data. Inline tool
  bodies (δ.2's QuickJS idea) remain a separate, explicitly-argued step —
  they change the trust model, not just the ergonomics.
- **Where code *earns* its lane:** loops over data ("one reviewer agent per
  service in this list"), typed composition of shared fragments (a team's
  standard context pipeline as an importable value), and tests against the
  synthesized IR (CDK-style snapshot/assertion tests — cheap to offer once
  the plan renderer exists, S-3).

**1b. The engine API — the automation-API pattern (Pulumi shape).**
Code that *drives* the toolchain: `build`, `check`, `verify`, `run`,
and the new `plan`/`apply` (§3.3) callable as functions, not just CLI verbs.
This is the lane the Constitution's "SDKs wrap the serve-mode protocol"
sentence was groping toward, and it is the lane every platform team needs to
embed tau in *their* control plane (their CI, their operator, their internal
developer platform) — which is exactly how tau gets an ops story at scale
without violating NG3 (tau never hosts anything; downstream platforms do).

- Rust-first: `tau-pkg` + `tau-ir-lower` + `tau-runtime-tokio` already *are*
  this API; the work is curating/pinning it as a supported surface (the
  `embed`-prelude discipline applied to the build side) rather than writing
  new machinery.
- Polyglot: serve mode (ADR-0033) is the existing process-boundary binding;
  extending its JSON-RPC vocabulary with toolchain verbs gives every
  language a driver without per-language binding maintenance (§2.4).

### 3.3 Lane 2 — IaC: pin, plan, promote, delegate

The design borrows tau's own precedent: ADR-0043's *"compile the trigger,
delegate the substrate."* The IaC lane is **"compile the deployment, delegate
the substrate"** — tau computes, diffs, records, and emits; it never runs
infrastructure (NG3) and never becomes the orchestrator (NG5).

**3.3.1 The artifact triad.** Mirror the pattern every mature system
converged on (§2.1–2.2), reusing what exists:

| Role | Artifact | Status |
|---|---|---|
| Intent | `tau.toml` (+ Lane 1a sources) | exists |
| Frozen resolution | the bundle: content-hashed, reproducible, governance-verdict-carrying | exists |
| Recorded state | **new:** per-environment *pins* — "environment E runs bundle `sha256:…` with scope-config overlay O" — committed to git | missing |

Recorded state is deliberately git-committed data (GitOps posture), not a
state service: no server (NG3), no secrets in state (Terraform's lesson),
reviewable like the lockfile. `tau-lock.toml` and `.tau/mcp/*.contract.json`
already establish the "committed pin" idiom in-repo.

**3.3.2 Environments and the fleet manifest.** A new, small, pure-data
manifest (working name `tau-fleet.toml`; naming is S-2) declares:

```toml
# Sketch — shape is illustrative, not a schema proposal.
[environments.staging]
bundle  = "sha256:ab12…"                # the pin: an immutable, already-built bundle
config  = "envs/staging.config.toml"    # ScopeConfig-shaped overlay: sandbox tier,
                                        # credential-chain order, model endpoint aliases
[environments.prod]
bundle  = "sha256:ab12…"                # promotion = same hash, different env
config  = "envs/prod.config.toml"

[matrix.tenants]                        # optional fleet generator (S-2):
values  = ["acme", "globex"]            # environments × values, stamped instances,
                                        # each planned independently (§2.8)
```

Two hard rules carried over from the audit:

- **Overlays configure, never define.** An env overlay is the existing
  `ScopeConfig` shape (sandbox tier, credentials chain, endpoints) — the
  things that already live in `.tau/config.toml`. Agents, tools, pipelines,
  capabilities are *not* env-overridable; a different workflow is a
  different build. This keeps "what runs" identical across envs — the
  whole point of promoting an immutable artifact.
- **The lattice gains an env link (S-6).** An environment may *narrow* the
  bundle's effective capabilities (e.g. staging denies `net.http` to prod
  hosts), never widen them: `[allow] ⊇ … ⊇ env-effective`. Same subset
  checker, one more link.

**3.3.3 `tau plan` — the semantic diff, with capability-diff as the
headline.** Three comparisons, one verb:

- *source vs pin*: what would change if env E were re-pinned to the current
  build (the pre-merge review artifact);
- *pin vs pin*: what changed between two bundles (the promotion review);
- *pin vs live*: drift (3.3.5).

Output is dual: a human rendering **and a versioned JSON plan schema**
(`schemas/plan/`) — §2.2's lesson that the plan schema, not the plan
command, is what spawns the policy/audit/CI ecosystem. The renderer must be
*semantic*, in IR vocabulary: agents added/removed, prompt asset hash
changes, model ref changes, pipeline shape changes, trigger changes — and,
rendered first and loudest, **governance deltas**: every capability
widening/narrowing per agent and per tool, and any governance-verdict
change. "This change widens `fs.read` from `/data/incidents/**` to `/**`"
in a PR comment is the single most differentiating ops feature available to
tau — no orchestrator or agent framework can produce it, because none has a
build-time capability contract to diff (§2.11).

**3.3.4 `tau apply` / `tau promote` — record and emit, delegate execution.**
`apply` updates the env pin, stamps the recorded state, and (re-)emits the
substrate adapters for that environment — the existing trigger-adapter
emitters (systemd units, k8s CronJob) generalize into per-substrate
deployment adapters; an OCI push adapter (§2.9: bundles as signed OCI
artifacts — OCI is a transport, not a tau-operated registry, so NG4 holds)
covers the "get the artifact where the host can pull it" step. `promote`
is pure pin-copying between environments — build once, promote the hash,
never rebuild per env (§2.8). Applying against a stale recorded state fails
closed (the optimistic-concurrency property plan files give Terraform).

**3.3.5 Drift, two kinds, both delegated.** (a) *Pin drift*: recorded state
vs git intent — pure file comparison, `tau plan` covers it. (b) *Substrate
drift*: recorded state vs live substrate — tau cannot and should not query
arbitrary infrastructure; the `tau mcp diff` precedent (pinned contract vs
live probe) extends per-adapter: each deployment adapter may implement an
optional probe. A GitOps reconciler holding each env at its pinned bundle
hash is the *documented pattern* (an Argo CD/Flux how-to beside
`run-tau-under-a-durable-orchestrator.md`), not a tau-shipped daemon —
push/reconcile complementarity (§2.3) with tau on the push side only,
at least until real demand argues otherwise (S-8).

**3.3.6 The catalog, before any control plane.** Fleet visibility starts as
`tau fleet list` reading committed fleet manifests + recorded state:
pipeline, env, bundle hash, IR version, governance verdict, owner. Backstage
lesson (§2): a queryable catalog is the prerequisite for every later ops
feature, and this one needs no infrastructure at all. Projecting a bundle's
agent surface to an A2A Agent Card is a cheap interop win from the same
data (S-9).

### 3.4 Why this fits the constitution

- **NG3 (no hosted service):** everything above is CLI + files in git +
  emitted adapters. The "control plane" is the user's CI and the user's
  GitOps operator, driven through Lane 1b.
- **NG4 (no marketplace):** OCI is a transport the user points at their own
  registry; discovery stays external.
- **NG5 (not a general workflow engine):** tau plans/pins/emits; when and
  where things *run* stays delegated (same posture as durability's
  delegated-canonical model).
- **NG12 (runtime + compiler, not framework):** all of Lane 2 operates on
  the frozen artifact the compiler already emits; nothing prescribes
  pipeline structure.
- **ADR-0065:** the fleet manifest, plan schema, and recorded state are new
  input surfaces and inherit the policy — authored files strict
  (`deny_unknown_fields`), interchange files version-gated.

---

## 4. UX/DX: the progressive-disclosure ladder

The scale features must be invisible until needed. Rungs, cumulative:

1. **Solo dev, day 1** — `tau init && tau dev` / `tau run`. No fleet
   manifest, no plan, no state. Nothing added by this framing appears.
   *(unchanged today)*
2. **Solo dev, shipping** — `tau build`, `tau verify --bundle`. *(unchanged)*
3. **Team, one env** — `tau plan` in CI renders the semantic diff (headlined
   by the capability diff) on every PR. First new surface; adopted by adding
   one CI step, no manifest changes.
4. **Team, multiple envs** — `tau-fleet.toml` with two environments;
   `tau apply` / `tau promote`. Env overlays are the already-familiar
   `ScopeConfig` shape.
5. **Platform team, fleet** — matrix generators, policy checks in
   `tau check`, OCI distribution, adapter probes/GitOps reconcile, Lane 1b
   driving all of it from their own platform.

Each rung is opt-in by *presence of a file or flag*, never by mode switches;
rung N never taxes rung N−1. This is the same progressive-disclosure
principle the philosophy already commits to for build rigor.

---

## 5. Anti-goals (things the audit says not to build)

- **No expressions/templating/includes in any TOML surface, ever** (§2.5).
- **No bespoke configuration language** (CUE/KCL/Dhall adoption reality;
  §2). Constraints yes, language no.
- **No hand-maintained per-language SDKs or bindings** (CDKTF, §2.4).
  Everything derives from published schemas or crosses one process protocol.
- **No env-level overrides of workflow definition** — envs configure and
  narrow; they never define or widen (§3.3.2).
- **No tau-operated state service, registry, or control plane** (NG3/NG4).
  State is git; distribution is the user's OCI registry; the control plane
  is the user's platform via Lane 1b.
- **No visual builder as a store of truth** — if a GUI ever exists it
  reads/writes Lane 0/2 files (§2.10).
- **No rebuild-per-environment promotion** — promotion moves a hash (§2.8).

---

## 6. Decisions this framing must reach (each an ADR when settled)

- **S-1. Lane naming and the Constitution reconciliation.** Adopt distinct
  names for 1a (authoring SDK) vs 1b (engine/automation API); amend the
  G6 text to the two-contracts framing with these lanes as bindings.
- **S-2. The fleet manifest.** New file (`tau-fleet.toml`) vs a section in
  `tau.toml` vs directory convention; whether the matrix generator ships in
  v1 or environments-only; grammar for env names; relation of overlays to
  `ScopeConfig` (recommendation: overlays *are* `ScopeConfig`, no second
  schema — the ADR-0069 discipline).
- **S-3. The plan contract.** `schemas/plan/` versioned JSON schema; the
  semantic-diff vocabulary (in IR terms, not TOML terms); capability-diff
  rendering; exit-code semantics for CI gating (0 no-change / N changes /
  M widens-capabilities).
- **S-4. Recorded state.** Shape and location of env pins + deploy records
  (recommendation: committed files beside the fleet manifest, ADR-0065
  interchange rules, explicitly secret-free); staleness/concurrency
  semantics for `apply`.
- **S-5. Deployment adapters.** Which substrates ship first (recommendation:
  the two that exist for triggers — systemd, k8s — plus OCI push); the
  adapter trait boundary; whether probes (drift) are part of the v1 trait or
  additive.
- **S-6. The environment lattice link.** Is `env-effective ⊆ bundle
  effective` a sixth governance link enforced by `tau check`/`tau plan`?
  (Recommendation: yes; one vocabulary, one subset checker, per ADR-0057.)
- **S-7. Schema-first codegen.** Publish `schemas/project-manifest/`
  generated + drift-tested like `schemas/ir/`; re-derive `authoring.rs`
  from it; define the full-coverage bar for the TS/Python factories
  (including `[allow]`) and the npm/PyPI publishing step.
- **S-8. Reconcile posture.** Documented GitOps pattern only (recommended
  start) vs an optional thin reconciler; revisit trigger = named user demand.
- **S-9. Distribution.** Bundle-as-OCI-artifact media type, cosign signing,
  relation to Framing G's git-URL resolver; optional A2A Agent Card
  projection from a bundle.
- **S-10. Compiler pinning.** Should `tau.toml` (or the fleet manifest) pin
  the tau/`ir_format` version BuildKit-`# syntax=`-style, so old sources
  build identically across tau releases? (Interacts with ADR-0056's
  versioning; cheap to reserve a field for now.)

---

## 7. Sequencing sketch (dependency-ordered, not scheduled)

1. **S-7 first** — the project-manifest schema unblocks SDK coverage, editor
   DX, and is pure consolidation of what exists. Extends EPIC 5.3.
2. **`tau plan` source-vs-build, single project** (S-3) — valuable with zero
   new manifests: a PR-time semantic/capability diff between the committed
   bundle (or last build) and the current source. This is the smallest
   shippable piece of the ops lane and the proof of the plan schema.
3. **Environments + pins + promote** (S-2, S-4, S-6).
4. **Apply + adapters + OCI** (S-5, S-9).
5. **Fleet matrix, policy checks, probes/GitOps how-to, Lane 1b toolchain
   verbs** (S-1, S-8) — the at-scale rungs.

Each step is additive on the load-bearing surface; nothing existing is
renamed, removed, or made mandatory (the ROADMAP's no-flag-day discipline).

---

## 8. Out of scope for this framing

Per the Phase-α rule that framing docs enumerate what they refuse to decide:

- Inline TS/JS tool bodies (δ.2 QuickJS) — separate trust-model discussion.
- Multi-file TS imports (tracked v1.1 work under ADR-0041).
- Anything hosted: remote state, remote build, a tau registry (NG3/NG4).
- Runtime orchestration/durability changes — the delegated-canonical model
  (NG5, ADR-0053) is untouched; Lane 2 operates strictly at/before deploy
  time.
- A visual builder.
- Cross-ecosystem package resolution (Framing G owns it).
