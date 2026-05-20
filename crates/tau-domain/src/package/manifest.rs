//! Package manifest types.
//!
//! Manifest deserialization (TOML/JSON) lives in `tau-pkg`; this module
//! owns only the data type, the typestate around validation, and serde
//! derives (under the `serde` feature).

use crate::id::PackageName;
use crate::package::capability::Capability;
use crate::package::plugin::PluginManifest;
use crate::package::source::PackageSource;
use crate::version::{Version, VersionReq};

/// A dependency declaration: a package name plus a SemVer requirement.
///
/// # Example
///
/// ```ignore
/// // Construction is performed inside `tau-domain` (e.g. by manifest
/// // validation in tau-pkg). External crates receive `PackageDep` values;
/// // they cannot be built via struct expression because the type is
/// // `#[non_exhaustive]`.
/// use tau_domain::{PackageDep, PackageName, VersionReq};
/// use std::str::FromStr;
///
/// let dep = PackageDep {
///     name: PackageName::from_str("fs-tools").unwrap(),
///     version_req: VersionReq::parse("^0.3").unwrap(),
/// };
/// assert_eq!(dep.name.as_str(), "fs-tools");
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PackageDep {
    /// Dependency package name.
    pub name: PackageName,
    /// SemVer version requirement.
    pub version_req: VersionReq,
}

/// A canonical package identity: `(name, version)`.
///
/// # Example
///
/// ```ignore
/// // Like `PackageDep`, `PackageId` is `#[non_exhaustive]` and cannot be
/// // built via struct expression from outside `tau-domain`. External
/// // crates receive instances from manifest validation in tau-pkg.
/// use tau_domain::{PackageId, PackageName, Version};
/// use std::str::FromStr;
///
/// let id = PackageId {
///     name: PackageName::from_str("fs-tools").unwrap(),
///     version: Version::parse("0.3.0").unwrap(),
/// };
/// assert_eq!(id.name.as_str(), "fs-tools");
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PackageId {
    /// Package name.
    pub name: PackageName,
    /// Package version.
    pub version: Version,
}

impl PackageId {
    /// Construct a [`PackageId`] from a validated name and version.
    ///
    /// `PackageId` is `#[non_exhaustive]`: external crates cannot use
    /// struct-literal construction. Callers (notably tau-runtime
    /// integration tests, which assemble `AgentDefinition`s by hand)
    /// use this constructor.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_domain::{PackageId, PackageName, Version};
    /// use std::str::FromStr;
    ///
    /// let id = PackageId::new(
    ///     PackageName::from_str("fs-tools").unwrap(),
    ///     Version::parse("0.3.0").unwrap(),
    /// );
    /// assert_eq!(id.name.as_str(), "fs-tools");
    /// ```
    pub fn new(name: PackageName, version: Version) -> Self {
        Self { name, version }
    }
}

/// Package kind. Structural at v0.1: every kind goes through `Custom`.
/// Typed variants land additively as tau-runtime gains plugin trait
/// awareness for each kind.
///
/// See: [escape-hatches.md#packagekind-custom](../../../../../docs/explanation/escape-hatches.md#packagekind-custom).
///
/// # Example
///
/// ```
/// // `PackageKind` is `#[non_exhaustive]` at the enum level (new variants
/// // are non-breaking), but the `Custom` variant itself is plain and can
/// // be constructed externally via struct-expression syntax.
/// use tau_domain::PackageKind;
/// let k = PackageKind::Custom { kind: "tool".into() };
/// assert!(matches!(k, PackageKind::Custom { ref kind } if kind == "tool"));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageKind {
    /// A package kind not yet typed in core.
    /// See: [escape-hatches.md#packagekind-custom](../../../../../docs/explanation/escape-hatches.md#packagekind-custom).
    Custom {
        /// The kind name. By convention one of [`crate::package::kinds`]'s
        /// constants (e.g. `"llm-backend"`, `"tool"`).
        kind: String,
    },
}

#[cfg(feature = "serde")]
impl serde::Serialize for PackageKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            PackageKind::Custom { kind } => serializer.serialize_str(kind),
        }
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for PackageKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = PackageKind;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a package kind string (e.g. \"tool\", \"llm-backend\")")
            }
            fn visit_str<E>(self, v: &str) -> Result<PackageKind, E>
            where
                E: serde::de::Error,
            {
                if v.is_empty() {
                    return Err(E::custom("package kind cannot be empty"));
                }
                Ok(PackageKind::Custom { kind: v.to_owned() })
            }
            fn visit_string<E>(self, v: String) -> Result<PackageKind, E>
            where
                E: serde::de::Error,
            {
                if v.is_empty() {
                    return Err(E::custom("package kind cannot be empty"));
                }
                Ok(PackageKind::Custom { kind: v })
            }
        }
        deserializer.deserialize_str(Visitor)
    }
}

/// Canonical kind strings for `PackageKind::Custom.kind` and manifest
/// `kind` fields. Recommended convention; tau-domain validates only
/// "non-empty" so plugin authors who want a non-conforming kind name
/// can use `Custom` with arbitrary text.
pub mod kinds {
    /// LLM backend plugin kind.
    pub const LLM_BACKEND: &str = "llm-backend";
    /// Tool plugin kind.
    pub const TOOL: &str = "tool";
    /// Skill plugin kind.
    pub const SKILL: &str = "skill";
    /// Pipeline plugin kind.
    pub const PIPELINE: &str = "pipeline";
    /// MCP server plugin kind.
    pub const MCP_SERVER: &str = "mcp-server";
    /// Storage plugin kind.
    pub const STORAGE: &str = "storage";
    /// Sandbox plugin kind.
    pub const SANDBOX: &str = "sandbox";
}

/// Raw manifest as it appears on disk or on the wire. Deserializes from
/// TOML/JSON directly. May contain field combinations that violate
/// cross-field invariants — call [`UncheckedManifest::validate`] to
/// obtain a verified [`PackageManifest`].
///
/// # Example
///
/// ```no_run
/// use tau_domain::UncheckedManifest;
/// // toml::from_str::<UncheckedManifest>(&raw)?.validate()?;
/// # let _ = std::any::type_name::<UncheckedManifest>();
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UncheckedManifest {
    /// Package name.
    pub name: PackageName,
    /// Package version.
    pub version: Version,
    /// Free-form description.
    pub description: String,
    /// Authors (free-form, e.g. `"Acme Inc <support@acme.dev>"`).
    pub authors: Vec<String>,
    /// SPDX license expression as opaque text. `None` for unlicensed.
    pub license: Option<String>,
    /// Where the package lives.
    pub source: PackageSource,
    /// What the package provides.
    pub kind: PackageKind,
    /// Required dependencies.
    pub dependencies: Vec<PackageDep>,
    /// Capability declarations (G14).
    pub capabilities: Vec<Capability>,
    /// Plugin manifest declared via the `[plugin]` table.
    ///
    /// `None` for data-only packages (no plugin table). `Some` for
    /// plugin packages — `tau-pkg` uses this to gate the build step
    /// during install (see plugin-loading spec §6.1, §6.3).
    #[cfg_attr(feature = "serde", serde(default))]
    pub plugin: Option<PluginManifest>,
    /// Plugin-side sandbox requirements declared via `[sandbox]` table.
    ///
    /// Optional. Default = `PluginSandboxRequirements::default()` (no
    /// tier floor; auto-derived shapes). See
    /// [`PluginSandboxRequirements`](crate::package::sandbox::PluginSandboxRequirements).
    #[cfg_attr(feature = "serde", serde(default))]
    pub sandbox: crate::package::sandbox::PluginSandboxRequirements,
    /// Skill manifest declared via the `[skill]` table.
    ///
    /// `None` for non-skill packages (no skill table). `Some` for skill
    /// packages — `tau-pkg::skill_check` (Skills-2) uses this to gate
    /// SKILL.md validation during install. See ROADMAP §16 and
    /// `docs/decisions/0025-skills-foundation.md`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub skill: Option<crate::package::skill::SkillManifest>,
}

/// Validated package manifest. By construction, satisfies all cross-field
/// invariants enforced by [`UncheckedManifest::validate`]. Cannot be
/// constructed directly — must go through validation.
///
/// To mutate a `PackageManifest`, downgrade via `Into<UncheckedManifest>`,
/// edit, then call [`UncheckedManifest::validate`] again.
///
/// # Example
///
/// ```no_run
/// use tau_domain::{UncheckedManifest, PackageManifest};
/// // let manifest: PackageManifest = unchecked.validate()?;
/// # let _ = std::any::type_name::<PackageManifest>();
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct PackageManifest(UncheckedManifest);

impl PackageManifest {
    /// Package name.
    pub fn name(&self) -> &PackageName {
        &self.0.name
    }
    /// Package version.
    pub fn version(&self) -> &Version {
        &self.0.version
    }
    /// Free-form description.
    pub fn description(&self) -> &str {
        &self.0.description
    }
    /// Authors.
    pub fn authors(&self) -> &[String] {
        &self.0.authors
    }
    /// SPDX license expression, if present.
    pub fn license(&self) -> Option<&str> {
        self.0.license.as_deref()
    }
    /// Source location.
    pub fn source(&self) -> &PackageSource {
        &self.0.source
    }
    /// Package kind.
    pub fn kind(&self) -> &PackageKind {
        &self.0.kind
    }
    /// Required dependencies.
    pub fn dependencies(&self) -> &[PackageDep] {
        &self.0.dependencies
    }
    /// Capability declarations.
    pub fn capabilities(&self) -> &[Capability] {
        &self.0.capabilities
    }
    /// Plugin manifest from the `[plugin]` table, if any.
    ///
    /// `None` for data-only packages; `Some` for plugin packages.
    /// Surfaced verbatim from the `[plugin]` TOML table; structurally
    /// validated by `PluginManifest`'s typed fields.
    pub fn plugin(&self) -> Option<&PluginManifest> {
        self.0.plugin.as_ref()
    }

    /// Plugin-side sandbox requirements (from `[sandbox]` table).
    pub fn sandbox(&self) -> &crate::package::sandbox::PluginSandboxRequirements {
        &self.0.sandbox
    }

    /// Skill manifest from the `[skill]` table, if any.
    ///
    /// `None` for non-skill packages; `Some` for `kind = "skill"`
    /// packages. Surfaced verbatim from the `[skill]` TOML table.
    pub fn skill(&self) -> Option<&crate::package::skill::SkillManifest> {
        self.0.skill.as_ref()
    }

    /// Wrap a checked `UncheckedManifest` without re-running validation.
    /// Internal use only — public API must go through
    /// [`UncheckedManifest::validate`].
    pub(crate) fn from_checked(u: UncheckedManifest) -> Self {
        Self(u)
    }
}

impl From<PackageManifest> for UncheckedManifest {
    fn from(m: PackageManifest) -> Self {
        m.0
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for PackageManifest {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn package_id_eq_works() {
        let a = PackageId {
            name: PackageName::from_str("foo").unwrap(),
            version: Version::parse("1.2.3").unwrap(),
        };
        let b = PackageId {
            name: PackageName::from_str("foo").unwrap(),
            version: Version::parse("1.2.3").unwrap(),
        };
        assert_eq!(a, b);
    }
}

#[cfg(test)]
mod manifest_tests {
    use super::*;
    use std::str::FromStr;

    fn fixture() -> UncheckedManifest {
        UncheckedManifest {
            name: PackageName::from_str("fs-tools").unwrap(),
            version: Version::parse("0.3.0").unwrap(),
            description: "fs tools".into(),
            authors: vec![],
            license: None,
            source: PackageSource::from_str("https://example.com/fs.git").unwrap(),
            kind: PackageKind::Custom {
                kind: "tool".into(),
            },
            dependencies: vec![],
            capabilities: vec![],
            plugin: None,
            sandbox: crate::package::sandbox::PluginSandboxRequirements::default(),
            skill: None,
        }
    }

    #[test]
    fn package_manifest_accessors_work() {
        let m = PackageManifest::from_checked(fixture());
        assert_eq!(m.name().as_str(), "fs-tools");
        assert_eq!(m.description(), "fs tools");
        assert_eq!(m.dependencies().len(), 0);
    }

    #[test]
    fn round_trip_through_unchecked() {
        let m = PackageManifest::from_checked(fixture());
        let u: UncheckedManifest = m.into();
        let m2 = PackageManifest::from_checked(u);
        assert_eq!(m2.name().as_str(), "fs-tools");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn manifest_with_sandbox_block_parses() {
        let toml = r#"
name = "my-plugin"
version = "0.1.0"
description = "test"
authors = []
source = "https://example.com/my-plugin.git"
kind = "tool"
dependencies = []

[[capabilities]]
kind = "fs.read"
paths = ["/tmp/**"]

[plugin]
provides = "tool"
kind = "rust-cargo"
bin = "my-plugin"

[sandbox]
required_tier = "strict"
"#;
        let unchecked: UncheckedManifest = toml::from_str(toml).expect("parse");
        let manifest = unchecked.validate().expect("validate");
        assert_eq!(
            manifest.sandbox().required_tier,
            Some(crate::package::sandbox::PluginRequiredTier::Strict)
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn manifest_without_sandbox_block_defaults() {
        let toml = r#"
name = "my-plugin"
version = "0.1.0"
description = "test"
authors = []
source = "https://example.com/my-plugin.git"
kind = "tool"
dependencies = []
capabilities = []

[plugin]
provides = "tool"
kind = "rust-cargo"
bin = "my-plugin"
"#;
        let unchecked: UncheckedManifest = toml::from_str(toml).expect("parse");
        let manifest = unchecked.validate().expect("validate");
        assert!(manifest.sandbox().required_tier.is_none());
    }

    /// Each shipped real plugin's manifest declares `required_tier =
    /// "strict"` in its `[sandbox]` block (sub-project B Task 1; ADR-0016
    /// Decision 5). Asserts via file-content check rather than full
    /// PackageManifest deserialization because plugin `tau.toml` is the
    /// thinner manifest format consumed at install time and combined
    /// with install-context metadata to form a `PackageManifest`.
    #[test]
    fn shipped_plugin_anthropic_declares_strict_tier() {
        let toml = include_str!("../../../tau-plugins/anthropic/tau.toml");
        assert!(
            toml.contains("[sandbox]") && toml.contains("required_tier = \"strict\""),
            "anthropic must declare strict tier per ADR-0016",
        );
    }

    #[test]
    fn shipped_plugin_fs_read_declares_strict_tier() {
        let toml = include_str!("../../../tau-plugins/fs-read/tau.toml");
        assert!(
            toml.contains("[sandbox]") && toml.contains("required_tier = \"strict\""),
            "fs-read must declare strict tier per ADR-0016",
        );
    }

    #[test]
    fn shipped_plugin_shell_declares_strict_tier() {
        let toml = include_str!("../../../tau-plugins/shell/tau.toml");
        assert!(
            toml.contains("[sandbox]") && toml.contains("required_tier = \"strict\""),
            "shell must declare strict tier per ADR-0016",
        );
    }
}

use crate::error::PackageManifestError;
use crate::package::capability::Capability as Cap;

impl UncheckedManifest {
    /// Run cross-field validation. Returns the validated manifest on
    /// success.
    ///
    /// Field types are already validated at construction (`PackageName`,
    /// `PackageSource`, etc.); this checks invariants those types
    /// can't enforce alone (non-empty description, non-empty Custom
    /// capability names, etc.).
    ///
    /// # Example
    ///
    /// ```ignore
    /// // `UncheckedManifest` is `#[non_exhaustive]`, so it cannot be
    /// // built via struct expression from outside `tau-domain`. In
    /// // practice, callers obtain one by deserializing a manifest file
    /// // (in tau-pkg) and then call `.validate()`.
    /// use tau_domain::{UncheckedManifest, PackageManifestError};
    /// // let err = unchecked.validate().unwrap_err();
    /// // assert_eq!(err, PackageManifestError::EmptyDescription);
    /// # let _ = std::any::type_name::<UncheckedManifest>();
    /// # let _ = std::any::type_name::<PackageManifestError>();
    /// ```
    pub fn validate(self) -> Result<PackageManifest, PackageManifestError> {
        if self.description.is_empty() {
            return Err(PackageManifestError::EmptyDescription);
        }
        // dependency names are already PackageName values (pre-validated),
        // but the loop is here as a hook for future per-dep invariants
        // (e.g. duplicate-name detection, version-range cross-checks).
        // The `index` is kept so it can be threaded into
        // `PackageManifestError::DependencyName { index, source }`.
        #[allow(clippy::unused_enumerate_index)]
        for (_index, _dep) in self.dependencies.iter().enumerate() {
            // no-op at v0.1
        }
        for (i, cap) in self.capabilities.iter().enumerate() {
            if let Cap::Custom { name, .. } = cap {
                if name.is_empty() {
                    return Err(PackageManifestError::CapabilityEmptyName { index: i });
                }
            }
        }
        // Skills-2: kind = "skill" rejects [plugin] block.
        if matches!(&self.kind, PackageKind::Custom { kind } if kind == kinds::SKILL)
            && self.plugin.is_some()
        {
            return Err(PackageManifestError::SkillCannotHavePluginBlock);
        }
        Ok(PackageManifest::from_checked(self))
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::str::FromStr;

    fn good() -> UncheckedManifest {
        UncheckedManifest {
            name: PackageName::from_str("fs-tools").unwrap(),
            version: Version::parse("0.3.0").unwrap(),
            description: "fs tools".into(),
            authors: vec![],
            license: None,
            source: PackageSource::from_str("https://example.com/fs.git").unwrap(),
            kind: PackageKind::Custom {
                kind: "tool".into(),
            },
            dependencies: vec![],
            capabilities: vec![],
            plugin: None,
            sandbox: crate::package::sandbox::PluginSandboxRequirements::default(),
            skill: None,
        }
    }

    #[test]
    fn good_manifest_validates() {
        let m = good().validate().unwrap();
        assert_eq!(m.name().as_str(), "fs-tools");
    }

    #[test]
    fn empty_description_rejected() {
        let mut u = good();
        u.description = String::new();
        assert_eq!(
            u.validate().unwrap_err(),
            PackageManifestError::EmptyDescription
        );
    }

    #[test]
    fn empty_custom_capability_name_rejected() {
        let mut u = good();
        u.capabilities = vec![Cap::Custom {
            name: String::new(),
            params: BTreeMap::new(),
        }];
        let err = u.validate().unwrap_err();
        assert_eq!(err, PackageManifestError::CapabilityEmptyName { index: 0 });
    }

    #[test]
    fn plugin_field_propagates_through_validation() {
        use crate::package::plugin::{PluginKind, PluginManifest, PortKind};

        let mut u = good();
        u.plugin = Some(PluginManifest::new(
            PortKind::LlmBackend,
            PluginKind::RustCargo,
            "echo-llm".into(),
        ));
        let m = u.validate().unwrap();
        let plugin = m.plugin().expect("plugin should round-trip");
        assert_eq!(plugin.bin, "echo-llm");
        assert_eq!(plugin.provides, PortKind::LlmBackend);
        assert_eq!(plugin.kind, PluginKind::RustCargo);
    }

    #[test]
    fn plugin_absent_validates_as_none() {
        let m = good().validate().unwrap();
        assert!(m.plugin().is_none());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn skill_block_minimal_round_trips_through_toml() {
        let toml_src = r#"
name = "critic"
version = "0.1.0"
description = "Reviews drafts."
authors = []
source = "https://example.com/critic.git"
kind = "skill"
dependencies = []
capabilities = []

[skill]
"#;
        let u: UncheckedManifest = toml::from_str(toml_src).expect("parse");
        let skill = u.skill.as_ref().expect("skill present");
        // Defaults applied.
        assert_eq!(skill.content, "SKILL.md");
        assert!(skill.requires_tools.is_empty());
        assert!(skill.requires_skills.is_empty());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn skill_block_full_round_trips_through_toml() {
        let toml_src = r#"
name = "critic"
version = "0.1.0"
description = "Reviews drafts."
authors = []
source = "https://example.com/critic.git"
kind = "skill"
dependencies = []
capabilities = []

[skill]
content = "skills/critic.md"

[[skill.requires_tools]]
name = "fs-read"
version_req = "^0.1"

[[skill.requires_skills]]
name = "fact-checker"
version_req = "^0.1"
"#;
        let u: UncheckedManifest = toml::from_str(toml_src).expect("parse");
        let skill = u.skill.as_ref().expect("skill present");
        assert_eq!(skill.content, "skills/critic.md");
        assert_eq!(skill.requires_tools.len(), 1);
        assert_eq!(skill.requires_tools[0].name.as_str(), "fs-read");
        assert_eq!(skill.requires_skills.len(), 1);
        assert_eq!(skill.requires_skills[0].name.as_str(), "fact-checker");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn manifest_without_skill_block_parses_with_skill_none() {
        let toml_src = r#"
name = "regular-tool"
version = "0.1.0"
description = "A tool, not a skill."
authors = []
source = "https://example.com/tool.git"
kind = "tool"
dependencies = []
capabilities = []
"#;
        let u: UncheckedManifest = toml::from_str(toml_src).expect("parse");
        assert!(u.skill.is_none());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn manifest_with_skill_round_trips_through_validate() {
        // Validate() succeeds for skill packages with the [skill] block;
        // skill-vs-plugin cross-field validation is Skills-2's job, so
        // for now the validator accepts the block as-is.
        let toml_src = r#"
name = "critic"
version = "0.1.0"
description = "Reviews drafts."
authors = []
source = "https://example.com/critic.git"
kind = "skill"
dependencies = []
capabilities = []

[skill]
"#;
        let u: UncheckedManifest = toml::from_str(toml_src).expect("parse");
        let manifest = u.validate().expect("validate");
        assert!(manifest.skill().is_some());
        assert_eq!(manifest.skill().unwrap().content, "SKILL.md");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn skill_kind_with_plugin_block_is_rejected() {
        // Skills-2: cross-field validation — a package declaring kind = "skill"
        // must NOT also carry a [plugin] table.
        let toml_src = r#"
name = "critic"
version = "0.1.0"
description = "Reviews drafts."
authors = []
source = "https://example.com/critic.git"
kind = "skill"
dependencies = []
capabilities = []

[plugin]
provides = "tool"
kind = "rust-cargo"
bin = "critic"

[skill]
"#;
        let u: UncheckedManifest = toml::from_str(toml_src).expect("parse");
        let err = u.validate().unwrap_err();
        assert!(
            matches!(err, PackageManifestError::SkillCannotHavePluginBlock),
            "expected SkillCannotHavePluginBlock, got {err:?}"
        );
    }
}
