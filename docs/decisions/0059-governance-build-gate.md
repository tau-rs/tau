# ADR-0059: Governance is a build gate; bundles pin their constitution

**Status:** Proposed
**Date:** 2026-07-18
**Deciders:** tau core

## Context

The root `[allow]` constitution (ADR-0057, EPIC 1 stories 1.2–1.5) declares a
capability ceiling and a closed world: every tool a project defines or
references, every tool→MCP binding, and every agent's package capabilities must
sit within the ceiling and be registered under `[allow.*]`. Until now that
constitution was enforced in exactly one place — `tau check`
(`crates/tau-cli/src/cmd/check/categories/governance.rs`), which reports
`Severity::Error` findings and exits 2.

`tau build` contained **zero** governance references. A project whose tool
capabilities exceeded its own ceiling would build cleanly and produce a
runnable `.tau` bundle; nothing re-checked the constitution at run time either.
Governance was therefore advisory — a developer who never ran `tau check` could
ship an over-reaching artifact. This contradicts tau's Rust-like discipline:
any check that *can* run at build time *must* run at build time, with escape
hatches made explicit rather than implicit.

A second gap: even a correctly-governed bundle carries no record of *which*
constitution it was checked against. Editing `[allow]` after a build — widening
the ceiling — leaves previously-built artifacts indistinguishable from ones
built under the looser rules. Artifacts can outlive the constitution that
sanctioned them.

## Decision

**1. Governance is a build gate, sharing one implementation with `tau check`.**

The governance lattice core is relocated from `tau-cli` into
`tau-pkg::governance` (`enforce_governance(project, scope) -> Vec<GovFinding>`),
so `tau build` can enforce it without depending on the CLI check subsystem.
`tau-pkg` does not depend on `tau-ir`, so the gate operates on the
*pre-lowering* view — the parsed `ProjectConfig` plus each agent's resolved
package capabilities (lockfile → installed manifest) — exactly the inputs the
check category already consumed.

`tau check`'s governance category becomes a thin presentation wrapper over the
relocated core (mapping `GovFinding` → `CheckFinding`, adding source location
and remediation). One implementation, two callers: **check and build cannot
drift** on what counts as a violation. A no-drift test asserts both entry points
agree on the Error finding set by `(rule_id, structured)` value, not by
rendered string.

The gate runs in `tau build` after lowering + typecheck succeed and before the
bundle is encoded, and on the dev-mode (`--bundle`-less) `tau run` path, which
lowers and executes the cwd project directly. Any `GovSeverity::Error` finding
is fatal (exit 2). `NeedsSetup` / `Warning` / `Note` findings do not fail the
gate — an uninstalled package cannot be lattice-checked at build time, matching
`tau check`'s non-fatal treatment of the same conditions.

**2. An absent `[allow]` section is a fatal build error (decision D2-B).**

`tau build` / dev-`tau run` refuse to build a project that declares no `[allow]`
constitution (exit 2). Governed-by-default is the Rust-like stance: you either
declare a constitution (an empty `[allow.models]`-only section is a conscious
declaration) or opt out explicitly. This is a deliberate behavior break from
the prior "ungoverned builds succeed silently" default.

`tau check` keeps treating an absent `[allow]` as a **non-fatal warning**
(`no_constitution`). The severity *policy* difference lives in the two callers;
the fact reported by the shared core is identical, so no logic is duplicated.

**3. `--no-governance` is the explicit escape hatch.**

`tau build --no-governance` / `tau run --no-governance` skip the gate, print a
WARNING banner naming the skipped checks (`over_reach, closed_world, lattice`),
and cause the bundle's governance record to be stamped `verdict = "skipped"`.

**4. Bundles pin the constitution they were checked against.**

The bundle manifest gains an additive, optional `governance` record:

```
"governance": {
  "ceiling_sha256": "<64 hex, or absent when no [allow] declared>",
  "checked_with": "<tau CARGO_PKG_VERSION>",
  "verdict": "pass" | "skipped"
}
```

`ceiling_sha256` is the SHA-256 of the **parsed** `AllowConfig` serialized
through the same canonical-JSON discipline as the IR (BTreeMap ordering) — so it
is insensitive to whitespace and comments in `tau.toml`. `tau verify --bundle`
and `tau run --bundle` recompute the hash from the cwd `[allow]` and compare;
a mismatch is a structured `GovernanceDrift` error, in the same UX family as the
existing `IrSourceDivergence`. A `verdict: "skipped"` bundle warns on verify and
requires `--no-governance` to run.

The field is optional: bundles built before this ADR (no `governance` record)
verify exactly as before.

## Consequences

- **Positive:** governance moves from advisory to enforced; artifacts can no
  longer outlive an edited constitution; check and build share one lattice
  implementation and are drift-proof by construction.
- **Negative / migration:** every existing project that relied on an implicit
  ungoverned build must now declare an `[allow]` section (moving `[models]` into
  `[allow.models]`) or pass `--no-governance`. Existing build/run test fixtures
  are migrated accordingly.
- The lattice *soundness* work (glob sampling, path normalization, host
  semantics — decisions D3/D4), runtime attenuation/clamp, and subflow
  `cap_subset` enforcement are explicitly **out of scope** here. This ADR is
  enforcement *placement* only; it does not change what the lattice computes.

## Alternatives considered

- **Absent `[allow]` warns and builds (D2-A).** Matches `tau check`'s current
  stance and avoids breaking existing projects, but leaves the ungoverned-build
  hole open by default — rejected in favor of Rust-like governed-by-default.
- **Keep governance in `tau-cli`, call it from build via a CLI-internal path.**
  Rejected: `tau build`'s core (`tau_pkg::bundle`) lives in `tau-pkg`, and a
  future non-CLI builder would re-open the drift risk. Relocating the core to
  `tau-pkg` makes the gate available wherever a bundle is produced.
- **Hash the raw `[allow]` TOML text.** Rejected: whitespace/comment edits would
  spuriously trip `GovernanceDrift`. Hashing the parsed, canonically-serialized
  `AllowConfig` pins semantics, not formatting.
