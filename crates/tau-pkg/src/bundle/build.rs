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
pub fn build(_opts: BuildOptions) -> Result<BundleArtifact, BuildError> {
    unimplemented!("filled in by subsequent tasks")
}
