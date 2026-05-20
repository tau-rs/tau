//! Install lifecycle for tau-pkg.
//!
//! [`install`] performs the 10-step pipeline:
//!
//! 1. Pre-flight: `git --version` check + acquire scope-level advisory file lock.
//! 2. Clone source into `<scope>/.tau/packages/.staging/<random>/`.
//! 3. Parse `tau.toml` from the staging directory.
//! 4. Validate the manifest structurally (handled by `read_manifest`).
//! 5. Verify the user-supplied source matches the manifest's declared source.
//! 6. Capability validation — warn (don't error) on unknown kinds and
//!    non-namespaced `Capability::Custom` names.
//! 7. Resolve the cloned repo's HEAD to a 40-char SHA.
//! 8. Materialize: atomically `fs::rename` staging → `<scope>/.tau/packages/<name>/<version>/`.
//! 9. Update the lockfile (atomic write).
//! 10. Release the advisory file lock.
//!
//! Failure at any step before (8) leaves no on-disk state (the staging
//! TempDir is auto-removed). At step (8), the staging directory's
//! auto-cleanup is disabled before the rename; on rename failure the
//! orphaned staging dir is best-effort removed. Failure at (8) post-
//! rename or at (9) is unlikely (both are atomic operations).
//!
//! Note: idempotency cannot be checked before the clone because the
//! package name (lockfile key) is not known until the manifest is
//! parsed. The clone is therefore always performed; the idempotency
//! short-circuit happens at step (8) before the rename, avoiding only
//! the disk-write phase.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use fs4::FileExt;
use tau_domain::{kinds, Capability, PackageName, PackageSource, PluginKind, Version};

use crate::error::{InstallError, UninstallError};
use crate::git::Git;
use crate::lockfile::{
    LockFile, LockedPackage, LockedPlugin, LockedSkill, LockedVersion, SkillFrontmatterSnapshot,
};
use crate::manifest::read_manifest;
use crate::scope::Scope;

/// Options governing plugin builds during install.
///
/// Used by [`InstallOptions::build`] to control the `cargo build` step
/// for plugin packages (those with a `[plugin]` table in `tau.toml`).
/// For data-only packages, these options are ignored.
///
/// `#[non_exhaustive]`: future fields are non-breaking. External
/// callers construct via [`BuildOptions::new`] / [`BuildOptions::default`]
/// and mutate fields by name.
///
/// # Example
///
/// ```
/// use tau_pkg::BuildOptions;
///
/// // `BuildOptions` is `#[non_exhaustive]`; struct-literal construction
/// // is blocked across crate boundaries. Default + field mutation is
/// // the canonical pattern.
/// let mut opts = BuildOptions::default();
/// opts.skip_build = true;
/// assert!(opts.skip_build);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    /// Skip the build step entirely. Used by tests that synthesize
    /// lockfiles against pre-built binaries, and by
    /// `tau install --no-build`.
    pub skip_build: bool,
    /// Override the cargo binary path. Defaults to the `cargo` on PATH.
    pub cargo_path: Option<PathBuf>,
    /// Extra arguments passed through to `cargo build`
    /// (e.g., `--features foo`).
    pub extra_args: Vec<String>,
}

impl BuildOptions {
    /// Construct a fresh `BuildOptions` with defaults: build enabled,
    /// `cargo` discovered on PATH, no extra args.
    ///
    /// `BuildOptions` is `#[non_exhaustive]`; external callers use this
    /// constructor (or [`BuildOptions::default`]) and mutate fields by
    /// name.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Options for [`install_with_options`].
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct InstallOptions {
    /// If `true` (default), wait indefinitely for a concurrent install
    /// to release the advisory file lock. If `false`, error
    /// immediately with [`InstallError::Locked`].
    pub block_on_lock: bool,
    /// If `true`, force re-clone even if the target version directory
    /// already exists. Default: `false` (idempotent — skip clone if
    /// the dir exists and the lockfile already records this version).
    pub force: bool,
    /// Plugin-build options, applied during the build step for
    /// packages whose `tau.toml` contains a `[plugin]` table.
    ///
    /// Defaults: build enabled, `cargo` from PATH, no extra args.
    /// Ignored for data-only packages (no `[plugin]` table).
    pub build: BuildOptions,
    /// If `true`, skip the Layer 2 cross-check at step 8.7 (sub-project B).
    ///
    /// Default: `false` — production installs always run the cross-check.
    /// Tests that build stub plugin binaries which don't implement the
    /// `meta.handshake` protocol set this to `true` to bypass the
    /// cross-check.
    pub skip_cross_check: bool,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            block_on_lock: true,
            force: false,
            build: BuildOptions::default(),
            skip_cross_check: false,
        }
    }
}

/// Outcome of a successful [`install`] call.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct InstalledPackage {
    /// The package name as declared in `tau.toml`.
    pub name: PackageName,
    /// The package version as declared in `tau.toml`.
    pub version: Version,
    /// The source the package was installed from.
    pub source: PackageSource,
    /// The on-disk path of the installed package directory.
    pub installed_path: PathBuf,
    /// When this version was installed.
    pub installed_at: SystemTime,
}

/// Install a package from `source` into `scope`. Equivalent to
/// `install_with_options(source, scope, InstallOptions::default())`.
///
/// # Example
///
/// ```ignore
/// // `Scope` is `#[non_exhaustive]`; use `Scope::resolve` / `global` /
/// // `new_project` to obtain one.
/// use std::str::FromStr;
/// use tau_domain::PackageSource;
/// use tau_pkg::{install, Scope};
///
/// let scope = Scope::global().unwrap();
/// let source = PackageSource::from_str("https://example.com/pkg.git").unwrap();
/// let installed = install(&source, &scope).unwrap();
/// println!("installed at {}", installed.installed_path.display());
/// ```
pub fn install(source: &PackageSource, scope: &Scope) -> Result<InstalledPackage, InstallError> {
    install_with_options(source, scope, InstallOptions::default())
}

/// Install a package from `source` into `scope` with explicit `options`.
/// See the module-level documentation for the 10-step lifecycle.
pub fn install_with_options(
    source: &PackageSource,
    scope: &Scope,
    options: InstallOptions,
) -> Result<InstalledPackage, InstallError> {
    // Step 1: pre-flight.
    let _git_version = Git::version_check()?;

    let lock_path = scope.install_lock_path();
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent).map_err(|e| InstallError::Internal {
            message: format!("creating lock directory {}: {e}", parent.display()),
        })?;
    }

    let lock_file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| InstallError::Internal {
            message: format!("opening install lock {}: {e}", lock_path.display()),
        })?;

    if options.block_on_lock {
        lock_file
            .lock_exclusive()
            .map_err(|e| InstallError::Internal {
                message: format!("acquiring install lock: {e}"),
            })?;
    } else {
        match lock_file.try_lock_exclusive() {
            Ok(()) => {}
            Err(e) if e.kind() == fs4::lock_contended_error().kind() => {
                return Err(InstallError::Locked {
                    scope: scope.state_path().display().to_string(),
                });
            }
            Err(e) => {
                return Err(InstallError::Internal {
                    message: format!("trying install lock: {e}"),
                });
            }
        }
    }

    // From here on, ensure the lock is released even on early returns.
    // `lock_file` stays in scope until the end; we explicitly unlock at step 10.

    let result = (|| -> Result<InstalledPackage, InstallError> {
        // Step 2: clone into staging.
        let staging_root = scope.packages_dir().join(".staging");
        fs::create_dir_all(&staging_root).map_err(|e| InstallError::Internal {
            message: format!("creating staging dir {}: {e}", staging_root.display()),
        })?;

        let staging_dir = tempfile::Builder::new()
            .prefix("staging-")
            .tempdir_in(&staging_root)
            .map_err(|e| InstallError::Internal {
                message: format!("creating staging tempdir: {e}"),
            })?;

        Git::clone(source, staging_dir.path())?;

        // Step 3 + 4: parse + validate manifest (validate is part of read_manifest).
        // Skills-5: auto-detect format. Tau-format workspaces read tau.toml (existing
        // path). Anthropic-format workspaces (SKILL.md only) synthesize a manifest
        // in-memory. Neither file → NotASkillPackage.
        let workspace_format = tau_domain::detect_format(staging_dir.path());
        let (manifest, synthesized_from) = match workspace_format {
            tau_domain::SkillFormat::Tau => {
                let m = read_manifest(&staging_dir.path().join("tau.toml"))?;
                (m, None)
            }
            tau_domain::SkillFormat::Anthropic => {
                let m = crate::synthesize::synthesize_anthropic_skill(
                    staging_dir.path(),
                    source.clone(),
                )
                .map_err(crate::error::InstallError::SynthesizeFailed)?;

                // Write the synthesized tau.toml to the staging workspace so
                // that it is present in the install directory after the rename
                // at step 8. This allows `find_installed_skill` (used by
                // `tau skill show` and `tau skill export`) to locate the
                // manifest without needing the lockfile.
                let toml_text = toml::to_string_pretty(&m).map_err(|e| InstallError::Internal {
                    message: format!("serializing synthesized tau.toml: {e}"),
                })?;
                fs::write(staging_dir.path().join("tau.toml"), &toml_text).map_err(|e| {
                    InstallError::Internal {
                        message: format!("writing synthesized tau.toml to staging dir: {e}"),
                    }
                })?;

                (m, Some(crate::lockfile::SynthesizedSource::Anthropic))
            }
            tau_domain::SkillFormat::Invalid => {
                return Err(crate::error::InstallError::NotASkillPackage {
                    path: staging_dir.path().to_owned(),
                    detail: "directory has neither tau.toml nor SKILL.md".into(),
                });
            }
            _ => {
                // Forward-compat: treat unknown variants as invalid.
                return Err(crate::error::InstallError::NotASkillPackage {
                    path: staging_dir.path().to_owned(),
                    detail: "unrecognized workspace format".into(),
                });
            }
        };

        // Step 5: source / manifest match.
        if *manifest.source() != *source {
            return Err(InstallError::SourceManifestMismatch {
                expected: source.to_string(),
                found: manifest.source().to_string(),
            });
        }

        // Step 6: capability validation (warnings only — NG12).
        warn_unknown_kind(&manifest);
        warn_non_namespaced_custom_capabilities(&manifest);

        // Step 7: resolve commit.
        let resolved_commit = Git::resolve_head(staging_dir.path())?;

        // Step 8: materialize.
        let target = scope.package_dir(manifest.name(), manifest.version());
        let lockfile_path = scope.lockfile_path();
        let mut lf = LockFile::load(&lockfile_path)?;

        let already_recorded = lf
            .find(manifest.name())
            .map(|p| {
                p.installed_versions
                    .iter()
                    .any(|v| v.version == *manifest.version())
            })
            .unwrap_or(false);

        if target.exists() {
            if !options.force && already_recorded {
                // Idempotent: already installed at this version; return the
                // existing entry's metadata.
                let installed_at = lf
                    .find(manifest.name())
                    .and_then(|p| {
                        p.installed_versions
                            .iter()
                            .find(|v| v.version == *manifest.version())
                            .map(|v| v.installed_at)
                    })
                    .ok_or_else(|| InstallError::Internal {
                        message: format!(
                            "lockfile recorded {}@{} but installed_at lookup returned None — \
                             inconsistent state",
                            manifest.name(),
                            manifest.version(),
                        ),
                    })?;
                return Ok(InstalledPackage {
                    name: manifest.name().clone(),
                    version: manifest.version().clone(),
                    source: source.clone(),
                    installed_path: target,
                    installed_at,
                });
            }
            // Otherwise: force OR orphaned directory — overwrite.
            fs::remove_dir_all(&target).map_err(|e| InstallError::Internal {
                message: format!("removing existing target {}: {e}", target.display()),
            })?;
        }

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| InstallError::Internal {
                message: format!("creating target parent {}: {e}", parent.display()),
            })?;
        }

        // Take ownership of the staging path before rename (disables auto-cleanup).
        let staged_path = staging_dir.keep();
        if let Err(e) = fs::rename(&staged_path, &target) {
            // Best-effort cleanup — the rename failed, so the staging dir is now
            // orphaned (tempfile no longer manages it). Ignore secondary failure.
            let _ = fs::remove_dir_all(&staged_path);
            return Err(InstallError::Internal {
                message: format!(
                    "renaming {} -> {}: {e}",
                    staged_path.display(),
                    target.display(),
                ),
            });
        }

        // Step 8.6: compute source tree SHA-256 (post-rename, the install
        // dir is in its final location; `target/` is excluded by tree_hash).
        let source_sha256 =
            crate::tree_hash::tree_hash(&target).map_err(|e| InstallError::Internal {
                message: format!("computing source tree hash: {e}"),
            })?;

        // Step 8.5: build (only for kind = "rust-cargo" plugin packages).
        //
        // Per spec §6.3, the build is invoked between materialization
        // (rename) and lockfile write. On failure the lockfile is NOT
        // updated, but the cloned source is left in place so users can
        // diagnose without re-cloning. Users retry with `tau install
        // --force` or inspect the failure under the package dir.
        let mut locked_plugin = build_plugin_if_needed(&manifest, &target, &options.build)?;

        // Step 8.7: Layer 2 cross-check — spawn the freshly-built binary,
        // perform handshake + per-method tool.describe_capabilities, compare
        // against the manifest's [[capabilities]]. On error, abort install
        // (binary stays on disk; user retries via `tau install --force`
        // after fixing the manifest).
        //
        // Skipped when:
        //   - Data-only package (no [plugin] table → locked_plugin is None).
        //   - Build was skipped via skip_build = true (locked_plugin is None
        //     even for plugin packages — test / --no-build path).
        //
        // cross_check_plugin_capabilities is async; install_with_options is
        // synchronous. Bridge via a current-thread tokio runtime spun up just
        // for this step.
        if !options.skip_cross_check {
            if let Some(ref mut lp) = locked_plugin {
                let binary_path = lp.binary_path.clone();
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| InstallError::Internal {
                        message: format!("build tokio runtime for cross-check: {e}"),
                    })?;
                let shapes = runtime
                    .block_on(crate::sandbox_check::cross_check_plugin_capabilities(
                        &binary_path,
                        &manifest,
                    ))
                    .map_err(|e| InstallError::CrossCheck {
                        message: e.to_string(),
                    })?;
                lp.required_shapes = shapes;
            }

            // Skills-2 cross-check: validates SKILL.md content + frontmatter +
            // reference lint. Hard-fails on missing capabilities.
            // Skills have no plugin process to spawn; no tokio runtime needed.
            if manifest.skill().is_some() {
                crate::skill_check::cross_check_skill_package(&target, &manifest)?;
            }
        }

        // Step 8.8: for skill packages, compute SHA-256 of the SKILL.md bytes
        // and snapshot the frontmatter into a LockedSkill for the lockfile.
        let locked_skill: Option<LockedSkill> = if manifest.skill().is_some() {
            let content_path = target.join(&manifest.skill().unwrap().content);
            let bytes = std::fs::read(&content_path).map_err(|e| InstallError::Internal {
                message: format!("reading SKILL.md for sha256 at {content_path:?}: {e}"),
            })?;
            let content_sha256 = {
                use sha2::{Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update(&bytes);
                crate::tree_hash::to_hex_lower(&hasher.finalize())
            };

            // Re-parse for the frontmatter snapshot. cross_check_skill_package
            // already validated; re-parsing here is a small cost to keep the
            // two concerns (validation vs snapshot) cleanly separated.
            let body_text = std::str::from_utf8(&bytes).map_err(|e| InstallError::Internal {
                message: format!("SKILL.md is not UTF-8 at {content_path:?}: {e}"),
            })?;
            let parsed = tau_domain::parse_skill_md(body_text).map_err(|e| {
                // Should be unreachable — cross_check just validated — but
                // surface as Internal if it fires.
                InstallError::Internal {
                    message: format!("post-validation SKILL.md re-parse failed: {e}"),
                }
            })?;

            Some(LockedSkill::new(
                content_sha256,
                SkillFrontmatterSnapshot {
                    name: parsed.frontmatter.name,
                    description: parsed.frontmatter.description,
                },
            ))
        } else {
            None
        };

        // Step 9: update lockfile.
        let now = SystemTime::now();
        let new_locked_version = LockedVersion {
            version: manifest.version().clone(),
            rev: rev_from_source(source),
            resolved_commit,
            sha256: source_sha256,
            installed_at: now,
        };

        let updated_package = match lf.find(manifest.name()).cloned() {
            Some(mut existing) => {
                // Replace the matching version (if present) or append.
                let mut found = false;
                for v in existing.installed_versions.iter_mut() {
                    if v.version == *manifest.version() {
                        *v = new_locked_version.clone();
                        found = true;
                        break;
                    }
                }
                if !found {
                    existing.installed_versions.push(new_locked_version.clone());
                }
                existing.active_version = manifest.version().clone();
                existing.source = source.clone();
                existing.plugin = locked_plugin.clone();
                existing.skill = locked_skill.clone();
                existing.synthesized_from = synthesized_from;
                // Sort installed_versions for deterministic lockfile diffs.
                existing
                    .installed_versions
                    .sort_by(|a, b| a.version.cmp(&b.version));
                existing
            }
            None => LockedPackage {
                name: manifest.name().clone(),
                active_version: manifest.version().clone(),
                source: source.clone(),
                installed_versions: vec![new_locked_version.clone()],
                plugin: locked_plugin.clone(),
                skill: locked_skill,
                synthesized_from,
            },
        };

        lf.upsert(updated_package);
        lf.packages
            .sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
        lf.generated_at = now;
        lf.save(&lockfile_path)?;

        Ok(InstalledPackage {
            name: manifest.name().clone(),
            version: manifest.version().clone(),
            source: source.clone(),
            installed_path: target,
            installed_at: now,
        })
    })();

    // Step 10: release the lock (explicit via FileExt::unlock; also released on drop).
    let _ = lock_file.unlock();

    result
}

/// Extract `rev` from a `PackageSource::Git`. Other variants (none at v0.1)
/// return `None`.
fn rev_from_source(source: &PackageSource) -> Option<String> {
    match source {
        PackageSource::Git { rev, .. } => rev.clone(),
        _ => None,
    }
}

/// Maximum bytes of cargo's stderr we keep in
/// [`InstallError::BuildFailed::stderr_tail`] for diagnostic display.
/// Keeps error payloads bounded; the full output is still streamed to
/// the user's stderr in real time during the build.
const STDERR_TAIL_BYTES: usize = 4096;

/// Build the plugin binary for `manifest` (if any), recording the
/// resulting [`LockedPlugin`] for the lockfile.
///
/// Returns `Ok(None)` when:
///
/// - The manifest carries no `[plugin]` table (data-only package), or
/// - `options.skip_build` is `true` (the test/`--no-build` path).
///
/// Returns `Ok(Some(LockedPlugin))` when the build succeeded, with
/// `binary_path` canonicalized.
///
/// Returns `Err(InstallError::CargoNotFound)` if the configured cargo
/// binary cannot be invoked, and `Err(InstallError::BuildFailed { .. })`
/// for non-zero cargo exits.
fn build_plugin_if_needed(
    manifest: &tau_domain::PackageManifest,
    package_dir: &Path,
    options: &BuildOptions,
) -> Result<Option<LockedPlugin>, InstallError> {
    let Some(plugin_manifest) = manifest.plugin() else {
        return Ok(None);
    };
    if options.skip_build {
        return Ok(None);
    }

    match plugin_manifest.kind {
        PluginKind::RustCargo => build_rust_cargo_plugin(plugin_manifest, package_dir, options),
        // `PluginKind` is `#[non_exhaustive]`. Future variants (e.g.
        // `PythonPip`, `NodeNpm`, `Prebuilt`) get their own build paths;
        // until then, surface unknown kinds via `Internal` so the user
        // gets a typed error rather than a silent no-op.
        other => Err(InstallError::Internal {
            message: format!("plugin kind {other} not yet supported by this tau-pkg version"),
        }),
    }
}

/// Run `cargo build --release --bin <plugin.bin>` in `package_dir` and
/// return the [`LockedPlugin`] for the lockfile.
fn build_rust_cargo_plugin(
    plugin_manifest: &tau_domain::PluginManifest,
    package_dir: &Path,
    options: &BuildOptions,
) -> Result<Option<LockedPlugin>, InstallError> {
    let cargo = options
        .cargo_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("cargo"));

    let mut cmd = Command::new(&cargo);
    cmd.arg("build")
        .arg("--release")
        .arg("--bin")
        .arg(&plugin_manifest.bin)
        .current_dir(package_dir);
    for arg in &options.extra_args {
        cmd.arg(arg);
    }

    eprintln!(
        "  building {bin} ({kind}) in {dir}...",
        bin = plugin_manifest.bin,
        kind = plugin_manifest.kind,
        dir = package_dir.display(),
    );

    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(InstallError::CargoNotFound);
        }
        Err(e) => {
            return Err(InstallError::Internal {
                message: format!("spawning cargo at {}: {e}", cargo.display()),
            });
        }
    };

    // Stream cargo's captured output to the host's stderr so users see
    // build progress. (We capture rather than inherit so we can record
    // the stderr tail on failure.)
    if !output.stdout.is_empty() {
        let s = String::from_utf8_lossy(&output.stdout);
        eprint!("{s}");
    }
    if !output.stderr.is_empty() {
        let s = String::from_utf8_lossy(&output.stderr);
        eprint!("{s}");
    }

    if !output.status.success() {
        return Err(InstallError::BuildFailed {
            exit_status: output.status,
            stderr_tail: tail_lossy_utf8(&output.stderr, STDERR_TAIL_BYTES),
        });
    }

    // Cargo appends the platform-specific executable extension
    // (`.exe` on Windows, empty elsewhere). Match it via
    // `std::env::consts::EXE_SUFFIX` so Windows installs find their
    // binaries.
    let bin_filename = format!("{}{}", plugin_manifest.bin, std::env::consts::EXE_SUFFIX);
    let binary_path = package_dir
        .join("target")
        .join("release")
        .join(&bin_filename);
    let canonical = binary_path
        .canonicalize()
        .map_err(|e| InstallError::BuildFailed {
            exit_status: output.status,
            stderr_tail: format!(
                "cargo succeeded but binary not found at {}: {e}",
                binary_path.display(),
            ),
        })?;

    let binary_sha256 =
        crate::tree_hash::sha256_of_file(&canonical).map_err(|e| InstallError::Internal {
            message: format!("computing plugin binary hash: {e}"),
        })?;

    Ok(Some(LockedPlugin::new(
        plugin_manifest.clone(),
        canonical,
        SystemTime::now(),
        binary_sha256,
    )))
}

/// Take the last `max_bytes` bytes of `buf` and decode lossily as UTF-8.
/// Used to bound the size of [`InstallError::BuildFailed::stderr_tail`].
fn tail_lossy_utf8(buf: &[u8], max_bytes: usize) -> String {
    let start = buf.len().saturating_sub(max_bytes);
    String::from_utf8_lossy(&buf[start..]).into_owned()
}

/// Warn (eprintln) if the manifest's kind isn't a known canonical from
/// `tau_domain::kinds`. v0.1 is permissive — unknown kinds are valid;
/// the runtime decides what to do with them. NG12: tau is a runtime,
/// not a framework.
///
/// At v0.1, `PackageKind` has a single variant `Custom { kind: String }`.
/// Canonical kind strings live in `tau_domain::kinds`.
fn warn_unknown_kind(manifest: &tau_domain::PackageManifest) {
    use tau_domain::PackageKind;

    let known_kinds: &[&str] = &[
        kinds::LLM_BACKEND,
        kinds::TOOL,
        kinds::SKILL,
        kinds::PIPELINE,
        kinds::MCP_SERVER,
        kinds::STORAGE,
        kinds::SANDBOX,
    ];

    let kind_str = match manifest.kind() {
        PackageKind::Custom { kind } => kind.as_str(),
        _ => return, // forward-compat: new typed variants are always "known"
    };

    if !known_kinds.contains(&kind_str) {
        eprintln!(
            "warning: package {} has unknown kind {:?}; tau-runtime will treat it as opaque",
            manifest.name(),
            kind_str,
        );
    }
}

/// Warn (eprintln) on `Capability::Custom { name }` without a dot-namespaced name.
/// Encourages `mcp.tool.use` style; permits non-namespaced for forward-compat.
fn warn_non_namespaced_custom_capabilities(manifest: &tau_domain::PackageManifest) {
    for cap in manifest.capabilities() {
        if let Capability::Custom { name, .. } = cap {
            if !name.contains('.') {
                eprintln!(
                    "warning: package {} declares Capability::Custom {{ name = {:?} }} \
                     without a dot-namespaced name; consider e.g. \"vendor.feature.action\"",
                    manifest.name(),
                    name,
                );
            }
        }
    }
}

/// Uninstall a package from `scope`.
///
/// If `version` is `None`, removes ALL installed versions and the
/// lockfile entry. If `Some`, removes just that version directory
/// and updates the lockfile. If the removed version was the active
/// version, promotes the highest remaining (semver-sorted) version
/// as active; if no versions remain, the package entry is removed
/// entirely.
///
/// # Errors
///
/// - [`UninstallError::NotInstalled`] — the package isn't recorded
///   in the lockfile at all.
/// - [`UninstallError::VersionNotInstalled`] — the package is
///   installed but not at the requested version.
/// - [`UninstallError::Io`] — file-system removal failed.
/// - [`UninstallError::Locked`] — the per-scope advisory file lock
///   is held by another process (we currently always block on the
///   lock; this is reserved for future non-blocking flavors).
/// - [`UninstallError::Registry`] — lockfile load/save failed.
/// - [`UninstallError::Scope`] — scope state directory issue.
///
/// # Example
///
/// ```ignore
/// // `Scope` and `PackageName` are constructed via their respective APIs.
/// use tau_pkg::{uninstall, Scope};
/// use std::str::FromStr;
///
/// let scope = Scope::global().unwrap();
/// let name: tau_domain::PackageName = "acme-tool".parse().unwrap();
/// uninstall(&name, None, &scope).unwrap();
/// ```
pub fn uninstall(
    name: &PackageName,
    version: Option<&Version>,
    scope: &Scope,
) -> Result<(), UninstallError> {
    let lock_path = scope.install_lock_path();
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent).map_err(|e| UninstallError::Io {
            message: format!("creating lock directory {}: {e}", parent.display()),
        })?;
    }

    let lock_file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| UninstallError::Io {
            message: format!("opening uninstall lock {}: {e}", lock_path.display()),
        })?;

    FileExt::lock_exclusive(&lock_file).map_err(|e| UninstallError::Io {
        message: format!("acquiring uninstall lock: {e}"),
    })?;

    let result = (|| -> Result<(), UninstallError> {
        let lockfile_path = scope.lockfile_path();
        let mut lf = LockFile::load(&lockfile_path)?;

        if lf.find(name).is_none() {
            return Err(UninstallError::NotInstalled {
                name: name.as_str().to_owned(),
            });
        }

        match version {
            None => {
                // Remove all versions of the package.
                let pkg_dir = scope.packages_dir().join(name.as_str());
                if pkg_dir.exists() {
                    fs::remove_dir_all(&pkg_dir).map_err(|e| UninstallError::Io {
                        message: format!("removing {}: {e}", pkg_dir.display()),
                    })?;
                }
                lf.remove(name);
            }
            Some(v) => {
                // Remove just one version.
                let pkg = lf.find(name).expect("verified above");
                let has_version = pkg.installed_versions.iter().any(|lv| lv.version == *v);
                if !has_version {
                    return Err(UninstallError::VersionNotInstalled {
                        name: name.as_str().to_owned(),
                        version: v.to_string(),
                    });
                }

                let version_dir = scope.package_dir(name, v);
                if version_dir.exists() {
                    fs::remove_dir_all(&version_dir).map_err(|e| UninstallError::Io {
                        message: format!("removing {}: {e}", version_dir.display()),
                    })?;
                }

                // Mutate the lockfile entry: drop the LockedVersion, possibly
                // promote a new active_version, or remove the package entirely.
                let mut pkg_clone = lf.find(name).expect("verified above").clone();
                pkg_clone.installed_versions.retain(|lv| lv.version != *v);

                if pkg_clone.installed_versions.is_empty() {
                    lf.remove(name);
                } else {
                    if pkg_clone.active_version == *v {
                        // Promote the highest remaining version (semver-sorted).
                        let highest = pkg_clone
                            .installed_versions
                            .iter()
                            .map(|lv| lv.version.clone())
                            .max()
                            .expect("non-empty after retain check");
                        pkg_clone.active_version = highest;
                    }
                    // Keep installed_versions sorted by semver for stable diffs.
                    pkg_clone
                        .installed_versions
                        .sort_by(|a, b| a.version.cmp(&b.version));
                    lf.upsert(pkg_clone);
                }
            }
        }

        // Top-level packages list also kept sorted for stable diffs.
        lf.packages
            .sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
        lf.generated_at = SystemTime::now();
        lf.save(&lockfile_path)?;
        Ok(())
    })();

    let _ = FileExt::unlock(&lock_file);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_options_default() {
        let opts = InstallOptions::default();
        assert!(opts.block_on_lock);
        assert!(!opts.force);
    }

    #[test]
    fn build_options_default_does_not_skip() {
        let opts = BuildOptions::default();
        assert!(!opts.skip_build);
        assert!(opts.cargo_path.is_none());
        assert!(opts.extra_args.is_empty());
    }

    #[test]
    fn build_options_new_matches_default() {
        let new_opts = BuildOptions::new();
        let default_opts = BuildOptions::default();
        assert_eq!(new_opts.skip_build, default_opts.skip_build);
        assert_eq!(new_opts.cargo_path, default_opts.cargo_path);
        assert_eq!(new_opts.extra_args, default_opts.extra_args);
    }

    #[test]
    fn install_options_default_includes_default_build_options() {
        let opts = InstallOptions::default();
        assert!(!opts.build.skip_build);
        assert!(opts.build.cargo_path.is_none());
        assert!(opts.build.extra_args.is_empty());
    }

    #[test]
    fn rev_from_source_extracts_some_rev() {
        let s = "https://x.com/y.git#main".parse::<PackageSource>().unwrap();
        assert_eq!(rev_from_source(&s), Some("main".to_string()));
    }

    #[test]
    fn rev_from_source_returns_none_when_absent() {
        let s = "https://x.com/y.git".parse::<PackageSource>().unwrap();
        assert_eq!(rev_from_source(&s), None);
    }

    // The bulk of install_with_options testing happens in Task 14's
    // integration test suite (tests/install_lifecycle.rs), which uses
    // file://-based git fixtures via `git init --bare`.

    use std::time::{Duration, UNIX_EPOCH};

    use tau_domain::Version;
    use tempfile::TempDir;

    use crate::lockfile::{LockFile, LockedPackage, LockedVersion};

    fn make_scope_with_lockfile(tmp: &TempDir) -> Scope {
        let global_path = tmp.path().join("tau-home");
        Scope::global_at(global_path).unwrap()
    }

    fn fixture_locked_version(version_str: &str) -> LockedVersion {
        LockedVersion {
            version: version_str.parse().unwrap(),
            rev: Some("main".into()),
            resolved_commit: "0123456789abcdef0123456789abcdef01234567".into(),
            sha256: String::new(),
            installed_at: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        }
    }

    #[test]
    fn uninstall_returns_not_installed_when_package_missing() {
        let tmp = TempDir::new().unwrap();
        let scope = make_scope_with_lockfile(&tmp);
        let name: PackageName = "ghost-pkg".parse().unwrap();

        let err = uninstall(&name, None, &scope).unwrap_err();
        assert!(matches!(
            err,
            crate::error::UninstallError::NotInstalled { .. }
        ));
    }

    #[test]
    fn uninstall_returns_version_not_installed_when_version_missing() {
        let tmp = TempDir::new().unwrap();
        let scope = make_scope_with_lockfile(&tmp);
        let name: PackageName = "acme-tool".parse().unwrap();
        let installed_version: Version = "1.0.0".parse().unwrap();
        let missing_version: Version = "2.0.0".parse().unwrap();

        let mut lf = LockFile::default();
        lf.upsert(LockedPackage {
            name: name.clone(),
            active_version: installed_version.clone(),
            source: "https://x.com/y.git".parse().unwrap(),
            installed_versions: vec![fixture_locked_version("1.0.0")],
            plugin: None,
            skill: None,
            synthesized_from: None,
        });
        lf.save(&scope.lockfile_path()).unwrap();
        fs::create_dir_all(scope.package_dir(&name, &installed_version)).unwrap();

        let err = uninstall(&name, Some(&missing_version), &scope).unwrap_err();
        assert!(matches!(
            err,
            crate::error::UninstallError::VersionNotInstalled { .. }
        ));
    }

    #[test]
    fn uninstall_all_versions_removes_dir_and_lockfile_entry() {
        let tmp = TempDir::new().unwrap();
        let scope = make_scope_with_lockfile(&tmp);
        let name: PackageName = "acme-tool".parse().unwrap();
        let v1: Version = "1.0.0".parse().unwrap();
        let v2: Version = "2.0.0".parse().unwrap();

        let mut lf = LockFile::default();
        lf.upsert(LockedPackage {
            name: name.clone(),
            active_version: v2.clone(),
            source: "https://x.com/y.git".parse().unwrap(),
            installed_versions: vec![
                fixture_locked_version("1.0.0"),
                fixture_locked_version("2.0.0"),
            ],
            plugin: None,
            skill: None,
            synthesized_from: None,
        });
        lf.save(&scope.lockfile_path()).unwrap();
        fs::create_dir_all(scope.package_dir(&name, &v1)).unwrap();
        fs::create_dir_all(scope.package_dir(&name, &v2)).unwrap();

        uninstall(&name, None, &scope).unwrap();

        assert!(!scope.packages_dir().join(name.as_str()).exists());
        let reloaded = LockFile::load(&scope.lockfile_path()).unwrap();
        assert!(reloaded.find(&name).is_none());
    }

    #[test]
    fn uninstall_specific_version_promotes_highest_remaining() {
        let tmp = TempDir::new().unwrap();
        let scope = make_scope_with_lockfile(&tmp);
        let name: PackageName = "acme-tool".parse().unwrap();
        let v1: Version = "1.0.0".parse().unwrap();
        let v2: Version = "2.0.0".parse().unwrap();
        let v3: Version = "3.0.0".parse().unwrap();

        let mut lf = LockFile::default();
        lf.upsert(LockedPackage {
            name: name.clone(),
            active_version: v3.clone(),
            source: "https://x.com/y.git".parse().unwrap(),
            installed_versions: vec![
                fixture_locked_version("1.0.0"),
                fixture_locked_version("2.0.0"),
                fixture_locked_version("3.0.0"),
            ],
            plugin: None,
            skill: None,
            synthesized_from: None,
        });
        lf.save(&scope.lockfile_path()).unwrap();
        fs::create_dir_all(scope.package_dir(&name, &v1)).unwrap();
        fs::create_dir_all(scope.package_dir(&name, &v2)).unwrap();
        fs::create_dir_all(scope.package_dir(&name, &v3)).unwrap();

        uninstall(&name, Some(&v3), &scope).unwrap();

        assert!(scope.package_dir(&name, &v1).exists(), "v1 should remain");
        assert!(scope.package_dir(&name, &v2).exists(), "v2 should remain");
        assert!(!scope.package_dir(&name, &v3).exists(), "v3 should be gone");

        let reloaded = LockFile::load(&scope.lockfile_path()).unwrap();
        let pkg = reloaded.find(&name).unwrap();
        assert_eq!(
            pkg.active_version, v2,
            "active_version should promote to v2 (highest remaining)"
        );
        assert_eq!(pkg.installed_versions.len(), 2);
    }

    #[test]
    fn uninstall_specific_version_keeps_active_when_not_active() {
        let tmp = TempDir::new().unwrap();
        let scope = make_scope_with_lockfile(&tmp);
        let name: PackageName = "acme-tool".parse().unwrap();
        let v1: Version = "1.0.0".parse().unwrap();
        let v2: Version = "2.0.0".parse().unwrap();

        let mut lf = LockFile::default();
        lf.upsert(LockedPackage {
            name: name.clone(),
            active_version: v2.clone(),
            source: "https://x.com/y.git".parse().unwrap(),
            installed_versions: vec![
                fixture_locked_version("1.0.0"),
                fixture_locked_version("2.0.0"),
            ],
            plugin: None,
            skill: None,
            synthesized_from: None,
        });
        lf.save(&scope.lockfile_path()).unwrap();
        fs::create_dir_all(scope.package_dir(&name, &v1)).unwrap();
        fs::create_dir_all(scope.package_dir(&name, &v2)).unwrap();

        uninstall(&name, Some(&v1), &scope).unwrap();

        let reloaded = LockFile::load(&scope.lockfile_path()).unwrap();
        let pkg = reloaded.find(&name).unwrap();
        assert_eq!(pkg.active_version, v2, "v2 should remain active");
        assert_eq!(pkg.installed_versions.len(), 1, "only v2 should remain");
        assert_eq!(pkg.installed_versions[0].version, v2);
        assert!(
            !scope.package_dir(&name, &v1).exists(),
            "v1 dir should be gone"
        );
        assert!(
            scope.package_dir(&name, &v2).exists(),
            "v2 dir should remain"
        );
    }
}
