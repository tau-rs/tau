# ADR-0036: Capability vocabulary forward-compatibility

**Status:** Accepted
**Date:** 2026-05-29
**Deciders:** titouanlebocq

## Context

A deployment bundle compiled against tau v1.2 must continue to run on
tau v1.3+ as new capability shapes are added (Phase 2 §D). The
mechanism already exists — `Capability`/`CapabilityShape` are
`#[non_exhaustive]` with `Custom` escape hatches (ADR-0002), and the
bundle format is additive with a `schema_version` hard-gate and no
`deny_unknown_fields` (ADR-0035). What was missing is *discipline*:
nothing prevented a future change from silently breaking these
guarantees, and the rules were not stated in one place.

## Decision

The following rules are binding and enforced by the forward-compat
guard tests in `crates/tau-pkg/src/bundle/manifest.rs` and
`crates/tau-pkg/src/bundle/build.rs`:

- **R1.** Wire-format structs (`BundleManifest` and its sub-structs;
  package/agent manifest structs) MUST NOT use
  `#[serde(deny_unknown_fields)]`. Unknown fields and tables are
  tolerated and ignored.
- **R2.** `Capability` and `CapabilityShape` remain `#[non_exhaustive]`
  and retain their `Custom` variants. An unknown capability `kind`
  deserializes to `Custom` (never an error); the `Custom` match arm is
  load-bearing.
- **R3.** `BundleManifest.schema_version` stays `1` for all additive
  changes. A bump to `2` is BREAKING, requires its own ADR and a
  migration story, and is not routine.
- **R4.** New capability shapes are added additively: a new typed
  `Capability` variant plus, where it fits the `Vec<String>` allow/deny
  shape, a new pair on `BundleEffectiveCapabilities` with
  `#[serde(default, skip_serializing_if = "Vec::is_empty")]`. Older
  parsers ignore the new field; the guard tests must keep passing.
- **R5.** Any new `BundleEffectiveCapabilities` field MUST also be
  emitted in `bundle/canonical.rs::write_effective_capabilities` (the
  hand-rolled canonical serializer bypasses serde) — otherwise it is
  excluded from the self-hash.

## Consequences

- Forward-compatibility is now regression-protected: the guard tests
  fail CI if any rule is violated.
- Adding `skill.spawn` to `BundleEffectiveCapabilities` (the §C.2.3
  gap) is the first worked example of R4/R5.
- `task_list`, `plan`, `custom`, and the `fs.write max_bytes` /
  `net.http methods` sub-fields remain unrepresented in the bundle
  record (documented limitation); the runtime still enforces them.
- New obligation: future capability-shape additions follow R4/R5 and
  must extend the guard tests.

## Alternatives considered

- **A `min_tau_version` / version-negotiation field on bundles.**
  Rejected: the `schema_version` hard-gate plus the additive rule
  already deliver the §D guarantee; negotiation adds wire complexity
  for no benefit at v1 (YAGNI).
- **`deny_unknown_fields` + explicit migration shims per version.**
  Rejected: it inverts the forward-compat default (old parsers would
  reject new bundles) — exactly the failure mode §D must prevent.
