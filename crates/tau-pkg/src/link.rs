//! Static link phase: validate the installed world the IR binds to.
//!
//! `link()` is a *static linker* — it checks that every plugin, model
//! backend, and skill the project references resolves against the
//! installed set (lockfile + manifests + on-disk `SKILL.md`), with **no
//! process spawning**. It produces a serializable [`LinkRecord`] embedded
//! in the bundle (PR 2) and trusted by `run --bundle` after verification
//! (PR 3). Tool-name binding and sandbox adapter probing are the runtime
//! *loader*'s job and are out of scope here.
//!
//! This module currently defines only the linked-world types
//! ([`LinkedPlugin`], [`LinkedSkill`], [`LinkRecord`], [`LinkOutcome`]) and
//! their error type ([`LinkError`]). The `link()` entry point that
//! produces a [`LinkOutcome`] lands in a later PR-1 task.

use std::collections::BTreeMap;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use tau_domain::package::plugin::PortKind;
use tau_domain::{PackageName, Version};
use tau_ir::model_ref::ModelRef;
use tau_ports::target::TargetTriple;

use crate::project::project::{AgentEntry, ModelEntry, ProjectConfig};

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

/// The verified binding record produced by `link()`. Embedded in the
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

/// The full result of `link()`: the serializable [`LinkRecord`] plus the
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

/// Errors from static linking. All variants are collected — `link()`
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
}

/// Resolve one plugin reference against the lockfile: installed, provides
/// the expected port, and has an installed version satisfying `version_req`.
///
/// Ports the installed/port checks from `tau-cli`'s `plugin_loader.rs`
/// (`resolve_plugin`) and the version-satisfiability search from
/// `tau-pkg`'s `project/agent.rs` (`build_agent_definition`) into a single
/// pure helper returning a structured [`LinkError`] instead of `anyhow`.
///
/// Not yet called outside `#[cfg(test)]` — `link()` (a later PR-1 task)
/// wires this in alongside `resolve_models`/`resolve_skills`.
#[allow(dead_code)]
fn resolve_one_plugin(
    lockfile: &crate::lockfile::LockFile,
    package: &PackageName,
    version_req: &semver::VersionReq,
    expected: PortKind,
) -> Result<LinkedPlugin, LinkError> {
    let pkg = lockfile
        .find(package)
        .ok_or_else(|| LinkError::PluginNotInstalled {
            package: package.to_string(),
        })?;

    // Must carry a [plugin] table and provide the required port
    // (plugin_loader.rs shape). A data-only package (no [plugin] table)
    // can't bind to a plugin reference, so it's PluginNotInstalled too.
    // Checked before version satisfiability: structural validity (does
    // this package even back a plugin?) takes precedence over "which
    // version" — a data-only package is PluginNotInstalled regardless of
    // what versions happen to be installed.
    let plugin = pkg
        .plugin
        .as_ref()
        .ok_or_else(|| LinkError::PluginNotInstalled {
            package: package.to_string(),
        })?;
    if plugin.manifest.provides != expected {
        return Err(LinkError::PluginPortMismatch {
            package: package.to_string(),
            found: plugin.manifest.provides,
            expected,
        });
    }

    // Highest installed version satisfying the requirement (agent.rs shape).
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

    Ok(LinkedPlugin {
        name: package.clone(),
        version,
        binary_sha256: plugin.binary_sha256.clone(),
        provides: plugin.manifest.provides,
    })
}

/// Resolve every agent's `model` alias against `[models]`, producing the
/// alias → [`ModelRef`] bindings [`LinkRecord::model_bindings`] carries.
/// Agents with an empty `model` (no model declared) are skipped.
///
/// Infallible: `ProjectConfig::validate` already rejects an agent whose
/// `model` is empty (`ProjectConfigError::MissingAgentModel`) or absent
/// from `[models]` (`ProjectConfigError::UnknownModelAlias`) at parse
/// time, so a `ProjectConfig` carrying either defect can't be
/// constructed through the public validation path — every alias this
/// function looks up is guaranteed present.
///
/// Thin wrapper over [`resolve_models_from`], which exists so this
/// function's logic stays directly unit-testable regardless.
///
/// Not yet called outside `#[cfg(test)]` — `link()` (a later PR-1 task)
/// wires this in alongside `resolve_one_plugin`/`resolve_skills`.
#[allow(dead_code)]
fn resolve_models(cfg: &ProjectConfig) -> BTreeMap<String, ModelRef> {
    resolve_models_from(&cfg.agents, &cfg.models)
}

/// Core logic for [`resolve_models`], taking the agent and model maps
/// directly. See [`resolve_models`] for why this split exists.
fn resolve_models_from(
    agents: &BTreeMap<String, AgentEntry>,
    models: &BTreeMap<String, ModelEntry>,
) -> BTreeMap<String, ModelRef> {
    let mut bindings = BTreeMap::new();
    for agent in agents.values() {
        if agent.model.is_empty() {
            continue; // agent declares no model
        }
        if let Some(ModelEntry { backend, model }) = models.get(&agent.model) {
            bindings.insert(
                agent.model.clone(),
                ModelRef {
                    backend: backend.clone(),
                    model_id: model.clone(),
                },
            );
        }
        // An absent alias is unreachable on validated input
        // (ProjectConfig::validate rejects it) — no error, no panic.
    }
    bindings
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

#[cfg(test)]
mod tests {
    use super::*;
    use tau_ports::target::TargetTriple;

    fn sample_record() -> LinkRecord {
        let mut model_bindings = BTreeMap::new();
        model_bindings.insert(
            "fast".to_string(),
            ModelRef {
                backend: "anthropic".into(),
                model_id: "claude-haiku-4-5".into(),
            },
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

    use crate::lockfile::{LockFile, LockedPackage, LockedPlugin, LockedVersion};
    use std::time::SystemTime;
    use tau_domain::package::plugin::PluginManifest;
    use tau_domain::{PackageSource, PluginKind};

    fn plugin_pkg(name: &str, version: &str, provides: PortKind, sha: &str) -> LockedPackage {
        let v = Version::parse(version).unwrap();
        LockedPackage {
            name: PackageName::from_str(name).unwrap(),
            active_version: v.clone(),
            source: PackageSource::Git {
                location: "https://x/y.git".parse().unwrap(),
                rev: None,
            },
            installed_versions: vec![LockedVersion {
                version: v,
                rev: None,
                resolved_commit: "0".repeat(40),
                sha256: String::new(),
                installed_at: SystemTime::UNIX_EPOCH,
            }],
            plugin: Some(LockedPlugin::new(
                PluginManifest::new(provides, PluginKind::RustCargo, "x".into()),
                "/tmp/x".into(),
                SystemTime::UNIX_EPOCH,
                sha.to_string(),
            )),
            skill: None,
            synthesized_from: None,
        }
    }

    fn data_only_pkg(name: &str, version: &str) -> LockedPackage {
        let v = Version::parse(version).unwrap();
        LockedPackage {
            name: PackageName::from_str(name).unwrap(),
            active_version: v.clone(),
            source: PackageSource::Git {
                location: "https://x/y.git".parse().unwrap(),
                rev: None,
            },
            installed_versions: vec![LockedVersion {
                version: v,
                rev: None,
                resolved_commit: "0".repeat(40),
                sha256: String::new(),
                installed_at: SystemTime::UNIX_EPOCH,
            }],
            plugin: None,
            skill: None,
            synthesized_from: None,
        }
    }

    fn lf_with(pkg: LockedPackage) -> LockFile {
        let mut lf = LockFile::default();
        lf.packages.push(pkg);
        lf
    }

    fn req(s: &str) -> semver::VersionReq {
        semver::VersionReq::parse(s).unwrap()
    }

    #[test]
    fn resolve_plugin_ok() {
        let lf = lf_with(plugin_pkg(
            "anthropic",
            "1.2.0",
            PortKind::LlmBackend,
            &"ab".repeat(32),
        ));
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
        assert!(
            matches!(r, Err(LinkError::PluginNotInstalled { .. })),
            "got {r:?}"
        );
    }

    #[test]
    fn resolve_plugin_port_mismatch() {
        let lf = lf_with(plugin_pkg(
            "some-tool",
            "1.0.0",
            PortKind::Tool,
            &"ab".repeat(32),
        ));
        let name = PackageName::from_str("some-tool").unwrap();
        let r = resolve_one_plugin(&lf, &name, &req("^1"), PortKind::LlmBackend);
        assert!(
            matches!(r, Err(LinkError::PluginPortMismatch { .. })),
            "got {r:?}"
        );
    }

    #[test]
    fn resolve_plugin_version_unsatisfied() {
        let lf = lf_with(plugin_pkg(
            "anthropic",
            "1.0.0",
            PortKind::LlmBackend,
            &"ab".repeat(32),
        ));
        let name = PackageName::from_str("anthropic").unwrap();
        let r = resolve_one_plugin(&lf, &name, &req("^2"), PortKind::LlmBackend);
        assert!(
            matches!(r, Err(LinkError::VersionUnsatisfied { .. })),
            "got {r:?}"
        );
    }

    #[test]
    fn resolve_plugin_data_only_not_installed() {
        // Data-only package (no [plugin] table) whose installed version
        // (1.0.0) also fails to satisfy the requested req (^2). Structural
        // validity must be checked before version satisfiability, so this
        // must be PluginNotInstalled, not VersionUnsatisfied.
        let lf = lf_with(data_only_pkg("some-data-pkg", "1.0.0"));
        let name = PackageName::from_str("some-data-pkg").unwrap();
        let r = resolve_one_plugin(&lf, &name, &req("^2"), PortKind::LlmBackend);
        assert!(
            matches!(r, Err(LinkError::PluginNotInstalled { .. })),
            "got {r:?}"
        );
    }

    use crate::project::project::{PromptEntry, RequiresEntry};

    fn agent_with_model(id: &str, model: &str) -> AgentEntry {
        let mut agent = AgentEntry::new(
            id.to_string(),
            id.to_string(),
            "some-pkg@^0.1".to_string(),
            RequiresEntry::default(),
            BTreeMap::new(),
            PromptEntry::None,
            vec![],
        );
        agent.model = model.to_string();
        agent
    }

    #[test]
    fn resolve_models_ok() {
        let mut agents = BTreeMap::new();
        agents.insert("a".to_string(), agent_with_model("a", "fast"));
        // "missing" isn't rejected here on purpose: ProjectConfig::validate
        // guarantees a real ProjectConfig never carries an unresolvable
        // alias, but resolve_models_from is exercised directly (without
        // going through validate) to confirm it's infallible even then —
        // an absent alias is simply omitted from the returned map.
        agents.insert("b".to_string(), agent_with_model("b", "missing"));

        let mut models = BTreeMap::new();
        models.insert(
            "fast".to_string(),
            ModelEntry {
                backend: "anthropic".into(),
                model: "claude-haiku-4-5".into(),
            },
        );

        let bindings = resolve_models_from(&agents, &models);

        assert_eq!(
            bindings.get("fast"),
            Some(&ModelRef {
                backend: "anthropic".into(),
                model_id: "claude-haiku-4-5".into(),
            }),
            "got {bindings:?}"
        );
        assert!(!bindings.contains_key("missing"), "got {bindings:?}");
        assert_eq!(bindings.len(), 1, "got {bindings:?}");
    }

    #[test]
    fn resolve_models_skips_agent_with_empty_model() {
        let mut agents = BTreeMap::new();
        agents.insert("c".to_string(), agent_with_model("c", ""));

        let models: BTreeMap<String, ModelEntry> = BTreeMap::new();

        let bindings = resolve_models_from(&agents, &models);
        assert!(bindings.is_empty(), "got {bindings:?}");
    }
}
