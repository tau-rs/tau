//! Skill resolution: lookup an installed skill by name.
//!
//! Skills-4 (ROADMAP §16). Reads the scope lockfile (one file open
//! handled by `LockFile::load`) + the skill package's `tau.toml` (one
//! additional file open). Returns a fully-built `InstalledSkill` with
//! resolved install path + parsed manifest + cached frontmatter.

use std::path::PathBuf;

use tau_domain::{Capability, PackageManifest, PackageName, SkillManifest, Version};

use crate::error::RegistryError;
use crate::lockfile::{LockFile, SkillFrontmatterSnapshot};
use crate::scope::Scope;

/// A fully-resolved installed skill, ready for runtime invocation.
#[derive(Debug, Clone)]
pub struct InstalledSkill {
    /// Package name (matches `tau.toml` `name` field).
    pub name: PackageName,
    /// Active version (from lockfile).
    pub version: Version,
    /// Absolute path to the installed package directory
    /// (`<scope_state>/.tau/packages/<name>/<version>/`).
    pub install_path: PathBuf,
    /// Parsed manifest from `<install_path>/tau.toml`.
    pub manifest: PackageManifest,
    /// Cached SKILL.md frontmatter snapshot (from lockfile).
    pub frontmatter: SkillFrontmatterSnapshot,
    /// Skill-specific manifest block (from manifest).
    pub skill: SkillManifest,
    /// Declared capabilities (from manifest).
    pub capabilities: Vec<Capability>,
}

/// Errors raised by [`find_installed_skill`].
///
/// # Example
///
/// ```
/// use tau_pkg::skill_resolve::FindSkillError;
///
/// let err = FindSkillError::InstallPathMissing {
///     name: "critic".to_string(),
///     path: std::path::PathBuf::from("/home/.tau/packages/critic/0.1.0"),
/// };
/// let display = format!("{err}");
/// assert!(display.contains("critic"));
/// assert!(display.contains("tau.toml"));
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FindSkillError {
    /// Failed to load the scope lockfile.
    #[error("lockfile load: {0}")]
    Lockfile(#[from] RegistryError),
    /// Lockfile entry exists, but the install path is missing on disk.
    #[error("skill {name:?} lockfile entry points at {path:?} but no tau.toml found there")]
    InstallPathMissing {
        /// Skill name.
        name: String,
        /// The expected install path.
        path: PathBuf,
    },
    /// I/O error reading the manifest.
    #[error("reading manifest at {path:?}: {source}")]
    ReadManifest {
        /// The manifest path.
        path: PathBuf,
        /// Underlying io error.
        #[source]
        source: std::io::Error,
    },
    /// TOML parse failure on the manifest.
    #[error("parsing manifest at {path:?}: {detail}")]
    ParseManifest {
        /// The manifest path.
        path: PathBuf,
        /// Parser error detail.
        detail: String,
    },
    /// Manifest validation failure.
    #[error("validating manifest at {path:?}: {detail}")]
    ValidateManifest {
        /// The manifest path.
        path: PathBuf,
        /// Validation error detail.
        detail: String,
    },
    /// Manifest declared a kind other than "skill" but was found in a
    /// skill entry in the lockfile.
    #[error(
        "manifest at {path:?} has kind != \"skill\" but is recorded as a skill in the lockfile"
    )]
    NotASkillManifest {
        /// The manifest path.
        path: PathBuf,
    },
}

/// Resolve an installed skill by name. Returns `Ok(None)` if no
/// installed package matches `name` and has a skill block.
///
/// Reads the scope lockfile + one `tau.toml` for the matched skill.
pub fn find_installed_skill(
    scope: &Scope,
    name: &str,
) -> Result<Option<InstalledSkill>, FindSkillError> {
    let lockfile_path = scope.lockfile_path();
    if !lockfile_path.exists() {
        return Ok(None);
    }
    let lockfile = LockFile::load(&lockfile_path)?;

    let pkg = match lockfile
        .packages
        .iter()
        .find(|p| p.name.as_str() == name && p.skill.is_some())
    {
        Some(p) => p,
        None => return Ok(None),
    };

    let locked_skill = pkg.skill.as_ref().expect("filtered to Some(skill) above");

    // Install path uses state_path (e.g. <project>/.tau/packages/<name>/<version>)
    let install_path = scope.package_dir(&pkg.name, &pkg.active_version);

    let toml_path = install_path.join("tau.toml");
    if !toml_path.exists() {
        return Err(FindSkillError::InstallPathMissing {
            name: name.to_string(),
            path: install_path,
        });
    }
    let toml_text =
        std::fs::read_to_string(&toml_path).map_err(|e| FindSkillError::ReadManifest {
            path: toml_path.clone(),
            source: e,
        })?;
    let unchecked: tau_domain::UncheckedManifest =
        toml::from_str(&toml_text).map_err(|e| FindSkillError::ParseManifest {
            path: toml_path.clone(),
            detail: e.to_string(),
        })?;
    let manifest = unchecked
        .validate()
        .map_err(|e| FindSkillError::ValidateManifest {
            path: toml_path.clone(),
            detail: e.to_string(),
        })?;

    let skill = match manifest.skill() {
        Some(s) => s.clone(),
        None => {
            return Err(FindSkillError::NotASkillManifest { path: toml_path });
        }
    };

    let capabilities = manifest.capabilities().to_vec();

    Ok(Some(InstalledSkill {
        name: pkg.name.clone(),
        version: pkg.active_version.clone(),
        install_path,
        manifest,
        frontmatter: locked_skill.frontmatter.clone(),
        skill,
        capabilities,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lockfile::{
        LockFile, LockedPackage, LockedSkill, LockedVersion, SkillFrontmatterSnapshot,
    };
    use std::fs;
    use std::time::Duration;
    use std::time::UNIX_EPOCH;
    use tempfile::tempdir;

    fn make_critic_scope(tmp: &std::path::Path) -> Scope {
        // Scope::resolve walks up looking for .tau/; we create .tau/ here.
        let tau_dir = tmp.join(".tau");
        fs::create_dir_all(&tau_dir).unwrap();
        fs::write(
            tau_dir.join("config.toml"),
            "schema_version = 3\nkind = \"project\"\ncreated_at = \"2026-05-14T00:00:00Z\"\ncreated_by_tau_version = \"0.0.0\"\n\n[sandbox]\nrequired_tier = \"none\"\n",
        )
        .unwrap();

        // Install path uses state_path: <tmp>/.tau/packages/<name>/<version>
        let install_dir = tau_dir.join("packages").join("critic").join("0.1.0");
        fs::create_dir_all(&install_dir).unwrap();
        fs::write(
            install_dir.join("tau.toml"),
            r#"name = "critic"
version = "0.1.0"
description = "Reviews drafts."
authors = []
source = "https://example.com/critic.git"
kind = "skill"
dependencies = []
capabilities = []

[skill]
"#,
        )
        .unwrap();
        fs::write(
            install_dir.join("SKILL.md"),
            "---\nname: critic\ndescription: x\n---\nbody\n",
        )
        .unwrap();

        // Build LockedPackage directly (not via TOML deserialization of a
        // nested struct literal — instead construct it in Rust).
        let locked_pkg = LockedPackage {
            name: "critic".parse().unwrap(),
            active_version: "0.1.0".parse().unwrap(),
            source: "https://example.com/critic.git".parse().unwrap(),
            installed_versions: vec![LockedVersion {
                version: "0.1.0".parse().unwrap(),
                rev: None,
                resolved_commit: "0000000000000000000000000000000000000000".into(),
                sha256: String::new(),
                installed_at: UNIX_EPOCH + Duration::from_secs(1_747_180_800),
            }],
            plugin: None,
            skill: Some(LockedSkill::new(
                "deadbeef".into(),
                SkillFrontmatterSnapshot {
                    name: "critic".into(),
                    description: "x".into(),
                },
            )),
            synthesized_from: None,
        };

        let mut lf = LockFile::default();
        lf.packages.push(locked_pkg);
        // Lockfile lives at scope.lockfile_path() = tmp/tau-lock.toml
        lf.save(&tmp.join("tau-lock.toml")).unwrap();

        Scope::resolve(tmp).unwrap()
    }

    #[test]
    fn returns_none_when_skill_absent() {
        let tmp = tempdir().unwrap();
        let tau_dir = tmp.path().join(".tau");
        fs::create_dir_all(&tau_dir).unwrap();
        fs::write(
            tau_dir.join("config.toml"),
            "schema_version = 3\nkind = \"project\"\ncreated_at = \"2026-05-14T00:00:00Z\"\ncreated_by_tau_version = \"0.0.0\"\n\n[sandbox]\nrequired_tier = \"none\"\n",
        )
        .unwrap();
        // No lockfile at all — find_installed_skill returns None.
        let scope = Scope::resolve(tmp.path()).unwrap();
        let result = find_installed_skill(&scope, "anything").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn returns_skill_when_found() {
        let tmp = tempdir().unwrap();
        let scope = make_critic_scope(tmp.path());
        let skill = find_installed_skill(&scope, "critic").unwrap().unwrap();
        assert_eq!(skill.name.as_str(), "critic");
        assert_eq!(skill.frontmatter.description, "x");
        // install_path is <tmp>/.tau/packages/critic/0.1.0
        assert!(skill.install_path.ends_with("packages/critic/0.1.0"));
    }

    #[test]
    fn returns_err_when_install_path_missing() {
        let tmp = tempdir().unwrap();
        let scope = make_critic_scope(tmp.path());
        // Remove the install path tau.toml
        let toml_path = scope
            .package_dir(&"critic".parse().unwrap(), &"0.1.0".parse().unwrap())
            .join("tau.toml");
        fs::remove_file(&toml_path).unwrap();
        let result = find_installed_skill(&scope, "critic");
        assert!(matches!(
            result,
            Err(FindSkillError::InstallPathMissing { .. })
        ));
    }
}
