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
    /// An agent references a model alias absent from `[models]`.
    #[error("agent {agent} references model alias {alias} absent from [models]")]
    ModelAliasUnknown {
        /// The agent id.
        agent: String,
        /// The unknown model alias.
        alias: String,
    },
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
}
