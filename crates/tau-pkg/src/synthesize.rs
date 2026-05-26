//! Bridge between tau-domain's pure synthesis logic and tau-pkg's
//! install pipeline.
//!
//! Skills-5. Reads SKILL.md from a cloned workspace, parses it, hands
//! off to tau-domain's `synthesize_manifest_from_skill_md`, and
//! returns a [`PackageManifest`] ready for the rest of the install
//! pipeline to consume.

use std::path::Path;

use tau_domain::{
    parse_skill_md, synthesize_manifest_from_skill_md, PackageManifest, PackageSource,
};

/// Read SKILL.md from `workspace`, parse, synthesize a manifest.
///
/// Called by [`crate::install::install_with_options`] when
/// [`tau_domain::detect_format`] classifies `workspace` as
/// [`tau_domain::SkillFormat::Anthropic`].
///
/// `source` is the original install URL — propagated into the
/// synthesized manifest's `source` field for the lockfile.
pub fn synthesize_anthropic_skill(
    workspace: &Path,
    source: PackageSource,
) -> Result<PackageManifest, SynthesizeError> {
    let skill_md_path = workspace.join("SKILL.md");
    let text =
        std::fs::read_to_string(&skill_md_path).map_err(|e| SynthesizeError::ReadSkillMd {
            path: skill_md_path.display().to_string(),
            detail: e.to_string(),
        })?;
    let parsed = parse_skill_md(&text).map_err(|e| SynthesizeError::ParseSkillMd {
        path: skill_md_path.display().to_string(),
        detail: e.to_string(),
    })?;
    synthesize_manifest_from_skill_md(&parsed, source).map_err(|e| {
        SynthesizeError::DomainSynthesize {
            detail: e.to_string(),
        }
    })
}

/// Errors raised by [`synthesize_anthropic_skill`].
///
/// All variants store human-readable strings (not raw I/O errors) so
/// the enum can derive `Clone + PartialEq + Eq` and compose cleanly
/// with [`crate::error::InstallError`]'s `#[from]` adapter.
///
/// # Example
///
/// ```
/// use tau_pkg::synthesize::SynthesizeError;
///
/// let err = SynthesizeError::ReadSkillMd {
///     path: "/workspace/SKILL.md".to_string(),
///     detail: "No such file or directory".to_string(),
/// };
/// let display = format!("{err}");
/// assert!(display.contains("SKILL.md"));
/// assert!(display.contains("/workspace/SKILL.md"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SynthesizeError {
    /// Failed to read SKILL.md from disk.
    #[error("reading SKILL.md at {path}: {detail}")]
    ReadSkillMd {
        /// Display path of the SKILL.md file that could not be read.
        path: String,
        /// Human-readable I/O error detail.
        detail: String,
    },
    /// SKILL.md parse failed (missing required frontmatter field, etc.).
    #[error("parsing SKILL.md at {path}: {detail}")]
    ParseSkillMd {
        /// Display path of the SKILL.md file that failed to parse.
        path: String,
        /// Human-readable parse error detail.
        detail: String,
    },
    /// tau-domain's `synthesize_manifest_from_skill_md` returned an
    /// error (e.g. invalid package name in frontmatter).
    #[error("synthesizing manifest: {detail}")]
    DomainSynthesize {
        /// Human-readable synthesis error detail.
        detail: String,
    },
}
