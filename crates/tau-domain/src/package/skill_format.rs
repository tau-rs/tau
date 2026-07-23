//! Skill-package format detection + manifest synthesis (Skills-5).
//!
//! Two responsibilities:
//!
//! 1. [`detect_format`] examines a directory and classifies it as
//!    [`SkillFormat::Tau`] (has `tau.toml`), [`SkillFormat::Anthropic`]
//!    (has `SKILL.md` but no `tau.toml`), or [`SkillFormat::Invalid`]
//!    (neither).
//!
//! 2. [`synthesize_manifest_from_skill_md`] takes a parsed
//!    [`SkillContent`] (from
//!    [`parse_skill_md`](crate::package::skill::parse_skill_md)) plus a source URL
//!    and produces a default [`PackageManifest`] equivalent to what
//!    a hand-written `tau.toml` would emit for an Anthropic skill.
//!
//! Pure logic — no I/O except the small directory peek in
//! [`detect_format`]. Used by `tau-pkg::synthesize` (the bridge
//! into the install pipeline) and by `tau-cli::cmd::skill::import`.

#[cfg(feature = "serde")]
use core::str::FromStr;

#[cfg(feature = "std")]
use std::path::Path;

use alloc::string::String;
#[cfg(feature = "serde")]
use alloc::string::ToString;

#[cfg(feature = "serde")]
use crate::id::PackageName;
#[cfg(feature = "serde")]
use crate::package::sandbox::PluginSandboxRequirements;
#[cfg(feature = "serde")]
use crate::package::skill::{SkillContent, SkillManifest};
#[cfg(feature = "serde")]
use crate::package::{kinds, PackageKind, PackageManifest, PackageSource, UncheckedManifest};
#[cfg(feature = "serde")]
use crate::version::Version;

/// Classification of a directory containing a skill package.
///
/// Returned by [`detect_format`]. Three variants cover all recognized and
/// unrecognized skill directory shapes.
///
/// # Example
///
/// ```
/// use tau_domain::SkillFormat;
///
/// // Variants are plain unit variants — construct them directly for
/// // comparison in router / dispatch logic.
/// let fmt = SkillFormat::Tau;
/// assert_eq!(fmt, SkillFormat::Tau);
/// assert_ne!(fmt, SkillFormat::Anthropic);
/// assert_ne!(fmt, SkillFormat::Invalid);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillFormat {
    /// Directory contains `tau.toml` — a tau-native package.
    Tau,
    /// Directory contains `SKILL.md` but no `tau.toml` — a vanilla
    /// Anthropic-format skill source.
    Anthropic,
    /// Directory contains neither file — not a recognized skill package.
    Invalid,
}

/// Classify `dir` by which manifest files it contains.
///
/// Checks two file names at the directory root:
/// - `tau.toml` → [`SkillFormat::Tau`]
/// - `SKILL.md` (only checked if no `tau.toml`) → [`SkillFormat::Anthropic`]
/// - neither → [`SkillFormat::Invalid`]
///
/// This is a peek, not a full validation. Both Skills-2's
/// `tau-pkg::install` and `tau skill import` re-read + validate
/// the file contents after this dispatch.
///
/// # Example
///
/// ```
/// use tau_domain::{detect_format, SkillFormat};
/// use std::fs;
///
/// let dir = tempfile::tempdir().expect("create tempdir");
/// fs::write(dir.path().join("tau.toml"), "name = \"x\"").expect("write");
/// assert_eq!(detect_format(dir.path()), SkillFormat::Tau);
///
/// let dir2 = tempfile::tempdir().expect("create tempdir");
/// // No recognized files → Invalid
/// assert_eq!(detect_format(dir2.path()), SkillFormat::Invalid);
/// ```
#[cfg(feature = "std")]
pub fn detect_format(dir: &Path) -> SkillFormat {
    if dir.join("tau.toml").is_file() {
        SkillFormat::Tau
    } else if dir.join("SKILL.md").is_file() {
        SkillFormat::Anthropic
    } else {
        SkillFormat::Invalid
    }
}

/// Synthesize a [`PackageManifest`] from a parsed SKILL.md
/// ([`SkillContent`]) plus a source URL.
///
/// Used when `tau install` auto-detects an Anthropic-format source
/// or when `tau skill import` produces a tau.toml on disk.
///
/// Defaults:
/// - `version`: `"0.1.0"` (Anthropic skills don't carry semver)
/// - `kind`: `"skill"`
/// - `capabilities`: empty (Anthropic skills declare none)
/// - `authors`: empty (Anthropic skills don't carry an authors field)
/// - `dependencies`: empty
///
/// Errors are propagated via the `PackageName` / `Version` parsers
/// (e.g. `Skills-1` rejects names containing `/` or whitespace).
#[cfg(feature = "serde")]
pub fn synthesize_manifest_from_skill_md(
    parsed: &SkillContent,
    source: PackageSource,
) -> Result<PackageManifest, SynthesizeError> {
    let name = PackageName::from_str(&parsed.frontmatter.name).map_err(|e| {
        SynthesizeError::InvalidName {
            name: parsed.frontmatter.name.clone(),
            detail: e.to_string(),
        }
    })?;
    let version = Version::parse("0.1.0").expect("0.1.0 is a valid semver");

    let skill = SkillManifest {
        content: "SKILL.md".to_string(),
        requires_tools: vec![],
        requires_skills: vec![],
    };

    let unchecked = UncheckedManifest {
        name,
        version,
        description: parsed.frontmatter.description.clone(),
        authors: vec![],
        license: None,
        source,
        kind: PackageKind::Custom {
            kind: kinds::SKILL.to_string(),
        },
        dependencies: vec![],
        capabilities: vec![],
        plugin: None,
        sandbox: PluginSandboxRequirements::default(),
        skill: Some(skill),
        vocab_version: crate::KNOWN_VOCAB,
    };

    unchecked
        .validate()
        .map_err(|e| SynthesizeError::ManifestBuild {
            detail: e.to_string(),
        })
}

/// Errors raised by [`synthesize_manifest_from_skill_md`].
///
/// # Example
///
/// ```
/// use tau_domain::SynthesizeError;
///
/// let err = SynthesizeError::InvalidName {
///     name: "bad name".into(),
///     detail: "contains a space".into(),
/// };
/// assert!(err.to_string().contains("bad name"));
/// ```
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum SynthesizeError {
    /// SKILL.md `frontmatter.name` is not a valid tau package name
    /// (e.g. contains `/`, whitespace, or invalid chars).
    #[error("invalid skill name {name:?}: {detail}")]
    InvalidName {
        /// The raw name string that failed validation.
        name: String,
        /// The underlying validation error message.
        detail: String,
    },

    /// `UncheckedManifest::validate` rejected the constructed manifest
    /// (would surprise: should not happen given valid inputs from
    /// `parse_skill_md`).
    #[error("manifest build failed: {detail}")]
    ManifestBuild {
        /// The underlying validation error message.
        detail: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn detect_format_returns_tau_when_tau_toml_present() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("tau.toml"), "name = \"x\"").unwrap();
        assert_eq!(detect_format(tmp.path()), SkillFormat::Tau);
    }

    #[test]
    fn detect_format_returns_anthropic_when_only_skill_md_present() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("SKILL.md"),
            "---\nname: critic\ndescription: x\n---\nbody\n",
        )
        .unwrap();
        assert_eq!(detect_format(tmp.path()), SkillFormat::Anthropic);
    }

    #[test]
    fn detect_format_returns_invalid_when_neither_present() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("other.md"), "x").unwrap();
        assert_eq!(detect_format(tmp.path()), SkillFormat::Invalid);
    }

    #[test]
    fn detect_format_tau_wins_when_both_present() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("tau.toml"), "name = \"x\"").unwrap();
        std::fs::write(
            tmp.path().join("SKILL.md"),
            "---\nname: x\ndescription: y\n---\n",
        )
        .unwrap();
        assert_eq!(detect_format(tmp.path()), SkillFormat::Tau);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn synthesize_produces_skill_kind_manifest_with_defaults() {
        use crate::package::skill::parse_skill_md;

        let parsed =
            parse_skill_md("---\nname: critic\ndescription: Reviews drafts.\n---\nBody.\n")
                .unwrap();
        let source = PackageSource::from_str("https://example.com/critic.git").unwrap();
        let manifest = synthesize_manifest_from_skill_md(&parsed, source).unwrap();

        assert_eq!(manifest.name().as_str(), "critic");
        assert_eq!(manifest.version().to_string(), "0.1.0");
        assert!(matches!(
            manifest.kind(),
            PackageKind::Custom { kind } if kind == kinds::SKILL
        ));
        assert!(manifest.capabilities().is_empty());
        assert!(manifest.authors().is_empty());
        assert!(manifest.dependencies().is_empty());
        assert_eq!(manifest.description(), "Reviews drafts.");

        let skill = manifest.skill().expect("skill block synthesized");
        assert_eq!(skill.content, "SKILL.md");
        assert!(skill.requires_tools.is_empty());
        assert!(skill.requires_skills.is_empty());
    }

    /// Defensive: if a SkillContent reaches synthesize with an empty
    /// `description` (bypassing parse_skill_md, which rejects it),
    /// the UncheckedManifest::validate step should surface a
    /// ManifestBuild error. Spec requires explicit coverage.
    #[cfg(feature = "serde")]
    #[test]
    fn synthesize_fails_on_empty_description() {
        use crate::package::skill::{SkillContent, SkillFrontmatter};

        let invalid = SkillContent {
            frontmatter: SkillFrontmatter {
                name: "critic".into(),
                description: String::new(),
            },
            body: "body".into(),
        };
        let source = PackageSource::from_str("https://example.com/critic.git").unwrap();
        let result = synthesize_manifest_from_skill_md(&invalid, source);
        assert!(
            matches!(result, Err(SynthesizeError::ManifestBuild { .. })),
            "expected ManifestBuild error, got {result:?}"
        );
    }
}
