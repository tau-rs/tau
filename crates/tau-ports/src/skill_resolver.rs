//! Skill resolver port — looks up an installed skill by name and
//! returns its install path, declared capabilities, and SKILL.md body.
//!
//! The kernel doesn't know how skills are stored on disk — that's a
//! host-shell concern (tau-runtime-tokio ships a `tau_pkg`-backed impl
//! that reads the scope lockfile + the skill's `tau.toml` + SKILL.md).
//! Embassy/wasm guest shells with no on-disk package store can ship the
//! [`NoSkillResolver`] (always `NotFound`).
//!
//! Routing skill resolution through a port is what lets
//! `tau-runtime-core::Runtime` drive the `skill.<name>.spawn` virtual
//! tool without linking `tau-pkg` (which pulls tokio/rustix and does not
//! cross-compile to `wasm32-wasip2`).

use alloc::string::String;
use alloc::vec::Vec;

use tau_domain::Capability;

/// A resolved installed skill, ready for the kernel's
/// `skill.<name>.spawn` dispatch.
///
/// Produced by [`SkillResolver::resolve`]; the kernel applies
/// `${SKILL_DIR}` substitution, scope narrowing, and the capability
/// subset law to these fields before spawning the child agent.
///
/// `install_path` is a `String` (not `PathBuf`) so the type stays usable
/// in `no_std + alloc` guest shells; host adapters pass
/// `path.display().to_string()`.
#[derive(Debug, Clone)]
pub struct ResolvedSkill {
    /// Absolute path to the installed skill directory, as a display
    /// string. Used as the `${SKILL_DIR}` substitution value.
    pub install_path: String,
    /// Declared capabilities from the skill's manifest (pre-substitution).
    pub capabilities: Vec<Capability>,
    /// The skill's default system prompt — the SKILL.md body, already
    /// read and parsed by the adapter. The kernel uses this unless the
    /// caller supplies a `system_prompt` override.
    pub system_prompt: String,
}

/// Error returned by [`SkillResolver::resolve`].
///
/// Variants map onto the kernel's `OrchestrationError` skill variants so
/// the kernel can surface a typed error without depending on the host's
/// concrete error type (`tau_pkg::FindSkillError`).
#[derive(Debug, Clone)]
pub enum SkillResolveError {
    /// No installed skill matches the requested name.
    NotFound,
    /// A lockfile entry exists but the install path is missing on disk.
    InstallPathMissing {
        /// The expected install path, as a display string.
        expected_path: String,
    },
    /// The skill's manifest or SKILL.md could not be read/parsed, or the
    /// scope itself could not be resolved.
    Invalid {
        /// Human-readable reason.
        detail: String,
    },
}

/// Resolve an installed skill by name.
///
/// Host shells implement this against their on-disk package store
/// (tau-runtime-tokio ships `TauPkgSkillResolver`). Guest shells with no
/// store ship [`NoSkillResolver`].
pub trait SkillResolver: Send + Sync {
    /// Look up `name` and return the resolved skill, or a typed error.
    fn resolve(&self, name: &str) -> Result<ResolvedSkill, SkillResolveError>;
}

/// A [`SkillResolver`] that always reports [`SkillResolveError::NotFound`].
///
/// Ships for guest shells (wasm/embassy) that have no on-disk skill store
/// but still link the kernel. A `skill.<name>.spawn` call then fails
/// gracefully with a skill-not-installed error instead of panicking.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSkillResolver;

impl SkillResolver for NoSkillResolver {
    fn resolve(&self, _name: &str) -> Result<ResolvedSkill, SkillResolveError> {
        Err(SkillResolveError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;

    #[test]
    fn no_skill_resolver_always_not_found() {
        let r: Arc<dyn SkillResolver> = Arc::new(NoSkillResolver);
        let err = r.resolve("anything").expect_err("should be NotFound");
        assert!(matches!(err, SkillResolveError::NotFound));
    }
}
