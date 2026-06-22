# Spec — EPIC 2.2: publish the IR JSON Schema + a conformance kit for frontend authors

**Story:** 2.2 (issue #384), milestone EPIC 2 — Lock the two contracts (public ABIs).
**Date:** 2026-06-22
**Accept:** the IR JSON Schema is published (version-pinned, drift-tested against
the serde types); a sample IR validates against it; invalid IR is rejected.
**Builds on:** ADR-0056 — the authoring contract is versioned by `ir_format`, so
the published schema is version-pinned and its `$id` carries that version.

## Purpose

ADR-0056 declared the IR JSON schema one of tau's two public contracts. This
story makes it real: generate a JSON Schema document **from the `tau-ir` serde
types** (never hand-maintained — the "one source → no drift" law), publish it
version-pinned, and ship a portable conformance kit a non-Rust frontend author
(EPIC 5) can use to validate their generated IR in any language.

## Context that constrains the design

- `tau-ir` is `no_std` (alloc), uses `serde` + `serde_json`, has **no `schemars`**
  today. Its own types have **no hand-written serde** (clean derives), but
  `IrModule` reaches into `tau-ports` (`TargetTriple`) and `tau-domain` types and
  chrono `DateTime`, some of which have custom serde (ADR-0005). A structurally
  derived schema can diverge from the actual wire format for those types.
- `jsonschema = "0.46"` is already a workspace dependency (std; fine for a
  test/CI validator, never the run path).
- The IR carries `IrFormatVersion::CURRENT` = `v2.2.0` (`tau-ir::module`).
- No `schemas/` directory exists yet — greenfield for the published artifact.
- The live docs site is `https://lebocqtitouan.github.io/tau/` (CLAUDE.md).

## Decision (Approach A — generated + drift-tested)

The schema is **generated from the serde types via `schemars`**, checked in
version-pinned, and guarded by two tests. Five units, each independently testable:

### 1. `schema` Cargo feature (std-only, opt-in)

A new `schema` feature gates every `schemars::JsonSchema` derive. It is **never
compiled in the no_std / wasm run path** — only CI, the generator, and the
validate test enable it. Because schemars' derive must live *on* the type
definition (orphan rule), the feature plumbs across **`tau-ir` + `tau-ports` +
`tau-domain`**: every type reachable from `IrModule` gains
`#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]`, and each crate
gets a `schema` feature that turns its dependencies' `schema` features on.

Foreign types and custom-serde types get **hand-written `JsonSchema` impls** (in
the crate that owns the type, behind that crate's `schema` feature) so the schema
matches the *actual JSON*, not a structural guess. Known candidates: chrono
`DateTime` (schemars has a chrono integration to reuse), `TargetTriple`, and any
`tau-domain` type with custom serde reachable from `IrModule`.

The exact reachable-type set is unknown until enumerated — **the first
implementation task is an inventory** of every type reachable from `IrModule`,
flagging which need hand-written impls. That inventory sizes the plumbing; if it
is materially larger than expected, scope is revisited before deriving begins.

### 2. Schema generator

A generator (a `#[test]` in `tau-ir` behind `schema`, or an xtask — the plan
picks one) builds the schema from `IrModule` via schemars configured for **JSON
Schema draft 2020-12**, then injects:

- `$id` = `https://lebocqtitouan.github.io/tau/schemas/ir/v2.2.0/tau-ir.schema.json`
- `title` = `tau IR module (ir_format v2.2.0)`
- `x-tau-ir-format` = `IrFormatVersion::CURRENT` (machine-readable, single-sourced)

and writes `schemas/ir/tau-ir.v2.2.0.schema.json`. The version segment is sourced
from `IrFormatVersion::CURRENT`, never typed by hand — so the next `ir_format`
bump produces a new file (`tau-ir.v2.3.0.schema.json`) and the old one stays
immutable (ADR-0056: previously-valid IR stays valid).

### 3. Drift test

Regenerate the schema in-memory and assert byte-equality with the checked-in
`schemas/ir/tau-ir.v2.2.0.schema.json`. This makes the published artifact
*provably* the serde types — the no-drift discipline ADR-0056 mandates. Mirrors
the IR's own `verify --bundle` byte-equality and the WIT drift test of Story 2.3.

### 4. Conformance kit (portable, language-neutral)

Co-located with the schema so an external author can copy it:

```
schemas/ir/
  tau-ir.v2.2.0.schema.json
  conformance/
    README.md            # validate IR in ANY language: point a JSON-Schema
                         #   validator at the schema + these samples
    valid/
      minimal.json       # smallest legal IrModule
      agents-tools.json  # agent + tool nodes, capability table
      triggers.json      # trigger bindings present
      durable.json       # durable / checkpoint fields present
    invalid/
      missing-ir-format.json   # required field absent     → MUST fail
      unknown-node-kind.json   # bad enum variant          → MUST fail
```

Valid samples are curated/derived from existing `tau-ir-conformance` fixtures
rather than invented. Invalid samples prove the schema is not too loose. MVP size
is 4 valid + 2 invalid — enough to cover the major IR shapes and prove tightness;
the set grows with the IR, not eagerly.

### 5. Validate test

One CI test (in `tau-ir` behind `schema`, using `jsonschema 0.46`): every
`valid/*.json` validates OK against the generated schema; every `invalid/*.json`
is rejected. This is simultaneously the **"a sample IR validates" acceptance
gate**, the **schema-tightness proof**, and the **custom-serde safety net** (a
schema that doesn't match real wire format fails here).

### 6. Published surface

- `schemas/ir/tau-ir.v2.2.0.schema.json` (version-pinned, checked in).
- One mdbook reference page `docs/reference/ir-json-schema.md` (added to
  `SUMMARY.md`) documenting the schema, its `$id`/versioning, and how to use the
  conformance kit — so "published" means *documented for authors*, not just a
  file in the tree.

## Consequences / obligations

- New dev-dependency surface: `schemars` (workspace dep, std, behind the `schema`
  feature only). The plan pins the exact version + draft-2020-12 configuration.
- New CI: a job that builds with `--features schema` and runs the drift +
  validate tests. The `schema` feature must NOT leak into the default/no_std
  build — a guard (e.g. the existing no-default-features CI lane) confirms it.
- `schemas/` is a new top-level published directory. Serving it on the live Pages
  site (so `$id` resolves) is a small follow-up — copy `schemas/` into the book
  output — and may be folded into this story or deferred; the `$id` convention is
  canonical regardless.

## Out of scope (later stories)

- Authoring-SDK codegen from the schema (EPIC 5.3).
- The WIT host world / embedding contract + its drift test (Story 2.3).
- Compat/versioning policy doc + the `tau-ports` version-decoupling question
  (Story 2.4).
- Exhaustive per-field sample coverage — the kit grows with the IR.
