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

    unimplemented!("steps 3-7 in subsequent tasks")
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
}
