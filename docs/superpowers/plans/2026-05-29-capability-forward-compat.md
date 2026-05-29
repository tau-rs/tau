# Capability vocabulary forward-compatibility — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Lock tau's existing capability/bundle forward-compatibility with contract guard tests, codify the rules in ADR-0036, and prove the discipline by additively adding `skill.spawn` to `BundleEffectiveCapabilities` (closing the §C.2.3 gap).

**Architecture:** Three parts — (1) the `skill.spawn` worked example (manifest field + `is_empty` + hand-rolled canonical emission + `effective_to_bundle` arm), (2) a forward-compat guard-test suite that asserts a synthetic "future bundle" parses on today's code, (3) ADR-0036 + an ADR-0035 amendment. No new tolerance mechanism — the mechanism already exists; this makes it enforced and documented.

**Tech Stack:** Rust, serde/toml, `tau_domain::{Capability, SkillCapability}`, `tau_pkg::bundle::{manifest, canonical, build}`. Workspace cargo rules in `CLAUDE.md` apply.

**Spec:** `docs/superpowers/specs/2026-05-29-capability-forward-compat-design.md`

---

## Cargo command reference (per `CLAUDE.md`)

Every cargo command MUST be wrapped (substitute your role for `impl`):

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-pkg
```

Never run bare `cargo`. Always `-p tau-pkg`. Commit identity:

```
git -c user.name="titouanlebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "<msg>"
```

---

## Verified anchors (current code)

- `crates/tau-pkg/src/bundle/manifest.rs`: `BundleEffectiveCapabilities` ends with `pub allow_agent_spawn` (line 152) / `pub deny_agent_spawn` (line 155), struct closes line 156. `is_empty()` (lines 173-184) ANDs all 10 `.is_empty()`, last two are `allow_agent_spawn`/`deny_agent_spawn` (lines 182-183).
- `crates/tau-pkg/src/bundle/canonical.rs`: `write_effective_capabilities` (lines 114-145); the `agent_spawn` emission is the last block (lines 139-144) before the closing `}`.
- `crates/tau-pkg/src/bundle/build.rs`: `effective_to_bundle` has the Agent arm (line 449) then `// skill.spawn / task_list / plan / custom: no bundle field.` + `_ => {}` (lines 457-458). Its `use` line imports `tau_domain::{AgentCapability, Capability, FsCapability, NetCapability, ProcessCapability}` (NO `SkillCapability` yet). The test module has `override_agent_project` (line 1155) whose home-package manifest already grants `skill.spawn allowed_skills = ["critic"]` (line ~1227), `read_agent_caps` (line 1233), `build_drops_skill_spawn_from_bundle` (lines 1262-1281), and `effective_to_bundle_drops_unrepresentable_shapes` (lines 1141-1150) using a `cap(json!)` + `eff(...)` helper.
- `BundleManifest::parse_str` only validates `schema_version == 1`; all sub-structs lack `deny_unknown_fields`. Existing forward-compat tests `unknown_top_level_field_is_accepted` and `backend_extra_captures_unknown_keys` live in `manifest.rs`'s `#[cfg(test)] mod tests` and hand-write full bundle TOML (the pattern to follow).
- String fields in the manifest (sha256s, etc.) are NOT format-validated at parse time, so short placeholder values are fine in fixtures.

---

## File Structure

- `crates/tau-pkg/src/bundle/manifest.rs` — add `allow_skill_spawn`/`deny_skill_spawn` fields + extend `is_empty` + a unit test + the 4 forward-compat guard tests (Tasks 1, 2).
- `crates/tau-pkg/src/bundle/canonical.rs` — emit the two new fields + a canonical unit test (Task 1).
- `crates/tau-pkg/src/bundle/build.rs` — add the `Skill` arm to `effective_to_bundle`, import `SkillCapability`, repurpose two tests (Task 1).
- `docs/decisions/0036-capability-forward-compatibility.md` (new) + `docs/decisions/0035-bundle-format.md` (one-line amendment) (Task 3).

---

## Task 1: Additively add `skill.spawn` to `BundleEffectiveCapabilities`

**Files:**
- Modify: `crates/tau-pkg/src/bundle/manifest.rs` (struct + is_empty + unit test)
- Modify: `crates/tau-pkg/src/bundle/canonical.rs` (emission + unit test)
- Modify: `crates/tau-pkg/src/bundle/build.rs` (effective_to_bundle arm + repurpose 2 tests)

- [ ] **Step 1: Write/repurpose the failing tests**

(a) In `crates/tau-pkg/src/bundle/manifest.rs` `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn effective_caps_is_empty_includes_skill_spawn() {
        let mut caps = BundleEffectiveCapabilities::default();
        assert!(caps.is_empty());
        caps.allow_skill_spawn.push("critic".to_string());
        assert!(!caps.is_empty(), "allow_skill_spawn must count toward non-empty");
    }
```

(b) In `crates/tau-pkg/src/bundle/canonical.rs` `#[cfg(test)] mod tests`, add (matching the file's existing test style — reuse its sample-manifest helper if present, else build a `BundleEffectiveCapabilities` directly):

```rust
    #[test]
    fn canonical_emits_skill_spawn_when_present_and_omits_when_empty() {
        use crate::bundle::manifest::BundleEffectiveCapabilities;
        let mut caps = BundleEffectiveCapabilities::default();
        caps.allow_skill_spawn.push("critic".to_string());
        caps.deny_skill_spawn.push("evil".to_string());
        let mut out = String::new();
        write_effective_capabilities(&mut out, &caps);
        assert!(out.contains("allow_skill_spawn"), "got: {out}");
        assert!(out.contains("deny_skill_spawn"), "got: {out}");
        // Empty caps emit nothing for skill_spawn.
        let mut out2 = String::new();
        write_effective_capabilities(&mut out2, &BundleEffectiveCapabilities::default());
        assert!(!out2.contains("skill_spawn"), "got: {out2}");
    }
```

(If `write_effective_capabilities` is private and the test module is `mod tests` in the same file, it is in scope via `super::`. Confirm the existing tests call sibling private fns; match that.)

(c) In `crates/tau-pkg/src/bundle/build.rs`, REPLACE the existing `build_drops_skill_spawn_from_bundle` test (lines ~1262-1281) with a test asserting skill.spawn is now RECORDED (the fixture `override_agent_project` already grants `skill.spawn allowed_skills=["critic"]`):

```rust
    #[test]
    fn build_records_skill_spawn_for_override_agent() {
        // skill.spawn is now a represented shape: the home package's
        // skill.spawn grant is recorded in the agent's effective caps.
        let tmp = tempdir().unwrap();
        override_agent_project(tmp.path());
        let caps = read_agent_caps(tmp.path());
        assert_eq!(caps.allow_skill_spawn, vec!["critic".to_string()]);
        // Sanity: the other representable shapes still recorded.
        assert_eq!(caps.allow_fs_read, vec!["/data/**".to_string()]);
    }
```

(d) In `crates/tau-pkg/src/bundle/build.rs`, UPDATE `effective_to_bundle_drops_unrepresentable_shapes` (lines ~1141-1150) to use a STILL-unrepresentable shape (`task_list`) instead of `skill.spawn`:

```rust
    #[test]
    fn effective_to_bundle_drops_unrepresentable_shapes() {
        // task_list has no BundleEffectiveCapabilities field and must be dropped.
        let e = eff(
            cap(serde_json::json!({"kind": "task_list", "mode": "read"})),
            None,
            &[],
        );
        let b = effective_to_bundle(&[e]);
        assert!(b.is_empty(), "task_list must be dropped: {b:?}");
    }
```

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::`
Expected: FAIL to compile — `BundleEffectiveCapabilities` has no `allow_skill_spawn`/`deny_skill_spawn` fields yet (the manifest/canonical/build tests reference them).

- [ ] **Step 2: Add the struct fields**

In `crates/tau-pkg/src/bundle/manifest.rs`, after `pub deny_agent_spawn: Vec<String>,` (line 155, before the struct's closing `}` on line 156), add:

```rust
    /// skill.spawn allow-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_skill_spawn: Vec<String>,
    /// skill.spawn deny-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_skill_spawn: Vec<String>,
```

- [ ] **Step 3: Extend `is_empty`**

In the same file, in `is_empty()` (lines 173-184), add two more `&&` clauses after `self.deny_agent_spawn.is_empty()`:

```rust
            && self.allow_agent_spawn.is_empty()
            && self.deny_agent_spawn.is_empty()
            && self.allow_skill_spawn.is_empty()
            && self.deny_skill_spawn.is_empty()
```

- [ ] **Step 4: Emit in the hand-rolled canonical serializer**

In `crates/tau-pkg/src/bundle/canonical.rs`, in `write_effective_capabilities`, after the `deny_agent_spawn` block (lines 142-144) and before the closing `}` (line 145), add:

```rust
    if !caps.allow_skill_spawn.is_empty() {
        write_string_array(out, "allow_skill_spawn", caps.allow_skill_spawn.clone());
    }
    if !caps.deny_skill_spawn.is_empty() {
        write_string_array(out, "deny_skill_spawn", caps.deny_skill_spawn.clone());
    }
```

- [ ] **Step 5: Add the `Skill` arm to `effective_to_bundle`**

In `crates/tau-pkg/src/bundle/build.rs`:
- Add `SkillCapability` to the `use tau_domain::{...}` line inside `effective_to_bundle` (it currently lists `AgentCapability, Capability, FsCapability, NetCapability, ProcessCapability`).
- Replace the comment+catch-all (lines 457-458):
  ```rust
              // skill.spawn / task_list / plan / custom: no bundle field.
              _ => {}
  ```
  with a real `Skill` arm followed by the (now narrower) catch-all:
  ```rust
              Capability::Skill(SkillCapability::Spawn { allowed_skills, .. }) => {
                  out.allow_skill_spawn
                      .extend(e.allow_override.clone().unwrap_or_else(|| allowed_skills.clone()));
                  out.deny_skill_spawn.extend(e.deny.clone());
              }
              // task_list / plan / custom: no bundle field (still dropped).
              _ => {}
  ```
- If `effective_to_bundle` has a doc-comment listing dropped shapes that includes `skill.spawn`, update it to remove `skill.spawn` (now represented).

- [ ] **Step 6: Run tests → PASS**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::`
Expected: PASS — `effective_caps_is_empty_includes_skill_spawn`, `canonical_emits_skill_spawn_*`, `build_records_skill_spawn_for_override_agent`, the updated `effective_to_bundle_drops_unrepresentable_shapes`, and all pre-existing bundle tests (the manifest round-trip test now carries skill fields too).

- [ ] **Step 7: Commit**

```bash
git add crates/tau-pkg/src/bundle/manifest.rs crates/tau-pkg/src/bundle/canonical.rs crates/tau-pkg/src/bundle/build.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(tau-pkg): record skill.spawn in BundleEffectiveCapabilities (additive)"
```

---

## Task 2: Forward-compat contract guard tests

**Files:**
- Modify: `crates/tau-pkg/src/bundle/manifest.rs` (add 4 tests to `#[cfg(test)] mod tests`)

These encode the forward-compat contract so a future change that breaks it fails CI. They use only `BundleManifest::parse_str` + public structs.

- [ ] **Step 1: Add the guard tests**

Add to `crates/tau-pkg/src/bundle/manifest.rs` `#[cfg(test)] mod tests`:

```rust
    /// A bundle emitted by a hypothetical future tau: it carries unknown
    /// fields/tables at every level plus a future effective-cap shape.
    /// Today's parser MUST accept it and read the known fields intact.
    #[test]
    fn future_bundle_parses_with_all_tolerance_points() {
        let toml_str = r#"
schema_version = 1

[bundle]
sha256 = "deadbeef"
created_at = "2026-01-01T00:00:00Z"
tau_version = "9.9.9"
target = "passthrough"
future_meta = "tolerated"

[project]
name = "fwd"
version = "0.1.0"
tau_toml_sha256 = "aaaa"

[[packages]]
name = "p"
version = "0.1.0"
source = "https://example.com/p.git"
tree_sha256 = "1111"
future_pkg_field = "tolerated"

[[agents]]
id = "r"
backend = { kind = "anthropic", future_backend_key = "tolerated" }
system_prompt_sha256 = "7777"
effective_capabilities = { allow_fs_read = ["/data/**"], allow_future_shape = ["/x/**"] }
future_agent_field = 1

[future_section]
future_key = "tolerated"
"#;
        let m = BundleManifest::parse_str(toml_str).expect("future bundle must parse");
        assert_eq!(m.schema_version, 1);
        assert_eq!(m.project.name, "fwd");
        assert_eq!(m.packages.len(), 1);
        assert_eq!(m.packages[0].name, "p");
        assert_eq!(m.agents.len(), 1);
        assert_eq!(m.agents[0].id.as_str(), "r");
        // Known effective-cap field read; unknown future shape ignored.
        assert_eq!(m.agents[0].effective_capabilities.allow_fs_read, vec!["/data/**".to_string()]);
        // Unknown backend key captured by the flatten catch-all.
        assert!(m.agents[0].backend.extra.contains_key("future_backend_key"));
    }

    /// The schema_version hard-gate must reject v2 loudly — pins it so a
    /// casual bump can't silently pass.
    #[test]
    fn schema_version_two_is_rejected() {
        let toml_str = r#"
schema_version = 2

[bundle]
sha256 = "x"
created_at = "2026-01-01T00:00:00Z"
tau_version = "0.1.0"
target = "passthrough"

[project]
name = "x"
version = "0.1.0"
tau_toml_sha256 = "x"
"#;
        match BundleManifest::parse_str(toml_str) {
            Err(crate::bundle::error::BundleParseError::UnsupportedSchemaVersion { found }) => {
                assert_eq!(found, 2);
            }
            other => panic!("expected UnsupportedSchemaVersion(2), got {other:?}"),
        }
    }

    /// An unknown allow_* key in effective_capabilities is ignored while
    /// the known field is read (the struct §C.2.3/§D extends).
    #[test]
    fn effective_capabilities_unknown_allow_field_is_ignored() {
        let toml_str = r#"
schema_version = 1

[bundle]
sha256 = "x"
created_at = "2026-01-01T00:00:00Z"
tau_version = "0.1.0"
target = "passthrough"

[project]
name = "x"
version = "0.1.0"
tau_toml_sha256 = "x"

[[agents]]
id = "r"
backend = { kind = "anthropic" }
system_prompt_sha256 = "7"
effective_capabilities = { allow_fs_read = ["/a/**"], allow_some_future_shape = ["/b/**"] }
"#;
        let m = BundleManifest::parse_str(toml_str).expect("must parse");
        assert_eq!(m.agents[0].effective_capabilities.allow_fs_read, vec!["/a/**".to_string()]);
    }
```

Plus a Custom-capability-through-build test in `crates/tau-pkg/src/bundle/build.rs` `#[cfg(test)] mod tests` (it needs the build pipeline + a fixture). Model it on `override_agent_project`/`read_agent_caps` but give the home package a `Custom` capability kind alongside fs.read:

```rust
    #[test]
    fn build_tolerates_custom_capability_kind_in_home_package() {
        // An OLD tau building against a package that grants a future
        // (Custom) capability kind must not crash; the Custom shape is
        // simply not represented in effective_capabilities.
        let tmp = tempdir().unwrap();
        // Reuse override_agent_project, then append a Custom capability to
        // the installed home-package manifest.
        override_agent_project(tmp.path());
        let pkg_manifest = tmp.path().join(".tau/packages/homepkg/0.1.0/tau.toml");
        let mut body = std::fs::read_to_string(&pkg_manifest).unwrap();
        body.push_str("\n[[capabilities]]\nkind = \"mcp.tool.use\"\nendpoint = \"x\"\n");
        std::fs::write(&pkg_manifest, body).unwrap();
        // Build must succeed and still record the known fs.read narrowing.
        let caps = read_agent_caps(tmp.path());
        assert_eq!(caps.allow_fs_read, vec!["/data/**".to_string()]);
    }
```

(If `override_agent_project`'s package name/path differs, adjust the path string to match the fixture — read the helper first.)

- [ ] **Step 2: Run → PASS**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::`
Expected: PASS. If `future_bundle_parses_with_all_tolerance_points` FAILS, that is a real finding — it means a tolerance point regressed (e.g. a struct gained `deny_unknown_fields`); report it rather than weakening the test.

If `build_tolerates_custom_capability_kind_in_home_package` fails because `compute_effective` rejects the override against a manifest that now has an extra Custom cap (it should NOT — the override only narrows fs.read, and the Custom cap is just an extra source grant), report the exact error.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-pkg/src/bundle/manifest.rs crates/tau-pkg/src/bundle/build.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "test(tau-pkg): forward-compat contract guard tests for bundles + capabilities"
```

---

## Task 3: ADR-0036 + ADR-0035 amendment

**Files:**
- Create: `docs/decisions/0036-capability-forward-compatibility.md`
- Modify: `docs/decisions/0035-bundle-format.md` (one-line pointer)

- [ ] **Step 1: Write ADR-0036**

Create `docs/decisions/0036-capability-forward-compatibility.md` following `docs/decisions/template.md` (Context / Decision / Consequences / Alternatives considered). Content:

```markdown
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
```

- [ ] **Step 2: Amend ADR-0035**

In `docs/decisions/0035-bundle-format.md`, add a single pointer line in its Consequences or References section (match the file's structure):

```markdown
- Capability-vocabulary forward-compatibility rules (the additive
  discipline for new capability shapes) are codified in ADR-0036.
```

- [ ] **Step 3: Commit**

```bash
git add docs/decisions/0036-capability-forward-compatibility.md docs/decisions/0035-bundle-format.md
git -c user.name="titouanlebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "docs(adr): ADR-0036 capability forward-compatibility discipline"
```

---

## Task 4: Final verification + PR

- [ ] **Step 1: Full suite + fmt + clippy + doctests**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-cli
timeout 30  env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-pkg -p tau-cli -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-pkg -p tau-cli --all-targets
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-pkg
```
Expected: all green / clean. tau-cli is included because adding fields to `BundleEffectiveCapabilities` could touch any CLI test that constructs or renders it — confirm none broke. If fmt --check fails, run without `--check` and commit as `style(...)`.

- [ ] **Step 2: Push + PR**

Per `CLAUDE.md`: do NOT plain `git push` from the agent runtime. If the local deep gate (Podman) is available use `scripts/agent-push.sh`; otherwise push `--no-verify` and document it (CI is the gate). Before pushing, run `git status --short` and ensure the tree is clean (no dangling fmt edits).

```bash
git status --short   # must be empty
git push --no-verify -u origin feat/cap-forward-compat
gh pr create --title "feat(tau-pkg): capability vocabulary forward-compatibility (Phase 2 D)" --body "$(cat <<'EOF'
## Summary
- Locks tau's existing capability/bundle forward-compatibility with a guard-test suite (a synthetic future bundle must parse on today's code; schema_version v2 rejected; unknown effective-cap fields ignored; Custom capability kinds tolerated through build).
- Codifies the rules in ADR-0036 (no deny_unknown_fields on wire structs; non_exhaustive + Custom retained; schema_version bumps are breaking; additive shape evolution; canonical-emission obligation).
- Worked example: adds skill.spawn to BundleEffectiveCapabilities (allow/deny), closing the C.2.3 limitation and proving additive evolution is safe.

## Out of scope
- max_bytes / http methods sub-fields (need richer types), task_list/plan/custom in the bundle record, version-negotiation machinery.

## Test plan
- [ ] cargo test -p tau-pkg (guard tests + skill.spawn + reproduce)
- [ ] cargo test -p tau-cli
- [ ] cargo test --doc -p tau-pkg, fmt, clippy
- [ ] CI required checks

Spec: docs/superpowers/specs/2026-05-29-capability-forward-compat-design.md
Plan: docs/superpowers/plans/2026-05-29-capability-forward-compat.md

Generated with Claude Code
EOF
)"
```

---

## Self-Review notes

- **Spec coverage:** §4.1 guard tests → Task 2 (4 tests: future-bundle, schema-v2-reject, unknown-effective-field, custom-through-build); §4.2 ADR → Task 3; §4.3 skill.spawn extension (manifest field + is_empty + canonical + effective_to_bundle arm + repurposed tests) → Task 1; §5 test plan → Tasks 1+2; §6 out-of-scope respected (no max_bytes/methods, no task_list/plan/custom fields, no version negotiation). Covered.
- **Type consistency:** `allow_skill_spawn`/`deny_skill_spawn: Vec<String>` used identically in manifest struct (Task 1 Step 2), is_empty (Step 3), canonical (Step 4), effective_to_bundle arm (Step 5), and tests. `SkillCapability::Spawn { allowed_skills, .. }` matches the `tau_domain` variant. `BundleParseError::UnsupportedSchemaVersion { found }` matches the existing variant used by parse_str.
- **Verify-at-impl-time (flagged inline):** `write_effective_capabilities` visibility for the canonical test (same-file `mod tests` → in scope); `override_agent_project` package path for the Custom-cap test (read the helper). Each has a fallback note.
