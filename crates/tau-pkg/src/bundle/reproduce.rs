//! `tau verify --bundle` reproducibility checker (Phase 2 §E).
//!
//! Rebuilds a fresh bundle from the local source tree and compares its
//! self-hash to a shipped bundle. See spec
//! `2026-05-28-tau-verify-bundle-design.md`.

use std::path::PathBuf;

use crate::bundle::manifest::BundleManifest;
use crate::bundle::reproduce_error::ReproError;

/// Inputs to [`verify_reproducible`].
#[derive(Debug, Clone)]
pub struct ReproOptions {
    /// Path to the shipped `.tau` bundle to reproduce.
    pub bundle_path: PathBuf,
    /// Local source tree to rebuild from (typically cwd).
    pub project_root: PathBuf,
}

/// Result of a reproducibility check.
#[derive(Debug, Clone)]
pub struct ReproReport {
    /// True when the rebuilt bundle's self-hash equals the shipped one's.
    pub reproducible: bool,
    /// The shipped bundle's self-hash.
    pub shipped_sha256: String,
    /// The rebuilt bundle's self-hash.
    pub rebuilt_sha256: String,
    /// Field-level divergences. Empty when `reproducible`.
    pub diffs: Vec<ManifestDiff>,
}

/// Which side of a comparison a one-sided item appears on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Side {
    /// Present only in the shipped bundle.
    ShippedOnly,
    /// Present only in the rebuilt bundle.
    RebuiltOnly,
}

/// A single field-level divergence between two manifests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestDiff {
    /// A `[project]` field differs.
    ProjectField {
        /// Field name.
        field: String,
        /// Shipped value.
        shipped: String,
        /// Rebuilt value.
        rebuilt: String,
    },
    /// A package is present on only one side.
    PackageMissing {
        /// Package name.
        name: String,
        /// Which side it appears on.
        side: Side,
    },
    /// A package field differs.
    PackageField {
        /// Package name.
        name: String,
        /// Field name.
        field: String,
        /// Shipped value.
        shipped: String,
        /// Rebuilt value.
        rebuilt: String,
    },
    /// An agent is present on only one side.
    AgentMissing {
        /// Agent id.
        id: String,
        /// Which side it appears on.
        side: Side,
    },
    /// An agent field differs.
    AgentField {
        /// Agent id.
        id: String,
        /// Field name.
        field: String,
        /// Shipped value.
        shipped: String,
        /// Rebuilt value.
        rebuilt: String,
    },
    /// A `[bundle]` metadata field differs (target, tau_version).
    BundleMetaField {
        /// Field name.
        field: String,
        /// Shipped value.
        shipped: String,
        /// Rebuilt value.
        rebuilt: String,
    },
    /// schema_version differs.
    SchemaVersionMismatch {
        /// Shipped version.
        shipped: u32,
        /// Rebuilt version.
        rebuilt: u32,
    },
}

/// Rebuild from `opts.project_root` and compare to the shipped bundle.
pub fn verify_reproducible(_opts: ReproOptions) -> Result<ReproReport, ReproError> {
    unimplemented!("Task 3")
}

/// Field-level diff between two manifests (Task 2).
pub(crate) fn diff_manifests(
    _shipped: &BundleManifest,
    _rebuilt: &BundleManifest,
) -> Vec<ManifestDiff> {
    unimplemented!("Task 2")
}
