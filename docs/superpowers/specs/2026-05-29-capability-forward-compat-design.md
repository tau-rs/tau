# Capability vocabulary forward-compatibility — Phase 2 §D design

**Status:** Accepted
**Date:** 2026-05-29
**Authors:** titouanlebocq
**Depends on:** ADR-0002 (manifest/capability format), ADR-0035 (bundle format), §C.2.3 effective-capabilities (PR #253)

## 1. Goal

A bundle compiled against tau v1.2 must continue to run on tau v1.3+ as new capability shapes are added. The supporting mechanism **already exists** (see §2); this sub-project's deliverable is **discipline**: lock the existing forward-compat behavior with contract tests that fail CI if it regresses, codify the rules in an ADR, and prove the discipline by performing one real additive evolution (`skill.spawn`).

## 2. What already works (do NOT rebuild)

Forward-compatibility is forward-compatible by construction today:

- `tau_domain::Capability` is `#[non_exhaustive]` with a `Custom { name, params }` escape hatch. An unknown `kind` deserializes to `Custom`, preserves all fields, and round-trips on serialize (`capability.rs`; ADR-0002).
- `tau_domain::CapabilityShape` is `#[non_exhaustive]` with `Custom { name }`; unmapped capabilities resolve to a `Custom` shape.
- `BundleManifest::parse_str` hard-gates `schema_version` (v1 accepted, v2+ → `BundleParseError::UnsupportedSchemaVersion`). No bundle struct uses `deny_unknown_fields`, so unknown future top-level tables and unknown fields inside `[bundle]`/`[[agents]]`/`[[packages]]` are silently tolerated (ADR-0035 "additive" rule). `BackendRef.extra` is a `#[serde(flatten)]` catch-all.
- `BundleEffectiveCapabilities` (all `Vec<String>` fields, no `deny_unknown_fields`) silently accepts a future `allow_*` field on an older parser.

This spec adds **no new tolerance mechanism**. It makes the existing tolerance *enforced and documented*, plus one worked schema extension.

## 3. Headline decisions

- **Contract is encoded as guard tests, not just prose.** A dedicated forward-compat test module exercises every tolerance point so a future change that breaks it (e.g. adding `deny_unknown_fields`, deleting a `Custom` arm, casually bumping `schema_version`) fails CI.
- **Rules are codified in a new ADR-0036**, cross-referencing ADR-0002 and ADR-0035, so the discipline is discoverable and binding.
- **Prove the discipline with one additive evolution: add `skill.spawn`** to `BundleEffectiveCapabilities` (`allow_skill_spawn`/`deny_skill_spawn`) and wire it through `effective_to_bundle`. This closes the §C.2.3 limitation AND demonstrates that adding a shape is additive (an older parser ignores the new field — asserted by the guard test).
- **Scope boundary:** only `skill.spawn` (it fits the existing `Vec<String>` allow/deny shape). The `fs.write max_bytes` and `net.http methods` sub-fields stay out — they need richer (non-`Vec<String>`) types and are a separate design decision, not forward-compat discipline. No version-negotiation machinery (the `schema_version` gate + additive rule already cover the contract; YAGNI).

## 4. Design

### 4.1 Forward-compat contract guard tests

New test module `crates/tau-pkg/src/bundle/forward_compat_tests.rs` (or a `#[cfg(test)] mod forward_compat` inside `manifest.rs` — match the crate's existing pattern; a dedicated file is cleaner). Tests:

1. **`future_bundle_parses_with_all_tolerance_points`** — a single hand-written `schema_version = 1` TOML fixture that simultaneously contains:
   - an unknown top-level table `[future_section]`,
   - an unknown key inside `[bundle]` (e.g. `future_meta = "x"`),
   - an unknown key inside an `[[agents]]` entry (e.g. `future_agent_field = 1`),
   - an unknown key inside `[[packages]]`,
   - an unknown `allow_*` key inside an agent's `[agents.effective_capabilities]` (e.g. `allow_future_shape = ["x"]`),
   - a `[[agents]]` `backend` table with an unknown key (lands in `BackendRef.extra`).
   Asserts: `BundleManifest::parse_str` succeeds AND the known fields are intact (project name, schema_version, the agent id, a known `allow_fs_read` value, and that the unknown backend key is captured in `extra`). This is the core contract: a future bundle parses on today's code.

2. **`schema_version_two_is_rejected`** — `schema_version = 2` → `BundleParseError::UnsupportedSchemaVersion { found: 2 }`. (Pins the hard-gate so a casual bump can't silently pass.)

3. **`custom_capability_survives_bundle_build`** — build a bundle from an override-agent project whose home package grants a `Custom` capability kind (e.g. `kind = "mcp.tool.use"`) plus a known `fs.read`; assert the build succeeds, records the known `fs.read`, and does NOT crash on the Custom kind (it is dropped from `effective_capabilities` via the catch-all — documented behavior, not an error). Proves an OLD tau building against a package with a NEW capability shape degrades gracefully.

4. **`effective_capabilities_unknown_allow_field_is_ignored`** — a bundle whose `effective_capabilities` table has `allow_some_future_shape = ["/x/**"]` plus a known `allow_fs_read`; parse and assert the known field is read and no error occurs. (Targeted forward-compat for the struct §C.2.3 extends.)

These tests reference only stable public API (`BundleManifest::parse_str`, the manifest structs). They encode the contract; if a future contributor adds `deny_unknown_fields` or removes a tolerance, the relevant test fails.

### 4.2 ADR-0036

New file `docs/decisions/0036-capability-forward-compatibility.md` (template format: Context / Decision / Consequences / Alternatives considered). It states, as binding rules:

- **R1.** Wire-format structs (`BundleManifest` and its sub-structs; package/agent manifest structs) MUST NOT use `#[serde(deny_unknown_fields)]`. Unknown fields/tables are tolerated and ignored.
- **R2.** `Capability` and `CapabilityShape` MUST remain `#[non_exhaustive]` and MUST retain their `Custom` variants. An unknown capability `kind` deserializes to `Custom` (never an error). The `Custom` arm in any capability `match` is load-bearing — do not remove it.
- **R3.** `BundleManifest.schema_version` stays at `1` for all additive changes. A bump to `2` is a BREAKING change requiring its own ADR and a migration story; it is not a routine action.
- **R4.** New capability shapes are added additively: a new typed `Capability` variant + (where it fits) a new `Vec<String>` allow/deny pair on `BundleEffectiveCapabilities` with `#[serde(default, skip_serializing_if)]`. Older parsers ignore the new field; the forward-compat guard tests (§4.1) must continue to pass.
- **R5.** The discipline is enforced by the guard tests in §4.1 and the canonical-emission completeness test added in §C.2.2 (`canonical_emits_all_bundle_meta_fields`). New `BundleEffectiveCapabilities` fields MUST also be emitted in `bundle/canonical.rs::write_effective_capabilities` (the hand-rolled canonical serializer bypasses serde).

ADR-0035 gets a one-line amendment pointing to ADR-0036 for the capability-vocabulary specifics.

### 4.3 Worked example: add `skill.spawn` to `BundleEffectiveCapabilities`

- **`crates/tau-pkg/src/bundle/manifest.rs`:** add two fields to `BundleEffectiveCapabilities` (after `deny_agent_spawn`), each `#[serde(default, skip_serializing_if = "Vec::is_empty")]`:
  ```rust
  pub allow_skill_spawn: Vec<String>,
  pub deny_skill_spawn: Vec<String>,
  ```
  Extend `BundleEffectiveCapabilities::is_empty()` to include both new fields (it currently ANDs the existing 10).
- **`crates/tau-pkg/src/bundle/canonical.rs`:** in `write_effective_capabilities` (line ~114), emit the two new fields after the `agent_spawn` block, each guarded by `if !caps.<field>.is_empty()` and using `write_string_array` (matching every other field). REQUIRED — the canonical serializer is hand-rolled (the §C.2.2 landmine).
- **`crates/tau-pkg/src/bundle/build.rs`:** in `effective_to_bundle`, replace the `_ => {}` drop for skill with a real arm:
  ```rust
  Capability::Skill(SkillCapability::Spawn { allowed_skills, .. }) => {
      out.allow_skill_spawn
          .extend(e.allow_override.clone().unwrap_or_else(|| allowed_skills.clone()));
      out.deny_skill_spawn.extend(e.deny.clone());
  }
  ```
  The catch-all `_ => {}` remains for the still-unrepresentable shapes (task_list, plan, custom). Update the §C.2.3 doc-comment listing dropped shapes (remove `skill.spawn` from it).

## 5. Test plan

**Forward-compat guard tests (§4.1):** the 4 tests above.

**`skill.spawn` extension:**
- **manifest.rs unit:** `effective_caps_is_empty_includes_skill_spawn` — a `BundleEffectiveCapabilities` with only `allow_skill_spawn` populated is NOT `is_empty()`; a default one IS. Plus the existing round-trip test gains skill fields (serialize→parse).
- **canonical.rs unit:** extend `canonical_emits_all_bundle_meta_fields` (or add a sibling) to assert `allow_skill_spawn` is emitted when present and omitted when empty.
- **build.rs unit (replaces the §C.2.3 `build_drops_skill_spawn_from_bundle`):** rename/repurpose to `build_records_skill_spawn_for_override_agent` — home package grants `skill.spawn allowed_skills=["critic"]`; agent has any override (to trigger the path); assert the built bundle's agent has `allow_skill_spawn == ["critic"]`. (The old "skill must be absent" assertion is now wrong — skill.spawn is recorded.)
- **effective_to_bundle unit:** update `effective_to_bundle_drops_unrepresentable_shapes` to use `task_list` or `plan` (a genuinely still-dropped shape) instead of `skill.spawn`; assert `is_empty()`.

**Reproducibility:** the existing `verify_reproducible_with_effective_caps` continues to pass; add coverage so a skill.spawn-bearing bundle reproduces (can fold into that test or add a sibling).

## 6. Out of scope

- `fs.write max_bytes` and `net.http methods` sub-fields (need non-`Vec<String>` representation; separate decision).
- `task_list`, `plan`, `custom` shapes in `BundleEffectiveCapabilities` (no demand; remain dropped, documented).
- Any `min_tau_version` / version-negotiation field on bundles (the `schema_version` gate + additive rule already deliver the §D guarantee).
- Changes to `compute_effective`, the runtime, or `Capability`/`CapabilityShape` themselves (they are already correct).

## 7. References

- ADR-0002 — manifest format / capability canonicalization + Custom escape hatch
- ADR-0035 — bundle format / additive stability discipline
- §C.2.3 spec — `2026-05-29-bundle-effective-capabilities-design.md` (the skill.spawn limitation this closes)
- `crates/tau-domain/src/package/capability.rs` — Capability / CapabilityShape (the existing forward-compat foundation)
- ADR-0034 — target triple registry (the stability-discipline pattern this mirrors)
