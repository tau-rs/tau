# PR 1 — `link()` + `LinkRecord` in tau-pkg — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a pure, static `link()` function to `tau-pkg` that validates the installed world the IR binds to (plugins, model backends, skills) and produces a serializable `LinkRecord`, collecting all errors. Nothing calls it yet.

**Architecture:** `link()` is a **static linker** — it validates the package-level symbol table from the lockfile + manifests + disk with **no process spawning**. It lives in a new flat module `crates/tau-pkg/src/link.rs` (sibling to `resolve.rs`/`verify.rs`). Tool-name binding and sandbox adapter probing are the runtime "dynamic loader" half and are out of scope. The record is embedded in the bundle in PR 2 and trusted by `run --bundle` in PR 3.

**Tech Stack:** Rust, `thiserror` (boundary errors), `serde`/`toml` (record serialization), `tempfile` + inline `#[cfg(test)] mod tests` (truth-table tests), `sha2` (lockfile hash, via the crate's existing helper).

**Spec:** `docs/superpowers/specs/2026-07-19-link-phase-linkrecord-design.md` (§1, §2 for skills; brainstorm decisions Q5/Q6).

## Global Constraints

- `crates/tau-pkg/src/lib.rs` has `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`, `#![deny(rustdoc::broken_intra_doc_links)]` — **every** public item, enum variant, and field needs a `///` doc comment.
- Error enums: `#[non_exhaustive]` + `#[derive(Debug, Clone, PartialEq, Eq, Error)]`; `#[error("…")]` messages lowercase, no trailing period, `{field}` interpolation; each enum carries free-form `String` fields and tests assert via `matches!(…, Err(Variant { .. }))` (never string-compare wording).
- Serializable data structs: `#[non_exhaustive]` + `#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]`.
- Cargo discipline (CLAUDE.md): every cargo call is `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <cmd> -p tau-pkg`. Tests via `cargo nextest run -p tau-pkg`; timeout 300 for tests, 180 for check.
- Conventional commits; commit at the end of each task.
- `tau-pkg` may depend on `tau-ir` (no cycle — `tau-ir` is the no_std base) but must NOT depend on `tau-runtime-tokio` (cycle) or `tau-ir-lower`.

---

## File Structure

- **Create** `crates/tau-pkg/src/link.rs` — the entire feature: types (`LinkRecord`, `LinkedPlugin`, `LinkedSkill`, `LinkOutcome`, `LinkError`), the `link()` entrypoint, three private resolvers (`resolve_one_plugin`, `resolve_models`, `resolve_skills`), and the inline test module.
- **Modify** `crates/tau-pkg/Cargo.toml` — add `tau-ir = { workspace = true }`.
- **Modify** `crates/tau-pkg/src/lib.rs` — add `pub mod link;` (alphabetical, between `install_sandbox` and `lockfile`) and a `pub use link::{link, LinkError, LinkRecord, LinkOutcome, LinkedPlugin, LinkedSkill};` block.
- **Create** `docs/decisions/0060-build-links-verified-linkrecord.md` + **Modify** `docs/SUMMARY.md`.

---

### Task 1: Module scaffold, types, and serde round-trip

**Files:**
- Create: `crates/tau-pkg/src/link.rs`
- Modify: `crates/tau-pkg/Cargo.toml` (add `tau-ir`)
- Modify: `crates/tau-pkg/src/lib.rs` (`pub mod link;` + re-export)
- Test: inline `#[cfg(test)] mod tests` in `crates/tau-pkg/src/link.rs`

**Interfaces:**
- Produces (consumed by all later tasks):
  ```rust
  pub struct LinkedPlugin { pub name: PackageName, pub version: Version,
      pub binary_sha256: String, pub provides: PortKind }
  pub struct LinkedSkill { pub name: PackageName, pub content_sha256: String, pub parsed_ok: bool }
  pub struct LinkRecord { pub resolved_plugins: Vec<LinkedPlugin>, pub resolved_skills: Vec<LinkedSkill>,
      pub model_bindings: BTreeMap<String, ModelRef>, pub platform: TargetTriple, pub lockfile_sha256: String }
  pub struct LinkOutcome { pub record: LinkRecord, pub parsed_skills: BTreeMap<String, SkillContent> }
  pub enum LinkError { PluginNotInstalled{..}, PluginPortMismatch{..}, VersionUnsatisfied{..},
      SkillMissing{..}, SkillParse{..} }  // no ModelAliasUnknown — validate() owns alias resolution
  ```
  Note `TargetTriple` is `Copy` and NOT `serde`-derived by default in `tau-ports`; it must serialize via its `Display`/`FromStr`. Use `#[serde(with = "...")]` or store `platform` as its string form internally — see Step 3.

- [ ] **Step 1: Add the `tau-ir` dependency**

In `crates/tau-pkg/Cargo.toml`, under `[dependencies]`, add (alphabetically near the other `tau-*` lines):
```toml
tau-ir = { workspace = true }
```

- [ ] **Step 2: Register the module and re-exports**

In `crates/tau-pkg/src/lib.rs`, add between `pub mod install_sandbox;` and `pub mod lockfile;`:
```rust
pub mod link;
```
And add a re-export block near the other `pub use` blocks (after the `install` block):
```rust
pub use link::{link, LinkError, LinkOutcome, LinkRecord, LinkedPlugin, LinkedSkill};
```

- [ ] **Step 3: Write the type definitions**

Create `crates/tau-pkg/src/link.rs`. `TargetTriple` has no serde derive in tau-ports, so serialize it as its `Display` string with a small helper module.

```rust
//! Static link phase: validate the installed world the IR binds to.
//!
//! `link()` is a *static linker* — it checks that every plugin, model
//! backend, and skill the project references resolves against the
//! installed set (lockfile + manifests + on-disk `SKILL.md`), with **no
//! process spawning**. It produces a serializable [`LinkRecord`] embedded
//! in the bundle (PR 2) and trusted by `run --bundle` after verification
//! (PR 3). Tool-name binding and sandbox adapter probing are the runtime
//! *loader*'s job and are out of scope here.

use std::collections::BTreeMap;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use tau_domain::package::plugin::PortKind;
use tau_domain::{PackageName, Version};
use tau_ir::model_ref::ModelRef;
use tau_ir::IrModule;
use tau_ports::target::TargetTriple;

use crate::lockfile::LockFile;
use crate::project::project::{ModelEntry, ProjectConfig};
use crate::scope::Scope;

/// A plugin package resolved against the installed set during linking.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LinkedPlugin {
    /// Validated package name.
    pub name: PackageName,
    /// The installed version selected (highest satisfying the requirement).
    pub version: Version,
    /// SHA-256 (hex) of the built plugin binary, copied from the lockfile.
    pub binary_sha256: String,
    /// The port this plugin provides (`LlmBackend` or `Tool`).
    pub provides: PortKind,
}

/// An installed skill package resolved + parsed during linking.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LinkedSkill {
    /// Validated skill package name.
    pub name: PackageName,
    /// SHA-256 (hex) of the `SKILL.md` bytes, for drift detection at verify.
    pub content_sha256: String,
    /// Whether `SKILL.md` parsed successfully at link time.
    pub parsed_ok: bool,
}

/// The verified binding record produced by [`link`]. Embedded in the
/// bundle manifest; the parsed skill bodies travel separately in
/// [`LinkOutcome::parsed_skills`], not in the serialized record.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LinkRecord {
    /// Every plugin (model backend + required tool) resolved, deterministically ordered by name.
    pub resolved_plugins: Vec<LinkedPlugin>,
    /// Every installed skill package resolved + parsed, ordered by name.
    pub resolved_skills: Vec<LinkedSkill>,
    /// Final model-alias → resolved [`ModelRef`] bindings; no later re-resolution.
    pub model_bindings: BTreeMap<String, ModelRef>,
    /// The IR target the bundle was linked for (run --bundle platform check).
    #[serde(with = "target_triple_str")]
    pub platform: TargetTriple,
    /// SHA-256 (hex) of the lockfile bytes the record was linked against.
    pub lockfile_sha256: String,
}

/// The full result of [`link`]: the serializable [`LinkRecord`] plus the
/// parsed skill bodies (kept out of the record; used to seed the runtime
/// `SkillResolver` — PR 2).
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct LinkOutcome {
    /// The verified, serializable record.
    pub record: LinkRecord,
    /// skill name → parsed `SKILL.md`, produced once at link time.
    pub parsed_skills: BTreeMap<String, tau_domain::SkillContent>,
}

/// Errors from static linking. All variants are collected — [`link`]
/// never stops at the first — so the operator sees the whole set.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LinkError {
    /// A referenced package is not installed in the scope.
    #[error("plugin {package} not installed in scope")]
    PluginNotInstalled {
        /// The referenced package name.
        package: String,
    },
    /// A referenced package is installed but provides the wrong port.
    #[error("plugin {package} provides {found:?} but {expected:?} was required")]
    PluginPortMismatch {
        /// The referenced package name.
        package: String,
        /// The port the plugin actually declares.
        found: PortKind,
        /// The port the reference required.
        expected: PortKind,
    },
    /// No installed version of a package satisfies the requirement.
    #[error("no installed version of {package} satisfies {req}")]
    VersionUnsatisfied {
        /// The referenced package name.
        package: String,
        /// The semver requirement string.
        req: String,
    },
    /// An installed skill package's `SKILL.md` is missing on disk.
    #[error("skill {package}: SKILL.md missing at {path}")]
    SkillMissing {
        /// The skill package name.
        package: String,
        /// The expected `SKILL.md` path (lossy UTF-8).
        path: String,
    },
    /// An installed skill package's `SKILL.md` failed to parse.
    #[error("skill {package}: SKILL.md parse failed: {detail}")]
    SkillParse {
        /// The skill package name.
        package: String,
        /// Parser detail.
        detail: String,
    },
    // No ModelAliasUnknown: ProjectConfig::validate() already rejects unknown
    // model aliases, so model resolution in link() is infallible.
}

/// Serialize [`TargetTriple`] as its `Display` string (tau-ports has no serde derive).
mod target_triple_str {
    use super::{FromStr, TargetTriple};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(t: &TargetTriple, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&t.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<TargetTriple, D::Error> {
        let s = String::deserialize(d)?;
        TargetTriple::from_str(&s).map_err(serde::de::Error::custom)
    }
}
```

*If `TargetTriple` has no `FromStr`, use the parser the CLI uses for `--target` (`tau_ports::target::parse` or equivalent — grep `impl FromStr for TargetTriple` / `fn parse` in `crates/tau-ports/src/target/`); wire whichever exists into `deserialize`.*

- [ ] **Step 4: Write the failing serde round-trip test**

Add to the bottom of `crates/tau-pkg/src/link.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tau_ports::target::TargetTriple;

    fn sample_record() -> LinkRecord {
        let mut model_bindings = BTreeMap::new();
        model_bindings.insert(
            "fast".to_string(),
            ModelRef { backend: "anthropic".into(), model_id: "claude-haiku-4-5".into() },
        );
        LinkRecord {
            resolved_plugins: vec![LinkedPlugin {
                name: PackageName::from_str("anthropic").unwrap(),
                version: Version::parse("1.0.0").unwrap(),
                binary_sha256: "ab".repeat(32),
                provides: PortKind::LlmBackend,
            }],
            resolved_skills: vec![LinkedSkill {
                name: PackageName::from_str("my-skill").unwrap(),
                content_sha256: "cd".repeat(32),
                parsed_ok: true,
            }],
            model_bindings,
            platform: TargetTriple::host(),
            lockfile_sha256: "ef".repeat(32),
        }
    }

    #[test]
    fn link_record_toml_round_trips() {
        let rec = sample_record();
        let toml = toml::to_string(&rec).expect("serialize");
        let back: LinkRecord = toml::from_str(&toml).expect("deserialize");
        assert_eq!(rec, back);
    }
}
```

- [ ] **Step 5: Run the test to verify it compiles and passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg link::tests::link_record_toml_round_trips`
Expected: PASS. (If `PortKind` isn't `Serialize`, add the derive at its definition in `tau-domain/src/package/plugin.rs` behind the existing `serde` feature, or map it to a string like `platform` — resolve against the compiler.)

- [ ] **Step 6: Commit**

```bash
git add crates/tau-pkg/src/link.rs crates/tau-pkg/src/lib.rs crates/tau-pkg/Cargo.toml
git commit -m "feat(link): LinkRecord/LinkError types + serde round-trip (tau-pkg)"
```

---

### Task 2: Plugin resolver (`resolve_one_plugin`) with truth-table tests

Ports the verified logic from `plugin_loader.rs:367-397` (installed / port checks) and `agent.rs:182-195` (version satisfiability) into one pure helper returning a structured `LinkError` instead of `anyhow`.

**Files:**
- Modify: `crates/tau-pkg/src/link.rs`
- Test: inline tests in the same file

**Interfaces:**
- Produces (consumed by Task 5):
  ```rust
  fn resolve_one_plugin(
      lockfile: &LockFile, package: &PackageName,
      version_req: &semver::VersionReq, expected: PortKind,
  ) -> Result<LinkedPlugin, LinkError>;
  ```
- Consumes: `LinkedPlugin`, `LinkError`, `PortKind` (Task 1).

- [ ] **Step 1: Write the failing truth-table tests**

Add inside `mod tests`. Reuse the tau-pkg fixture idiom (build a `LockFile` in memory; a `TempDir` scope isn't needed since `resolve_one_plugin` reads only the lockfile):
```rust
    use crate::lockfile::{LockFile, LockedPackage, LockedPlugin, LockedVersion};
    use std::time::SystemTime;
    use tau_domain::package::plugin::PluginManifest;
    use tau_domain::PackageSource;

    fn plugin_pkg(name: &str, version: &str, provides: PortKind, sha: &str) -> LockedPackage {
        let v = Version::parse(version).unwrap();
        LockedPackage {
            name: PackageName::from_str(name).unwrap(),
            active_version: v.clone(),
            source: PackageSource::Git { location: "https://x/y.git".parse().unwrap(), rev: None },
            installed_versions: vec![LockedVersion {
                version: v, rev: None, resolved_commit: "0".repeat(40),
                sha256: String::new(), installed_at: SystemTime::UNIX_EPOCH,
            }],
            plugin: Some(LockedPlugin::new(
                PluginManifest { provides, kind: Default::default(), bin: "x".into() },
                "/tmp/x".into(), SystemTime::UNIX_EPOCH, sha.to_string(),
            )),
            skill: None,
            synthesized_from: None,
        }
    }

    fn lf_with(pkg: LockedPackage) -> LockFile {
        let mut lf = LockFile::default();
        lf.packages.push(pkg);
        lf
    }

    fn req(s: &str) -> semver::VersionReq { semver::VersionReq::parse(s).unwrap() }

    #[test]
    fn resolve_plugin_ok() {
        let lf = lf_with(plugin_pkg("anthropic", "1.2.0", PortKind::LlmBackend, &"ab".repeat(32)));
        let name = PackageName::from_str("anthropic").unwrap();
        let got = resolve_one_plugin(&lf, &name, &req("^1"), PortKind::LlmBackend).unwrap();
        assert_eq!(got.version, Version::parse("1.2.0").unwrap());
        assert_eq!(got.binary_sha256, "ab".repeat(32));
        assert_eq!(got.provides, PortKind::LlmBackend);
    }

    #[test]
    fn resolve_plugin_not_installed() {
        let lf = LockFile::default();
        let name = PackageName::from_str("anthropic").unwrap();
        let r = resolve_one_plugin(&lf, &name, &req("^1"), PortKind::LlmBackend);
        assert!(matches!(r, Err(LinkError::PluginNotInstalled { .. })), "got {r:?}");
    }

    #[test]
    fn resolve_plugin_port_mismatch() {
        let lf = lf_with(plugin_pkg("some-tool", "1.0.0", PortKind::Tool, &"ab".repeat(32)));
        let name = PackageName::from_str("some-tool").unwrap();
        let r = resolve_one_plugin(&lf, &name, &req("^1"), PortKind::LlmBackend);
        assert!(matches!(r, Err(LinkError::PluginPortMismatch { .. })), "got {r:?}");
    }

    #[test]
    fn resolve_plugin_version_unsatisfied() {
        let lf = lf_with(plugin_pkg("anthropic", "1.0.0", PortKind::LlmBackend, &"ab".repeat(32)));
        let name = PackageName::from_str("anthropic").unwrap();
        let r = resolve_one_plugin(&lf, &name, &req("^2"), PortKind::LlmBackend);
        assert!(matches!(r, Err(LinkError::VersionUnsatisfied { .. })), "got {r:?}");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg link::tests::resolve_plugin`
Expected: FAIL — `resolve_one_plugin` not found.

- [ ] **Step 3: Implement `resolve_one_plugin`**

Add to `link.rs` (module body). Note: a data-only package (no `[plugin]` table) is treated as `PluginNotInstalled` for link purposes (a plugin reference to a data-only package can't bind).
```rust
fn resolve_one_plugin(
    lockfile: &LockFile,
    package: &PackageName,
    version_req: &semver::VersionReq,
    expected: PortKind,
) -> Result<LinkedPlugin, LinkError> {
    let pkg = lockfile
        .find(package)
        .ok_or_else(|| LinkError::PluginNotInstalled { package: package.to_string() })?;

    // Highest installed version satisfying the requirement (agent.rs:182-195 shape).
    let version = pkg
        .installed_versions
        .iter()
        .map(|v| &v.version)
        .filter(|v| version_req.matches(v))
        .max()
        .cloned()
        .ok_or_else(|| LinkError::VersionUnsatisfied {
            package: package.to_string(),
            req: version_req.to_string(),
        })?;

    // Must carry a [plugin] table and provide the required port (plugin_loader.rs:381-394 shape).
    let plugin = pkg
        .plugin
        .as_ref()
        .ok_or_else(|| LinkError::PluginNotInstalled { package: package.to_string() })?;
    if plugin.manifest.provides != expected {
        return Err(LinkError::PluginPortMismatch {
            package: package.to_string(),
            found: plugin.manifest.provides,
            expected,
        });
    }

    Ok(LinkedPlugin {
        name: package.clone(),
        version,
        binary_sha256: plugin.binary_sha256.clone(),
        provides: plugin.manifest.provides,
    })
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg link::tests::resolve_plugin`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/link.rs
git commit -m "feat(link): resolve_one_plugin (installed/port/version) + truth-table tests"
```

---

### Task 3: Model-binding resolver (`resolve_models`)

> **AMENDED (implemented):** `resolve_models` is **infallible** — `ProjectConfig::validate()`
> already rejects unknown model aliases (`ProjectConfigError::UnknownModelAlias`), so there
> is no `ModelAliasUnknown` error. For each agent with a non-empty `model`, look it up in
> `[models]` and record `alias → ModelRef`; an absent alias is simply not inserted
> (unreachable on validated input). The live "backend installed as an LlmBackend plugin"
> check is `link()`'s plugin resolution (Task 5), not here.

**Files:**
- Modify: `crates/tau-pkg/src/link.rs`
- Test: inline tests

**Interfaces:**
- Produces (consumed by Task 5):
  ```rust
  fn resolve_models(cfg: &ProjectConfig) -> BTreeMap<String, ModelRef>;
  // thin wrapper over the testable inner fn:
  fn resolve_models_from(
      agents: &BTreeMap<String, AgentEntry>, models: &BTreeMap<String, ModelEntry>,
  ) -> BTreeMap<String, ModelRef>;
  ```
  Returns the alias→ModelRef map for every alias referenced by some agent. Agents with an empty `model` string are skipped. (`ProjectConfig` is `#[non_exhaustive]` and hard to build in a unit test, so tests exercise `resolve_models_from` on the two maps directly.)

- [ ] **Step 1: Write the failing tests**

Add a helper to build a minimal `ProjectConfig`. `ProjectConfig` is `#[non_exhaustive]` and normally built via validation; for unit tests, construct it through the public builder path if one is exposed, else via `..Default::default()` if the crate provides a test constructor. **First grep** `crates/tau-pkg/src/project/` for an existing test that builds a `ProjectConfig` (e.g. `UncheckedProjectConfig { … }.validate()`); reuse that idiom. Sketch of the assertions:
Test `resolve_models_from` on the two maps directly (`ProjectConfig` is `#[non_exhaustive]` and impractical to build in a unit test; build `AgentEntry`/`ModelEntry` via same-crate access). Sketch:
```rust
    #[test]
    fn resolve_models_ok() {
        // agents: {"a" -> model "fast", "b" -> model "unknown"}
        // models: {"fast" -> ModelEntry { backend: "anthropic", model: "claude-haiku-4-5" }}
        let bindings = resolve_models_from(&agents, &models);
        assert_eq!(
            bindings.get("fast"),
            Some(&ModelRef { backend: "anthropic".into(), model_id: "claude-haiku-4-5".into() })
        );
        assert!(bindings.get("unknown").is_none()); // absent alias simply not inserted (no error)
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg link::tests::resolve_models`
Expected: FAIL — `resolve_models` not found.

- [ ] **Step 3: Implement `resolve_models`**

```rust
fn resolve_models(cfg: &ProjectConfig) -> BTreeMap<String, ModelRef> {
    resolve_models_from(&cfg.agents, &cfg.models)
}

fn resolve_models_from(
    agents: &BTreeMap<String, AgentEntry>,
    models: &BTreeMap<String, ModelEntry>,
) -> BTreeMap<String, ModelRef> {
    let mut bindings = BTreeMap::new();
    for agent in agents.values() {
        if agent.model.is_empty() {
            continue; // agent declares no model
        }
        if let Some(m) = models.get(&agent.model) {
            bindings.insert(
                agent.model.clone(),
                ModelRef { backend: m.backend.clone(), model_id: m.model.clone() },
            );
        }
        // absent alias: unreachable on validated input; simply not inserted (no error)
    }
    bindings
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg link::tests::resolve_models`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/link.rs
git commit -m "feat(link): resolve_models (alias -> ModelRef) + tests"
```

---

### Task 4: Skill resolver (`resolve_skills`)

Enumerate every installed skill package (lockfile entries with `skill.is_some()` — the same filter `find_installed_skill` uses), read + parse its `SKILL.md` **once**, record `content_sha256` + `parsed_ok`, and return the parsed bodies for the runtime resolver seed. `SkillMissing` when the file is absent, `SkillParse` when `parse_skill_md` fails.

**Files:**
- Modify: `crates/tau-pkg/src/link.rs`
- Test: inline tests (build a `TempDir` scope with an installed skill package + `SKILL.md` on disk).

**Interfaces:**
- Produces (consumed by Task 5):
  ```rust
  fn resolve_skills(
      scope: &Scope, lockfile: &LockFile,
  ) -> (Vec<LinkedSkill>, BTreeMap<String, tau_domain::SkillContent>, Vec<LinkError>);
  ```
- Uses: `tau_pkg::find_installed_skill` / `InstalledSkill` (to locate the install path + `skill.content` filename), `tau_domain::parse_skill_md`, and the crate's existing sha256-hex helper (see `tree_hash.rs` for the `sha2` usage pattern; if a `pub(crate) fn sha256_hex(bytes: &[u8]) -> String` exists, reuse it, else add one to `link.rs`).

- [ ] **Step 1: Write the failing tests**

Follow the `verify.rs`/`skill_check.rs` fixture recipe: `Scope::new_project(td.path())`, write a `LockFile` with a skill `LockedPackage` (`skill: Some(LockedSkill { content_sha256, frontmatter })`), `fs::create_dir_all(scope.package_dir(&name, &version))`, and write `SKILL.md`/`tau.toml` into it. Grep `crates/tau-pkg/src/skill_check.rs` `mod tests` for the exact skill-package fixture (it already builds an installed skill tree) and reuse it. Assertions:
```rust
    #[test]
    fn resolve_skills_ok() {
        // scope with one installed skill package "greeter" + valid SKILL.md
        let (_td, scope, lf) = /* skill fixture */;
        let (linked, parsed, errs) = resolve_skills(&scope, &lf);
        assert!(errs.is_empty(), "got {errs:?}");
        assert_eq!(linked.len(), 1);
        assert!(linked[0].parsed_ok);
        assert!(parsed.contains_key("greeter"));
    }

    #[test]
    fn resolve_skills_missing_file() {
        // lockfile lists the skill, but SKILL.md not written to disk
        let (_td, scope, lf) = /* skill fixture, skip writing SKILL.md */;
        let (_linked, _parsed, errs) = resolve_skills(&scope, &lf);
        assert!(errs.iter().any(|e| matches!(e, LinkError::SkillMissing { .. })), "got {errs:?}");
    }

    #[test]
    fn resolve_skills_parse_error() {
        // SKILL.md written with invalid frontmatter
        let (_td, scope, lf) = /* skill fixture with corrupt SKILL.md */;
        let (linked, _parsed, errs) = resolve_skills(&scope, &lf);
        assert!(errs.iter().any(|e| matches!(e, LinkError::SkillParse { .. })), "got {errs:?}");
        assert!(linked.iter().any(|s| !s.parsed_ok));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg link::tests::resolve_skills`
Expected: FAIL — `resolve_skills` not found.

- [ ] **Step 3: Implement `resolve_skills`**

```rust
fn resolve_skills(
    scope: &Scope,
    lockfile: &LockFile,
) -> (Vec<LinkedSkill>, BTreeMap<String, tau_domain::SkillContent>, Vec<LinkError>) {
    let mut linked = Vec::new();
    let mut parsed_map = BTreeMap::new();
    let mut errors = Vec::new();

    for pkg in &lockfile.packages {
        if pkg.skill.is_none() {
            continue; // not a skill package
        }
        let name = pkg.name.clone();
        // Locate the install path + SKILL.md filename via the existing helper.
        let installed = match crate::find_installed_skill(scope, name.as_str()) {
            Ok(Some(s)) => s,
            Ok(None) => continue, // not resolvable as a skill (shouldn't happen given the filter)
            Err(crate::FindSkillError::InstallPathMissing { path, .. }) => {
                errors.push(LinkError::SkillMissing {
                    package: name.to_string(),
                    path: path.display().to_string(),
                });
                continue;
            }
            Err(e) => {
                errors.push(LinkError::SkillParse { package: name.to_string(), detail: e.to_string() });
                continue;
            }
        };
        let skill_md = installed.install_path.join(&installed.skill.content);
        let bytes = match std::fs::read(&skill_md) {
            Ok(b) => b,
            Err(_) => {
                errors.push(LinkError::SkillMissing {
                    package: name.to_string(),
                    path: skill_md.display().to_string(),
                });
                continue;
            }
        };
        let content_sha256 = sha256_hex(&bytes);
        let text = String::from_utf8_lossy(&bytes);
        match tau_domain::parse_skill_md(&text) {
            Ok(content) => {
                linked.push(LinkedSkill { name: name.clone(), content_sha256, parsed_ok: true });
                parsed_map.insert(name.to_string(), content);
            }
            Err(e) => {
                linked.push(LinkedSkill { name: name.clone(), content_sha256, parsed_ok: false });
                errors.push(LinkError::SkillParse { package: name.to_string(), detail: e.to_string() });
            }
        }
    }
    (linked, parsed_map, errors)
}

/// SHA-256 hex of `bytes`. (Reuse the crate helper if one already exists.)
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}
```
*If `crate::tree_hash` already exposes a hex helper, delete the local `sha256_hex` and use it. Confirm `sha2` is a `tau-pkg` dependency (it is, via `tree_hash.rs`); if not, add it.*

- [ ] **Step 4: Run to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg link::tests::resolve_skills`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/link.rs
git commit -m "feat(link): resolve_skills (parse-once, sha, missing/parse errors) + tests"
```

---

### Task 5: `link()` entrypoint — assemble + collect all errors

Compose Tasks 2–4. Enumerate plugins to resolve: each agent's model backend (`[models][alias].backend`, port `LlmBackend`, no version constraint → `VersionReq::STAR`) and each agent's `requires.tools[]` (port `Tool`, with the tool's `version_req`). Deduplicate by name. Resolve models + skills. Collect **all** errors into one `Vec`. On empty errors, assemble `LinkRecord` (deterministic ordering) + `LinkOutcome`.

**Files:**
- Modify: `crates/tau-pkg/src/link.rs`
- Test: inline tests (happy path + multi-error collection).

**Interfaces:**
- Produces (public API; consumed by PR 2 callers):
  ```rust
  pub fn link(
      cfg: &ProjectConfig, module: &IrModule,
      lockfile: &LockFile, scope: &Scope,
  ) -> Result<LinkOutcome, Vec<LinkError>>;
  ```
- Consumes: `resolve_one_plugin`, `resolve_models`, `resolve_skills`, all Task-1 types.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn link_happy_path_produces_record() {
        // scope+lockfile with: LlmBackend plugin "anthropic"@1, a Tool plugin, one skill;
        // cfg: one agent using model "fast" (-> anthropic) requiring the tool;
        // module: IrModule with target = TargetTriple::host().
        let (_td, scope, lf, cfg, module) = /* full fixture */;
        let out = link(&cfg, &module, &lf, &scope).expect("link ok");
        assert_eq!(out.record.platform, TargetTriple::host());
        assert!(out.record.resolved_plugins.iter().any(|p| p.provides == PortKind::LlmBackend));
        assert!(out.record.model_bindings.contains_key("fast"));
        assert_eq!(out.record.lockfile_sha256.len(), 64);
    }

    #[test]
    fn link_collects_all_errors() {
        // Two DISTINCT faults so we prove multi-error collection:
        //  (a) an uninstalled required tool  -> PluginNotInstalled
        //  (b) an installed skill with a corrupt SKILL.md -> SkillParse
        // (ModelAliasUnknown no longer exists — validate() owns that; pick two
        //  faults that live in link()'s own resolution surface.)
        let (_td, scope, lf, cfg, module) = /* broken fixture (>=2 faults) */;
        let errs = link(&cfg, &module, &lf, &scope).unwrap_err();
        assert!(errs.iter().any(|e| matches!(e, LinkError::PluginNotInstalled { .. })), "got {errs:?}");
        assert!(errs.iter().any(|e| matches!(e, LinkError::SkillParse { .. })), "got {errs:?}");
        assert!(errs.len() >= 2, "must collect all, got {errs:?}");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg link::tests::link_`
Expected: FAIL — `link` not found.

- [ ] **Step 3: Implement `link()`**

```rust
/// Statically link the project against the installed set. Validates every
/// referenced plugin, model backend, and skill; collects **all** errors.
///
/// On success returns a [`LinkOutcome`] carrying the serializable
/// [`LinkRecord`] plus the parsed skill bodies (for seeding the runtime
/// `SkillResolver`). Does not spawn processes or probe sandbox adapters.
pub fn link(
    cfg: &ProjectConfig,
    module: &IrModule,
    lockfile: &LockFile,
    scope: &Scope,
) -> Result<LinkOutcome, Vec<LinkError>> {
    let mut errors = Vec::new();

    // 1. Gather (package, version_req, expected_port) for every referenced plugin.
    //    Model backends: from [models] entries referenced by an agent (port LlmBackend, any version).
    //    Tools: each agent's requires.tools (port Tool, the tool's version_req).
    // resolve_models is infallible — ProjectConfig::validate already guarantees
    // aliases resolve; this only builds the final alias->ModelRef map.
    let model_bindings = resolve_models(cfg);

    let mut wanted: BTreeMap<PackageName, (semver::VersionReq, PortKind)> = BTreeMap::new();
    for m in cfg.models.values() {
        if let Ok(name) = PackageName::from_str(&m.backend) {
            wanted.entry(name).or_insert((semver::VersionReq::STAR, PortKind::LlmBackend));
        }
    }
    for agent in cfg.agents.values() {
        for tool in &agent.requires.tools {
            wanted
                .entry(tool.name.clone())
                .or_insert((tool.version_req.clone(), PortKind::Tool));
        }
    }

    // 2. Resolve every wanted plugin, collecting errors; keep successes ordered by name.
    let mut resolved_plugins = Vec::new();
    for (name, (req, port)) in &wanted {
        match resolve_one_plugin(lockfile, name, req, *port) {
            Ok(p) => resolved_plugins.push(p),
            Err(e) => errors.push(e),
        }
    }
    resolved_plugins.sort_by(|a, b| a.name.cmp(&b.name));

    // 3. Skills.
    let (mut resolved_skills, parsed_skills, skill_errs) = resolve_skills(scope, lockfile);
    errors.extend(skill_errs);
    resolved_skills.sort_by(|a, b| a.name.cmp(&b.name));

    if !errors.is_empty() {
        errors.sort_by_key(link_error_sort_key); // deterministic, no-drift bar
        return Err(errors);
    }

    let lockfile_sha256 = sha256_hex(&lockfile_bytes(scope));
    Ok(LinkOutcome {
        record: LinkRecord {
            resolved_plugins,
            resolved_skills,
            model_bindings,
            platform: module.target,
            lockfile_sha256,
        },
        parsed_skills,
    })
}

/// Stable sort key for deterministic error ordering across callers.
fn link_error_sort_key(e: &LinkError) -> (u8, String) {
    match e {
        LinkError::PluginNotInstalled { package } => (0, package.clone()),
        LinkError::PluginPortMismatch { package, .. } => (1, package.clone()),
        LinkError::VersionUnsatisfied { package, .. } => (2, package.clone()),
        LinkError::SkillMissing { package, .. } => (3, package.clone()),
        LinkError::SkillParse { package, .. } => (4, package.clone()),
    }
}

/// Read the lockfile bytes for hashing (empty if unreadable — the sha then
/// simply won't match at verify, which is the correct fail-closed behavior).
fn lockfile_bytes(scope: &Scope) -> Vec<u8> {
    std::fs::read(scope.lockfile_path()).unwrap_or_default()
}
```

- [ ] **Step 4: Run all link tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg link::`
Expected: PASS (all tasks' tests).

- [ ] **Step 5: Clippy + fmt the crate**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-pkg --all-targets` then `cargo fmt -p tau-pkg`.
Expected: no warnings; fmt clean.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-pkg/src/link.rs
git commit -m "feat(link): link() entrypoint — assemble record + collect all errors"
```

---

### Task 6: ADR-0060 + SUMMARY.md

**Files:**
- Create: `docs/decisions/0060-build-links-verified-linkrecord.md`
- Modify: `docs/SUMMARY.md` (add the ADR line under the decisions section)

**Interfaces:** none (docs).

- [ ] **Step 1: Write ADR-0060**

Use `docs/decisions/template.md` structure. Title: "tau build links; bundles carry a verified LinkRecord; run trusts after verify." Status: Proposed. Content: the static-linker vs loader split (Q6), why sandbox + tool-binding stay runtime (Q5/Q6), the `LinkRecord` shape, the 4-PR rollout. Reference the spec at `docs/superpowers/specs/2026-07-19-link-phase-linkrecord-design.md`. Keep messages/paths consistent with the code from Tasks 1–5.

- [ ] **Step 2: Add to SUMMARY.md**

Add the line `- [0060 — Build links; verified LinkRecord](decisions/0060-build-links-verified-linkrecord.md)` in the decisions list, after the 0058 entry.

- [ ] **Step 3: Build the book to verify no broken links**

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build` then `rm -rf book`.
Expected: only `[INFO]` lines; no linkcheck errors.

- [ ] **Step 4: Commit**

```bash
git add docs/decisions/0060-build-links-verified-linkrecord.md docs/SUMMARY.md
git commit -m "docs(link): ADR-0060 — build links; verified LinkRecord; run trusts after verify"
```

---

## Self-Review

**Spec coverage:**
- §1 `LinkRecord`/`LinkError` types → Task 1. ✓ (`tool_bindings`/`SandboxUnavailable` correctly absent per Q5/Q6.)
- §1 plugin resolution (installed/port/version) → Task 2. ✓
- §1 model bindings (alias→ModelRef, no re-resolution) → Task 3. ✓
- §2 skills: parse-once, sha + parsed_ok, parsed map for resolver seed → Task 4 (+ `LinkOutcome.parsed_skills`). ✓
- §1 collect-all-errors + deterministic ordering → Task 5. ✓
- Deliverables: ADR-0060 + SUMMARY → Task 6. ✓
- Out of scope (correct for PR 1): callers (PR 2), run --bundle trust (PR 3), credential flip (PR 4), the runtime record-seeded `SkillResolver` adapter (PR 2 — Task 4 only produces the parsed map).

**Placeholder scan:** Two tests (Task 3, Task 5 fixtures) intentionally defer the `ProjectConfig`/full fixture construction to "grep the existing project-validation test idiom" because `ProjectConfig` is `#[non_exhaustive]` and its test-construction path is crate-local — the implementer must reuse the existing idiom rather than invent one. This is a real instruction, not a TODO; the assertions are fully concrete. Flag for the executor: resolve these two fixtures first.

**Type consistency:** `LinkedPlugin`/`LinkedSkill`/`LinkRecord`/`LinkOutcome`/`LinkError` field names are consistent across Tasks 1–5. `resolve_one_plugin`, `resolve_models`, `resolve_skills`, `link` signatures match their call sites in Task 5. `ModelRef { backend, model_id }` matches the verified tau-ir definition. `PortKind::{LlmBackend, Tool}` and `LockedPlugin::new(...)` match verified APIs.

**Known compiler-resolved unknowns (call out to executor):**
1. `PortKind` and `TargetTriple` serde: may need a derive behind the `serde` feature or the string-newtype treatment shown (Task 1 Step 3/5).
2. `TargetTriple::from_str` existence — wire the actual parser (Task 1 note).
3. `PluginManifest { kind: Default::default() }` — confirm `PluginKind: Default`; else name a variant (Task 2 fixture).
4. The `sha256_hex` helper — prefer an existing crate helper over the local copy (Task 4).
