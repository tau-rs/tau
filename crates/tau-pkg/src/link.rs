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
//! This module defines the linked-world types ([`LinkedPlugin`],
//! [`LinkedSkill`], [`LinkRecord`], [`LinkOutcome`]), their error type
//! ([`LinkError`]), and the [`link()`](link) entry point that produces a
//! [`LinkOutcome`] from a `ProjectConfig` + `IrModule` + `LockFile` +
//! `Scope`.

use std::collections::BTreeMap;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use tau_domain::package::plugin::PortKind;
use tau_domain::{PackageName, Version};
use tau_ir::model_ref::ModelRef;
use tau_ir::IrModule;
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
    /// A skill package's `SKILL.md` failed to parse, or its `tau.toml`/
    /// manifest (or the on-disk lockfile) couldn't be read/parsed;
    /// `detail` names the specific cause.
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
/// Called from [`link()`] for every model backend and required tool.
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
/// Called from [`link()`] to populate [`LinkRecord::model_bindings`].
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

/// Resolve every installed skill package against the on-disk `SKILL.md`,
/// parsing it exactly once and recording the outcome.
///
/// Enumerates lockfile entries in the *passed* `lockfile` where
/// `pkg.skill.is_some()` (the same filter [`crate::find_installed_skill`]
/// uses internally), locates each package's install path via that
/// helper, reads + hashes `SKILL.md`, and parses it via
/// [`tau_domain::parse_skill_md`]. Every skill produces exactly one
/// [`LinkedSkill`] entry (with `parsed_ok` set accordingly), except for
/// two classes of resolution failure that short-circuit before parsing:
/// a missing install artifact — no `tau.toml` on disk
/// (`FindSkillError::InstallPathMissing`), or — because
/// `find_installed_skill` re-resolves against the on-disk lockfile via
/// `scope` rather than trusting the passed `lockfile` — a lockfile/disk
/// divergence where the on-disk lockfile no longer lists this package as
/// a skill (`Ok(None)`) — surfaces as [`LinkError::SkillMissing`]; a
/// present-but-unparseable `SKILL.md`, or a malformed `tau.toml`/
/// manifest/lockfile (any other `find_installed_skill` error), surfaces
/// as [`LinkError::SkillParse`] with the reason in `detail`. All errors
/// are collected — this never stops at the first skill that fails, and a
/// skill declared in `lockfile` is never silently dropped from the
/// result.
///
/// Called from [`link()`] to populate [`LinkRecord::resolved_skills`] and
/// [`LinkOutcome::parsed_skills`].
fn resolve_skills(
    scope: &crate::scope::Scope,
    lockfile: &crate::lockfile::LockFile,
) -> (
    Vec<LinkedSkill>,
    BTreeMap<String, tau_domain::SkillContent>,
    Vec<LinkError>,
) {
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
            Ok(None) => {
                // The passed `lockfile` lists this package as a skill, but
                // `find_installed_skill` re-resolves against the on-disk
                // lockfile via `scope` and found no matching skill entry
                // there (lockfile/disk divergence). That's a missing skill
                // install, not "no skills" — record it instead of silently
                // dropping it, or `resolve_skills` could return an empty
                // result indistinguishable from "no skills declared".
                let path = scope.package_dir(&name, &pkg.active_version);
                errors.push(LinkError::SkillMissing {
                    package: name.to_string(),
                    path: path.display().to_string(),
                });
                continue;
            }
            Err(crate::FindSkillError::InstallPathMissing { path, .. }) => {
                errors.push(LinkError::SkillMissing {
                    package: name.to_string(),
                    path: path.display().to_string(),
                });
                continue;
            }
            Err(e) => {
                errors.push(LinkError::SkillParse {
                    package: name.to_string(),
                    detail: e.to_string(),
                });
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
                linked.push(LinkedSkill {
                    name: name.clone(),
                    content_sha256,
                    parsed_ok: true,
                });
                parsed_map.insert(name.to_string(), content);
            }
            Err(e) => {
                linked.push(LinkedSkill {
                    name: name.clone(),
                    content_sha256,
                    parsed_ok: false,
                });
                errors.push(LinkError::SkillParse {
                    package: name.to_string(),
                    detail: e.to_string(),
                });
            }
        }
    }
    (linked, parsed_map, errors)
}

/// Statically link the project against the installed set. Validates every
/// referenced plugin, model backend, and skill; collects **all** errors
/// (never stops at the first) so the operator sees the whole set at once.
///
/// On success returns a [`LinkOutcome`] carrying the serializable
/// [`LinkRecord`] plus the parsed skill bodies (for seeding the runtime
/// `SkillResolver`, PR 2). Does not spawn processes or probe sandbox
/// adapters — that's the runtime *loader*'s job.
///
/// # Precondition
///
/// `lockfile` MUST be the same lockfile persisted at
/// `scope.lockfile_path()`. Plugin resolution ([`resolve_one_plugin`])
/// reads only the passed `lockfile`, but skill resolution
/// ([`resolve_skills`]) and [`LinkRecord::lockfile_sha256`] both read
/// straight from disk via `scope`. Passing a `lockfile` that has drifted
/// from disk is undefined territory for skills specifically: a skill
/// package listed in the passed `lockfile` but absent from the on-disk
/// lockfile surfaces as [`LinkError::SkillMissing`] rather than being
/// silently skipped (see [`resolve_skills`]'s doc comment). Callers (the
/// `build`/`dev` entry points in PR 2) always pass the lockfile they just
/// loaded from `scope.lockfile_path()`, so this invariant holds in
/// practice.
pub fn link(
    cfg: &ProjectConfig,
    module: &IrModule,
    lockfile: &crate::lockfile::LockFile,
    scope: &crate::scope::Scope,
) -> Result<LinkOutcome, Vec<LinkError>> {
    let mut errors = Vec::new();

    // 1. Gather (package, version_req, expected_port) for every referenced
    //    plugin: model backends (any version) and each agent's required
    //    tools. Keyed by the package's string form — `PackageName` has no
    //    `Ord`/`PartialOrd` impl, so it can't itself be a `BTreeMap` key.
    let mut wanted: BTreeMap<String, (PackageName, semver::VersionReq, PortKind)> = BTreeMap::new();
    for m in cfg.models.values() {
        // validate() already guarantees every [models] backend parses as a
        // package name; skip unparseable defensively rather than panic.
        if let Ok(name) = PackageName::from_str(&m.backend) {
            wanted.entry(name.to_string()).or_insert((
                name,
                semver::VersionReq::STAR,
                PortKind::LlmBackend,
            ));
        }
    }
    for agent in cfg.agents.values() {
        for tool in &agent.requires.tools {
            wanted.entry(tool.name.to_string()).or_insert((
                tool.name.clone(),
                tool.version_req.clone(),
                PortKind::Tool,
            ));
        }
    }

    // 2. Resolve every wanted plugin, collecting errors; keep successes
    //    ordered by name (BTreeMap iteration is already name-ordered).
    let mut resolved_plugins = Vec::new();
    for (name, req, port) in wanted.values() {
        match resolve_one_plugin(lockfile, name, req, *port) {
            Ok(p) => resolved_plugins.push(p),
            Err(e) => errors.push(e),
        }
    }
    resolved_plugins.sort_by_key(|a| a.name.to_string());

    // 3. Models — infallible; ProjectConfig::validate already guarantees
    //    every agent's alias resolves.
    let model_bindings = resolve_models(cfg);

    // 4. Skills.
    let (mut resolved_skills, parsed_skills, skill_errs) = resolve_skills(scope, lockfile);
    errors.extend(skill_errs);
    resolved_skills.sort_by_key(|a| a.name.to_string());

    if !errors.is_empty() {
        errors.sort_by_key(link_error_sort_key); // deterministic, no-drift bar
        return Err(errors);
    }

    // lockfile_sha256 hashes the ON-DISK lockfile, not the passed one — see
    // the precondition doc above. `run --bundle` re-derives this same hash
    // from disk to detect drift.
    let lockfile_sha256 = sha256_hex(&std::fs::read(scope.lockfile_path()).unwrap_or_default());

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

/// Stable sort key for deterministic [`LinkError`] ordering across callers.
fn link_error_sort_key(e: &LinkError) -> (u8, String) {
    match e {
        LinkError::PluginNotInstalled { package } => (0, package.clone()),
        LinkError::PluginPortMismatch { package, .. } => (1, package.clone()),
        LinkError::VersionUnsatisfied { package, .. } => (2, package.clone()),
        LinkError::SkillMissing { package, .. } => (3, package.clone()),
        LinkError::SkillParse { package, .. } => (4, package.clone()),
    }
}

/// SHA-256 hex of `bytes`. `tree_hash::to_hex_lower` handles the
/// digest-to-hex step; no crate-wide `sha256_hex(bytes) -> String`
/// one-shot helper exists yet (`tree_hash::sha256_of_file` takes a
/// path, not bytes), so this is the local equivalent.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    crate::tree_hash::to_hex_lower(&hasher.finalize())
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

    // --- resolve_skills -----------------------------------------------
    //
    // Fixture recipe reused from `skill_resolve.rs`'s `make_critic_scope`
    // (the exact installed-skill-tree builder: `.tau/config.toml` +
    // `.tau/packages/<name>/<version>/{tau.toml,SKILL.md}` +
    // `LockedPackage { skill: Some(LockedSkill::new(..)) }` saved to
    // `tau-lock.toml`), parameterized here so the SKILL.md body/presence
    // can vary per test.

    use crate::lockfile::{LockedSkill, SkillFrontmatterSnapshot};
    use crate::scope::Scope;
    use tempfile::tempdir;

    /// Build a scope with one installed skill package "greeter". Writes
    /// `tau.toml` always; writes `SKILL.md` with `skill_md_body` unless
    /// `write_skill_md` is `false` (to exercise the missing-file case).
    fn skill_scope(
        tmp: &std::path::Path,
        skill_md_body: &str,
        write_skill_md: bool,
    ) -> (Scope, LockFile) {
        let tau_dir = tmp.join(".tau");
        std::fs::create_dir_all(&tau_dir).unwrap();
        std::fs::write(
            tau_dir.join("config.toml"),
            "schema_version = 3\nkind = \"project\"\ncreated_at = \"2026-05-14T00:00:00Z\"\ncreated_by_tau_version = \"0.0.0\"\n\n[sandbox]\nrequired_tier = \"none\"\n",
        )
        .unwrap();

        let install_dir = tau_dir.join("packages").join("greeter").join("0.1.0");
        std::fs::create_dir_all(&install_dir).unwrap();
        std::fs::write(
            install_dir.join("tau.toml"),
            r#"name = "greeter"
version = "0.1.0"
description = "Greets people."
authors = []
source = "https://example.com/greeter.git"
kind = "skill"
dependencies = []
capabilities = []

[skill]
"#,
        )
        .unwrap();
        if write_skill_md {
            std::fs::write(install_dir.join("SKILL.md"), skill_md_body).unwrap();
        }

        let locked_pkg = LockedPackage {
            name: PackageName::from_str("greeter").unwrap(),
            active_version: Version::parse("0.1.0").unwrap(),
            source: PackageSource::Git {
                location: "https://example.com/greeter.git".parse().unwrap(),
                rev: None,
            },
            installed_versions: vec![LockedVersion {
                version: Version::parse("0.1.0").unwrap(),
                rev: None,
                resolved_commit: "0".repeat(40),
                sha256: String::new(),
                installed_at: SystemTime::UNIX_EPOCH,
            }],
            plugin: None,
            skill: Some(LockedSkill::new(
                "deadbeef".into(),
                SkillFrontmatterSnapshot {
                    name: "greeter".into(),
                    description: "Greets people.".into(),
                },
            )),
            synthesized_from: None,
        };

        let mut lf = LockFile::default();
        lf.packages.push(locked_pkg);
        lf.save(&tmp.join("tau-lock.toml")).unwrap();

        (Scope::resolve(tmp).unwrap(), lf)
    }

    const VALID_SKILL_MD: &str = "---\nname: greeter\ndescription: Greets people.\n---\nHello!\n";

    #[test]
    fn resolve_skills_ok() {
        let tmp = tempdir().unwrap();
        let (scope, lf) = skill_scope(tmp.path(), VALID_SKILL_MD, true);

        let (linked, parsed, errs) = resolve_skills(&scope, &lf);
        assert!(errs.is_empty(), "got {errs:?}");
        assert_eq!(linked.len(), 1, "got {linked:?}");
        assert!(linked[0].parsed_ok, "got {:?}", linked[0]);
        assert!(parsed.contains_key("greeter"), "got {parsed:?}");
    }

    #[test]
    fn resolve_skills_missing_file() {
        let tmp = tempdir().unwrap();
        // tau.toml written, SKILL.md deliberately not written.
        let (scope, lf) = skill_scope(tmp.path(), VALID_SKILL_MD, false);

        let (_linked, _parsed, errs) = resolve_skills(&scope, &lf);
        assert!(
            errs.iter()
                .any(|e| matches!(e, LinkError::SkillMissing { .. })),
            "got {errs:?}"
        );
    }

    #[test]
    fn resolve_skills_declared_but_unresolvable_is_missing() {
        // Regression test for the "silent empty result" finding: the
        // *passed* `lockfile` declares a skill package, but
        // `find_installed_skill` re-resolves against the on-disk lockfile
        // via `scope` (not the passed `lockfile`) and finds nothing there
        // — no `tau-lock.toml` on disk at all, so `find_installed_skill`
        // takes its `Ok(None)` branch (scope.lockfile_path() doesn't
        // exist). Under the old `Ok(None) => continue` behavior this test
        // would fail: `errs` would be empty and `resolve_skills` would
        // return `(vec![], {}, vec![])`, indistinguishable from "no
        // skills declared", even though `lockfile` lists one.
        let tmp = tempdir().unwrap();
        let tau_dir = tmp.path().join(".tau");
        std::fs::create_dir_all(&tau_dir).unwrap();
        std::fs::write(
            tau_dir.join("config.toml"),
            "schema_version = 3\nkind = \"project\"\ncreated_at = \"2026-05-14T00:00:00Z\"\ncreated_by_tau_version = \"0.0.0\"\n\n[sandbox]\nrequired_tier = \"none\"\n",
        )
        .unwrap();
        // Deliberately no tau-lock.toml written to disk.
        let scope = Scope::resolve(tmp.path()).unwrap();

        let locked_pkg = LockedPackage {
            name: PackageName::from_str("greeter").unwrap(),
            active_version: Version::parse("0.1.0").unwrap(),
            source: PackageSource::Git {
                location: "https://example.com/greeter.git".parse().unwrap(),
                rev: None,
            },
            installed_versions: vec![LockedVersion {
                version: Version::parse("0.1.0").unwrap(),
                rev: None,
                resolved_commit: "0".repeat(40),
                sha256: String::new(),
                installed_at: SystemTime::UNIX_EPOCH,
            }],
            plugin: None,
            skill: Some(LockedSkill::new(
                "deadbeef".into(),
                SkillFrontmatterSnapshot {
                    name: "greeter".into(),
                    description: "Greets people.".into(),
                },
            )),
            synthesized_from: None,
        };
        let mut lf = LockFile::default();
        lf.packages.push(locked_pkg);

        let (linked, parsed, errs) = resolve_skills(&scope, &lf);
        assert!(linked.is_empty(), "got {linked:?}");
        assert!(parsed.is_empty(), "got {parsed:?}");
        assert!(
            errs.iter()
                .any(|e| matches!(e, LinkError::SkillMissing { .. })),
            "got {errs:?}"
        );
    }

    #[test]
    fn resolve_skills_parse_error() {
        let tmp = tempdir().unwrap();
        // No leading `---` frontmatter delimiter — parse_skill_md fails.
        let (scope, lf) = skill_scope(tmp.path(), "not frontmatter at all\n", true);

        let (linked, _parsed, errs) = resolve_skills(&scope, &lf);
        assert!(
            errs.iter()
                .any(|e| matches!(e, LinkError::SkillParse { .. })),
            "got {errs:?}"
        );
        assert!(linked.iter().any(|s| !s.parsed_ok), "got {linked:?}");
    }

    // --- link() ---------------------------------------------------------
    //
    // Full-stack fixture: on-disk scope with a `greeter` skill plus (as
    // requested) an `anthropic` LlmBackend plugin and — for the happy path
    // only — a `curl` Tool plugin, all written to a single `tau-lock.toml`.
    // `ProjectConfig` is built via `UncheckedProjectConfig::validate()` (the
    // only public construction path — `ProjectConfig` is `#[non_exhaustive]`
    // with no public struct-literal or builder), mirroring the pattern used
    // throughout `project/project.rs`'s own tests.

    use crate::project::project::UncheckedProjectConfig;
    use tau_ir::{IrFormatVersion, Workflow};

    /// Build a scope on disk with a `greeter` skill package and an
    /// `anthropic` (LlmBackend) plugin package, optionally also a `curl`
    /// (Tool) plugin package. `skill_md_body` controls the installed
    /// skill's `SKILL.md` contents, so callers can produce a corrupt skill.
    fn link_fixture_scope(
        tmp: &std::path::Path,
        include_tool_plugin: bool,
        skill_md_body: &str,
    ) -> (Scope, LockFile) {
        let tau_dir = tmp.join(".tau");
        std::fs::create_dir_all(&tau_dir).unwrap();
        std::fs::write(
            tau_dir.join("config.toml"),
            "schema_version = 3\nkind = \"project\"\ncreated_at = \"2026-05-14T00:00:00Z\"\ncreated_by_tau_version = \"0.0.0\"\n\n[sandbox]\nrequired_tier = \"none\"\n",
        )
        .unwrap();

        let install_dir = tau_dir.join("packages").join("greeter").join("0.1.0");
        std::fs::create_dir_all(&install_dir).unwrap();
        std::fs::write(
            install_dir.join("tau.toml"),
            r#"name = "greeter"
version = "0.1.0"
description = "Greets people."
authors = []
source = "https://example.com/greeter.git"
kind = "skill"
dependencies = []
capabilities = []

[skill]
"#,
        )
        .unwrap();
        std::fs::write(install_dir.join("SKILL.md"), skill_md_body).unwrap();

        let mut lf = LockFile::default();
        lf.packages.push(plugin_pkg(
            "anthropic",
            "1.0.0",
            PortKind::LlmBackend,
            &"ab".repeat(32),
        ));
        if include_tool_plugin {
            lf.packages.push(plugin_pkg(
                "curl",
                "1.0.0",
                PortKind::Tool,
                &"cd".repeat(32),
            ));
        }
        lf.packages.push(LockedPackage {
            name: PackageName::from_str("greeter").unwrap(),
            active_version: Version::parse("0.1.0").unwrap(),
            source: PackageSource::Git {
                location: "https://example.com/greeter.git".parse().unwrap(),
                rev: None,
            },
            installed_versions: vec![LockedVersion {
                version: Version::parse("0.1.0").unwrap(),
                rev: None,
                resolved_commit: "0".repeat(40),
                sha256: String::new(),
                installed_at: SystemTime::UNIX_EPOCH,
            }],
            plugin: None,
            skill: Some(LockedSkill::new(
                "deadbeef".into(),
                SkillFrontmatterSnapshot {
                    name: "greeter".into(),
                    description: "Greets people.".into(),
                },
            )),
            synthesized_from: None,
        });
        lf.save(&tmp.join("tau-lock.toml")).unwrap();

        (Scope::resolve(tmp).unwrap(), lf)
    }

    /// A `ProjectConfig` with one agent (`reviewer`) whose model alias
    /// `"fast"` resolves to backend `"anthropic"`, and which — when
    /// `require_tool` is set — requires the `curl` tool.
    fn link_fixture_cfg(require_tool: bool) -> ProjectConfig {
        let tools_block = if require_tool {
            r#"
            [[agents.reviewer.requires.tools]]
            name = "curl"
            source = "https://example.com/curl.git"
            version = "^1"
            "#
        } else {
            ""
        };
        let toml_str = format!(
            r#"
            [project]
            name = "demo"

            [models]
            fast = {{ backend = "anthropic", model = "claude-haiku-4-5" }}

            [agents.reviewer]
            display_name = "Reviewer"
            package      = "anthropic@^1"
            model        = "fast"
            {tools_block}
            "#
        );
        toml::from_str::<UncheckedProjectConfig>(&toml_str)
            .expect("parse")
            .validate()
            .expect("valid config")
    }

    fn link_fixture_module() -> IrModule {
        IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: "0.0.0".into(),
            target: TargetTriple::host(),
            workflow: Workflow::default(),
            triggers: Vec::new(),
        }
    }

    #[test]
    fn link_happy_path_produces_record() {
        let tmp = tempdir().unwrap();
        let (scope, lf) = link_fixture_scope(tmp.path(), true, VALID_SKILL_MD);
        let cfg = link_fixture_cfg(true);
        let module = link_fixture_module();

        let out = link(&cfg, &module, &lf, &scope).expect("link ok");

        assert_eq!(out.record.platform, TargetTriple::host());
        assert!(
            out.record
                .resolved_plugins
                .iter()
                .any(|p| p.provides == PortKind::LlmBackend),
            "got {:?}",
            out.record.resolved_plugins
        );
        assert!(
            out.record
                .resolved_plugins
                .iter()
                .any(|p| p.provides == PortKind::Tool),
            "got {:?}",
            out.record.resolved_plugins
        );
        assert!(
            out.record.model_bindings.contains_key("fast"),
            "got {:?}",
            out.record.model_bindings
        );
        assert_eq!(out.record.lockfile_sha256.len(), 64);
        assert_eq!(out.record.resolved_skills.len(), 1);
        assert!(out.parsed_skills.contains_key("greeter"));
    }

    #[test]
    fn link_collects_all_errors() {
        // Two DISTINCT faults:
        //  (a) the agent requires tool "curl", but no curl plugin is
        //      installed -> PluginNotInstalled.
        //  (b) the installed "greeter" skill has a corrupt SKILL.md
        //      (no frontmatter) -> SkillParse.
        let tmp = tempdir().unwrap();
        let (scope, lf) = link_fixture_scope(tmp.path(), false, "not frontmatter at all\n");
        let cfg = link_fixture_cfg(true);
        let module = link_fixture_module();

        let errs = link(&cfg, &module, &lf, &scope).unwrap_err();

        assert!(
            errs.iter()
                .any(|e| matches!(e, LinkError::PluginNotInstalled { .. })),
            "got {errs:?}"
        );
        assert!(
            errs.iter()
                .any(|e| matches!(e, LinkError::SkillParse { .. })),
            "got {errs:?}"
        );
        assert!(errs.len() >= 2, "must collect all, got {errs:?}");
    }

    #[test]
    fn link_error_sort_key_orders_deterministically() {
        // Deliberately out of order: variant rank descending in places,
        // and "z" before "b"/"a" — pins the (variant_rank, package)
        // ordering contract `link()` relies on for no-drift output.
        let mut errs = vec![
            LinkError::SkillParse {
                package: "z".to_string(),
                detail: "bad".to_string(),
            },
            LinkError::PluginNotInstalled {
                package: "b".to_string(),
            },
            LinkError::PluginNotInstalled {
                package: "a".to_string(),
            },
            LinkError::VersionUnsatisfied {
                package: "m".to_string(),
                req: "^1".to_string(),
            },
        ];

        errs.sort_by_key(link_error_sort_key);

        let got: Vec<&str> = errs
            .iter()
            .map(|e| match e {
                LinkError::PluginNotInstalled { package } => package.as_str(),
                LinkError::PluginPortMismatch { package, .. } => package.as_str(),
                LinkError::VersionUnsatisfied { package, .. } => package.as_str(),
                LinkError::SkillMissing { package, .. } => package.as_str(),
                LinkError::SkillParse { package, .. } => package.as_str(),
            })
            .collect();
        assert_eq!(got, vec!["a", "b", "m", "z"], "got {errs:?}");

        assert!(matches!(errs[0], LinkError::PluginNotInstalled { .. }));
        assert!(matches!(errs[1], LinkError::PluginNotInstalled { .. }));
        assert!(matches!(errs[2], LinkError::VersionUnsatisfied { .. }));
        assert!(matches!(errs[3], LinkError::SkillParse { .. }));
    }
}
