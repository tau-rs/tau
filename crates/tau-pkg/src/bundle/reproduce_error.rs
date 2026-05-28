//! `ReproError` — failures that prevent producing a reproducibility
//! comparison (Phase 2 §E). A successful *non-reproducible* result is
//! NOT an error — it's `Ok(ReproReport { reproducible: false, .. })`.

use std::path::PathBuf;

/// Errors from [`crate::bundle::verify_reproducible`].
#[derive(Debug, thiserror::Error)]
pub enum ReproError {
    /// The shipped bundle file could not be read.
    #[error("failed to read bundle at {path:?}: {source}")]
    BundleRead {
        /// Path attempted.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },

    /// The shipped bundle failed to parse.
    #[error("bundle parse failed: {source}")]
    BundleParse {
        /// Underlying parse error.
        #[source]
        source: crate::bundle::error::BundleParseError,
    },

    /// The shipped bundle's own self-hash is invalid — it is corrupt
    /// and cannot serve as a reproducibility reference.
    #[error("shipped bundle self-hash is invalid ({detail}); it is corrupt — cannot use it as a reproducibility reference")]
    ShippedSelfHashInvalid {
        /// Human-readable detail from the integrity check.
        detail: String,
    },

    /// Could not create the temp dir for the rebuild.
    #[error("could not create temp dir for rebuild: {source}")]
    TempDir {
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },

    /// The rebuild from the local tree failed.
    #[error("rebuild failed: {source}")]
    Rebuild {
        /// Underlying build error.
        #[source]
        source: crate::bundle::build_error::BuildError,
    },

    /// The rebuilt bundle could not be read back.
    #[error("failed to read rebuilt bundle at {path:?}: {source}")]
    RebuiltRead {
        /// Path attempted.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },

    /// The rebuilt bundle failed to parse.
    #[error("rebuilt bundle parse failed: {source}")]
    RebuiltParse {
        /// Underlying parse error.
        #[source]
        source: crate::bundle::error::BundleParseError,
    },
}
