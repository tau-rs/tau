# ADR-0034: Tau target triple registry

**Status:** Accepted
**Date:** 2026-05-19
**Deciders:** titouanlebocq

## Context

Phase 2 §B per ROADMAP.md. Tau treats agent workflows as a compiled
language (see `docs/explanation/tau-as-language.md`): write a tau
program once, compile it for a sandbox target triple, and the bundle
runs anywhere a matching adapter exists. §C (`tau build --target`)
will produce content-hashed deployment bundles pinning a target; §D
will guarantee forward-compat for capability vocabulary. Both §C and
§D need a stable, parseable, structural identifier for what a bundle
is built for.

Before this ADR, target triples were informal strings only in
`tau-as-language.md` prose (`linux-native-strict`, `container-podman`,
`remote-vercel`, `wasi-p2`). The shapes were inconsistent and the
identifiers were not code-callable.

## Decision

Codify the target triple as a Bazel-inspired 3-axis structural
identifier living in `tau-ports::target`:

- `Platform`: Linux | Darwin | Windows | Any
- `AdapterFamily`: Native | Container | Remote | Wasi | Passthrough
- `SandboxTier`: Strict | Light | None (existing in `tau-ports`)

Canonical display: `<platform>-<adapter>-<tier>`. Single-segment
`passthrough` is a reserved special.

Adapter ↔ triple satisfaction is a struct comparison: an adapter
satisfies a triple when its platform set includes the triple's
platform, its `RegistryKind` maps to the triple's `AdapterFamily`,
and its `tiers_supported` contains the triple's tier.

v1 ships 5 Available triples and reserves 3 namespaces:

| Triple | Status |
|---|---|
| `linux-native-strict` | Available |
| `linux-native-light` | Available |
| `linux-container-strict` | Available |
| `darwin-native-strict` | Available |
| `passthrough` | Available |
| `windows-native-strict` | Reserved (scaffold; probe Unavailable) |
| `remote-*` | namespace reserved (no individual entries) |
| `wasi-*` | namespace reserved (no individual entries) |

CLI surface: `tau target list`, `tau target show <triple>`, `tau check
--target <triple>`.

## Stability discipline

Once a triple ships as Available, it is **immutable**:

- Adding a new triple is forward-compatible.
- Renaming an Available triple is forbidden.
- Removing an Available triple is forbidden.
- Changing an Available triple's required_shapes set is forbidden
  (would silently invalidate bundles compiled before the change).
- Promoting Reserved → Available is allowed (adds a working adapter).
- Demoting Available → Reserved is forbidden.

Adding a new triple lands via an amendment to this ADR plus a
registry entry.

## Forward-compat hook

§D may add a `capability_vocab_version: u32` field to `TargetTriple`
with default `1`. Old bundles parse with the default; the new field
surfaces only when an explicit non-default value is requested.

## Out of scope

- `tau build --target` — Phase 2 §C.
- Bundle format — Phase 2 §C.
- Capability vocabulary versioning — Phase 2 §D.

## Spec

See `docs/superpowers/specs/2026-05-19-target-triple-registry-design.md`.
