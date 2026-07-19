# ADR-0057: Root `[allow]` governance + build-time enforcement

**Status:** Accepted
**Date:** 2026-06-22 (accepted 2026-07-19; EPIC 1 shipped via #445/#451/#453)
**Deciders:** titouanlebocq

## Context

The tau philosophy's Conviction 3 ([`tau-philosophy.md`](../explanation/tau-philosophy.md))
states that every tool declares its capabilities **once, in the root `tau.toml`
constitution**, and that `tau build` lowers that one declaration per target.
ADR-0055 locks the precise product boundary that conviction implies: the root
`[allow]` section is the **governance section of the authoring contract** — not
a separate ABI, but the capability ceiling and resource registry of the
`tau.toml` constitution.

Today, no such root ceiling exists. The capability model is **two layers**:

```
package manifest caps  ──(ceiling)──►  agent [capabilities] override  ──(narrows)──►  effective
        ▲                                       ▲
   "what the tool CAN do"              "what THIS project lets it do"
   (ships with the package)            (per-agent, narrows only)
```

`tau_pkg::capability_override::compute_effective` already enforces
*override ⊆ package* — the integrator may only **narrow** a package's grants,
never widen them. But the package manifest is the de-facto top: there is no
repo-root constitution above it, so a package author's declared grants are the
ceiling. A project cannot express "no agent in this repo may ever touch the
network, regardless of what a package requests."

This is the gap EPIC 1 closes. tau's discipline (per the user's standing
principle and Conviction 3) is **Rust-like**: any check that *can* run at build
time *must* run at build time; deferring an enforceable constraint to runtime
instantiation is a regression. The root constitution must therefore be proven
at `tau check`, not merely honored at runtime.

This ADR is **constitutional framing**: it locks the model that stories 1.2–1.6
implement. It writes no code.

### Existing machinery this builds on

- `Capability` / `FsCapability` / `NetCapability` / `ProcessCapability`
  (`tau-domain`) — the capability vocabulary, `#[non_exhaustive]`.
- `compute_effective` + `glob_subset::is_glob_subset_set` (`tau-pkg`) — the
  subset relation (globs for filesystem paths, exact-set inclusion for hosts /
  commands, `≤` for `max_bytes`). ADR-0032 records its relocation to `tau-pkg`;
  ADR-0036 records its forward-compatibility stance.
- `[models]` alias→`{backend, model}` map (ADR-0052).
- `[tools]` definitions for IR lowering (β.2.2).
- `[mcp.<name>]` server bindings (philosophy.md, β.3 MCP facilitator).

## Decision

### 1. `[allow]` is an additive top ceiling above packages

The root `tau.toml` gains an `[allow]` section that is a **new top of the
lattice**, inserted *above* the existing package ceiling. Packages declare what
a tool *needs* (a request); `[allow]` declares what the project *permits* (the
ceiling). The existing `override ⊆ package` relation is unchanged; `[allow]`
adds one link at the top:

```
[allow]  ⊇  package  ⊇  agent override  ⊇  …  ⊇  tool
```

A package may declare `fs.read = ["/**"]`, but if root `[allow]` caps
`fs.read = ["/proj/**"]`, the package is bounded at build time. Packages become
*requests*, not unconditional grants.

### 2. One capability vocabulary, one subset checker, for every link

`[allow]` is expressed in the **same `Capability` kinds** used everywhere else
(`fs.read`, `fs.write`, `fs.exec`, `net.http`, `process.spawn`, `agent.spawn`),
with the **same field shapes** (paths = globs, hosts / commands = exact sets,
`max_bytes`). Consequently **every lattice link is decided by the one reused
subset function** — there is exactly one comparison code path, already tested.

```toml
[allow]
"fs.read"       = { paths = ["/proj/**"] }
"fs.write"      = { paths = ["/proj/build/**"], max_bytes = 5_000_000 }
"net.http"      = { hosts = ["api.weather.com"] }
"process.spawn" = { commands = ["git", "rg"] }
```

**Deny-by-default within a present `[allow]`:** a capability *kind* that is
absent from an `[allow]` block carries a ceiling of ∅ for that kind — nothing
below it may use that capability. (Absence of the *entire* `[allow]` block is
governed by §5.)

### 3. The resource registry is the governance ceiling — one block per resource

`[allow]` is **pure governance**: it carries the ceiling, never concrete
configuration noise. The named-resource registries are:

- **`[allow.models]`** — the **sole home** for the alias→`{backend, model}`
  map. This folds in today's top-level `[models]` table (ADR-0052): there is
  nothing to *narrow* about a model identity, so the permitted-alias map and the
  identity map are one and the same. Checked by set-membership.
- **`[allow.mcp.<name>]`** — one block per MCP server. The `url` **is** the
  grant of network reach: the reachable-host ceiling is **derived from the
  url's host**. An explicit `hosts = [...]` is permitted only to widen (a
  multi-host contract) or narrow. There is **no separate `[mcp.*]` instance
  table** — the registry block is the binding.
- **`[allow.tools.<name>]`** — one block per tool: its binding (`native = …`
  or `mcp = …`) plus an optional per-tool capability ceiling.

```toml
[allow.models]
fast = { backend = "anthropic", model = "claude-haiku-4-5" }

[allow.mcp.weather]
url = "https://api.weather.com/mcp"          # host ceiling derived from url

[allow.tools.read_temp]
native = "ReadTemp"
"fs.read" = { paths = ["/proj/sensors/**"] }
```

**Closed-world (deny-by-default for references):** an agent that references a
model, MCP, or tool **name** not registered in the corresponding `[allow.*]`
table fails `tau check`. Referencing an unregistered resource is an error, not
a silent pass.

### 4. The lattice and the build / runtime split

The complete capability lattice is **five links**, each the same `⊇` subset
relation from §2:

```
[allow]  ──┐  (new link)
           ▼   build:  agent.effective ⊆ root
        agent
           ▼   build:  declared region envelope ⊆ agent
   dyn-region  ·····   runtime: actual in-region caps ⊆ envelope
           ▼   build where static / runtime where spawned dynamically
        spawn          (attenuation: child ⊆ parent)
           ▼   build:  tool grant ⊆ spawn / agent
         tool          (leaf — most narrow)
```

The governing principle — the Rust-like build-enforcement stance — is:

> **Every statically-decidable link is enforced at build time (`tau check`).
> Runtime enforcement exists only for the single genuinely runtime-determined
> input — the actual capabilities a dynamic region selects at runtime — and
> even that is bounded by a build-checked envelope. No capability is ever
> enforced *only* at runtime when it could have been proven at build.**

This ADR declares **all five links** as the constitution so that EPIC 4
(dynamic regions, `StepRun::Dynamic`) slots into a pre-specified envelope
contract without re-litigating the model. Enforcement scope is staged:

| link | endpoints exist today | build / runtime | enforced by |
|---|---|---|---|
| root ⊇ agent | agents yes; root new | **build** | **EPIC 1** (1.4 / 1.5) |
| agent ⊇ tool | yes (`tool_refs`, `[allow.tools]`) | **build** | **EPIC 1** |
| agent ⊇ spawn | yes (`agent.spawn`, ADR-0024) | **build** (sub-agent kinds are static) | **EPIC 1** |
| agent ⊇ dyn-region envelope | `StepRun::Dynamic` not built | **build** (envelope ⊆ agent) | declared here; enforced **EPIC 4** (4.4) |
| dyn-region actual ⊇ spawn / tool | not built | **runtime** (membership + bounds counters) | declared here; enforced **EPIC 4** (4.5) |

### 5. Backward compatibility: governed by default, with explicit opt-out

This ADR originally framed governance as *opt-in* (an absent `[allow]` block
emitting only a warning). The shipped default is stricter: **D2 (ADR-0059,
governance build gate, #455) makes governance the default**, consistent with the
Rust-like build-enforcement stance — a missing constitution is a defect, not a
soft nudge. The progressive-disclosure spirit is preserved through **explicit,
named opt-out flags** rather than a silent warning:

- **No `[allow]` block** = **hard error `GOV000`** at `tau build` / `tau run` /
  `tau check` (exit 2): "no constitution declared." A workflow does not build or
  run ungoverned unless the author explicitly asks.
- **Opt out per invocation:**
  - `--allow-ungoverned` — proceed despite a *missing* `[allow]` ceiling
    (ungoverned build/run, by explicit choice).
  - `--no-governance` — skip governance checking even when a ceiling *does*
    exist; the bundle records the governance verdict as `skipped`, so the
    escape is auditable in the artifact.
- **An `[allow]` block present** = strict. Anything unlisted is denied (§2's
  deny-by-default within `[allow]`, and §3's closed-world for references).

Existing projects and fixtures adopt an `[allow]` block (or an explicit opt-out
flag) to build; the strictness bites the moment an author writes `[allow]`, and
the *absence* of one is now surfaced as an error rather than tolerated silently.

## Consequences

### Positive

- A project can express a true constitution: "no agent here may touch the
  network," enforced at build time, regardless of what any package requests.
- One capability vocabulary and one subset checker cover every lattice link —
  no second comparison code path, no coarse-vs-fine mapping to maintain.
- `[allow]` reads as a reviewable constitution: a reviewer audits one section
  to see *what is permitted*, separate from *how things are wired*.
- The Rust-like guarantee is realized: every enforceable constraint is proven
  at `tau check`; runtime gating shrinks to the irreducibly dynamic case.
- EPIC 4 inherits a complete, pre-specified lattice and envelope contract.

### Negative / obligations

- **`[models]` folds into `[allow.models]`** and MCP caps move from
  `[mcp.<name>]` to `[allow.mcp.<name>]` — an authoring-schema change that
  touches ADR-0052's and β.2.2's surfaces and the philosophy examples. Because
  governance is opt-in (§5), this is not a forced migration, but the docs and
  example projects that adopt `[allow]` must use the new homes.
- The deny-by-default semantics (§2, §3) mean an author who *does* opt into
  `[allow]` must enumerate every capability and resource — more up-front
  ceremony, by design, in exchange for a provable ceiling. Story 1.6's lint
  (coarse ceilings such as `hosts = ["*"]`) partially offsets the temptation to
  over-grant.
- A new public authoring-schema surface (`[allow]`, `[allow.mcp]`,
  `[allow.tools]`, `[allow.models]`) is added. Per ADR-0055 it versions as the
  *governance section of the authoring contract*, not a separate ABI.

### Neutral

- No code in this ADR. Stories 1.2 (config + registry parse / round-trip), 1.3
  (elevate `capability_override` + `glob_subset` to the root ceiling), 1.4
  (`tau check` failure on over-reach / unregistered reference), 1.5 (enforce the
  three static lattice links), and 1.6 (coarse-ceiling lint) implement it.

## Alternatives considered

| Alternative | Why rejected |
|---|---|
| **`[allow]` replaces the package ceiling** (packages declare nothing binding; root is the sole source of truth). | Breaks the existing `override ⊆ package` semantics and the "package author declares what the tool needs, integrator governs what's permitted" separation. The additive model preserves both and makes 1.3 *add a layer* rather than *rewrite one*. |
| **Coarse category ceiling** (`network = ["*"]`, `filesystem = ["read","write"]`) distinct from the fine-grained `Capability`. | Two vocabularies plus a mapping; lattice links stop being homogeneous (root⊇agent coarse-vs-fine, agent⊇tool fine-vs-fine). Conviction 3 requires uniform declaration. The coarse forms become *lint-detectable patterns within the same shape* (Story 1.6), not a separate type. |
| **`[allow.*]` as a thin name allow-list** (registers existence only, carries no ceiling). | Cannot stop a registered MCP from declaring wider network reach than the constitution intends; the registry would not participate in the lattice. Fails EPIC 1's goal of a governed resource ceiling. |
| **`[allow.*]` as the canonical home, relocating concrete config (urls, bindings) into `[allow]`.** | Mixes governance with configuration noise; `[allow]` stops reading as a pure constitution. The chosen model keeps the *ceiling* in `[allow]` (incl. the url-as-grant for MCP) while excluding incidental config. |
| **Strict deny-all when `[allow]` is absent** (no escape hatch). | A *silent* strict-by-default instantly breaks every existing project and fixture. What shipped (D2 / ADR-0059) is the middle ground §5 describes: a missing `[allow]` is a hard error (`GOV000`) **with an explicit per-invocation opt-out** (`--allow-ungoverned` / `--no-governance`), so the strictness is the default but the migration path stays open and auditable. |
| **Enforce only root ⊇ agent in EPIC 1; let EPIC 4 define the rest of the lattice.** | A constitutional ADR should define the whole rule once. Declaring all five links now (even with staged enforcement) prevents EPIC 4 from redesigning the envelope contract. |

## References

- ADR-0055 — tau identity; `[allow]` is the governance section of the authoring
  contract (not a separate ABI).
- ADR-0059 — governance build gate (D2): a missing `[allow]` is a hard error
  (`GOV000`) with `--allow-ungoverned` / `--no-governance` opt-out (§5).
- ADR-0052 — per-agent model resolution (`[models]`, folded into
  `[allow.models]`).
- ADR-0032 — `CapabilityOverride` relocation to `tau-pkg`.
- ADR-0036 — capability forward-compatibility.
- ADR-0024 — multi-agent orchestration (`agent.spawn`, recursive spawn).
- `docs/explanation/tau-philosophy.md` — Conviction 3 (capability-safe by
  construction; per-target lowering).
- `docs/superpowers/plans/vision-roadmap.md` — EPIC 1 (stories 1.1–1.6),
  EPIC 4 (dynamic regions).
