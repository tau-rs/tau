#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! Tau package manager. Resolves, installs, and verifies extension
//! packages declared by users via `tau install`.
//!
//! tau-pkg implements:
//!
//! - **Scope detection** (G8): walks up from the cwd looking for a
//!   `.tau/` directory, falls back to global scope (`~/.tau`).
//! - **Manifest parsing** (G14): reads and structurally validates
//!   `tau.toml` files from disk via `tau_domain::UncheckedManifest`.
//! - **Install / uninstall**: shells out to `git clone`, materializes
//!   the package source tree, updates the lockfile.
//! - **Lockfile**: versioned TOML at `<project>/tau-lock.toml`
//!   (committed) or `~/.tau/tau-lock.toml` (local).
//!
//! See `docs/decisions/0004-tau-pkg.md` for the design rationale.

pub mod bundle;
pub mod capability_override;
pub mod error;
pub(crate) mod git;
pub mod install;
pub mod lockfile;
pub mod manifest;
pub mod project;
pub mod registry;
pub mod resolve;
pub mod sandbox_check;
pub mod scope;
pub mod skill_check;
pub mod skill_resolve;
pub mod source_list;
pub mod synthesize;
pub mod tree_hash;
pub mod update;
pub mod verify;

pub use bundle::{
    compute_self_hash, to_canonical_toml, verify_self_hash, BackendRef, BundleAgent,
    BundleEffectiveCapabilities, BundleIntegrityError, BundleIoError, BundleManifest, BundleMeta,
    BundlePackage, BundleParseError, ProjectInfo,
};
pub use error::{
    GitError, InstallError, ManifestReadError, RegistryError, ScopeError, UninstallError,
};
pub use install::{
    install, install_with_options, uninstall, BuildOptions, InstallOptions, InstalledPackage,
};
pub use lockfile::{LockFile, LockedPackage, LockedPlugin, LockedVersion, SynthesizedSource};
pub use manifest::read_manifest;
pub use project::{
    build_agent_definition, AgentEntry, AgentResolutionError, ProjectConfig, ProjectConfigError,
    PromptEntry, RequiresEntry,
};
pub use registry::{get, list};
pub use resolve::{
    resolve_requires_tools, PlannedInstall, RequiredTool, ResolutionPlan, ResolveError,
    ReusedInstall,
};
pub use scope::{Scope, ScopeConfig, ScopeKind};
pub use skill_check::cross_check_skill_package;
pub use skill_resolve::{find_installed_skill, FindSkillError, InstalledSkill};
pub use source_list::{list_versions_at_source, SourceListError};
pub use synthesize::{synthesize_anthropic_skill, SynthesizeError};
pub use tree_hash::{sha256_of_file, tree_hash, FileHash, TreeHashError};
pub use update::{update_package, UpdateError, UpdateResult};
pub use verify::{
    verify, verify_all, verify_all_with_options, AnthropicConformanceIssue, VerifyError,
    VerifyReport, VerifyStatus,
};
