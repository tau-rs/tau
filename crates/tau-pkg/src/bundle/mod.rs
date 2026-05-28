//! Tau bundle format (Phase 2 §C.1).
//!
//! See spec `docs/superpowers/specs/2026-05-19-bundle-format-design.md`
//! and ADR-0035.
//!
//! Public surface:
//! - [`BundleManifest`] — the top-level struct + sub-structs (manifest module).
//! - [`BundleParseError`] / [`BundleIoError`] / [`BundleIntegrityError`] (error module).
//! - Canonical TOML serialization (canonical module, Task 2).
//! - Self-hash compute + verify (hash module, Task 3).

pub mod build;
pub mod build_error;
pub mod canonical;
pub mod error;
pub mod hash;
pub mod manifest;
pub mod reproduce;
pub mod reproduce_error;
pub mod verify;
pub mod verify_error;

pub use build::{build, BuildOptions, BundleArtifact};
pub use build_error::BuildError;
pub use canonical::to_canonical_toml;
pub use error::{BundleIntegrityError, BundleIoError, BundleParseError};
pub use hash::{compute_self_hash, verify_self_hash};
pub use manifest::{
    BackendRef, BundleAgent, BundleEffectiveCapabilities, BundleManifest, BundleMeta,
    BundlePackage, ProjectInfo,
};
pub use reproduce::{verify_reproducible, ManifestDiff, ReproOptions, ReproReport, Side};
pub use reproduce_error::ReproError;
pub use verify::{verify_bundle, ResolvedAgent, VerifyOptions, VerifyReport};
pub use verify_error::VerifyError;
