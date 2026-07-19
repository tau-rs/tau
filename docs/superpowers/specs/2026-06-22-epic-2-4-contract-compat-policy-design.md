# Spec — EPIC 2.4: compat/versioning policy for both contracts

**Story:** 2.4 (issue #386), milestone EPIC 2 — Lock the two contracts (public ABIs).
**Date:** 2026-06-22
**Accept:** the compat/versioning policy is documented for both contracts; the
`tau-ports` Rust binding is enforced by `cargo-semver-checks` in CI.
**Closes:** EPIC 2 (both contracts published + versioned + drift-tested).

## Purpose

ADR-0056 fixed the *normative* versioning decision (the two contracts are the
semver surface; per-surface breaking-change rules). Stories 2.2 and 2.3 published
and drift-tested the two contracts (IR JSON Schema; WIT host world). 2.4 is the
operator-facing **policy doc** that consolidates the model and resolves every loose
end the earlier ADRs/stories deferred to it — and it wires the one enforcement
ADR-0056 named but never implemented: `cargo-semver-checks` on the `tau-ports`
binding. Landing it closes EPIC 2.

## Context that constrains the design

- The whole workspace is `version = "0.0.0"`; `tau-ports` uses
  `version.workspace = true`. Nothing is independently versioned yet, and no crate
  is published.
- `cargo-semver-checks` is wired nowhere (named only in ADR-0056 / 2.1 docs).
- The two published contracts are already drift-tested: IR schema
  (`tau-ir` `schema_export`/`schema_conformance`, 2.2) and WIT host world
  (`tau-wasm-host/tests/wit_host_drift.rs`, 2.3). Both gated in CI.
- `main` carries the WIT package as `tau:run@0.1.0` (a parallel duplicate effort,
  #434, intended a `tau:host` rename but it did not survive the merge against the
  landed #433 — `tau:run` is the live name).

## Decision

### 1. `tau-ports` gets an independent version line

`tau-ports` leaves the workspace `0.0.0` and declares its own `version = "0.1.0"`
in `crates/tau-ports/Cargo.toml`. It is the one crate with an independent line
because it is the embedding-contract **Rust binding** (ADR-0056). Path-dep
consumers (`tau-ports = { workspace = true }`) are unaffected — path deps ignore
the version field. No other crate changes.

### 2. `cargo-semver-checks` CI lane (the enforcement)

A gated CI job runs `cargo-semver-checks` comparing the PR's `tau-ports` public API
against `origin/main` (baseline-rev, no published baseline needed). It **fails when
`tau-ports` has a breaking API change not matched by an adequate version bump.**
Additive changes pass; a genuine break must bump `0.1.0 → 0.2.0` (at `0.x`, a break
is a minor bump). Breaking is allowed but must be **declared via the bump** — an
explicit, versioned acknowledgement (the "explicit escape hatch, never implicit"
discipline applied to the ABI).

- Mechanism: the `obi1kenobi/cargo-semver-checks-action@v2` (or an explicit
  `cargo semver-checks --baseline-rev origin/main --package tau-ports` after
  installing the binary), in a job mirroring 2.2's `schema-conformance` / 2.3's
  `wit-host-drift` lanes; gated via `ci-summary`'s dynamic aggregation.
- Pre-1.0 posture: started as a hard gate. If it proves noisy under heavy parallel
  churn (two PRs both bumping the version → a rebase collision), the documented
  dial-down is to flip the lane to advisory — without removing it.

### 3. The policy doc — `docs/explanation/contract-compatibility.md`

A new Diátaxis *explanation* page (added to `SUMMARY.md`), the operator-facing
companion to ADR-0056. It consolidates:

- **The versioning model** for all three surfaces — IR `ir_format`, WIT
  `package tau:run@x.y.z`, `tau-ports` crate semver — *referencing* ADR-0056's
  normative breaking-change rules, not duplicating them.
- **The `tau-ports` version resolution** (deferred from 2.1): independent `0.1.0`,
  why it is special, and the `cargo-semver-checks` "break ⇒ bump" rule.
- **The naming + wording clarifications** (deferred from 2.3): the WIT package is
  `tau:run` (ADR-0056's `tau:host` was illustrative of the *mechanism*); ADR-0055's
  "generated from ports" means "**provably non-drifting**" — there is no
  Rust-trait→WIT generator, so the guarantee is delivered by the drift test.
- **The enforcement map** — one table of what guards each surface today: IR schema
  drift test (2.2), WIT freeze/drift test (2.3), `tau-ports` `cargo-semver-checks`
  (this story).
- **Deprecation + migration policy** — the pre-1.0 stance (`0.x`: breaking allowed
  but declared via bump), the path to `1.0.0` (when crates baseline/publish, the IR
  and WIT contracts graduate and semver tightens), and a short "how to make a
  breaking change to each contract" guide.

## Consequences / obligations

- New CI lane + the `cargo-semver-checks` tooling (the action installs the binary).
  `ci-summary.yml` is untouched (it aggregates the CI conclusion dynamically).
- `tau-ports` now carries `0.1.0`; the policy doc records this as deliberate so a
  future reader does not "fix" it back to the workspace version.
- A doc note (in the policy page, not a new ADR) records the `tau:run` naming and
  the "generated = provably non-drifting" reading, reconciling ADR-0055/0056's
  illustrative wording with the shipped reality.

## Out of scope (later milestones)

- Publishing any crate to a registry; graduating any contract to `1.0.0`.
- Giving any crate *other* than `tau-ports` an independent version.
- Migrating the IR/WIT drift tests (already shipped in 2.2/2.3) — only referenced.
- Retroactively amending ADR-0055/0056 text (the policy doc clarifies; the ADRs
  stand).
