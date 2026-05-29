# Bundle effective-capabilities: close the build stubs — Phase 2 §C.2.3 design

**Status:** Accepted
**Date:** 2026-05-29
**Authors:** titouanlebocq
**Depends on:** §C.2 `tau build` MVP producer (PR #242), §C.2.2 agent slicing (PR #252), the capability-override machinery (`tau_pkg::capability_override::compute_effective`)

## 1. Goal

`tau build` currently records **empty** `effective_capabilities` for every agent, even agents that declare `[[capabilities]]` overrides, because two functions in `crates/tau-pkg/src/bundle/build.rs` are stubs:

- `collect_package_caps(packages, required_tools) -> Result<Vec<Capability>, BuildError>` → returns `Ok(Vec::new())`.
- `effective_to_bundle(eff) -> BundleEffectiveCapabilities` → returns `Default` (all-empty).

This is a fidelity gap in the bundle — the security-relevant deployment record silently omits the compiled grant set for any agent with capability overrides. This spec closes both stubs so the bundle records the **same** effective grants the runtime enforces.

## 2. Headline decisions

- **Mirror the runtime exactly.** The runtime (`tau-runtime/src/builder.rs:568`) computes an agent's effective capabilities as `compute_effective(package_manifest.capabilities(), &project_override)` where `package_manifest` is the agent's **home package** (`[agents.<id>].package`). The build pipeline must use the same source — the home-package manifest — so the bundle's `effective_capabilities` equals what runs. **This corrects the old stub's comment**, which proposed unioning grants from the agent's *required tools* (the wrong model).
- **Trigger unchanged: overrides-only.** Build computes `effective_capabilities` only when `entry.capability_overrides` is non-empty (today's trigger). Agents without overrides continue to record no `effective_capabilities`. Extending recording to *all* agents would force every agent's home package to be installed with a valid manifest at build time — breaking minimal/fixture projects that declare an uninstalled `package = "p@^0.1"` — and is a separate, larger decision (candidate for §D). Out of scope here.
- **Record only the shapes the bundle schema models; drop the rest with a documented limitation.** `BundleEffectiveCapabilities` has allow/deny lists for exactly 5 shapes (fs.read, fs.write, exec, net.http, agent.spawn). `Capability` also has `Skill::Spawn`, `TaskList`, `Plan`, `Custom`, and sub-fields (`fs.write max_bytes`, `net.http methods`) with no bundle representation. These are dropped (not recorded). The bundle is a *record*; the runtime still enforces everything. Extending the schema belongs to §D (capability vocabulary forward-compatibility).
- **Fail loudly, never silently empty.** If an agent has overrides but its home-package manifest cannot be located or parsed, return a `BuildError` (do not fall back to empty capabilities). A bundle that silently under-records grants is worse than a build failure.
- **No change to `compute_effective` or the runtime.** Only the two `build.rs` stubs + a new `BuildError` variant + tests.

## 3. Current call site (for reference)

`build.rs` step 5, per agent (today):

```rust
let effective_capabilities = if entry.capability_overrides.is_empty() {
    BundleEffectiveCapabilities::default()
} else {
    let package_caps = collect_package_caps(&packages, &required_tools)?;   // STUB → []
    let eff = crate::capability_override::compute_effective(
        &package_caps, &entry.capability_overrides,
    ).map_err(|source| BuildError::CapabilityOverrideFailed { id: id.clone(), source })?;
    effective_to_bundle(&eff)                                               // STUB → default
};
```

`compute_effective` and the `CapabilityOverrideFailed` mapping already work; only the two stub inputs/outputs are empty.

## 4. Design

### 4.1 Replace `collect_package_caps` with a home-package-manifest loader

New signature (the `required_tools` parameter is removed — it reflected the wrong model):

```rust
/// Load the agent's home-package manifest and return its declared
/// capability grants — the same source the runtime feeds to
/// `compute_effective` (tau-runtime builder.rs:568).
///
/// `home_package` is the `<name>` half of `[agents.<id>].package`
/// (parsed via `parse_package_ref`). The resolved version comes from the
/// already-gathered `packages` list (one `BundlePackage` per locked
/// package). The manifest lives at
/// `<packages_root>/<name>/<version>/tau.toml`.
fn agent_home_package_caps(
    home_package: &str,
    packages: &[BundlePackage],
    packages_root: &std::path::Path,
    agent_id: &str,
) -> Result<Vec<tau_domain::Capability>, BuildError> {
    let pkg = packages
        .iter()
        .find(|p| p.name == home_package)
        .ok_or_else(|| BuildError::AgentHomePackageMissing {
            id: agent_id.to_owned(),
            package: home_package.to_owned(),
        })?;
    let manifest_path = packages_root
        .join(&pkg.name)
        .join(pkg.version.to_string())
        .join("tau.toml");
    let manifest = crate::read_manifest(&manifest_path).map_err(|source| {
        BuildError::AgentHomePackageManifest {
            id: agent_id.to_owned(),
            package: home_package.to_owned(),
            source,
        }
    })?;
    Ok(manifest.capabilities().to_vec())
}
```

Notes:
- `packages_root` is already computed in `build()` step 3 (`<project_root>/.tau/packages`). Thread it into step 5 (it is a local in the same fn).
- `pkg.version` is `semver::Version`; `.to_string()` gives the install-dir segment (consistent with the install layout already used by step 3/4).
- `parse_package_ref(&entry.package)` (already `pub(crate)`) yields the `<name>`. If the package field is empty/unparseable the agent cannot have a resolvable home package; treat as `AgentHomePackageMissing` (an agent with overrides must reference a real package).
- `crate::read_manifest` is `tau_pkg::read_manifest(path: &Path) -> Result<PackageManifest, ManifestReadError>`; `PackageManifest::capabilities() -> &[Capability]`.

### 4.2 Implement `effective_to_bundle`

```rust
/// Flatten `compute_effective`'s output into the bundle's per-shape
/// allow/deny lists. For each entry the effective allow-list is
/// `allow_override` when present, else the source capability's own
/// field; deny is `e.deny`. Shapes with no `BundleEffectiveCapabilities`
/// representation (skill.spawn, tasklist, plan, custom) and sub-fields
/// without representation (fs.write max_bytes, net.http methods) are
/// dropped — see spec §2 (recorded shapes) and §6 (limitation).
fn effective_to_bundle(
    eff: &[crate::capability_override::EffectiveCapability],
) -> BundleEffectiveCapabilities {
    use tau_domain::{Capability, FsCapability, NetCapability, ProcessCapability, AgentCapability};
    let mut out = BundleEffectiveCapabilities::default();
    for e in eff {
        // Effective allow = narrowed override if present, else the
        // source variant's own allow field.
        match &e.source {
            Capability::Filesystem(FsCapability::Read { paths }) => {
                out.allow_fs_read.extend(e.allow_override.clone().unwrap_or_else(|| paths.clone()));
                out.deny_fs_read.extend(e.deny.clone());
            }
            Capability::Filesystem(FsCapability::Write { paths, .. }) => {
                out.allow_fs_write.extend(e.allow_override.clone().unwrap_or_else(|| paths.clone()));
                out.deny_fs_write.extend(e.deny.clone());
            }
            Capability::Filesystem(FsCapability::Exec { paths }) => {
                out.allow_exec.extend(e.allow_override.clone().unwrap_or_else(|| paths.clone()));
                out.deny_exec.extend(e.deny.clone());
            }
            Capability::Process(ProcessCapability::Spawn { commands }) => {
                out.allow_exec.extend(e.allow_override.clone().unwrap_or_else(|| commands.clone()));
                out.deny_exec.extend(e.deny.clone());
            }
            Capability::Network(NetCapability::Http { hosts, .. }) => {
                out.allow_net_http.extend(e.allow_override.clone().unwrap_or_else(|| hosts.clone()));
                out.deny_net_http.extend(e.deny.clone());
            }
            Capability::Agent(AgentCapability::Spawn { allowed_kinds }) => {
                out.allow_agent_spawn.extend(e.allow_override.clone().unwrap_or_else(|| allowed_kinds.clone()));
                out.deny_agent_spawn.extend(e.deny.clone());
            }
            // Skill::Spawn, TaskList, Plan, Custom: no bundle field — dropped (§6).
            _ => {}
        }
    }
    out
}
```

Notes:
- `fs.exec` (`paths`) and `process.spawn` (`commands`) both feed `allow_exec`/`deny_exec`, mirroring `CapabilityShape::ProcessExec` (which covers both). They union into the one exec list.
- `EffectiveCapability` is `#[non_exhaustive]`; we read `source`, `allow_override`, `deny` (all `pub`). `max_bytes_override` is intentionally ignored (no bundle field).
- The non-exhaustive `_ => {}` is required (Capability is `#[non_exhaustive]`); the comment documents what falls through.

### 4.3 Determinism

`eff` is produced from `manifest.capabilities()` in manifest declaration order; `extend` preserves that order. Two builds of the same installed tree produce byte-identical lists → the self-hash is stable and `tau verify --bundle` reproduces (the §C.2.2 reproducibility contract holds). **No sorting** — order mirrors the runtime/source and is already deterministic.

### 4.4 New `BuildError` variants

```rust
/// An agent declares capability overrides but its home package (the
/// `[agents.<id>].package` ref) is not present in the resolved package
/// set — the effective grant set cannot be computed.
#[error("agent `{id}` has capability overrides but its home package `{package}` is not in the bundle's package set; run `tau install`")]
AgentHomePackageMissing { id: String, package: String },

/// An agent's home-package manifest could not be read/parsed while
/// computing its effective capabilities.
#[error("failed to read home-package manifest for agent `{id}` (package `{package}`): {source}")]
AgentHomePackageManifest {
    id: String,
    package: String,
    #[source]
    source: crate::manifest::ManifestReadError,
},
```

CLI exit codes (`tau-cli/src/cmd/build.rs::exit_code_for`):
- `AgentHomePackageMissing` → **3** (install-state, alongside `MissingLockfile`/`PackageNotInstalled`).
- `AgentHomePackageManifest` → **2** (config/parse, alongside `ManifestInvalid`).

### 4.5 Updated call site

```rust
let effective_capabilities = if entry.capability_overrides.is_empty() {
    BundleEffectiveCapabilities::default()
} else {
    let (home_pkg, _req) = crate::project::agent::parse_package_ref(&entry.package)
        .map_err(|_| BuildError::AgentHomePackageMissing {
            id: id.clone(),
            package: entry.package.clone(),
        })?;
    let package_caps = agent_home_package_caps(&home_pkg, &packages, &packages_root, id)?;
    let eff = crate::capability_override::compute_effective(&package_caps, &entry.capability_overrides)
        .map_err(|source| BuildError::CapabilityOverrideFailed { id: id.clone(), source })?;
    effective_to_bundle(&eff)
};
```

`packages_root` must be in scope at step 5. It is the same `<project_root>/.tau/packages` computed in step 3; bind it once (e.g. lift the step-3 local so step 5 can reuse it) rather than recompute.

## 5. Test plan

**Builder unit tests (`bundle/build.rs`):** a fixture installs a home package whose `tau.toml` declares capabilities, with an agent that overrides a subset.

- `build_records_effective_caps_for_override_agent` — home package grants `fs.read paths=["/data/**","/tmp/**"]`; agent overrides `fs.read allow_paths=["/data/**"] deny_paths=["/data/secret/**"]`. Built bundle's agent has `allow_fs_read == ["/data/**"]`, `deny_fs_read == ["/data/secret/**"]`. Proves override narrowing is recorded.
- `build_records_unoverridden_source_caps` — home package also grants `net.http hosts=["api.example.com"]` with NO net.http override. Built bundle has `allow_net_http == ["api.example.com"]` (falls back to source field). Proves un-overridden grants still recorded.
- `build_no_override_agent_records_empty_caps` — agent with no `[[capabilities]]` → `effective_capabilities` omitted/empty (regression guard for the unchanged trigger; home package need not be installed).
- `build_drops_unrepresentable_shapes` — home package grants `skill.spawn allowed_skills=["critic"]` + an `fs.read` override; bundle records `allow_fs_read` but has NO skill data anywhere (documents the §6 drop).
- `build_exec_unions_fs_exec_and_process_spawn` — home package grants both `fs.exec paths=["/usr/bin/git"]` and `process.spawn commands=["ls"]`; agent has an fs.read override (to trigger the path); `allow_exec` contains both. (If wiring both into one fixture is awkward, split into two asserts.)
- `build_errors_when_override_agent_home_package_missing` — agent has overrides but home package not in lockfile/packages → `BuildError::AgentHomePackageMissing { id, package }`.
- `build_errors_when_home_package_manifest_unreadable` — home package in packages list but its `tau.toml` absent on disk → `BuildError::AgentHomePackageManifest { .. }`.
- `build_effective_caps_is_reproducible` — two builds of the same tree (>1s apart) → equal self-hash (determinism).

**CLI exit-code test (`cmd/build.rs`):** extend `exit_code_mapping_per_spec` — `AgentHomePackageMissing` → 3, `AgentHomePackageManifest` → 2.

**Reproduce regression (`bundle/reproduce.rs` or e2e):** a sliced/full bundle built from an override-agent project verifies reproducible (guards that populated `effective_capabilities` round-trips through canonical TOML + self-hash). The existing `diff_manifests` already compares agent fields.

## 6. Out of scope / known limitations

- **Unrepresentable capability shapes are dropped from the bundle record:** `skill.spawn`, `tasklist`, `plan`, `custom`, and the `fs.write max_bytes` / `net.http methods` sub-fields. The runtime still enforces them; the bundle just doesn't record them. Extending `BundleEffectiveCapabilities` to cover these is deferred to §D (capability vocabulary forward-compatibility).
- **No-override agents record no `effective_capabilities`** (unchanged trigger). Recording the full home-package grant for every agent would require every agent's home package to be installed at build time; deferred.
- No change to `compute_effective`, the runtime, or the bundle schema (`schema_version` stays 1).

## 7. References

- §C.2 spec — `2026-05-27-tau-build-design.md` (introduced the stubs as deferred follow-ups #4/#6)
- §C.2.2 spec — `2026-05-28-tau-build-agent-slice-design.md` (reproducibility contract this must not break)
- ADR-0035 — bundle format (`BundleEffectiveCapabilities` shape)
- `tau-runtime/src/builder.rs:568` — the gold-standard runtime computation this mirrors
- `tau-pkg/src/capability_override/mod.rs` — `compute_effective`, `EffectiveCapability`, `CapabilityOverride`
