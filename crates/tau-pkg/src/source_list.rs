//! Enumerate available versions at a [`PackageSource`].
//!
//! Used by the resolver (`crate::resolve`) to pick a concrete
//! `Version` for a `(name, source, version_req)` triple. For
//! `Git { rev: None }` we shell out to `git ls-remote --tags` and
//! filter the tag list to those parsing as `semver::Version` (after
//! stripping a leading `v` if present). For `Git { rev: Some(_) }`
//! the source is a single point in version space — we shallow-clone
//! the rev into a tempdir, read the manifest, and return that one
//! version.
//!
//! See `docs/superpowers/specs/2026-04-30-transitive-deps-design.md` §5.4.

use std::process::Command;

use semver::Version;
use tau_domain::{GitLocation, PackageSource};
use tempfile::TempDir;

use crate::manifest::read_manifest;

/// List the versions available at `source`.
pub fn list_versions_at_source(source: &PackageSource) -> Result<Vec<Version>, SourceListError> {
    match source {
        PackageSource::Git {
            location,
            rev: None,
        } => list_git_tags(location),
        PackageSource::Git {
            location,
            rev: Some(rev),
        } => single_version_at_rev(location, rev),
        // Unreachable today (Git is the only variant), but the enum is
        // `#[non_exhaustive]`, so the catch-all is required for forward
        // compatibility. Future variants land here as `Unsupported`
        // until the resolver explicitly handles them.
        _ => Err(SourceListError::Unsupported),
    }
}

fn list_git_tags(location: &GitLocation) -> Result<Vec<Version>, SourceListError> {
    let url = location.to_string();
    let output = Command::new("git")
        .arg("ls-remote")
        .arg("--tags")
        .arg(&url)
        .output()
        .map_err(|e| SourceListError::GitInvoke {
            message: format!("spawning `git ls-remote`: {e}"),
        })?;
    if !output.status.success() {
        return Err(SourceListError::GitLsRemote {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_ls_remote_tags(&stdout))
}

/// Parse `git ls-remote --tags` stdout into a sorted `Vec<Version>`.
///
/// Format: each line is `<sha>\trefs/tags/<tag>` (or `refs/tags/<tag>^{}`
/// for annotated-tag peels). We extract the tag, strip a leading `v`
/// if present, parse as `semver::Version`, drop non-semver tags. The
/// returned vec is sorted ascending; resolver picks the last entry
/// satisfying constraints (= the highest).
fn parse_ls_remote_tags(stdout: &str) -> Vec<Version> {
    let mut versions: Vec<Version> = stdout
        .lines()
        .filter_map(|line| {
            let tag = line.split('\t').nth(1)?;
            // Strip refs/tags/ prefix.
            let tag = tag.strip_prefix("refs/tags/")?;
            // Drop the ^{} peel suffix (annotated tag commit pointer).
            let tag = tag.strip_suffix("^{}").unwrap_or(tag);
            // Strip leading `v`.
            let tag = tag.strip_prefix('v').unwrap_or(tag);
            Version::parse(tag).ok()
        })
        .collect();
    versions.sort();
    versions.dedup();
    versions
}

fn single_version_at_rev(
    location: &GitLocation,
    rev: &str,
) -> Result<Vec<Version>, SourceListError> {
    let tempdir = TempDir::new().map_err(|e| SourceListError::TempDir {
        message: format!("creating tempdir for shallow clone: {e}"),
    })?;
    let dest = tempdir.path().join("repo");
    let url = location.to_string();
    let output = Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg("--branch")
        .arg(rev)
        .arg("--single-branch")
        .arg(&url)
        .arg(&dest)
        .output()
        .map_err(|e| SourceListError::GitInvoke {
            message: format!("spawning `git clone`: {e}"),
        })?;
    if !output.status.success() {
        return Err(SourceListError::GitClone {
            rev: rev.to_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    let manifest_path = dest.join("tau.toml");
    let manifest = read_manifest(&manifest_path).map_err(SourceListError::Manifest)?;
    Ok(vec![manifest.version().clone()])
}

/// Errors produced by [`list_versions_at_source`].
///
/// # Example
///
/// ```
/// use tau_pkg::source_list::SourceListError;
///
/// let err = SourceListError::GitInvoke {
///     message: "spawning `git ls-remote`: No such file or directory".to_string(),
/// };
/// let display = format!("{err}");
/// assert!(display.contains("git invocation failed"));
///
/// let err2 = SourceListError::Unsupported;
/// let display2 = format!("{err2}");
/// assert!(display2.contains("not supported"));
/// ```
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum SourceListError {
    /// `git` binary not invocable.
    #[error("git invocation failed: {message}")]
    GitInvoke {
        /// Human-readable failure context.
        message: String,
    },
    /// `git ls-remote` exited non-zero.
    #[error("git ls-remote failed: {stderr}")]
    GitLsRemote {
        /// Captured stderr.
        stderr: String,
    },
    /// `git clone` for a `rev`-pinned source failed.
    #[error("git clone of {rev:?} failed: {stderr}")]
    GitClone {
        /// The rev that was passed to `--branch`.
        rev: String,
        /// Captured stderr.
        stderr: String,
    },
    /// Could not create a tempdir for the shallow clone.
    #[error("tempdir creation failed: {message}")]
    TempDir {
        /// Human-readable failure context.
        message: String,
    },
    /// Reading the manifest at the cloned rev failed.
    #[error("manifest read failed: {0}")]
    Manifest(#[from] crate::error::ManifestReadError),
    /// `PackageSource` variant not supported by the resolver. Reserved
    /// for future variants — `Git` is the only variant at v0.1.
    #[error("source kind not supported by resolver")]
    Unsupported,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    /// Build a stdout fixture mimicking `git ls-remote --tags` output.
    /// Each line: `<40-char-sha>\trefs/tags/<tag>`.
    fn fake_ls_remote(tags: &[&str]) -> String {
        tags.iter()
            .map(|t| format!("0123456789abcdef0123456789abcdef01234567\trefs/tags/{t}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn parse_ls_remote_tags_returns_only_semver_parsable_tags() {
        let stdout = fake_ls_remote(&["v0.1.0", "v0.2.0", "release-2024", "main"]);
        let versions = parse_ls_remote_tags(&stdout);
        assert_eq!(
            versions,
            vec![
                Version::parse("0.1.0").unwrap(),
                Version::parse("0.2.0").unwrap(),
            ]
        );
    }

    #[test]
    fn parse_ls_remote_tags_strips_v_prefix() {
        let stdout = fake_ls_remote(&["v1.2.3", "0.4.5"]);
        let versions = parse_ls_remote_tags(&stdout);
        assert_eq!(
            versions,
            vec![
                Version::parse("0.4.5").unwrap(),
                Version::parse("1.2.3").unwrap(),
            ]
        );
    }

    #[test]
    fn parse_ls_remote_tags_drops_annotated_tag_peels() {
        // Annotated tags appear twice: once as `<tag>` and once as `<tag>^{}`.
        // Both should resolve to the same Version, deduped.
        let stdout = fake_ls_remote(&["v1.0.0", "v1.0.0^{}"]);
        let versions = parse_ls_remote_tags(&stdout);
        assert_eq!(versions, vec![Version::parse("1.0.0").unwrap()]);
    }

    #[test]
    fn parse_ls_remote_tags_returns_empty_for_no_semver_tags() {
        let stdout = fake_ls_remote(&["release-1", "rc-foo", "untagged"]);
        let versions = parse_ls_remote_tags(&stdout);
        assert!(versions.is_empty());
    }

    #[test]
    fn parse_ls_remote_tags_returns_sorted_ascending() {
        let stdout = fake_ls_remote(&["v2.0.0", "v0.5.0", "v1.0.0"]);
        let versions = parse_ls_remote_tags(&stdout);
        assert_eq!(
            versions,
            vec![
                Version::parse("0.5.0").unwrap(),
                Version::parse("1.0.0").unwrap(),
                Version::parse("2.0.0").unwrap(),
            ]
        );
    }

    /// Set up a local git repository in a tempdir, with a tau.toml
    /// declaring `name = "test-tool"` and `version = "<version>"`,
    /// then commit + tag `v<version>`. Returns the tempdir guard +
    /// the GitLocation of the repo.
    fn make_local_git_fixture(version: &str) -> (TempDir, GitLocation) {
        let tempdir = TempDir::new().unwrap();
        let repo = tempdir.path().join("test-tool");
        std::fs::create_dir(&repo).unwrap();
        let manifest_body = format!(
            r#"
name = "test-tool"
version = "{version}"
description = "fixture"
authors = []
source = "https://example.com/test.git"
kind = "tool"
dependencies = []
capabilities = []
"#
        );
        std::fs::write(repo.join("tau.toml"), manifest_body).unwrap();

        let run = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(&repo)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "fixture"]);
        run(&["tag", &format!("v{version}")]);

        let url = format!("file://{}", repo.display());
        let location = GitLocation::from_str(&url).unwrap();
        (tempdir, location)
    }

    #[test]
    fn list_git_tags_against_local_file_url_finds_tag() {
        let (_tempdir, location) = make_local_git_fixture("0.1.0");
        let versions = list_git_tags(&location).unwrap();
        assert_eq!(versions, vec![Version::parse("0.1.0").unwrap()]);
    }

    #[test]
    fn single_version_at_rev_clones_and_reads_manifest() {
        let (_tempdir, location) = make_local_git_fixture("0.3.5");
        let versions = single_version_at_rev(&location, "v0.3.5").unwrap();
        assert_eq!(versions, vec![Version::parse("0.3.5").unwrap()]);
    }
}
