# Contract compatibility & versioning

tau's public stability surface is the **two contracts** of
[ADR-0056](../decisions/0056-contract-versioning-stability-surface.md): the
authoring/IR schema and the WIT host world. This page is the operator-facing
companion to that ADR — the *how it works in practice*. The ADR holds the
normative breaking-change rules; this page does not restate them, it maps them to
what ships.

## The three version surfaces

| surface | versioned by | who tracks it |
|---|---|---|
| Authoring (IR JSON schema) | the IR `ir_format` field (e.g. `v2.3.0`) | frontend / SDK authors |
| Embedding (WIT host world) | the WIT package version `package tau:run@x.y.z` | wasm-guest embedders (any language) |
| `tau-ports` (the embedding contract's Rust binding) | `tau-ports` crate semver (`0.1.0`) | no_std Rust embedders |

The two *published, conformance-kitted* contracts are the IR schema and the WIT
world. `tau-ports` is the **Rust-native binding** of the embedding contract, not a
third published contract — its stability is delivered by ordinary crate semver.

## What guards each surface today

| surface | guard | where |
|---|---|---|
| IR schema | byte-equal drift test + conformance kit | `tau-ir` `schema_export` / `schema_conformance` |
| WIT host world | parse-based freeze/drift test (frozen 3-function surface) | `tau-wasm-host/tests/wit_host_drift.rs` |
| `tau-ports` ABI | `cargo-semver-checks` vs `origin/main` (break ⇒ version bump) | CI job `ports-semver` |

## `tau-ports` is the one independently-versioned crate

The workspace is `0.0.0`; `tau-ports` deliberately carries its own `0.1.0`. It is
the embedding contract's Rust binding (ADR-0056), so it is the one crate whose ABI
is semver-gated. Path-dependency consumers are unaffected. **Do not "fix" this back
to the workspace version** — the independent line is what makes the
`cargo-semver-checks` gate meaningful.

The gate: a breaking change to `tau-ports`'s public API must be **declared** by an
adequate version bump (at `0.x`, a break is a minor bump, `0.1.0 → 0.2.0`). Additive
changes need no bump. Breaking is allowed — but never implicitly; the bump is the
explicit, versioned acknowledgement.

## Pre-1.0 posture and the path to 1.0

Everything is `0.x` and unpublished. Breaking changes are permitted, but each must
be declared (a version bump for `tau-ports`; an `ir_format` / WIT-package bump for
the contracts, per ADR-0056). When the project baselines and publishes crates, the
IR and WIT contracts graduate to `1.0.0` and semver tightens to full
backwards-compatibility guarantees.

## Naming & wording notes

- The WIT package is **`tau:run`**, not `tau:host`. ADR-0056 wrote `tau:host@x.y.z`
  *illustratively* (to show the version-by-WIT-package mechanism); the shipped
  package is `tau:run` because it carries both the host imports and the `run` export.
- ADR-0055 says the WIT world is "generated from the ports." Read this as **provably
  non-drifting**: there is no Rust-trait→WIT generator (and the boundary is
  JSON-stringly-typed by design), so the guarantee is delivered by the drift test,
  not literal code generation.

## How to make a breaking change to each contract

- **IR schema:** bump `ir_format` per ADR-0056 (major for a removed/retyped field,
  minor for additive), regenerate `schemas/ir/tau-ir.v<X>.schema.json`, update the
  conformance kit. The drift test enforces regeneration.
- **WIT host world:** edit `wit/tau-run.wit` and update `wit_host_drift.rs` + the
  bindings deliberately; bump the WIT package version. The freeze test is the
  tripwire.
- **`tau-ports`:** make the change and bump `tau-ports`'s version. `cargo-semver-checks`
  fails the PR if the bump is inadequate for the API delta.
