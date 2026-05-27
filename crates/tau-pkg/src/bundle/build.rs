//! `tau build` producer — gathers a fully-installed project's
//! resolved state into a §C.1 bundle artifact.
//!
//! See spec `2026-05-27-tau-build-design.md` and ADR-0035.

use std::path::PathBuf;

use tau_ports::target::TargetTriple;

use crate::bundle::build_error::BuildError;

/// Inputs to [`build`].
#[derive(Debug, Clone)]
pub struct BuildOptions {
    /// Path to the project root (the directory containing `tau.toml`).
    pub project_root: PathBuf,
    /// Target triple to bake into the bundle. Use
    /// [`TargetTriple::host`] for the default.
    pub target: TargetTriple,
    /// Optional explicit output path. When `None`, defaults to
    /// `<project_root>/<project-name>-<project-version>.tau`.
    pub output_path: Option<PathBuf>,
}

/// Result of a successful build.
#[derive(Debug, Clone)]
pub struct BundleArtifact {
    /// Absolute path to the written bundle file.
    pub path: PathBuf,
    /// The bundle's self-hash (hex SHA-256).
    pub sha256: String,
    /// On-disk size of the bundle in bytes.
    pub size_bytes: u64,
}

/// Build a bundle from the project at [`BuildOptions::project_root`].
///
/// Strict mode: returns [`BuildError::MissingLockfile`] or
/// [`BuildError::PackageNotInstalled`] if the project isn't fully
/// installed. The function does NOT attempt to install anything.
pub fn build(opts: BuildOptions) -> Result<BundleArtifact, BuildError> {
    // Step 1: Load tau.toml.
    let tau_toml_path = opts.project_root.join("tau.toml");
    let tau_toml_bytes = std::fs::read(&tau_toml_path)
        .map_err(|e| BuildError::ProjectConfig(format!("read {tau_toml_path:?}: {e}")))?;
    let _project_toml: toml::Value = toml::from_str(
        std::str::from_utf8(&tau_toml_bytes)
            .map_err(|e| BuildError::ProjectConfig(format!("tau.toml is not utf-8: {e}")))?,
    )
    .map_err(|e| BuildError::ProjectConfig(format!("parse {tau_toml_path:?}: {e}")))?;

    // Step 2: Load tau.lock. Distinguish missing (run `tau install`)
    // from present-but-invalid (config error).
    let lockfile_path = opts.project_root.join("tau.lock");
    if !lockfile_path.exists() {
        return Err(BuildError::MissingLockfile);
    }
    let _lockfile = crate::lockfile::LockFile::load(&lockfile_path)
        .map_err(|e| BuildError::LockfileLoad(e.to_string()))?;

    // Step 3: Verify every locked package is materialized on disk.
    //
    // Install layout (per `Scope::package_dir`, see `crates/tau-pkg/src/scope.rs`):
    //     <project_root>/.tau/packages/<name>/<version>/
    //
    // The build is per-project, so the canonical install root is the
    // project scope's `state_path` (= `<project_root>/.tau`). We
    // compute the path directly rather than calling
    // `Scope::new_project`, which would side-effect by creating
    // `.tau/` and writing a default config — undesirable for a
    // read-only build pipeline that's supposed to fail loudly if the
    // project isn't installed.
    let packages_root = opts.project_root.join(".tau").join("packages");
    for pkg in &_lockfile.packages {
        let pkg_dir = packages_root
            .join(pkg.name.as_str())
            .join(pkg.active_version.to_string());
        if !pkg_dir.exists() {
            return Err(BuildError::PackageNotInstalled {
                name: pkg.name.as_str().to_owned(),
                path: pkg_dir,
            });
        }
    }

    unimplemented!("steps 4-7 in subsequent tasks")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_ports::target::TargetTriple;
    use tempfile::tempdir;

    fn opts(root: &std::path::Path) -> BuildOptions {
        BuildOptions {
            project_root: root.to_path_buf(),
            target: TargetTriple::host(),
            output_path: None,
        }
    }

    #[test]
    fn build_fails_on_missing_project_toml() {
        let tmp = tempdir().unwrap();
        let err = build(opts(tmp.path())).unwrap_err();
        assert!(
            matches!(err, BuildError::ProjectConfig(_)),
            "expected ProjectConfig, got {err:?}",
        );
    }

    #[test]
    fn build_fails_on_missing_lockfile() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("tau.toml"),
            r#"
[project]
name = "test-project"
version = "0.1.0"
"#,
        ).unwrap();
        let err = build(opts(tmp.path())).unwrap_err();
        assert!(
            matches!(err, BuildError::MissingLockfile),
            "expected MissingLockfile, got {err:?}",
        );
    }

    #[test]
    fn build_fails_when_locked_package_dir_missing() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("tau.toml"),
            r#"
[project]
name = "test-project"
version = "0.1.0"
"#,
        ).unwrap();
        // Minimal lockfile (schema v6) naming one package whose
        // installed dir does not exist anywhere on disk.
        let lockfile_toml = r#"
schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"

[[package]]
name = "ghost-plugin"
active_version = "0.1.0"
source = "https://example.com/ghost.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0000000000000000000000000000000000000001"
installed_at = "2024-01-01T00:00:00Z"
"#;
        std::fs::write(tmp.path().join("tau.lock"), lockfile_toml).unwrap();
        let err = build(opts(tmp.path())).unwrap_err();
        match err {
            BuildError::PackageNotInstalled { name, .. } => {
                assert_eq!(name, "ghost-plugin");
            }
            other => panic!("expected PackageNotInstalled, got {other:?}"),
        }
    }
}
