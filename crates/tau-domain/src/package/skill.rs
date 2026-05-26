//! Skill-specific manifest block + SKILL.md parsing.
//!
//! ROADMAP §16 (Skills as first-class packages, Constitution G10).
//! v1 design ratified in
//! `docs/superpowers/specs/2026-05-12-skills-1-manifest-design.md`.
//!
//! A tau skill package is a directory containing both an
//! Anthropic-format `SKILL.md` (content) and a tau-format `tau.toml`
//! (packaging). This module owns:
//!
//! - [`SkillManifest`] — the typed `[skill]` block parsed from
//!   `tau.toml`.
//! - [`SkillFrontmatter`] + [`SkillContent`] — the parsed YAML
//!   frontmatter + Markdown body of `SKILL.md`.
//! - [`parse_skill_md`] — the frontmatter splitter + YAML parser.
//! - [`SKILL_DIR_VAR`] — the public string constant for the
//!   `${SKILL_DIR}` interpolation variable. Substitution itself
//!   is Skills-4's responsibility (runtime invocation); Skills-1
//!   just establishes the variable as a recognized symbolic form.
//!
//! See `docs/decisions/0025-skills-foundation.md` for the ADR.

use crate::package::manifest::PackageDep;

/// Public string constant for the `${SKILL_DIR}` interpolation
/// variable that resolves at runtime to the absolute path of the
/// installed skill's directory.
///
/// Parallel to the conventional `${SCOPE}` and `${PROJECT}` variables
/// (also symbolic in v1; substitution lives outside `tau-domain`).
///
/// Used in capability `paths` entries and `SKILL.md` body references.
/// Validated for syntactic recognition at install time (Skills-2);
/// substituted at spawn time (Skills-4).
pub const SKILL_DIR_VAR: &str = "${SKILL_DIR}";

/// Skill-specific manifest block (parsed from `[skill]` in `tau.toml`).
///
/// `content` defaults to `"SKILL.md"` (the canonical Anthropic skill
/// content filename); `requires_tools` and `requires_skills` default
/// to empty lists.
///
/// `SkillManifest` is `#[non_exhaustive]` — construction outside the crate
/// requires the `serde` feature (TOML deserialization). The canonical path
/// to a `SkillManifest` is through `UncheckedManifest::skill` after parsing.
///
/// # Example
///
/// ```no_run
/// use tau_domain::{UncheckedManifest, SkillManifest};
/// // Obtained by deserializing a tau.toml with a [skill] block:
/// // let u: UncheckedManifest = toml::from_str(toml_str)?;
/// // let skill: &SkillManifest = u.skill.as_ref().unwrap();
/// # let _ = std::any::type_name::<SkillManifest>();
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SkillManifest {
    /// Path to the SKILL.md content file, relative to the package
    /// root. Defaults to `"SKILL.md"`.
    #[cfg_attr(feature = "serde", serde(default = "default_skill_content"))]
    pub content: String,

    /// Tool dependencies (same shape as the top-level
    /// `[[requires.tools]]`).
    #[cfg_attr(feature = "serde", serde(default))]
    pub requires_tools: Vec<PackageDep>,

    /// Sub-skill dependencies. Resolved at install time
    /// (Skills-2); the runtime side wires through `agent.<kind>.spawn`
    /// in Skills-4.
    #[cfg_attr(feature = "serde", serde(default))]
    pub requires_skills: Vec<PackageDep>,
}

#[cfg(feature = "serde")]
fn default_skill_content() -> String {
    "SKILL.md".to_string()
}

/// Parsed YAML frontmatter from a `SKILL.md` file.
///
/// Both `name` and `description` are required by the Anthropic skill
/// format. Other frontmatter fields are tolerated and discarded
/// in v1 — Skills-5 may surface them if Agent Skills spec compliance
/// requires it.
///
/// `SkillFrontmatter` is `#[non_exhaustive]` — struct-literal construction
/// is blocked across crate boundaries. Instances come from
/// [`parse_skill_md`] (requires the `serde` feature).
///
/// # Example
///
/// ```no_run
/// use tau_domain::{SkillFrontmatter, SkillContent};
/// // Obtained by calling parse_skill_md (requires serde feature):
/// // let content: SkillContent = tau_domain::parse_skill_md(raw_md)?;
/// // assert_eq!(content.frontmatter.name, "critic");
/// # let _ = std::any::type_name::<SkillFrontmatter>();
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SkillFrontmatter {
    /// The skill's canonical name. Must equal the `name` field of
    /// the containing package's `tau.toml`; mismatch is rejected at
    /// install time (Skills-2).
    pub name: String,

    /// Short human-readable description.
    pub description: String,
}

/// Parsed `SKILL.md` content: frontmatter + body.
///
/// `body` is the verbatim text between the closing `---` frontmatter
/// delimiter and end of file. Becomes the spawned child agent's
/// `system_prompt` at runtime (Skills-4).
///
/// `SkillContent` is `#[non_exhaustive]` — struct-literal construction
/// is blocked across crate boundaries. Instances come from
/// [`parse_skill_md`] (requires the `serde` feature).
///
/// # Example
///
/// ```no_run
/// use tau_domain::SkillContent;
/// // Obtained by calling parse_skill_md (requires serde feature):
/// // let content: SkillContent = tau_domain::parse_skill_md(raw_md)?;
/// // assert_eq!(&content.body, "\nYou are a strict editor.\n");
/// # let _ = std::any::type_name::<SkillContent>();
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SkillContent {
    /// The frontmatter (parsed from the YAML block).
    pub frontmatter: SkillFrontmatter,
    /// The Markdown body (becomes the spawned child's
    /// `system_prompt`).
    pub body: String,
}

/// Errors raised by [`parse_skill_md`].
///
/// # Example
///
/// ```
/// use tau_domain::SkillContentError;
///
/// // MissingFrontmatterOpener is returned when the file doesn't begin with `---`.
/// let err = SkillContentError::MissingFrontmatterOpener;
/// assert!(err.to_string().contains("---"));
///
/// // MissingName is returned when the YAML block has no `name` field.
/// let err2 = SkillContentError::MissingName;
/// assert!(err2.to_string().contains("name"));
/// ```
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillContentError {
    /// File did not begin with a `---` frontmatter opener (on its
    /// own line, optionally with surrounding whitespace).
    #[error("missing leading `---` frontmatter delimiter")]
    MissingFrontmatterOpener,

    /// File began with `---` but no closing `---` was found.
    #[error("missing closing `---` frontmatter delimiter")]
    MissingFrontmatterCloser,

    /// YAML parse failure inside the frontmatter block.
    #[error("frontmatter YAML parse error: {0}")]
    YamlParse(String),

    /// Frontmatter parsed but is missing the required `name` field.
    #[error("frontmatter missing required field `name`")]
    MissingName,

    /// Frontmatter parsed but is missing the required `description`
    /// field.
    #[error("frontmatter missing required field `description`")]
    MissingDescription,
}

/// Parse a `SKILL.md` file's text into [`SkillContent`].
///
/// Format:
/// ```text
/// ---
/// name: foo
/// description: ...
/// ---
///
/// Markdown body...
/// ```
///
/// The body is everything after the closing `---` line.
#[cfg(feature = "serde")]
pub fn parse_skill_md(input: &str) -> Result<SkillContent, SkillContentError> {
    // Helper: strip an optional trailing `\r` from a line slice.
    fn trim_cr(s: &str) -> &str {
        s.strip_suffix('\r').unwrap_or(s)
    }

    let mut lines = input.split_inclusive('\n');

    // First line must be `---` (with optional whitespace + CR).
    let first = lines
        .next()
        .ok_or(SkillContentError::MissingFrontmatterOpener)?;
    let first_stripped = first.strip_suffix('\n').unwrap_or(first);
    let first_stripped = trim_cr(first_stripped);
    if first_stripped.trim() != "---" {
        return Err(SkillContentError::MissingFrontmatterOpener);
    }

    // Collect lines until the closing `---`.
    let mut yaml_buf = String::new();
    let mut closer_found = false;
    let mut consumed = first.len();
    for line in lines.by_ref() {
        consumed += line.len();
        let line_stripped = line.strip_suffix('\n').unwrap_or(line);
        let line_stripped = trim_cr(line_stripped);
        if line_stripped.trim() == "---" {
            closer_found = true;
            break;
        }
        yaml_buf.push_str(line);
    }
    if !closer_found {
        return Err(SkillContentError::MissingFrontmatterCloser);
    }

    // Parse YAML body into a generic map first so we can produce
    // field-specific errors before serde_yaml's generic missing-field
    // message.
    let map: serde_yaml::Mapping =
        serde_yaml::from_str(&yaml_buf).map_err(|e| SkillContentError::YamlParse(e.to_string()))?;

    let name = map
        .get(serde_yaml::Value::String("name".into()))
        .and_then(|v| v.as_str().map(String::from))
        .ok_or(SkillContentError::MissingName)?;
    let description = map
        .get(serde_yaml::Value::String("description".into()))
        .and_then(|v| v.as_str().map(String::from))
        .ok_or(SkillContentError::MissingDescription)?;

    let frontmatter = SkillFrontmatter { name, description };

    // Body is everything after the closing `---` line.
    let body = input[consumed..].to_string();

    Ok(SkillContent { frontmatter, body })
}

#[cfg(all(test, feature = "serde"))]
mod parse_tests {
    use super::*;

    #[test]
    fn parses_valid_skill_md() {
        let input =
            "---\nname: critic\ndescription: Reviews drafts.\n---\n\nYou are a strict editor.\n";
        let parsed = parse_skill_md(input).unwrap();
        assert_eq!(parsed.frontmatter.name, "critic");
        assert_eq!(parsed.frontmatter.description, "Reviews drafts.");
        assert_eq!(parsed.body, "\nYou are a strict editor.\n");
    }

    #[test]
    fn rejects_missing_opener() {
        let input = "no frontmatter here\nname: critic\n";
        assert_eq!(
            parse_skill_md(input).unwrap_err(),
            SkillContentError::MissingFrontmatterOpener
        );
    }

    #[test]
    fn rejects_missing_closer() {
        let input = "---\nname: critic\ndescription: stuck\n";
        assert_eq!(
            parse_skill_md(input).unwrap_err(),
            SkillContentError::MissingFrontmatterCloser
        );
    }

    #[test]
    fn rejects_malformed_yaml() {
        let input = "---\nname: critic\ndescription: : :\n  - bad indent\n---\nbody\n";
        let err = parse_skill_md(input).unwrap_err();
        assert!(
            matches!(err, SkillContentError::YamlParse(_)),
            "expected YamlParse, got {err:?}"
        );
    }

    #[test]
    fn rejects_missing_name() {
        let input = "---\ndescription: Reviews drafts.\n---\nbody\n";
        assert_eq!(
            parse_skill_md(input).unwrap_err(),
            SkillContentError::MissingName
        );
    }

    #[test]
    fn rejects_missing_description() {
        let input = "---\nname: critic\n---\nbody\n";
        assert_eq!(
            parse_skill_md(input).unwrap_err(),
            SkillContentError::MissingDescription
        );
    }

    #[test]
    fn tolerates_extra_frontmatter_fields() {
        // Future-compat: extra YAML keys are accepted and ignored.
        let input = "---\nname: critic\ndescription: x\ntags: [editing]\nversion: 0.1\n---\nbody\n";
        let parsed = parse_skill_md(input).unwrap();
        assert_eq!(parsed.frontmatter.name, "critic");
    }

    #[test]
    fn tolerates_crlf_line_endings() {
        let input = "---\r\nname: critic\r\ndescription: x\r\n---\r\nbody\r\n";
        let parsed = parse_skill_md(input).unwrap();
        assert_eq!(parsed.frontmatter.name, "critic");
    }
}

#[cfg(all(test, feature = "serde"))]
mod skill_dir_var_tests {
    use super::*;
    use crate::package::manifest::UncheckedManifest;

    #[test]
    fn skill_dir_var_constant_is_the_canonical_string() {
        assert_eq!(SKILL_DIR_VAR, "${SKILL_DIR}");
    }

    #[test]
    fn skill_dir_var_in_capability_path_round_trips_verbatim() {
        // Skills-1 is purely symbolic — the ${SKILL_DIR} token survives
        // serde round-trip without expansion. Substitution is Skills-4.
        let toml_src = r#"
name = "critic"
version = "0.1.0"
description = "Reviews drafts."
authors = []
source = "https://example.com/critic.git"
kind = "skill"
dependencies = []

[[capabilities]]
kind = "fs.read"
paths = ["${SKILL_DIR}/references/**", "${SKILL_DIR}/templates/**"]

[skill]
"#;
        let u: UncheckedManifest = toml::from_str(toml_src).expect("parse");
        let cap = &u.capabilities[0];
        match cap {
            crate::package::capability::Capability::Filesystem(
                crate::package::capability::FsCapability::Read { paths },
            ) => {
                assert_eq!(paths.len(), 2);
                assert!(paths[0].contains(SKILL_DIR_VAR));
                assert!(paths[1].contains(SKILL_DIR_VAR));
                // Verbatim — no expansion happened.
                assert_eq!(paths[0], "${SKILL_DIR}/references/**");
            }
            other => panic!("expected fs.read, got {other:?}"),
        }
    }
}
