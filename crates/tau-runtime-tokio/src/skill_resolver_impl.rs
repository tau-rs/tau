//! `tau_pkg`-backed implementation of the [`SkillResolver`] port.
//!
//! Wraps `tau_pkg::find_installed_skill` + the SKILL.md read so the
//! kernel can resolve `skill.<name>.spawn` targets through a stable trait
//! without linking `tau-pkg` (which pulls tokio/rustix and does not
//! cross-compile to `wasm32-wasip2`).
//!
//! Construct with [`TauPkgSkillResolver::new`] from the project scope
//! root; the kernel calls `resolve()` each time a `skill.<name>.spawn`
//! virtual tool fires.

use std::path::PathBuf;

use tau_pkg::{find_installed_skill, FindSkillError, Scope};
use tau_ports::{ResolvedSkill, SkillResolveError, SkillResolver};

/// Production skill resolver: resolves a `tau_pkg::Scope` from the
/// project scope root, looks up the installed skill, and reads its
/// SKILL.md body.
///
/// Build once per orchestrated run from the scope root and stuff
/// `Arc<TauPkgSkillResolver>` into `RunOptions.skill_resolver`.
pub struct TauPkgSkillResolver {
    scope_root: PathBuf,
}

impl TauPkgSkillResolver {
    /// Construct from the project scope root (the directory containing
    /// `.tau/`). Scope resolution is deferred to `resolve()` so a bad
    /// root surfaces as a typed `SkillResolveError::Invalid` rather than
    /// a constructor panic.
    pub fn new(scope_root: PathBuf) -> Self {
        Self { scope_root }
    }
}

impl SkillResolver for TauPkgSkillResolver {
    fn resolve(&self, name: &str) -> Result<ResolvedSkill, SkillResolveError> {
        let scope = Scope::resolve(&self.scope_root).map_err(|e| SkillResolveError::Invalid {
            detail: format!("resolving scope at {:?}: {e}", self.scope_root),
        })?;

        let installed = match find_installed_skill(&scope, name) {
            Ok(Some(s)) => s,
            Ok(None) => return Err(SkillResolveError::NotFound),
            Err(FindSkillError::InstallPathMissing { path, .. }) => {
                return Err(SkillResolveError::InstallPathMissing {
                    expected_path: path.display().to_string(),
                });
            }
            Err(e) => {
                return Err(SkillResolveError::Invalid {
                    detail: e.to_string(),
                });
            }
        };

        // Read + parse SKILL.md for the default system prompt.
        let skill_md_path = installed.install_path.join(&installed.skill.content);
        let text =
            std::fs::read_to_string(&skill_md_path).map_err(|e| SkillResolveError::Invalid {
                detail: format!("reading SKILL.md at {skill_md_path:?}: {e}"),
            })?;
        let parsed = tau_domain::parse_skill_md(&text).map_err(|e| SkillResolveError::Invalid {
            detail: format!("parsing SKILL.md: {e}"),
        })?;

        Ok(ResolvedSkill {
            install_path: installed.install_path.display().to_string(),
            capabilities: installed.capabilities,
            system_prompt: parsed.body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Point `$TAU_HOME` at a process-local tempdir so the global-scope
    /// fallback resolves.
    ///
    /// `Scope::resolve` walks up for `.tau/`, finds none, and falls back to
    /// `Scope::global()`, which reads `$TAU_HOME`/`$XDG_DATA_HOME`/`$HOME`. On
    /// Windows CI none are set, so resolution fails with `HomeNotFound`
    /// (surfacing as `SkillResolveError::Invalid`) instead of the intended
    /// `NotFound`. A dedicated tempdir with `config.toml` pre-created, set once,
    /// gives it a real home without racing on the write. Mirrors
    /// `e2e_common::ensure_home_env` (added in #545). See #530.
    fn ensure_home_env() {
        use std::sync::OnceLock;
        static TAU_HOME_INIT: OnceLock<()> = OnceLock::new();
        TAU_HOME_INIT.get_or_init(|| {
            let dir = std::env::temp_dir()
                .join(format!("tau-skill-resolver-test-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("create tau_home tempdir");
            let cfg = dir.join("config.toml");
            if !cfg.exists() {
                let _ = std::fs::write(&cfg, "");
            }
            std::env::set_var("TAU_HOME", dir);
        });
    }

    #[test]
    fn no_lockfile_scope_returns_not_found() {
        // A tempdir with no `.tau/` anywhere up the tree resolves to a
        // global scope whose lockfile does not exist, so
        // `find_installed_skill` returns `Ok(None)` → `NotFound`.
        // This exercises the real `Scope::resolve` + lookup path
        // without requiring an installed skill.
        ensure_home_env();
        let dir = tempfile::tempdir().expect("tempdir");
        let resolver = TauPkgSkillResolver::new(dir.path().to_path_buf());
        let err = resolver.resolve("definitely-not-installed").unwrap_err();
        assert!(matches!(err, SkillResolveError::NotFound));
    }
}
