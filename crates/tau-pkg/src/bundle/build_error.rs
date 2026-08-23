//! `BuildError` — typed failure type for the bundle build pipeline.

use std::path::PathBuf;

/// Errors returned by [`crate::bundle::build`](crate::bundle::build()).
///
/// Each variant maps to a CLI exit code via `tau-cli::cmd::build`:
/// config/parse errors → 2, install-state errors → 3, internal/IO
/// errors → 70.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// Failed to load or parse the project's `tau.toml`.
    #[error("failed to load project tau.toml: {0}")]
    ProjectConfig(String),

    /// The project lockfile (`tau.lock`) is absent.
    #[error("missing lockfile; run `tau install` first")]
    MissingLockfile,

    /// Lockfile present but could not be loaded or parsed.
    #[error("failed to load lockfile: {0}")]
    LockfileLoad(String),

    /// A package recorded in the lockfile is missing from the local
    /// install cache.
    #[error("package `{name}` is locked but not installed at {path:?}; run `tau install`")]
    PackageNotInstalled {
        /// Logical name of the missing package.
        name: String,
        /// Expected install path on disk.
        path: PathBuf,
    },

    /// Recomputing the tree hash of a package payload failed.
    #[error("tree hash failed for `{name}`: {source}")]
    TreeHashFailed {
        /// Logical name of the package whose tree hash failed.
        name: String,
        /// Underlying tree-hash error.
        #[source]
        source: crate::tree_hash::TreeHashError,
    },

    /// Resolving an agent's system prompt (file or inline) failed.
    #[error("system prompt resolution failed for agent `{id}`: {source}")]
    PromptResolveFailed {
        /// Agent id whose prompt failed to resolve.
        id: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Computing the effective capabilities for an agent failed.
    #[error("capability override compute failed for agent `{id}`: {source}")]
    CapabilityOverrideFailed {
        /// Agent id whose capability override failed.
        id: String,
        /// Underlying override-expansion error.
        #[source]
        source: crate::capability_override::OverrideExpandError,
    },

    /// Assembled bundle manifest failed validation.
    #[error("manifest assembly failed: {0}")]
    ManifestInvalid(String),

    /// An agent declares capability overrides but its home package (the
    /// `[agents.<id>].package` ref) is absent from the resolved package
    /// set — the effective grant set cannot be computed.
    #[error("agent `{id}` has capability overrides but its home package `{package}` is not in the bundle's package set; run `tau install`")]
    AgentHomePackageMissing {
        /// Agent id with the dangling home-package reference.
        id: String,
        /// The unresolved home-package name.
        package: String,
    },

    /// An agent's home-package manifest could not be read or parsed while
    /// computing its effective capabilities.
    #[error(
        "failed to read home-package manifest for agent `{id}` (package `{package}`): {source}"
    )]
    AgentHomePackageManifest {
        /// Agent id whose home-package manifest failed to load.
        id: String,
        /// The home-package name.
        package: String,
        /// Underlying manifest read/parse error.
        #[source]
        source: crate::error::ManifestReadError,
    },

    /// `--agent <id>` named an agent not present in the project config.
    #[error("unknown agent `{id}`; available agents: {}", available.join(", "))]
    UnknownAgent {
        /// The requested agent id that does not exist.
        id: String,
        /// All agent ids declared in the project (sorted), for the hint.
        available: Vec<String>,
    },

    /// Writing the bundle artifact to disk failed.
    #[error("bundle write failed at {path:?}: {source}")]
    WriteFailed {
        /// Path the bundle was being written to.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}
