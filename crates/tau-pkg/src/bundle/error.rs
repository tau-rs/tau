//! Error types for bundle parsing, IO, and integrity checks.

/// Errors raised when parsing bundle TOML.
///
/// # Example
///
/// ```
/// use tau_pkg::bundle::error::BundleParseError;
///
/// let err = BundleParseError::UnsupportedSchemaVersion { found: 99 };
/// let display = format!("{err}");
/// assert!(display.contains("99"));
/// assert!(display.contains("unsupported"));
/// ```
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
    /// A bundle declares `[[trigger]]` entries but its `schema_version` is
    /// below 3. Triggers require schema_version 3 (an old tau must reject a
    /// trigger-bearing bundle rather than silently drop the bindings).
    #[error("bundle declares triggers but schema_version is {schema_version} (triggers require schema_version >= 3)")]
    TriggerSchemaVersionMismatch {
        /// The bundle's declared schema_version.
        schema_version: u32,
    },
    /// A bundle carries a `[governance]` record but its `schema_version` is
    /// below 4. Governance requires schema_version 4 (an old tau must reject
    /// a governance-bearing bundle rather than silently ignore the verdict).
    #[error("bundle declares a [governance] record but schema_version is {schema_version} (governance requires schema_version >= 4)")]
    GovernanceSchemaVersionMismatch {
        /// The bundle's declared schema_version.
        schema_version: u32,
    },
    /// A bundle carries an `[[assets]]` store but its `schema_version` is
    /// below 5. The asset store (D6-B) requires schema_version 5 (an old tau
    /// must reject the bundle rather than silently drop the prompt bytes the
    /// IR references).
    #[error("bundle declares an [[assets]] store but schema_version is {schema_version} (assets require schema_version >= 5)")]
    AssetSchemaVersionMismatch {
        /// The bundle's declared schema_version.
        schema_version: u32,
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
///
/// # Example
///
/// ```
/// use tau_pkg::bundle::error::BundleIntegrityError;
///
/// let err = BundleIntegrityError::HashFieldEmpty;
/// let display = format!("{err}");
/// assert!(display.contains("empty"));
///
/// let err2 = BundleIntegrityError::HashMismatch {
///     claimed: "aaaa".to_string(),
///     computed: "bbbb".to_string(),
/// };
/// let display2 = format!("{err2}");
/// assert!(display2.contains("mismatch"));
/// ```
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
