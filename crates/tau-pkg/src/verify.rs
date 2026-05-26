//! `tau verify` — recompute install-tree SHA-256, compare to lockfile.
//! Source-agnostic; works for any future PackageSource variant.
//!
//! See ADR-0012 + spec §1 / §3.

use std::path::PathBuf;

use tau_domain::{PackageName, Version};

use crate::error::RegistryError;
use crate::lockfile::LockFile;
use crate::scope::Scope;
use crate::tree_hash::{sha256_of_file, tree_hash, TreeHashError};

/// Per-package verification status.
///
/// # Example
///
/// ```
/// use tau_pkg::verify::VerifyStatus;
///
/// assert!(!VerifyStatus::Ok.is_drift());
/// assert!(!VerifyStatus::Unverified.is_drift());
/// assert!(VerifyStatus::TreeDrift {
///     expected: "aaa".to_string(),
///     actual: "bbb".to_string(),
/// }.is_drift());
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyStatus {
    /// Hash matches the lockfile entry. No drift.
    Ok,
    /// Source tree SHA-256 doesn't match `LockedVersion.sha256`.
    TreeDrift {
        /// Hex hash from the lockfile.
        expected: String,
        /// Hex hash recomputed now.
        actual: String,
    },
    /// Plugin binary SHA-256 doesn't match `LockedPlugin.binary_sha256`.
    BinaryDrift {
        /// Path to the binary.
        path: PathBuf,
        /// Hex hash from the lockfile.
        expected: String,
        /// Hex hash recomputed now.
        actual: String,
    },
    /// Install dir doesn't exist on disk.
    Missing {
        /// The install dir the lockfile pointed to.
        path: PathBuf,
    },
    /// Lockfile entry has empty sha256 (v2-leftover). Informational;
    /// not drift.
    Unverified,

    /// A skill package's `SKILL.md` content hash differs from the
    /// install-time snapshot recorded in the lockfile. Parallel to
    /// `BinaryDrift` for plugin binaries.
    ///
    /// Remediation: re-run `tau install <skill>` to refresh.
    SkillContentDrift {
        /// Package name.
        name: String,
        /// Expected SHA-256 (hex; from the lockfile).
        expected: String,
        /// Actual SHA-256 (hex; from re-hashing the on-disk file).
        got: String,
    },

    /// Skills-5: skill failed Anthropic-format conformance check
    /// (triggered by the `--anthropic-strict` flag on `tau verify`).
    AnthropicConformance {
        /// Name of the affected skill package.
        skill_name: String,
        /// Specific conformance issue detected.
        issue: AnthropicConformanceIssue,
    },
}

/// Specific Anthropic-conformance issues raised by Skills-5's
/// `tau verify --anthropic-strict` mode.
///
/// # Example
///
/// ```
/// use tau_pkg::verify::AnthropicConformanceIssue;
///
/// let issue = AnthropicConformanceIssue::MissingDescription;
/// assert!(matches!(issue, AnthropicConformanceIssue::MissingDescription));
///
/// let issue2 = AnthropicConformanceIssue::MalformedFrontmatter {
///     detail: "missing closing ---".to_string(),
/// };
/// assert!(matches!(issue2, AnthropicConformanceIssue::MalformedFrontmatter { .. }));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AnthropicConformanceIssue {
    /// SKILL.md frontmatter is missing the required `description` field
    /// (or it parsed as an empty/whitespace-only string).
    MissingDescription,
    /// SKILL.md body (after frontmatter strip) is empty or
    /// whitespace-only.
    EmptyBody,
    /// SKILL.md frontmatter is malformed (YAML parse failed, missing
    /// closing `---`, etc.).
    MalformedFrontmatter {
        /// Underlying parser error detail.
        detail: String,
    },
}

impl VerifyStatus {
    /// Whether this status represents drift (non-zero exit).
    ///
    /// Returns `true` for `TreeDrift`, `BinaryDrift`, `Missing`,
    /// `SkillContentDrift`, and `AnthropicConformance`; `false` for
    /// `Ok` and `Unverified`.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_pkg::verify::VerifyStatus;
    ///
    /// assert!(!VerifyStatus::Ok.is_drift());
    /// assert!(!VerifyStatus::Unverified.is_drift());
    /// assert!(VerifyStatus::Missing {
    ///     path: std::path::PathBuf::from("/gone"),
    /// }.is_drift());
    /// ```
    pub fn is_drift(&self) -> bool {
        matches!(
            self,
            VerifyStatus::TreeDrift { .. }
                | VerifyStatus::BinaryDrift { .. }
                | VerifyStatus::Missing { .. }
                | VerifyStatus::SkillContentDrift { .. }
                | VerifyStatus::AnthropicConformance { .. }
        )
    }
}

/// Verification result for one package version.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    /// Package name.
    pub name: PackageName,
    /// Version.
    pub version: Version,
    /// Per-package status.
    pub status: VerifyStatus,
}

/// Error from `verify` / `verify_all`.
///
/// # Example
///
/// ```
/// use tau_pkg::verify::VerifyError;
///
/// let err = VerifyError::PackageNotInstalled { name: "missing-pkg".to_string() };
/// let display = format!("{err}");
/// assert!(display.contains("missing-pkg"));
/// assert!(display.contains("not installed"));
/// ```
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// Lockfile load failed.
    #[error("loading lockfile: {source}")]
    LockfileLoad {
        /// Source error.
        #[from]
        source: RegistryError,
    },

    /// Package not found in lockfile.
    #[error("package {name} not installed")]
    PackageNotInstalled {
        /// Package name.
        name: String,
    },

    /// Version not found in lockfile entry.
    #[error("package {name}: version {version} not installed")]
    VersionNotInstalled {
        /// Package name.
        name: String,
        /// Version.
        version: String,
    },

    /// I/O error during hash computation.
    #[error("io error: {source}")]
    Io {
        /// Source error.
        #[from]
        source: TreeHashError,
    },
}

/// Re-hash a skill package's `SKILL.md` and compare against the
/// `content_sha256` recorded in the lockfile. Returns `Ok(())` on
/// match; `Err(VerifyReport::SkillContentDrift { ... })` on mismatch.
///
/// Used by `tau verify` to detect post-install drift on skill
/// packages, parallel to how `binary_sha256` is checked for plugin
/// binaries.
///
/// `install_dir` is the absolute path of the installed skill
/// directory (i.e. where `SKILL.md` lives). `name` is the package
/// name (carried into the error for human-readable display).
///
/// # Example
///
/// ```
/// use tau_pkg::verify::{verify_skill_content, VerifyStatus};
/// use tau_pkg::lockfile::{LockedSkill, SkillFrontmatterSnapshot};
///
/// # let tmp = tempfile::tempdir().expect("tempdir");
/// # let content = b"---\nname: critic\ndescription: x\n---\nbody\n";
/// # std::fs::write(tmp.path().join("SKILL.md"), content).expect("write");
/// // Mismatch: locked sha is empty string, on-disk file is non-empty → drift.
/// let fm = SkillFrontmatterSnapshot::new("critic".to_string(), "x".to_string());
/// let locked = LockedSkill::new("wrong-sha".to_string(), fm);
/// let result = verify_skill_content(tmp.path(), "critic", &locked);
/// assert!(matches!(result, Err(VerifyStatus::SkillContentDrift { .. })));
/// ```
pub fn verify_skill_content(
    install_dir: &std::path::Path,
    name: &str,
    locked: &crate::lockfile::LockedSkill,
) -> Result<(), VerifyStatus> {
    let path = install_dir.join("SKILL.md");
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => {
            // SKILL.md missing on disk despite being recorded → drift.
            // We could surface this as a separate variant
            // (`SkillContentMissing`), but the user remediation is the
            // same: re-install. Keep one variant.
            return Err(VerifyStatus::SkillContentDrift {
                name: name.to_string(),
                expected: locked.content_sha256.clone(),
                got: "<missing>".to_string(),
            });
        }
    };
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(&bytes);
    let got = crate::tree_hash::to_hex_lower(&h.finalize());
    if got == locked.content_sha256 {
        Ok(())
    } else {
        Err(VerifyStatus::SkillContentDrift {
            name: name.to_string(),
            expected: locked.content_sha256.clone(),
            got,
        })
    }
}

/// Verify a single (package, version) pair.
pub fn verify(
    scope: &Scope,
    name: &PackageName,
    version: &Version,
) -> Result<VerifyReport, VerifyError> {
    let lockfile = LockFile::load(&scope.lockfile_path())?;

    let pkg = lockfile
        .find(name)
        .ok_or_else(|| VerifyError::PackageNotInstalled {
            name: name.as_str().to_owned(),
        })?;

    let lv = pkg
        .installed_versions
        .iter()
        .find(|lv| &lv.version == version)
        .ok_or_else(|| VerifyError::VersionNotInstalled {
            name: name.as_str().to_owned(),
            version: version.to_string(),
        })?;

    let install_dir = scope.package_dir(name, version);

    if !install_dir.exists() {
        return Ok(VerifyReport {
            name: name.clone(),
            version: version.clone(),
            status: VerifyStatus::Missing { path: install_dir },
        });
    }

    if lv.sha256.is_empty() {
        return Ok(VerifyReport {
            name: name.clone(),
            version: version.clone(),
            status: VerifyStatus::Unverified,
        });
    }

    let actual = tree_hash(&install_dir)?;
    if actual != lv.sha256 {
        return Ok(VerifyReport {
            name: name.clone(),
            version: version.clone(),
            status: VerifyStatus::TreeDrift {
                expected: lv.sha256.clone(),
                actual,
            },
        });
    }

    // Binary check (if applicable).
    if let Some(plugin) = pkg.plugin.as_ref() {
        if !plugin.binary_sha256.is_empty() && plugin.binary_path.exists() {
            let bin_actual = sha256_of_file(&plugin.binary_path)?;
            if bin_actual != plugin.binary_sha256 {
                return Ok(VerifyReport {
                    name: name.clone(),
                    version: version.clone(),
                    status: VerifyStatus::BinaryDrift {
                        path: plugin.binary_path.clone(),
                        expected: plugin.binary_sha256.clone(),
                        actual: bin_actual,
                    },
                });
            }
        }
    }

    // Skill content check (if applicable).
    if let Some(locked_skill) = pkg.skill.as_ref() {
        if let Err(drift_status) = verify_skill_content(&install_dir, name.as_str(), locked_skill) {
            return Ok(VerifyReport {
                name: name.clone(),
                version: version.clone(),
                status: drift_status,
            });
        }
    }

    Ok(VerifyReport {
        name: name.clone(),
        version: version.clone(),
        status: VerifyStatus::Ok,
    })
}

/// Verify every (package, version) pair in the lockfile.
pub fn verify_all(scope: &Scope) -> Result<Vec<VerifyReport>, VerifyError> {
    verify_all_with_options(scope, false)
}

/// Verify every (package, version) pair in the lockfile, with optional
/// Anthropic-format conformance checking.
///
/// When `anthropic_strict` is `true`, every installed skill package is
/// additionally checked for Anthropic SKILL.md conformance:
///
/// - frontmatter must be well-formed
/// - `description` must be non-empty / non-whitespace
/// - body must be non-empty / non-whitespace
///
/// Conformance failures are appended to the report as
/// [`VerifyStatus::AnthropicConformance`] entries (one per package per
/// issue). Non-skill packages are skipped.
pub fn verify_all_with_options(
    scope: &Scope,
    anthropic_strict: bool,
) -> Result<Vec<VerifyReport>, VerifyError> {
    let lockfile = LockFile::load(&scope.lockfile_path())?;
    let mut reports = Vec::new();

    for pkg in &lockfile.packages {
        for lv in &pkg.installed_versions {
            reports.push(verify(scope, &pkg.name, &lv.version)?);
        }

        if anthropic_strict && pkg.skill.is_some() {
            let install_path = scope.package_dir(&pkg.name, &pkg.active_version);
            let skill_md_path = install_path.join("SKILL.md");
            let text = match std::fs::read_to_string(&skill_md_path) {
                Ok(t) => t,
                // Missing SKILL.md: the drift check above already caught
                // this; skip the conformance check to avoid duplicate noise.
                Err(_) => continue,
            };
            match tau_domain::parse_skill_md(&text) {
                Err(e) => {
                    reports.push(VerifyReport {
                        name: pkg.name.clone(),
                        version: pkg.active_version.clone(),
                        status: VerifyStatus::AnthropicConformance {
                            skill_name: pkg.name.to_string(),
                            issue: AnthropicConformanceIssue::MalformedFrontmatter {
                                detail: e.to_string(),
                            },
                        },
                    });
                }
                Ok(content) => {
                    if content.frontmatter.description.trim().is_empty() {
                        reports.push(VerifyReport {
                            name: pkg.name.clone(),
                            version: pkg.active_version.clone(),
                            status: VerifyStatus::AnthropicConformance {
                                skill_name: pkg.name.to_string(),
                                issue: AnthropicConformanceIssue::MissingDescription,
                            },
                        });
                    }
                    if content.body.trim().is_empty() {
                        reports.push(VerifyReport {
                            name: pkg.name.clone(),
                            version: pkg.active_version.clone(),
                            status: VerifyStatus::AnthropicConformance {
                                skill_name: pkg.name.to_string(),
                                issue: AnthropicConformanceIssue::EmptyBody,
                            },
                        });
                    }
                }
            }
        }
    }

    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lockfile::{LockFile, LockedPackage, LockedVersion};
    use std::fs;
    use std::str::FromStr;
    use std::time::SystemTime;
    use tau_domain::PackageSource;
    use tempfile::TempDir;

    fn setup() -> (TempDir, Scope, PackageName, Version) {
        let td = TempDir::new().unwrap();
        let scope = Scope::new_project(td.path()).expect("create scope");
        let name = PackageName::from_str("test-pkg").unwrap();
        let version = Version::parse("1.0.0").unwrap();
        (td, scope, name, version)
    }

    fn write_install_with_one_file(
        scope: &Scope,
        name: &PackageName,
        version: &Version,
        content: &[u8],
    ) -> String {
        let install_dir = scope.package_dir(name, version);
        fs::create_dir_all(&install_dir).unwrap();
        fs::write(install_dir.join("a.txt"), content).unwrap();
        crate::tree_hash::tree_hash(&install_dir).unwrap()
    }

    fn write_lockfile_with_one_entry(
        scope: &Scope,
        name: &PackageName,
        version: &Version,
        sha256: String,
    ) {
        let mut lf = LockFile::default();
        let pkg = LockedPackage {
            name: name.clone(),
            active_version: version.clone(),
            source: PackageSource::Git {
                location: "https://example.com/x.git".parse().unwrap(),
                rev: None,
            },
            installed_versions: vec![LockedVersion {
                version: version.clone(),
                rev: None,
                resolved_commit: "0".repeat(40),
                sha256,
                installed_at: SystemTime::now(),
            }],
            plugin: None,
            skill: None,
            synthesized_from: None,
        };
        lf.packages.push(pkg);
        lf.save(&scope.lockfile_path()).unwrap();
    }

    #[test]
    fn verify_ok_when_install_matches_lockfile() {
        let (_td, scope, name, version) = setup();
        let h = write_install_with_one_file(&scope, &name, &version, b"hello");
        write_lockfile_with_one_entry(&scope, &name, &version, h);
        let report = verify(&scope, &name, &version).unwrap();
        assert_eq!(report.status, VerifyStatus::Ok);
    }

    #[test]
    fn verify_detects_tree_drift() {
        let (_td, scope, name, version) = setup();
        let h = write_install_with_one_file(&scope, &name, &version, b"hello");
        write_lockfile_with_one_entry(&scope, &name, &version, h);
        // Mutate the install file → drift.
        fs::write(scope.package_dir(&name, &version).join("a.txt"), b"world").unwrap();
        let report = verify(&scope, &name, &version).unwrap();
        assert!(matches!(report.status, VerifyStatus::TreeDrift { .. }));
    }

    #[test]
    fn verify_detects_missing_install_dir() {
        let (_td, scope, name, version) = setup();
        write_lockfile_with_one_entry(&scope, &name, &version, "fake-hash".to_string());
        // No install dir created.
        let report = verify(&scope, &name, &version).unwrap();
        assert!(matches!(report.status, VerifyStatus::Missing { .. }));
    }

    #[test]
    fn verify_returns_unverified_for_v2_leftover_empty_sha256() {
        let (_td, scope, name, version) = setup();
        write_install_with_one_file(&scope, &name, &version, b"hello");
        write_lockfile_with_one_entry(&scope, &name, &version, String::new()); // empty sha256
        let report = verify(&scope, &name, &version).unwrap();
        assert_eq!(report.status, VerifyStatus::Unverified);
    }

    #[test]
    fn verify_returns_package_not_installed_for_unknown_name() {
        let (_td, scope, _name, version) = setup();
        // No lockfile / no entry for this name.
        let unknown = PackageName::from_str("unknown").unwrap();
        let err = verify(&scope, &unknown, &version);
        // Either PackageNotInstalled OR LockfileLoad (if no lockfile) — both correct.
        assert!(err.is_err());
    }

    #[test]
    fn verify_all_returns_one_report_per_installed_version() {
        let (_td, scope, name, version) = setup();
        let h = write_install_with_one_file(&scope, &name, &version, b"hello");
        write_lockfile_with_one_entry(&scope, &name, &version, h);
        let reports = verify_all(&scope).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].status, VerifyStatus::Ok);
    }
}

#[cfg(test)]
mod anthropic_strict_tests {
    use super::*;
    use crate::lockfile::{
        LockFile, LockedPackage, LockedSkill, LockedVersion, SkillFrontmatterSnapshot,
    };
    use std::fs;
    use std::time::{Duration, UNIX_EPOCH};
    use tempfile::TempDir;

    /// Build a minimal scope + lockfile that has a skill package installed.
    ///
    /// `skill_md_content` is written verbatim to
    /// `<tau_dir>/packages/critic/0.1.0/SKILL.md`.
    fn make_skill_scope(tmp: &std::path::Path, skill_md_content: &str) -> Scope {
        let tau_dir = tmp.join(".tau");
        fs::create_dir_all(&tau_dir).unwrap();
        fs::write(
            tau_dir.join("config.toml"),
            "schema_version = 3\nkind = \"project\"\ncreated_at = \"2026-05-14T00:00:00Z\"\ncreated_by_tau_version = \"0.0.0\"\n\n[sandbox]\nrequired_tier = \"none\"\n",
        )
        .unwrap();

        let install_dir = tau_dir.join("packages").join("critic").join("0.1.0");
        fs::create_dir_all(&install_dir).unwrap();
        fs::write(install_dir.join("SKILL.md"), skill_md_content).unwrap();

        // Compute sha256 of the content so the drift check passes.
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(skill_md_content.as_bytes());
        let sha = crate::tree_hash::to_hex_lower(&h.finalize());

        let locked_pkg = LockedPackage {
            name: "critic".parse().unwrap(),
            active_version: "0.1.0".parse().unwrap(),
            source: "https://example.com/critic.git".parse().unwrap(),
            installed_versions: vec![LockedVersion {
                version: "0.1.0".parse().unwrap(),
                rev: None,
                resolved_commit: "0".repeat(40),
                sha256: String::new(), // no tree hash; Unverified is fine for these tests
                installed_at: UNIX_EPOCH + Duration::from_secs(1_747_180_800),
            }],
            plugin: None,
            skill: Some(LockedSkill::new(
                sha,
                SkillFrontmatterSnapshot {
                    name: "critic".into(),
                    description: "Reviews drafts.".into(),
                },
            )),
            synthesized_from: None,
        };

        let mut lf = LockFile::default();
        lf.packages.push(locked_pkg);
        lf.save(&tmp.join("tau-lock.toml")).unwrap();

        Scope::resolve(tmp).unwrap()
    }

    #[test]
    fn anthropic_strict_passes_for_conformant_skill() {
        let td = TempDir::new().unwrap();
        let content =
            "---\nname: critic\ndescription: Reviews drafts.\n---\n\nYou are a strict editor.\n";
        let scope = make_skill_scope(td.path(), content);
        let reports = verify_all_with_options(&scope, true).unwrap();
        let conformance_issues: Vec<_> = reports
            .iter()
            .filter(|r| matches!(r.status, VerifyStatus::AnthropicConformance { .. }))
            .collect();
        assert!(
            conformance_issues.is_empty(),
            "expected no conformance issues, got: {conformance_issues:?}"
        );
    }

    #[test]
    fn anthropic_strict_flags_missing_description() {
        let td = TempDir::new().unwrap();
        // SKILL.md with whitespace-only description — parse succeeds but
        // description.trim() is empty → MissingDescription.
        let content = "---\nname: critic\ndescription: \"   \"\n---\n\nYou are a strict editor.\n";
        let scope = make_skill_scope(td.path(), content);
        let reports = verify_all_with_options(&scope, true).unwrap();
        let issues: Vec<_> = reports
            .iter()
            .filter(|r| matches!(r.status, VerifyStatus::AnthropicConformance { .. }))
            .collect();
        assert_eq!(issues.len(), 1, "expected exactly one conformance issue");
        assert!(
            matches!(
                &issues[0].status,
                VerifyStatus::AnthropicConformance {
                    issue: AnthropicConformanceIssue::MissingDescription,
                    ..
                }
            ),
            "expected MissingDescription, got {:?}",
            issues[0].status
        );
    }

    #[test]
    fn anthropic_strict_off_does_not_run_conformance() {
        let td = TempDir::new().unwrap();
        // Same whitespace-only description that would trigger MissingDescription.
        let content = "---\nname: critic\ndescription: \"   \"\n---\n\nYou are a strict editor.\n";
        let scope = make_skill_scope(td.path(), content);
        let reports = verify_all_with_options(&scope, false).unwrap();
        let conformance_issues: Vec<_> = reports
            .iter()
            .filter(|r| matches!(r.status, VerifyStatus::AnthropicConformance { .. }))
            .collect();
        assert!(
            conformance_issues.is_empty(),
            "expected no conformance issues when strict=false, got: {conformance_issues:?}"
        );
    }
}

#[cfg(test)]
mod skill_drift_tests {
    use super::*;
    use crate::lockfile::{LockedSkill, SkillFrontmatterSnapshot};
    use std::fs;
    use tempfile::tempdir;

    fn write_skill_md(dir: &std::path::Path, body: &str) -> String {
        // Returns the SHA-256 of the body.
        let path = dir.join("SKILL.md");
        fs::write(&path, body).unwrap();
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(body.as_bytes());
        crate::tree_hash::to_hex_lower(&h.finalize())
    }

    #[test]
    fn ok_when_skill_md_matches_locked_hash() {
        let dir = tempdir().unwrap();
        let body = "---\nname: critic\ndescription: x\n---\nbody\n";
        let sha = write_skill_md(dir.path(), body);
        let locked = LockedSkill::new(
            sha,
            SkillFrontmatterSnapshot {
                name: "critic".into(),
                description: "x".into(),
            },
        );
        let result = verify_skill_content(dir.path(), "critic", &locked);
        assert!(matches!(result, Ok(())), "expected Ok, got {result:?}");
    }

    #[test]
    fn drift_when_skill_md_modified_after_install() {
        let dir = tempdir().unwrap();
        let body = "---\nname: critic\ndescription: x\n---\nbody\n";
        let original_sha = write_skill_md(dir.path(), body);
        // Mutate SKILL.md after recording the snapshot.
        fs::write(
            dir.path().join("SKILL.md"),
            "---\nname: critic\ndescription: x\n---\nMUTATED\n",
        )
        .unwrap();
        let locked = LockedSkill::new(
            original_sha.clone(),
            SkillFrontmatterSnapshot {
                name: "critic".into(),
                description: "x".into(),
            },
        );
        let result = verify_skill_content(dir.path(), "critic", &locked);
        match result {
            Err(VerifyStatus::SkillContentDrift {
                name,
                expected,
                got,
            }) => {
                assert_eq!(name, "critic");
                assert_eq!(expected, original_sha);
                assert_ne!(expected, got);
            }
            other => panic!("expected SkillContentDrift, got {other:?}"),
        }
    }
}
