# ADR-0055: tau identity — a compiler+engine between two contracts; the CLI is the reference host

**Status:** Accepted
**Date:** 2026-06-22
**Deciders:** tau core

## Context

The 2026-06-20 roadmap-challenge / vision-reframe session (verified deep
research) concluded that tau is a **combination play**: every individual axis
(IR/durability, edge-wasm, MCP-hosting, wasm-sandboxing, prompt-compilation,
per-tool sandboxing, on-MCU agents) already has a more-focused rival ahead.
The only unoccupied square is the **intersection + conformance +
vendor-independence + root-governed capability safety**.

To defend that square, the project must be precise about *what tau's product
is*. Historically the docs lead with "the `tau` CLI" and frame edge / browser
/ embedded as downstream build targets of a CLI-centric application. That
framing privileges one host, makes the other hosts look like afterthoughts,
and — most damagingly — risks tying tau's public stability surface to CLI
verbs that should be free to churn. Parallel work is in flight (durability
#373, β.7.5 #369/#372); without a locked identity it will drift.

[`docs/explanation/tau-philosophy.md`](../explanation/tau-philosophy.md)
already establishes Conviction 1 ("tau is a *compiler*, not a framework").
This ADR locks the precise product boundary that conviction implies.

## Decision

**tau's product is `tau-runtime-core` (the engine) plus TWO versioned public
contracts:**

1. **The authoring / IR schema** (JSON). This INCLUDES the root `[allow]`
   governance section — the capability ceiling and resource registry of the
   `tau.toml` constitution. `[allow]` is the *governance section of the
   authoring contract*, not a separate ABI (there are two contracts, not
   three).
2. **The WIT host world** (the embedding interface). The WIT world is
   **generated from the no_std ports** — it is never hand-maintained, so it
   cannot drift from the engine it describes.

**The `tau` CLI is the REFERENCE HOST, not the product.** The analogy is
LLVM: the product is the LLVM core + the IR; `clang` is one reference
frontend/driver built on it. For tau, the product is `tau-runtime-core` + the
two contracts; the `tau` CLI is one reference host/embedder that exercises
them.

**The public stability / semver surface is the two contracts + the no_std
ports API.** CLI verbs get a separate, looser compatibility policy (documented
with the CLI, not governed by the contract semver).

**The CLI is held to the highest quality bar.** It is the on-ramp and the
example that edge / browser / embedded embedders copy. This decision demotes
the CLI's *architectural privilege* (it is one host among peers), not its
*importance* (it remains the reference standard).

## Consequences

- **Docs lead with component + contracts.** Features are framed engine-first;
  `philosophy.md` (Story 8.2) and `ROADMAP.md` (Story 8.3) are reframed to
  match. Future ADRs frame features as engine + contract changes, then note
  the CLI surface.
- **Edge / browser / embedded hosts are PEERS of the CLI** — each is a host of
  one component, not a downstream target of a CLI-centric product.
- **Versioning + conformance attach to the two contracts**; CLI verbs evolve
  under the looser policy. This is what lets the CLI stay the
  highest-quality, fastest-moving reference surface without destabilising
  embedders.
- **New obligation:** the WIT host world must be *generated* from the ports
  (no hand-maintained drift). Locking and codegen of both contracts is EPIC 2
  ("Lock the two contracts"); this ADR is the identity premise EPIC 2
  implements.
- **Neutral:** no code changes in this ADR. It is constitutional framing that
  constrains how subsequent engine, durability, and β.7.5 work is described
  and versioned.

## Alternatives considered

- **CLI-as-product (status-quo framing).** Rejected: it privileges one host,
  makes edge / browser / embedded read as afterthoughts, and ties the public
  stability surface to CLI verbs that must be free to churn. The trade-off it
  imposes — a frozen CLI or unstable embedders — is exactly the failure this
  ADR prevents.
- **Three contracts (authoring schema, `[allow]` governance, WIT world).**
  Rejected: `[allow]` is the governance *section* of the authoring contract,
  not an independent ABI with its own version line (reconciliation R1 of the
  vision audit). Counting it separately would create a third versioned surface
  that always moves in lockstep with the first — ceremony without value.
- **Engine-only product (contracts are implementation detail, not product).**
  Rejected: an engine without versioned, conformance-checked contracts is not
  embeddable or provable-identical-across-targets. The contracts *are* the
  product surface; hiding them would forfeit the moat (conformance +
  vendor-independence).
