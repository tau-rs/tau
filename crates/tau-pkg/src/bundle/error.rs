//! Error types for bundle parsing, IO, and integrity checks.

/// Errors raised when parsing bundle TOML.
#[derive(Debug, thiserror::Error)]
pub enum BundleParseError {
    /// Underlying TOML syntax/schema error.
    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),
    /// Bundle declares a `schema_version` this binary does not support.
    #[error("unsupported schema_version {found}; this tau binary supports v1.x only")]
    UnsupportedSchemaVersion {
        /// The schema_version found in the manifest.
        found: u32,
    },
}

/// Errors raised when reading + parsing a bundle from disk.
#[derive(Debug, thiserror::Error)]
pub enum BundleIoError {
    /// Could not read the bundle file.
    #[error("could not read bundle at {path}: {source}")]
    Read {
        /// Path attempted.
        path: std::path::PathBuf,
        /// Underlying IO error.
        source: std::io::Error,
    },
    /// Parsing the bundle contents failed.
    #[error(transparent)]
    Parse(#[from] BundleParseError),
}

/// Errors raised when verifying a bundle's self-hash. Used by Task 3.
#[derive(Debug, thiserror::Error)]
pub enum BundleIntegrityError {
    /// `bundle.sha256` does not match the recomputed canonical-TOML SHA-256.
    #[error("bundle self-hash mismatch: claimed {claimed}, computed {computed}")]
    HashMismatch {
        /// Hash claimed by the bundle's `bundle.sha256` field.
        claimed: String,
        /// Hash computed from the bundle's canonical-TOML form.
        computed: String,
    },
    /// `bundle.sha256` field is empty (zero-length string).
    #[error("bundle.sha256 field is empty")]
    HashFieldEmpty,
}
