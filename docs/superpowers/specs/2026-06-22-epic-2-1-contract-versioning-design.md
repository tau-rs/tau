# Spec — EPIC 2.1: the two contracts are the semver stability surface

**Story:** 2.1 (issue #383), milestone EPIC 2 — Lock the two contracts (public ABIs).
**Date:** 2026-06-22
**Deliverable:** one ADR, `docs/decisions/0056-contract-versioning-stability-surface.md`.
**Accept:** ADR merged; mdbook builds; linkcheck clean. **No code in this story.**

## Purpose

ADR-0055 fixed tau's *identity* — the product is `tau-runtime-core` plus two
versioned public contracts, and the CLI is the reference host. But 0055 left the
stability *mechanics* implicit and carried a latent ambiguity: its decision text
says the surface is "the two contracts **+ the no_std ports API**" (reads like
three surfaces) while two lines later it says "versioning + conformance attach to
**the two contracts**" and its alternatives section explicitly rejects a
three-contract model.

Story 2.1 makes the versioning scheme **normative and precise** so the rest of
EPIC 2 (2.2 schema publish, 2.3 WIT generation + drift test, 2.4 operator policy)
and the parallel in-flight work (durability #373, β.7.5) cannot diverge from a
single agreed contract shape. 2.1 is constitutional: it lands the *decision*;
2.4 lands the *operator policy* built on it.

## Context that constrains the decision

- The IR already carries a version: `tau-ir::module` has an `ir_format` semver
  field (currently **v2.2.0**), separate from `tau_version`; both feed the IR
  content hash.
- `tau-ports` is a broad no_std Rust trait surface (llm/tool/storage/
  capability_gate + credential/orchestration/target/time/random/skill_resolver/
  capability_resolver). The **WIT host world** is only the "minimal 3-function"
  subset, **generated from** ports (Story 2.3) — it is never hand-maintained.
- The three surfaces have **different consumers** and a **containment**
  relationship:

  | surface | consumer | churn tolerance |
  |---|---|---|
  | IR JSON schema | frontend / SDK authors (EPIC 5) | track `ir_format` |
  | WIT host world | wasm-guest embedders, any language (EPIC 7 Variant A) | want **zero** false-alarm bumps |
  | ports API | no_std Rust embedders (EPIC 7 Variant B) | see the full surface, correctly |

  `WIT ⊊ ports`, WIT generated from ports ⟹ every WIT-breaking change implies a
  ports-breaking change, but **not** vice-versa (storage/orchestration churn need
  not touch the 3 host functions).

## Decision (Approach D — native-mechanism versioning)

1. **The public ABI is exactly two published, conformance-kitted contracts**,
   each versioned by its **own native mechanism** — "one source → no drift"
   applied to versioning itself (no bespoke version registry, no hand-maintained
   version constant):
   - **Authoring contract** — IR JSON (including the root `[allow]` governance
     section), versioned by the existing `ir_format` field.
   - **Embedding contract** — versioned by the **WIT package version**,
     `package tau:host@x.y.z;`.
2. **The no_std ports API is the Rust-native binding of the embedding
   contract** — versioned by `tau-ports` **crate semver**, enforced by
   **`cargo-semver-checks`** in CI. It is *not* a third published contract. This
   is the precise meaning of 0055's "+ the no_std ports API" phrasing.
3. **Containment invariant:** `WIT ⊊ ports`; WIT is generated from ports
   (Story 2.3) ⟹ every WIT-breaking change implies a ports-breaking change. The
   2.3 drift test enforces this.
4. **Breaking-change definition per surface** (the normative semver semantics):
   - *IR `ir_format`* — **major:** remove/rename/retype a field, or change the
     canonical encoding such that an older runtime cannot load the IR or a
     previously-valid IR becomes invalid; **minor:** additive (new optional
     field / forward-compatible variant); **patch:** non-semantic / doc only.
   - *WIT `tau:host`* — **major:** remove, re-signature, or re-semantic any of
     the 3 host functions; **minor:** additive host function or additive optional
     parameter; **patch:** doc only.
   - *ports crate* — standard Rust semver, enforced by `cargo-semver-checks`.
5. **CLI verbs are explicitly OUT of this surface** — they evolve under a looser
   compatibility policy documented with the CLI (restates 0055).
6. **Relationship to ADR-0055:** 0055 is the *premise*, not superseded. 2.1
   resolves the "+ ports API" ambiguity (point 2) and makes 0055's "versioning
   attaches to the two contracts" operational. A short "Relationship to ADR-0055"
   note in the ADR states this; 0055's status is unchanged.

## Consequences (obligations this ADR names)

- **New CI obligation:** a `cargo-semver-checks` lane for `tau-ports` (does not
  exist today). Named here; wired by 2.3/2.4.
- **Wires the rest of EPIC 2:** 2.2 surfaces the IR JSON Schema `$id`/version
  from `ir_format`; 2.3 emits WIT carrying `@x.y.z` plus the containment drift
  test; 2.4 owns the operator policy.
- **`tau-ports` versioning wrinkle:** `tau-ports` likely shares the workspace
  version (`version.workspace = true`) today. To give it an independent semver
  signal it may need to break out of the workspace version, *or* we accept a
  coarse workspace version and rely on `cargo-semver-checks` to flag inadequate
  bumps. Flagged for 2.4 to resolve; 2.1 only names the constraint.
- **No code in 2.1** — pure ADR; the acceptance gate is "ADR merged" + clean
  mdbook/linkcheck.

## Alternatives considered and rejected

- **A — one bespoke `host-abi` version covering WIT ⊕ ports together.**
  Rejected: a ports-internal break (e.g. `storage`) that never touches the 3 host
  functions would still bump the exact number a wasm-guest author tracks —
  false-alarm major bumps land on tau's flagship "embed in any language" persona.
  A hand-maintained `HOST_ABI_VERSION` constant also violates one-source-no-drift.
- **B — three independent published version lines** (`ir_format` +
  `wit-world-version` + `ports-crate-semver`). Rejected: reintroduces the
  three-surface ceremony 0055 explicitly killed, and creates WIT↔ports version
  drift risk (two hand-coordinated numbers for one generated relationship).
- **C — one unified `tau-abi` semver** covering IR + WIT + ports together.
  Rejected: couples authoring and embedding — an IR-only additive change forces
  an embedding-version bump and churns embedders who depend on only one contract,
  and vice-versa.

## Out of scope (belongs to later stories)

- Publishing the IR JSON Schema / conformance kit (2.2).
- Generating the WIT world + the containment drift test + the `cargo-semver-checks`
  CI lane (2.3).
- Deprecation windows, migration how-tos, the support matrix, and the
  `tau-ports`-version-decoupling resolution (2.4).
