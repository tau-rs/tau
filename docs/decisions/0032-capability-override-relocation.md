# ADR-0032: Relocate CapabilityOverride from tau-runtime to tau-pkg

> **Updated 2026-05-31 (β.1.4):** `tau-runtime` has since split into
> `tau-runtime-core` (kernel) + `tau-runtime-tokio` (host shell). The
> `tau-pkg` location for `CapabilityOverride` is unchanged.

**Status:** Accepted
**Date:** 2026-05-17
**Deciders:** titouanlebocq

## Context

`CapabilityOverride` was introduced in tau-runtime as part of Tier 2 priority 4 (capability override implementation, see ROADMAP). The type describes how a project's `tau.toml` `[[agents.<id>.capabilities]]` block narrows the grants in a package manifest. Until now, the canonical home was `tau_runtime::capability_override`.

Phase 1 priority §15 (`tau serve` mode) requires a new binary in `tau-app` that resolves agents and constructs runtime invocations. To avoid a dependency cycle once `tau-cli` depends on `tau-app` for the `tau serve` subcommand wiring, the project-config types (`AgentEntry`, `ProjectConfig`, `build_agent_definition`) move from `tau-cli` to `tau-pkg` (see the diff in this PR's first commit and §13 in the tau-serve-mode spec).

Project-config types reference `CapabilityOverride` directly — a `tau.toml` agent entry CAN contain a `[capabilities]` override that the kernel applies at runtime. Moving project config into `tau-pkg` would create a cycle:

```
tau-pkg::project ──imports──► tau_runtime::CapabilityOverride
tau-runtime ───────────────► tau-pkg                          (existing dep)
```

Two ways to break the cycle:

1. **Don't import the type.** tau-pkg::project keeps the field but uses a sentinel type (`serde_json::Value`, a string, a private wrapper). Defers the binding to runtime call sites. Worse ergonomics; loses compile-time validation.
2. **Move `CapabilityOverride` to tau-pkg.** The type is more naturally a tau-pkg concept anyway — it lives in `tau.toml`, which tau-pkg owns. tau-runtime keeps the public symbol via a re-export shim.

Constitution G6 / QG12: tau-runtime's public API surface is one of two SemVer-stable commitments. Per QG18, changes to that surface require an ADR. This ADR records the relocation.

## Decision

`CapabilityOverride`, `EffectiveCapability`, `OverrideExpandError`, `compute_effective`, and the `glob_subset` helper move from `crates/tau-runtime/src/capability_override/` to `crates/tau-pkg/src/capability_override/`.

`crates/tau-runtime/src/capability_override/mod.rs` becomes a thin re-export shim:

```rust
pub use tau_pkg::capability_override::*;
```

`tau-runtime`'s public API is unchanged from a consumer's perspective — `tau_runtime::CapabilityOverride` and friends remain importable at the same paths and resolve to identical types. Direct importers from `tau_pkg::capability_override` are also supported (and preferred for new code).

The relocation is purely a refactor: no behavior change, no SemVer-breaking change, no test deltas in this PR.

## Consequences

### Positive

- Breaks the dependency cycle that would have blocked Phase 1 §15.
- `CapabilityOverride` now lives next to the types it operates on (`AgentEntry`, `ProjectConfig`, lockfile entries). Reduces conceptual scatter.
- Future tau bindings (`tau-app::serve`, hypothetical `tau-tui`, etc.) can import directly from tau-pkg without pulling in the full runtime.

### Negative

- `tau-runtime` now has a re-export shim that future maintainers must remember exists. If `tau-runtime::capability_override::mod.rs` is ever rewritten without the re-export, downstream importers break.
- One more crate-level entry point that needs versioning discipline. `tau-pkg` is also a public-surface concern per G6 (the lockfile + manifest formats are stable surfaces), so this isn't a new responsibility category, just a new symbol within it.

### Neutral

- No ADR amendments required to ADR-0006 / ADR-0007 / ADR-0014 (which introduced and refined the capability machinery). Those ADRs described semantics; the location was not load-bearing.

### Follow-ups

- A future ADR may officially deprecate the `tau_runtime::capability_override` re-export and point all consumers at `tau_pkg::capability_override`. Not in scope for this ADR; the shim has zero cost as long as it stays.

## Alternatives considered

| Alternative | Why rejected |
|---|---|
| Keep `CapabilityOverride` in `tau-runtime`, change `tau-pkg::project` field to opaque `Value` or wrapper | Loses compile-time validation. Pushes type-correctness into runtime checks. Bad ergonomics for tau-pkg's downstream consumers. |
| Keep `CapabilityOverride` in `tau-runtime`, duplicate logic in `tau-app::serve` (don't lift project config to tau-pkg) | Creates two implementations of `build_agent_definition` (tau-cli + tau-app). Drift risk. The same forces that motivated lifting project config to tau-pkg apply here. |
| Move both `project` AND `capability_override` to a new `tau-config` crate | Crate proliferation (tau already has 14 crates). YAGNI — no third consumer in sight. |
| Keep status quo, accept dependency cycle | Cargo does not permit dependency cycles. Not an option. |

## References

- Constitution G6, QG12, QG18.
- ROADMAP §15 (tau serve mode), §4 (capability override implementation).
- This PR: `refactor: lift tau-cli::config to tau-pkg::project` (#133).
- Spec for tau-serve-mode: `docs/superpowers/specs/2026-05-17-tau-serve-mode-design.md` §13.
