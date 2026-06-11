//! Project + agent resolution wrapper for serve mode.
//!
//! Thin orchestration over `tau_pkg::project::*`. Loads a project's
//! `tau.toml`, holds the parsed `ProjectConfig`, and resolves
//! per-call agent ids to `(AgentDefinition, PackageManifest)` pairs
//! via `tau_pkg::project::build_agent_definition`.
//!
//! See ADR-0031 spec §4 and the post-refactor note in the plan.
//!
//! ## Reconciliation note (post-PR #133)
//!
//! The plan assumed `Scope::default_for_cwd` and
//! `AgentResolutionError::AgentNotFound`; neither exists in the actual
//! tau-pkg API. Adaptations made:
//!
//! - `Scope::resolve(root)` is used instead (walk-up from project root,
//!   no side-effecting directory creation).
//! - Unknown-agent detection is done in [`Project::resolve`] by returning
//!   the typed [`ResolveError::AgentNotFound`] variant; the dispatcher
//!   (Task 10) maps that single typed path to JSON-RPC `-32010 Unknown
//!   agent`. There is no string-prefix error contract.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tau_domain::{AgentDefinition, PackageManifest};
use tau_pkg::project::{build_agent_definition, AgentResolutionError, ProjectConfig};
use tau_pkg::Scope;

/// Loaded project state held by a serve-mode process for its lifetime.
///
/// Built once at startup from `--project <path>` (see Task 12).
/// Read-only thereafter. `Arc<Project>` is shared across concurrent
/// dispatcher tasks.
#[derive(Debug, Clone)]
pub struct Project {
    /// Canonical project root (the directory containing tau.toml).
    pub root: PathBuf,
    /// Scope used for agent resolution. Captured at load time.
    pub scope: Scope,
    /// Parsed tau.toml.
    pub config: ProjectConfig,
}

/// Error returned by [`Project::resolve`].
///
/// Typed so the dispatcher routes unknown agents to `-32010 Unknown agent`
/// through one match arm — no string-prefix sniffing.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// `agent_id` is not defined in the project's `tau.toml`.
    #[error("agent {agent_id:?} is not defined in {root}")]
    AgentNotFound {
        /// The unknown agent id, echoed back in the JSON-RPC error data.
        agent_id: String,
        /// Project root, for the human-readable message.
        root: String,
    },
    /// Package resolution failed (not installed, version unsatisfied, manifest
    /// invalid). Flows to `-32006 RUNTIME_ERROR` at the dispatcher.
    #[error(transparent)]
    Resolution(#[from] AgentResolutionError),
}

impl Project {
    /// Load a project from disk.
    ///
    /// Reads + parses `tau.toml`, captures the active `Scope` via
    /// `Scope::resolve` (walk-up from `root`). Does not resolve agents
    /// (that happens lazily per-call via [`Project::resolve`]).
    pub async fn load(root: &Path) -> Result<Self> {
        let root = std::fs::canonicalize(root)
            .with_context(|| format!("canonicalize project root {}", root.display()))?;
        let config = ProjectConfig::from_path(root.join("tau.toml"))
            .with_context(|| format!("parse tau.toml at {}", root.display()))?;
        let scope = Scope::resolve(&root).with_context(|| "compute scope for project root")?;
        Ok(Self {
            root,
            scope,
            config,
        })
    }

    /// Resolve an agent id to `(AgentDefinition, PackageManifest)`.
    ///
    /// Returns [`ResolveError::AgentNotFound`] for an unknown agent id and
    /// [`ResolveError::Resolution`] for package/manifest failures. The
    /// dispatcher maps these to `-32010` and `-32006` respectively.
    pub fn resolve(
        &self,
        agent_id: &str,
    ) -> std::result::Result<(AgentDefinition, PackageManifest), ResolveError> {
        let entry =
            self.config
                .agents
                .get(agent_id)
                .ok_or_else(|| ResolveError::AgentNotFound {
                    agent_id: agent_id.to_string(),
                    root: self.root.display().to_string(),
                })?;
        Ok(build_agent_definition(entry, &self.root, &self.scope)?)
    }

    /// List all agent ids defined in the project's `tau.toml`.
    pub fn agent_ids(&self) -> Vec<String> {
        self.config.agents.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/handshake-only")
    }

    #[tokio::test]
    async fn resolve_unknown_agent_returns_typed_variant() {
        // The handshake-only fixture defines zero agents, so any id is unknown.
        let tau_home = std::env::temp_dir().join("tau-project-unit-test");
        std::fs::create_dir_all(&tau_home).unwrap();
        std::env::set_var("TAU_HOME", &tau_home);

        let project = Project::load(&fixture_dir()).await.expect("load fixture");
        let err = project.resolve("no-such-agent").unwrap_err();
        assert!(
            matches!(err, ResolveError::AgentNotFound { ref agent_id, .. } if agent_id == "no-such-agent"),
            "expected typed AgentNotFound, got: {err:?}"
        );
    }
}
