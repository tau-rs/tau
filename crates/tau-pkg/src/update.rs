//! Update an installed package to a newer (or specific) version.
//!
//! The entry point is [`update_package`]. It composes the existing
//! `source_list`, `install_with_options`, and optional `uninstall` paths to
//! produce an [`UpdateResult`] describing what changed.
//!
//! See `docs/superpowers/specs/2026-05-01-tau-lifecycle-design.md` §2.

use semver::Version;
use tau_domain::{PackageName, PackageSource};

use crate::error::{RegistryError, UninstallError};
use crate::install::{install_with_options, uninstall, InstallOptions};
use crate::lockfile::LockFile;
use crate::resolve::ResolveError;
use crate::scope::Scope;
use crate::source_list::{list_versions_at_source, SourceListError};

// ── Error ──────────────────────────────────────────────────────────────────

/// Errors returned by [`update_package`].
///
/// # Note
///
/// `#[non_exhaustive]`: new variants may be added in future versions without
/// a semver break.
///
/// ```
/// use tau_pkg::update::UpdateError;
///
/// // `UpdateError` is `#[non_exhaustive]`; match arms need a catch-all.
/// fn describe(e: &UpdateError) -> &'static str {
///     match e {
///         UpdateError::PackageNotInstalled { .. } => "not installed",
///         _ => "other",
///     }
/// }
/// ```
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    /// Loading the lockfile failed.
    #[error("lockfile load: {source}")]
    LockfileLoad {
        /// Underlying registry error.
        #[from]
        source: RegistryError,
    },
    /// The named package is not recorded in the scope's lockfile.
    #[error("package {name:?} is not installed")]
    PackageNotInstalled {
        /// The package name that was not found.
        name: String,
    },
    /// `list_versions_at_source` failed while enumerating available versions.
    #[error("listing versions for source: {source}")]
    SourceList {
        /// Underlying source-list error.
        #[source]
        source: SourceListError,
    },
    /// Version resolution failed (e.g. the requested pin is not reachable).
    #[error("resolving package {name:?}: {source}")]
    Resolve {
        /// The package name we tried to resolve.
        name: String,
        /// Underlying resolve error.
        #[source]
        source: ResolveError,
    },
    /// Installing the new version failed.
    #[error("installing {name}@{version}: {source}")]
    Install {
        /// The package name.
        name: String,
        /// The version string we tried to install.
        version: String,
        /// Underlying install error.
        #[source]
        source: crate::error::InstallError,
    },
    /// Uninstalling the old version (pruning) failed.
    #[error("uninstalling old version of {name}: {source}")]
    Uninstall {
        /// The package name.
        name: String,
        /// Underlying uninstall error.
        #[source]
        source: UninstallError,
    },
}

// ── Result type ────────────────────────────────────────────────────────────

/// Outcome of a successful [`update_package`] call.
///
/// # Note
///
/// `#[non_exhaustive]`: new fields may be added without a semver break.
///
/// ```
/// use tau_pkg::update::UpdateResult;
///
/// // `UpdateResult` is `#[non_exhaustive]`; values are produced by
/// // `update_package`. The fields are read-only from the caller's
/// // perspective.
/// fn log_update(r: &UpdateResult) {
///     println!("updated {} -> {}", r.from_version, r.to_version);
/// }
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateResult {
    /// The version that was active before the update.
    pub from_version: Version,
    /// The new active version after the update.
    pub to_version: Version,
    /// Transitive dependencies that were added or updated as a side-effect.
    ///
    /// Each entry is `(name, new_version)`. Dependencies that did not change
    /// are not included.
    pub transitive_deps_changed: Vec<(PackageName, Version)>,
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Update an installed package.
///
/// # Arguments
///
/// - `name` — the package to update (must already be installed).
/// - `version_pin` — if `None`, pick the highest available version from the
///   source listing; if `Some(v)`, validate that `v` is reachable then
///   install it.
/// - `scope` — the install scope to operate on.
/// - `prune` — if `true`, uninstall the previous active version after the
///   new one is successfully installed.
///
/// # Errors
///
/// See [`UpdateError`] for the full list of failure modes.
// `UpdateError::Resolve` carries `ResolveError` which contains diagnostic Vecs
// (available versions, constraints). The data is intentional — suppress the lint.
#[allow(clippy::result_large_err)]
pub fn update_package(
    name: &PackageName,
    version_pin: Option<Version>,
    scope: &Scope,
    prune: bool,
) -> Result<UpdateResult, UpdateError> {
    let lockfile_path = scope.lockfile_path();

    // ── 1. Load lockfile; find the current entry ───────────────────────────
    let lf = LockFile::load(&lockfile_path)?;

    let locked_pkg = lf
        .find(name)
        .ok_or_else(|| UpdateError::PackageNotInstalled {
            name: name.as_str().to_owned(),
        })?;

    let from_version = locked_pkg.active_version.clone();
    let source = locked_pkg.source.clone();

    // Snapshot the package list before install so we can diff for transitive deps.
    let packages_before: Vec<PackageName> = lf.packages.iter().map(|p| p.name.clone()).collect();

    // ── 2. Determine the target version ───────────────────────────────────
    //
    // When listing available versions we always use the source *without* a
    // rev pin so that `list_versions_at_source` uses `git ls-remote --tags`
    // to enumerate all tags. A rev-pinned source would return only the single
    // version at that rev, which is useless for an update operation.
    let listing_source = without_rev(&source);

    let to_version = match version_pin {
        None => {
            // List all available versions and pick the highest.
            let available = list_versions_at_source(&listing_source)
                .map_err(|e| UpdateError::SourceList { source: e })?;
            available
                .into_iter()
                .max()
                .ok_or_else(|| UpdateError::Resolve {
                    name: name.as_str().to_owned(),
                    source: ResolveError::NoCompatibleVersion {
                        name: name.clone(),
                        at_source: listing_source.clone(),
                        constraints: vec![semver::VersionReq::STAR],
                        available: vec![],
                    },
                })?
        }
        Some(pin) => {
            // Validate the requested version is reachable.
            let available = list_versions_at_source(&listing_source)
                .map_err(|e| UpdateError::SourceList { source: e })?;
            if !available.contains(&pin) {
                return Err(UpdateError::Resolve {
                    name: name.as_str().to_owned(),
                    source: ResolveError::NoCompatibleVersion {
                        name: name.clone(),
                        at_source: listing_source.clone(),
                        constraints: vec![semver::VersionReq::parse(&pin.to_string())
                            .unwrap_or(semver::VersionReq::STAR)],
                        available,
                    },
                });
            }
            pin
        }
    };

    // ── 3. Build the new PackageSource with the version tag as rev ─────────
    let new_source = with_rev(&source, &format!("v{to_version}"));

    // ── 4. Install the new version ─────────────────────────────────────────
    install_with_options(&new_source, scope, InstallOptions::default()).map_err(|e| {
        UpdateError::Install {
            name: name.as_str().to_owned(),
            version: to_version.to_string(),
            source: e,
        }
    })?;

    // ── 5. Promote active_version in the lockfile ──────────────────────────
    {
        let mut lf2 = LockFile::load(&lockfile_path)?;
        if let Some(pkg) = lf2.packages.iter_mut().find(|p| p.name == *name) {
            pkg.active_version = to_version.clone();
        }
        lf2.save(&lockfile_path)?;
    }

    // ── 6. Optionally prune the old version ────────────────────────────────
    if prune && from_version != to_version {
        uninstall(name, Some(&from_version), scope).map_err(|e| UpdateError::Uninstall {
            name: name.as_str().to_owned(),
            source: e,
        })?;
    }

    // ── 7. Compute transitive deps diff ────────────────────────────────────
    let lf_after = LockFile::load(&lockfile_path)?;
    let transitive_deps_changed: Vec<(PackageName, Version)> = lf_after
        .packages
        .iter()
        .filter(|p| p.name != *name && !packages_before.contains(&p.name))
        .map(|p| (p.name.clone(), p.active_version.clone()))
        .collect();

    Ok(UpdateResult {
        from_version,
        to_version,
        transitive_deps_changed,
    })
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Clone a `PackageSource`, replacing its `rev` with `new_rev`.
fn with_rev(source: &PackageSource, new_rev: &str) -> PackageSource {
    match source {
        PackageSource::Git { location, .. } => PackageSource::Git {
            location: location.clone(),
            rev: Some(new_rev.to_owned()),
        },
        // Forward-compat catch-all: preserve other variants unchanged.
        other => other.clone(),
    }
}

/// Clone a `PackageSource`, stripping any rev pin.
///
/// Used before `list_versions_at_source` so we always enumerate all tags
/// regardless of what rev pin the lockfile recorded.
fn without_rev(source: &PackageSource) -> PackageSource {
    match source {
        PackageSource::Git { location, .. } => PackageSource::Git {
            location: location.clone(),
            rev: None,
        },
        other => other.clone(),
    }
}
