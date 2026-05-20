//! Scope detection, configuration, and management for `tau-pkg`.
//!
//! Defines:
//!
//! - [`ScopeKind`] — `Global` vs `Project`. Determines path conventions
//!   (project scope keeps its lockfile at the project root and `.tau/`
//!   under it; global scope keeps everything under `~/.tau`).
//! - [`ScopeConfig`] — the on-disk `<scope>/config.toml` schema. Records
//!   `schema_version`, `kind`, `created_at`, `created_by_tau_version`,
//!   and a forward-compatible `defaults` map.
//!
//! The `Scope` struct itself (with `resolve()`, `global()`, `new_project()`,
//! and path accessors) lands in Task 5.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::error::ScopeError;

/// Maximum `ScopeConfig::schema_version` this tau version recognizes.
/// A `config.toml` with a higher value rejects with
/// [`ScopeError::ConfigSchemaTooNew`].
pub const MAX_SUPPORTED_SCOPE_CONFIG_SCHEMA_VERSION: u32 = 3;

/// Distinguishes a global scope (default `~/.tau`) from a project scope
/// (a `.tau/` directory inside a project's source tree).
///
/// Serialized lowercase (`"global"`, `"project"`) so TOML reads naturally.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScopeKind {
    /// Global scope (default `~/.tau`, or `$TAU_HOME` / `$XDG_DATA_HOME/tau`).
    Global,
    /// Project scope (a `.tau/` directory in the project's source tree).
    Project,
}

/// Per-scope sandbox requirements.
///
/// Lives under `[sandbox]` in `<scope>/config.toml`. Schema version 3.
///
/// Replaces the schema-v2 `SandboxConfig` (which used `chain` +
/// `minimum_tier`). v2 configs auto-migrate at load time with a
/// `tracing::warn!`. See [`deserialize_sandbox_with_migration`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SandboxRequirements {
    /// Minimum sandbox tier this project requires. Defaults to `Strict`.
    /// Setting `None` is the persistent opt-out (allows passthrough to satisfy).
    #[serde(default)]
    pub required_tier: SandboxRequiredTier,
    /// Explicit shape requirements. Optional. When empty, the resolver
    /// auto-derives the union of shapes from each plugin's declared
    /// capabilities.
    #[serde(default)]
    pub required_shapes: Vec<tau_domain::CapabilityShape>,
}

impl SandboxRequirements {
    /// Construct with an explicit tier. Shapes default to empty
    /// (auto-derive at resolution time).
    pub fn with_tier(required_tier: SandboxRequiredTier) -> Self {
        Self {
            required_tier,
            required_shapes: Vec::new(),
        }
    }
}

/// Required sandbox tier for a project (or plugin). Mirrors
/// `tau_ports::SandboxTier` but lives at the config layer (we don't
/// depend on tau-ports from tau-pkg). The runtime maps these to
/// `SandboxTier`.
#[non_exhaustive]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxRequiredTier {
    /// No tier requirement (allows passthrough to satisfy).
    None,
    /// Filesystem isolation at minimum.
    Light,
    /// Full strict tier required.
    #[default]
    Strict,
}

/// `serde` `deserialize_with` handler for the `sandbox` field of `ScopeConfig`.
///
/// Detects v2 schema (`chain` + `minimum_tier` keys) and best-effort
/// migrates to v3 (`required_tier`). Emits a once-per-process
/// `tracing::warn!` on migration. v3 configs (with `required_tier`)
/// pass through unchanged.
fn deserialize_sandbox_with_migration<'de, D>(
    deserializer: D,
) -> Result<SandboxRequirements, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;

    // Permissive raw shape that accepts both v2 and v3 keys.
    #[derive(Deserialize)]
    struct Raw {
        // v3 fields
        #[serde(default)]
        required_tier: Option<SandboxRequiredTier>,
        #[serde(default)]
        required_shapes: Option<Vec<tau_domain::CapabilityShape>>,
        // v2 fields (best-effort migration)
        #[serde(default)]
        chain: Option<toml::Value>,
        #[serde(default)]
        minimum_tier: Option<SandboxRequiredTier>, // same names, same serde
    }

    let raw = Raw::deserialize(deserializer)?;

    // v3 path: required_tier is the canonical signal.
    if raw.required_tier.is_some() || raw.required_shapes.is_some() {
        if raw.chain.is_some() || raw.minimum_tier.is_some() {
            warn_v2_v3_mixed();
        }
        return Ok(SandboxRequirements {
            required_tier: raw.required_tier.unwrap_or_default(),
            required_shapes: raw.required_shapes.unwrap_or_default(),
        });
    }

    // v2 path: derive required_tier from minimum_tier (or default to Strict);
    // ignore chain entries (the v3 resolver handles platform matching).
    if raw.chain.is_some() || raw.minimum_tier.is_some() {
        warn_v2_migration();
        return Ok(SandboxRequirements {
            required_tier: raw.minimum_tier.unwrap_or_default(),
            required_shapes: Vec::new(),
        });
    }

    // Empty `[sandbox]` block — pure default.
    Ok(SandboxRequirements::default())
}

fn warn_v2_migration() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        tracing::warn!(
            "scope config uses deprecated v2 [sandbox] schema (chain + minimum_tier). \
             Auto-migrating to v3 (required_tier derived from minimum_tier). \
             Run `tau sandbox setup` to rewrite the config in v3 form."
        );
    });
}

fn warn_v2_v3_mixed() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        tracing::warn!(
            "scope config mixes v2 ([sandbox] chain/minimum_tier) and v3 (required_tier) keys. \
             Honoring v3 keys; ignoring v2 keys."
        );
    });
}

/// Schema for `<scope>/config.toml`. Future-grown additively.
///
/// Stored at `<scope.state_path()>/config.toml`. Created automatically
/// by `Scope::global()` (Task 5) when missing, and by
/// `Scope::new_project()` (Task 5) when a new project scope is materialized.
///
/// Round-trips through TOML via `serde`. `humantime-serde` produces
/// RFC-3339 timestamps so the file is human-readable.
///
/// # Example
///
/// ```
/// // Constructed via [`ScopeConfig::new`] (the type is `#[non_exhaustive]`,
/// // so direct struct-literal construction is forbidden from external crates).
/// use tau_pkg::scope::{ScopeConfig, ScopeKind};
///
/// let cfg = ScopeConfig::new(ScopeKind::Global);
/// let toml = cfg.to_toml_string().unwrap();
/// assert!(toml.contains("schema_version"));
/// assert!(toml.contains("kind = \"global\""));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopeConfig {
    /// Schema version of this config. v0.1 ships `1`.
    pub schema_version: u32,
    /// Whether this is a global or project scope.
    pub kind: ScopeKind,
    /// When the scope was first materialized.
    #[serde(with = "humantime_serde")]
    pub created_at: SystemTime,
    /// `CARGO_PKG_VERSION` of the tau-pkg crate that created this scope.
    pub created_by_tau_version: String,
    /// Reserved for future scope-level defaults (default LLM backend,
    /// timeouts, etc.). Empty at v0.1.
    #[serde(default)]
    pub defaults: BTreeMap<String, tau_domain::Value>,
    /// Sandbox requirements. Defaults to `required_tier = "strict"` and
    /// empty `required_shapes` (auto-derive at resolution time).
    #[serde(default, deserialize_with = "deserialize_sandbox_with_migration")]
    pub sandbox: SandboxRequirements,
}

impl ScopeConfig {
    /// Construct a new `ScopeConfig` with the current time, the current
    /// crate version, schema version 3, and default sandbox requirements.
    pub fn new(kind: ScopeKind) -> Self {
        Self {
            schema_version: MAX_SUPPORTED_SCOPE_CONFIG_SCHEMA_VERSION,
            kind,
            created_at: SystemTime::now(),
            created_by_tau_version: env!("CARGO_PKG_VERSION").to_owned(),
            defaults: BTreeMap::new(),
            sandbox: SandboxRequirements::default(),
        }
    }

    /// Parse a `ScopeConfig` from a TOML string. Validates
    /// `schema_version` against [`MAX_SUPPORTED_SCOPE_CONFIG_SCHEMA_VERSION`].
    pub fn read_from_str(text: &str) -> Result<Self, ScopeError> {
        let mut cfg: Self = toml::from_str(text).map_err(|e| ScopeError::ConfigParse {
            reason: e.to_string(),
        })?;
        if cfg.schema_version > MAX_SUPPORTED_SCOPE_CONFIG_SCHEMA_VERSION {
            return Err(ScopeError::ConfigSchemaTooNew {
                found: cfg.schema_version,
                supported: MAX_SUPPORTED_SCOPE_CONFIG_SCHEMA_VERSION,
            });
        }
        // Auto-upgrade older configs in memory; next save() writes v3.
        // Mirrors the lockfile v3 -> v4 migration pattern in lockfile.rs.
        if cfg.schema_version < MAX_SUPPORTED_SCOPE_CONFIG_SCHEMA_VERSION {
            cfg.schema_version = MAX_SUPPORTED_SCOPE_CONFIG_SCHEMA_VERSION;
        }
        Ok(cfg)
    }

    /// Serialize the config as TOML.
    pub fn to_toml_string(&self) -> Result<String, ScopeError> {
        toml::to_string_pretty(self).map_err(|e| ScopeError::Internal {
            message: format!("config TOML serialization: {e}"),
        })
    }
}

/// A resolved scope: either the global `~/.tau` directory or a project-local
/// `.tau/` directory discovered by walking up from the current working directory.
///
/// # Example
///
/// ```ignore
/// // `Scope` is `#[non_exhaustive]`; constructed via `resolve`, `global`, or `new_project`.
/// use std::path::Path;
/// use tau_pkg::scope::Scope;
///
/// let scope = Scope::resolve(Path::new(".")).unwrap();
/// println!("{:?}", scope.kind());
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    /// Scope root.
    ///
    /// - `Global`: `~/.tau` (or `$TAU_HOME` / `$XDG_DATA_HOME/tau`).
    /// - `Project`: the project root directory (parent of `.tau/`).
    path: PathBuf,
    /// Local state directory.
    ///
    /// - `Global`: same as `path`.
    /// - `Project`: `<path>/.tau`.
    state_path: PathBuf,
    /// Whether this is a global or project scope.
    kind: ScopeKind,
}

impl Scope {
    /// Resolve the active scope by walking up from `cwd`.
    ///
    /// Checks each ancestor (starting with `cwd` itself) for a `.tau/`
    /// directory. Returns a `Project` scope on the first hit, or falls back
    /// to `Scope::global()` if none is found.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use std::path::Path;
    /// use tau_pkg::scope::Scope;
    ///
    /// let scope = Scope::resolve(Path::new(".")).unwrap();
    /// println!("{:?}", scope.kind());
    /// ```
    pub fn resolve(cwd: &Path) -> Result<Self, ScopeError> {
        if let Some((path, state_path)) = walk_up_for_dot_tau(cwd) {
            return Ok(Self {
                path,
                state_path,
                kind: ScopeKind::Project,
            });
        }
        Self::global()
    }

    /// Materialize (or open) the global scope directory.
    ///
    /// Resolves the global path via precedence:
    /// 1. `$TAU_HOME` if set and non-empty.
    /// 2. `$XDG_DATA_HOME/tau` if `$XDG_DATA_HOME` is set and non-empty.
    /// 3. `$HOME/.tau`.
    ///
    /// Creates the directory and a default `config.toml` if they are missing.
    /// Returns `ScopeError::HomeNotFound` if none of the three env vars are set.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use tau_pkg::scope::Scope;
    ///
    /// let scope = Scope::global().unwrap();
    /// println!("{:?}", scope.path());
    /// ```
    pub fn global() -> Result<Self, ScopeError> {
        let global_path = resolve_global_path()?;
        Self::materialize_global(global_path)
    }

    /// Test-only constructor: materialize the global scope at a given path
    /// without reading environment variables.
    #[cfg(test)]
    pub(crate) fn global_at(path: PathBuf) -> Result<Self, ScopeError> {
        Self::materialize_global(path)
    }

    /// Test-only walk-up: same as `resolve` but uses `fallback_home` instead
    /// of env-var lookup when no `.tau/` directory is found.
    #[cfg(test)]
    pub(crate) fn resolve_with_fallback(
        cwd: &Path,
        fallback_home: PathBuf,
    ) -> Result<Self, ScopeError> {
        if let Some((path, state_path)) = walk_up_for_dot_tau(cwd) {
            return Ok(Self {
                path,
                state_path,
                kind: ScopeKind::Project,
            });
        }
        Self::materialize_global(fallback_home)
    }

    /// Internal helper shared by `global()` and `global_at()`.
    fn materialize_global(global_path: PathBuf) -> Result<Self, ScopeError> {
        // Ensure the directory exists.
        fs::create_dir_all(&global_path).map_err(|e| ScopeError::Io {
            message: e.to_string(),
        })?;

        // Verify it really is a directory (create_dir_all might succeed even
        // if a symlink or regular file exists at the path in edge cases).
        if !fs::metadata(&global_path)
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            return Err(ScopeError::NotADirectory {
                path: global_path.to_string_lossy().into_owned(),
            });
        }

        // Write default config.toml if missing.
        let config_path = global_path.join("config.toml");
        if !config_path.exists() {
            write_default_config(&config_path, ScopeKind::Global)?;
        }

        Ok(Self {
            path: global_path.clone(),
            state_path: global_path,
            kind: ScopeKind::Global,
        })
    }

    /// Materialize a new project scope at `<project_root>/.tau/`.
    ///
    /// Creates the `.tau/` directory (idempotent) and writes a default
    /// `config.toml` if missing. Does **not** modify `.gitignore` — that
    /// hint is printed by `tau init` (Phase 1, in `tau-cli`) instead.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use std::path::Path;
    /// use tau_pkg::scope::Scope;
    ///
    /// let scope = Scope::new_project(Path::new("/my/project")).unwrap();
    /// println!("{:?}", scope.state_path());
    /// ```
    pub fn new_project(project_root: &Path) -> Result<Self, ScopeError> {
        // Verify that project_root exists and is a directory.
        let meta = fs::metadata(project_root).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ScopeError::Io {
                    message: format!("project root not found: {}", project_root.display()),
                }
            } else {
                ScopeError::Io {
                    message: format!("stat-ing project root {}: {}", project_root.display(), e),
                }
            }
        })?;
        if !meta.is_dir() {
            return Err(ScopeError::NotADirectory {
                path: project_root.to_string_lossy().into_owned(),
            });
        }

        let state_path = project_root.join(".tau");

        // Create .tau/ directory (idempotent).
        fs::create_dir_all(&state_path).map_err(|e| ScopeError::Io {
            message: e.to_string(),
        })?;

        // Write default config.toml if missing.
        let config_path = state_path.join("config.toml");
        if !config_path.exists() {
            write_default_config(&config_path, ScopeKind::Project)?;
        }

        Ok(Self {
            path: project_root.to_path_buf(),
            state_path,
            kind: ScopeKind::Project,
        })
    }

    /// Returns the scope root path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the local state directory path.
    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    /// Returns the `ScopeKind` for this scope.
    pub fn kind(&self) -> ScopeKind {
        self.kind
    }

    /// Returns the path to the lockfile: `<path>/tau-lock.toml`.
    pub fn lockfile_path(&self) -> PathBuf {
        self.path.join("tau-lock.toml")
    }

    /// Returns the path to the config file: `<state_path>/config.toml`.
    pub fn config_path(&self) -> PathBuf {
        self.state_path.join("config.toml")
    }

    /// Returns the path to the packages directory: `<state_path>/packages`.
    pub fn packages_dir(&self) -> PathBuf {
        self.state_path.join("packages")
    }

    /// Returns the path to the install advisory lock: `<state_path>/locks/install.lock`.
    pub fn install_lock_path(&self) -> PathBuf {
        self.state_path.join("locks").join("install.lock")
    }

    /// Returns the path to a specific package version:
    /// `<state_path>/packages/<name>/<version>`.
    pub fn package_dir(
        &self,
        name: &tau_domain::PackageName,
        version: &tau_domain::Version,
    ) -> PathBuf {
        self.state_path
            .join("packages")
            .join(name.as_str())
            .join(version.to_string())
    }
}

/// Walk up from `cwd` looking for a `.tau/` directory.
/// Returns `Some((scope_root, state_path))` on the first hit or `None` if no `.tau/` is found.
fn walk_up_for_dot_tau(cwd: &Path) -> Option<(PathBuf, PathBuf)> {
    for ancestor in cwd.ancestors() {
        let candidate = ancestor.join(".tau");
        if fs::metadata(&candidate)
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            return Some((ancestor.to_path_buf(), candidate));
        }
    }
    None
}

/// Resolve the global scope path from explicit env values (testable).
///
/// Precedence:
/// 1. `tau_home` if set and non-empty.
/// 2. `<xdg_data_home>/tau` if `xdg_data_home` is set and non-empty.
/// 3. `<home>/.tau`.
///
/// Returns [`ScopeError::HomeNotFound`] if all three are missing/empty.
fn resolve_global_path_from(
    tau_home: Option<std::ffi::OsString>,
    xdg_data_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Result<PathBuf, ScopeError> {
    fn non_empty(s: Option<std::ffi::OsString>) -> Option<std::ffi::OsString> {
        s.filter(|v| !v.is_empty())
    }

    if let Some(p) = non_empty(tau_home) {
        return Ok(PathBuf::from(p));
    }
    if let Some(p) = non_empty(xdg_data_home) {
        return Ok(PathBuf::from(p).join("tau"));
    }
    if let Some(home) = non_empty(home) {
        return Ok(PathBuf::from(home).join(".tau"));
    }
    Err(ScopeError::HomeNotFound)
}

/// Determine the global tau data directory from env vars.
///
/// Precedence:
/// 1. `$TAU_HOME`
/// 2. `$XDG_DATA_HOME/tau`
/// 3. `$HOME/.tau`
fn resolve_global_path() -> Result<PathBuf, ScopeError> {
    resolve_global_path_from(
        env::var_os("TAU_HOME"),
        env::var_os("XDG_DATA_HOME"),
        env::var_os("HOME"),
    )
}

/// Atomically write the default `ScopeConfig` TOML to `target_path`.
///
/// Uses a temp file in the same directory (so rename is atomic on POSIX).
fn write_default_config(target_path: &Path, kind: ScopeKind) -> Result<(), ScopeError> {
    let parent = target_path.parent().ok_or_else(|| ScopeError::Io {
        message: format!(
            "config path has no parent directory: {}",
            target_path.display()
        ),
    })?;

    let content = ScopeConfig::new(kind).to_toml_string()?;

    // Write to a named temp file in the same directory, then rename.
    let tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| ScopeError::Io {
        message: e.to_string(),
    })?;

    fs::write(tmp.path(), content.as_bytes()).map_err(|e| ScopeError::Io {
        message: format!("writing temp file: {e}"),
    })?;

    // Flush to disk before rename so a crash between write and rename
    // leaves the target either non-existent or fully-written, never zero bytes.
    tmp.as_file().sync_all().map_err(|e| ScopeError::Io {
        message: format!("fsync temp file: {e}"),
    })?;

    tmp.persist(target_path).map_err(|e| ScopeError::Io {
        message: format!("persisting config: {}", e.error),
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    fn make_project_with_dot_tau(parent: &Path) -> std::path::PathBuf {
        let project = parent.join("my-project");
        fs::create_dir_all(project.join(".tau")).unwrap();
        project
    }

    #[test]
    fn scope_resolve_finds_dot_tau_in_cwd() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_with_dot_tau(tmp.path());

        let scope = Scope::resolve(&project).unwrap();
        assert_eq!(scope.kind(), ScopeKind::Project);
        assert_eq!(scope.path(), project.as_path());
        assert_eq!(scope.state_path(), project.join(".tau").as_path());
    }

    #[test]
    fn scope_resolve_walks_up_to_find_dot_tau() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_with_dot_tau(tmp.path());
        let nested = project.join("src").join("deep").join("nested");
        fs::create_dir_all(&nested).unwrap();

        let scope = Scope::resolve(&nested).unwrap();
        assert_eq!(scope.kind(), ScopeKind::Project);
        assert_eq!(scope.path(), project.as_path());
    }

    #[test]
    fn scope_resolve_falls_back_to_global_when_no_dot_tau() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().join("no-tau-here");
        fs::create_dir_all(&cwd).unwrap();

        // Use the test-only helper so we never touch real env vars.
        let fake_home = tmp.path().join("fake-tau-home");
        let scope = Scope::resolve_with_fallback(&cwd, fake_home.clone()).unwrap();
        assert_eq!(scope.kind(), ScopeKind::Global);
        assert_eq!(scope.path(), fake_home.as_path());
    }

    #[test]
    fn scope_global_at_uses_passed_path_and_creates_dir() {
        let tmp = TempDir::new().unwrap();
        let fake_tau = tmp.path().join("fake-tau");

        // Use the test-only `global_at` helper to avoid unsafe env mutations.
        let scope = Scope::global_at(fake_tau.clone()).unwrap();
        assert_eq!(scope.kind(), ScopeKind::Global);
        assert_eq!(scope.path(), fake_tau.as_path());
        assert_eq!(scope.state_path(), fake_tau.as_path());
        assert!(fake_tau.is_dir(), "global() should create the directory");
        assert!(
            fake_tau.join("config.toml").is_file(),
            "global() should write a default config.toml"
        );
    }

    #[test]
    fn resolve_global_path_from_prefers_tau_home() {
        use std::ffi::OsString;
        let p = resolve_global_path_from(
            Some(OsString::from("/x/tau-home")),
            Some(OsString::from("/x/xdg")),
            Some(OsString::from("/x/home")),
        )
        .unwrap();
        assert_eq!(p, std::path::Path::new("/x/tau-home"));
    }

    #[test]
    fn resolve_global_path_from_falls_back_to_xdg() {
        use std::ffi::OsString;
        let p = resolve_global_path_from(
            None,
            Some(OsString::from("/x/xdg")),
            Some(OsString::from("/x/home")),
        )
        .unwrap();
        assert_eq!(p, std::path::Path::new("/x/xdg/tau"));
    }

    #[test]
    fn resolve_global_path_from_falls_back_to_home() {
        use std::ffi::OsString;
        let p = resolve_global_path_from(None, None, Some(OsString::from("/x/home"))).unwrap();
        assert_eq!(p, std::path::Path::new("/x/home/.tau"));
    }

    #[test]
    fn resolve_global_path_from_treats_empty_as_unset() {
        use std::ffi::OsString;
        let p = resolve_global_path_from(
            Some(OsString::from("")),
            Some(OsString::from("")),
            Some(OsString::from("/x/home")),
        )
        .unwrap();
        assert_eq!(p, std::path::Path::new("/x/home/.tau"));
    }

    #[test]
    fn resolve_global_path_from_returns_home_not_found_when_all_missing() {
        let err = resolve_global_path_from(None, None, None).unwrap_err();
        assert!(matches!(err, ScopeError::HomeNotFound));
    }

    #[test]
    fn scope_new_project_creates_dot_tau_and_config() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("a-fresh-project");
        fs::create_dir_all(&project).unwrap();

        let scope = Scope::new_project(&project).unwrap();
        assert_eq!(scope.kind(), ScopeKind::Project);
        assert_eq!(scope.path(), project.as_path());
        assert_eq!(scope.state_path(), project.join(".tau").as_path());
        assert!(
            project.join(".tau").is_dir(),
            "new_project should create .tau/"
        );
        assert!(
            project.join(".tau").join("config.toml").is_file(),
            "new_project should write a default config.toml"
        );
    }

    #[test]
    fn scope_new_project_does_not_modify_gitignore() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project-with-gitignore");
        fs::create_dir_all(&project).unwrap();

        // Pre-existing gitignore — verify Scope::new_project leaves it alone.
        let gitignore = project.join(".gitignore");
        fs::write(&gitignore, "target/\n").unwrap();

        let _ = Scope::new_project(&project).unwrap();

        let contents = fs::read_to_string(&gitignore).unwrap();
        assert_eq!(
            contents, "target/\n",
            "Scope::new_project must not modify .gitignore"
        );
    }

    #[test]
    fn scope_path_accessors_global() {
        let scope = Scope {
            path: "/x/global".into(),
            state_path: "/x/global".into(),
            kind: ScopeKind::Global,
        };
        assert_eq!(scope.lockfile_path(), Path::new("/x/global/tau-lock.toml"));
        assert_eq!(scope.config_path(), Path::new("/x/global/config.toml"));
        assert_eq!(scope.packages_dir(), Path::new("/x/global/packages"));
        assert_eq!(
            scope.install_lock_path(),
            Path::new("/x/global/locks/install.lock"),
        );
    }

    #[test]
    fn scope_path_accessors_project() {
        let scope = Scope {
            path: "/proj".into(),
            state_path: "/proj/.tau".into(),
            kind: ScopeKind::Project,
        };
        assert_eq!(scope.lockfile_path(), Path::new("/proj/tau-lock.toml"));
        assert_eq!(scope.config_path(), Path::new("/proj/.tau/config.toml"));
        assert_eq!(scope.packages_dir(), Path::new("/proj/.tau/packages"));
        assert_eq!(
            scope.install_lock_path(),
            Path::new("/proj/.tau/locks/install.lock"),
        );
    }

    #[test]
    fn scope_package_dir_uses_name_and_version() {
        let scope = Scope {
            path: "/proj".into(),
            state_path: "/proj/.tau".into(),
            kind: ScopeKind::Project,
        };
        let name: tau_domain::PackageName = "acme-tool".parse().unwrap();
        let version: tau_domain::Version = "1.2.3".parse().unwrap();
        assert_eq!(
            scope.package_dir(&name, &version),
            Path::new("/proj/.tau/packages/acme-tool/1.2.3"),
        );
    }

    #[test]
    fn scope_kind_serde_lowercase_global() {
        let json = serde_json::to_string(&ScopeKind::Global).unwrap();
        assert_eq!(json, "\"global\"");
    }

    #[test]
    fn scope_kind_serde_lowercase_project() {
        let json = serde_json::to_string(&ScopeKind::Project).unwrap();
        assert_eq!(json, "\"project\"");
    }

    #[test]
    fn scope_config_new_populates_defaults() {
        let cfg = ScopeConfig::new(ScopeKind::Global);
        assert_eq!(cfg.schema_version, 3);
        assert_eq!(cfg.kind, ScopeKind::Global);
        assert_eq!(cfg.created_by_tau_version, env!("CARGO_PKG_VERSION"));
        assert!(cfg.defaults.is_empty());
        assert_eq!(cfg.sandbox, SandboxRequirements::default());
    }

    #[test]
    fn scope_config_round_trips_through_toml() {
        let cfg = ScopeConfig::new(ScopeKind::Project);
        let toml_str = cfg.to_toml_string().unwrap();
        let parsed = ScopeConfig::read_from_str(&toml_str).unwrap();

        assert_eq!(parsed.schema_version, cfg.schema_version);
        assert_eq!(parsed.kind, cfg.kind);
        assert_eq!(parsed.created_by_tau_version, cfg.created_by_tau_version);
        assert!(parsed.defaults.is_empty());

        // SystemTime round-trip via humantime_serde may lose sub-second
        // precision; compare at second granularity.
        let cfg_secs = cfg
            .created_at
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let parsed_secs = parsed
            .created_at
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(parsed_secs, cfg_secs);
    }

    #[test]
    fn scope_config_rejects_too_new_schema_version() {
        let cfg = r#"
            schema_version = 999
            kind = "global"
            created_at = "2026-04-27T10:00:00Z"
            created_by_tau_version = "0.0.0"
        "#;
        let err = ScopeConfig::read_from_str(cfg).unwrap_err();
        assert!(matches!(
            err,
            ScopeError::ConfigSchemaTooNew {
                found: 999,
                supported: 3,
            }
        ));
    }

    #[test]
    fn scope_config_rejects_malformed_toml() {
        let bad = "this is not valid TOML = = =";
        let err = ScopeConfig::read_from_str(bad).unwrap_err();
        assert!(matches!(err, ScopeError::ConfigParse { .. }));
    }

    #[test]
    fn scope_config_defaults_field_optional_in_toml() {
        let cfg = r#"
            schema_version = 1
            kind = "global"
            created_at = "2026-04-27T10:00:00Z"
            created_by_tau_version = "0.0.0"
        "#;
        let parsed = ScopeConfig::read_from_str(cfg).unwrap();
        assert!(parsed.defaults.is_empty());
    }

    // -----------------------------------------------------------------------
    // SandboxRequirements tests (schema v3)
    // -----------------------------------------------------------------------

    #[test]
    fn sandbox_requirements_default_is_strict() {
        let req = SandboxRequirements::default();
        assert_eq!(req.required_tier, SandboxRequiredTier::Strict);
        assert!(req.required_shapes.is_empty());
    }

    #[test]
    fn scope_config_default_sandbox_is_strict() {
        let cfg = ScopeConfig::new(ScopeKind::Project);
        assert_eq!(cfg.schema_version, 3);
        assert_eq!(cfg.sandbox.required_tier, SandboxRequiredTier::Strict);
    }

    #[test]
    fn v3_config_round_trips_through_toml() {
        let cfg = ScopeConfig::new(ScopeKind::Project);
        let toml = cfg.to_toml_string().unwrap();
        let parsed = ScopeConfig::read_from_str(&toml).unwrap();
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn v3_config_with_explicit_required_shapes() {
        let toml = r#"
schema_version = 3
kind = "project"
created_at = "2026-05-04T00:00:00Z"
created_by_tau_version = "0.0.0"

[sandbox]
required_tier = "light"
required_shapes = ["FilesystemRead", "NetworkHttp"]
"#;
        let parsed = ScopeConfig::read_from_str(toml).unwrap();
        assert_eq!(parsed.sandbox.required_tier, SandboxRequiredTier::Light);
        assert_eq!(parsed.sandbox.required_shapes.len(), 2);
    }

    #[test]
    fn v2_config_with_chain_auto_migrates_to_v3() {
        let toml = r#"
schema_version = 2
kind = "project"
created_at = "2026-05-04T00:00:00Z"
created_by_tau_version = "0.0.0"

[sandbox]
chain = [{ kind = "native" }, { kind = "container" }]
minimum_tier = "strict"
"#;
        let parsed = ScopeConfig::read_from_str(toml).unwrap();
        // schema_version is bumped to 3 in memory (next save() writes v3).
        assert_eq!(
            parsed.schema_version, 3,
            "schema_version should bump to 3 in memory"
        );
        assert_eq!(parsed.sandbox.required_tier, SandboxRequiredTier::Strict);
    }

    #[test]
    fn mixed_v2_v3_keys_honor_v3_and_ignore_v2() {
        let toml = r#"
schema_version = 3
kind = "project"
created_at = "2026-05-04T00:00:00Z"
created_by_tau_version = "0.0.0"

[sandbox]
required_tier = "light"
chain = [{ kind = "native" }]
minimum_tier = "strict"
"#;
        let parsed = ScopeConfig::read_from_str(toml).unwrap();
        // v3 required_tier wins; v2 minimum_tier is ignored.
        assert_eq!(parsed.sandbox.required_tier, SandboxRequiredTier::Light);
        assert!(parsed.sandbox.required_shapes.is_empty());
    }

    #[test]
    fn v2_config_without_minimum_tier_defaults_to_strict() {
        let toml = r#"
schema_version = 2
kind = "project"
created_at = "2026-05-04T00:00:00Z"
created_by_tau_version = "0.0.0"

[sandbox]
chain = [{ kind = "native" }]
"#;
        let parsed = ScopeConfig::read_from_str(toml).unwrap();
        assert_eq!(parsed.sandbox.required_tier, SandboxRequiredTier::Strict);
    }

    #[test]
    fn v1_config_loads_with_default_v3_sandbox() {
        let toml = r#"
schema_version = 1
kind = "project"
created_at = "2026-05-04T00:00:00Z"
created_by_tau_version = "0.0.0"
"#;
        let parsed = ScopeConfig::read_from_str(toml).unwrap();
        assert_eq!(parsed.sandbox.required_tier, SandboxRequiredTier::Strict);
    }

    #[test]
    fn schema_too_new_rejected() {
        let toml = r#"
schema_version = 999
kind = "project"
created_at = "2026-05-04T00:00:00Z"
created_by_tau_version = "0.0.0"
"#;
        let err = ScopeConfig::read_from_str(toml).unwrap_err();
        assert!(matches!(
            err,
            ScopeError::ConfigSchemaTooNew {
                found: 999,
                supported: 3
            }
        ));
    }
}
