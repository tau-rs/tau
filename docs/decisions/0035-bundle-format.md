# ADR-0035: Tau bundle format (§C.1)

**Status:** Accepted
**Date:** 2026-05-19
**Deciders:** titouanlebocq

## Context

Phase 2 §C produces the deployment artifact for tau workflows. The
scope estimate (~6 weeks) decomposes into §C.1 (this ADR — bundle
format), §C.2 (`tau build` producer), and §C.3 (`tau run --bundle`
consumer). C.1 lands first because C.2 and C.3 both depend on a
stable format.

## Decision

A bundle is a **single `.tau` TOML file**, reference-only (no
embedded plugin binaries). Schema v1 holds:

- `schema_version` (u32, currently 1)
- `[bundle]` — self-hash + created_at + tau_version + target
- `[project]` — name + version + tau_toml_sha256
- `[[packages]]` — one per resolved package (name, version, source,
  tree_sha256, optional binary_sha256, required_shapes)
- `[[agents]]` — one per agent (id, backend, system_prompt_sha256,
  required_tools, optional effective_capabilities table)

See spec §4 for the full schema and §6 for code surface.

### Self-hash

`bundle.sha256` is the SHA-256 of the canonical-TOML serialization
of the manifest with `bundle.sha256` itself set to the empty string.
A hand-written `to_canonical_toml` emitter guarantees byte-stable
output across `toml` crate versions; the same emitter is used at
both producer and consumer ends.

### Stability discipline

v1.x is **additive**:
- New optional fields land with `#[serde(default)]` defaults.
- New top-level tables are reserved (e.g., `[binaries]` for a future
  self-contained mode); v1 producers MUST NOT emit reserved tables.
  v1 consumers MUST ignore unknown top-level tables.

v2 is a **breaking change**. Consumers fail loudly
(`BundleParseError::UnsupportedSchemaVersion`) when they meet a
schema_version they don't support.

### Reference-only deferral

Self-contained bundles (with embedded plugin binaries) are deferred
indefinitely per the §C brainstorm. The reservations in v1's schema
preserve forward-compat: a future `[binaries]` table can be added
without breaking existing v1 bundles. The decision to embed binaries
is gated on a concrete air-gap or remote-runner use case.

## Out of scope

- `tau build --target` — Phase 2 §C.2.
- `tau run --bundle` — Phase 2 §C.3.
- Bundle signing / authenticity — Phase 3+.
- Cross-machine reproducibility verification — Phase 2 §E.
- Embedded plugin binaries — deferred per §C brainstorm.

## Spec

See `docs/superpowers/specs/2026-05-19-bundle-format-design.md`.
