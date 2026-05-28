//! `VerifyError` — typed failure type for `tau run --bundle` integrity
//! verification (Phase 2 §C.3).

use std::path::PathBuf;

use tau_ports::target::TargetTriple;

/// Errors returned by [`crate::bundle::verify_bundle`].
///
/// CLI exit-code mapping (`tau-cli::cmd::run`):
/// bad-input/config → 2, integrity/install-state → 3, internal/IO → 70.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// The bundle file could not be read.
    #[error("failed to read bundle at {path:?}: {source}")]
    BundleRead {
        /// Path attempted.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },

    /// The bundle TOML failed to parse.
    #[error("bundle parse failed: {source}")]
    BundleParse {
        /// Underlying parse error.
        #[source]
        source: crate::bundle::error::BundleParseError,
    },

    /// The bundle's recorded self-hash does not match its content.
    #[error("bundle self-hash mismatch — claimed {claimed}, computed {computed}; the bundle was tampered with or corrupted")]
    SelfHashMismatch {
        /// Hash stored in the bundle.
        claimed: String,
        /// Hash recomputed from the bundle content.
        computed: String,
    },

    /// The bundle's schema_version isn't supported by this tau.
    #[error("unsupported bundle schema_version {found}; this tau supports {supported}")]
    UnsupportedSchemaVersion {
        /// Version found in the bundle.
        found: u32,
        /// Version this binary supports.
        supported: u32,
    },

    /// The bundle's target triple doesn't match the host.
    #[error("bundle target {bundle} does not match host {host}; rebuild for this host or run on a matching machine")]
    TargetMismatch {
        /// Triple baked into the bundle.
        bundle: TargetTriple,
        /// Triple of the running host.
        host: TargetTriple,
    },

    /// The cwd's tau.toml has drifted from the bundle's record.
    #[error("tau.toml drift — claimed sha256 {claimed} but cwd has {computed}; rebuild the bundle or check out the source at the recorded version")]
    TauTomlDrift {
        /// Hash recorded in the bundle.
        claimed: String,
        /// Hash of the cwd's tau.toml.
        computed: String,
    },

    /// The project tau.toml is missing or unreadable.
    #[error("project tau.toml missing or unreadable at {path:?}: {source}")]
    ProjectTomlRead {
        /// Path attempted.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },

    /// A locked package isn't installed on disk.
    #[error(
        "locked package `{name}` missing from {expected_path:?}; run `tau install` in this project"
    )]
    PackageMissing {
        /// Package name.
        name: String,
        /// Where it was expected.
        expected_path: PathBuf,
    },

    /// An installed package's tree hash drifted from the bundle.
    #[error("package `{name}` tree drift — claimed {claimed}, computed {computed}; reinstall or rebuild bundle")]
    PackageDrift {
        /// Package name.
        name: String,
        /// Hash recorded in the bundle.
        claimed: String,
        /// Hash recomputed from the installed tree.
        computed: String,
    },

    /// Tree-hashing an installed package failed.
    #[error("package `{name}` tree-hash failed: {source}")]
    PackageTreeHash {
        /// Package name.
        name: String,
        /// Underlying error.
        #[source]
        source: crate::tree_hash::TreeHashError,
    },

    /// An agent's system prompt drifted from the bundle.
    #[error("agent `{id}` system prompt drift — claimed {claimed}, computed {computed}")]
    AgentPromptDrift {
        /// Agent id.
        id: String,
        /// Hash recorded in the bundle.
        claimed: String,
        /// Hash recomputed from the prompt content.
        computed: String,
    },

    /// Resolving an agent's prompt (e.g. reading system_file) failed.
    #[error("agent `{id}` prompt resolve failed: {source}")]
    AgentPromptResolve {
        /// Agent id.
        id: String,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },

    /// An agent in the bundle isn't present in the cwd's tau.toml.
    #[error("agent `{id}` named in the bundle but missing from tau.toml; rebuild bundle")]
    AgentSetMismatch {
        /// Agent id.
        id: String,
    },
}
