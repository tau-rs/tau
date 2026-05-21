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

pub mod canonical;
pub mod error;
pub mod hash;
pub mod manifest;

pub use canonical::to_canonical_toml;
pub use error::{BundleIntegrityError, BundleIoError, BundleParseError};
pub use hash::{compute_self_hash, verify_self_hash};
pub use manifest::{
    BackendRef, BundleAgent, BundleEffectiveCapabilities, BundleManifest, BundleMeta,
    BundlePackage, ProjectInfo,
};
