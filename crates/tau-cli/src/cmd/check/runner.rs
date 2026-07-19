//! Sequential orchestrator. Builds `CheckCtx` once, runs selected
//! categories one-at-a-time, returns `Vec<CheckResult>`.
//!
//! Tasks 4-9 implement each category. Until then, the runner returns
//! Skipped placeholders for every category.

use super::result::{CheckCategory, CheckResult};
use anyhow::Result;
use std::path::PathBuf;
use std::time::Instant;
use tau_pkg::{project::ProjectConfig, Scope};
use tau_ports::target::TargetTriple;

/// Shared context for all checks. Built once at runner start.
pub struct CheckCtx {
    /// Project root (the directory containing tau.toml).
    pub project_root: PathBuf,
    /// Resolved scope (project or global). Carries lockfile path, config path, etc.
    pub scope: Scope,
    /// Parsed `tau.toml`. None when the file is malformed (the `config` check
    /// reports the error and other checks early-skip).
    pub project: Option<ProjectConfig>,
    /// `--fast` flag passthrough.
    pub fast: bool,
    /// `--target <triple>` passthrough. When set, the sandbox category
    /// validates against the target's documented profile instead of the
    /// locally resolved adapter.
    pub target: Option<TargetTriple>,
}

impl CheckCtx {
    /// Build context from a project path.
    ///
    /// `project_path` may be a directory (TOML-based project) or a `.ts` file
    /// (β.8 TypeScript-based project). File-extension dispatch happens inside
    /// [`crate::cmd::project_load::load_project`].
    pub async fn load(
        project_path: PathBuf,
        fast: bool,
        target: Option<TargetTriple>,
    ) -> Result<Self> {
        // Derive the project root for scope resolution regardless of whether
        // the path is a directory or a .ts file.
        let project_root = if project_path.is_dir() {
            project_path.clone()
        } else {
            project_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| project_path.clone())
        };

        let scope =
            Scope::resolve(&project_root).map_err(|e| anyhow::anyhow!("resolve scope: {e}"))?;
        // Project load may legitimately fail (malformed source). Record
        // None and let the `config` check report the error.
        let project = crate::cmd::project_load::load_project(&project_path)
            .ok()
            .map(|l| l.project);
        Ok(Self {
            project_root,
            scope,
            project,
            fast,
            target,
        })
    }
}

/// Run a list of categories sequentially. Returns one result per category.
pub async fn run_categories(ctx: &CheckCtx, categories: &[CheckCategory]) -> Vec<CheckResult> {
    let mut results = Vec::with_capacity(categories.len());
    for cat in categories {
        let started = Instant::now();
        let result = run_one(ctx, *cat).await;
        results.push(CheckResult {
            duration: started.elapsed(),
            ..result
        });
    }
    results
}

async fn run_one(ctx: &CheckCtx, cat: CheckCategory) -> CheckResult {
    match cat {
        CheckCategory::Config => super::categories::config::run_config(ctx),
        CheckCategory::Lockfile => super::categories::lockfile::run_lockfile(ctx),
        CheckCategory::Packages => super::categories::packages::run_packages(ctx),
        CheckCategory::Sandbox => super::categories::sandbox::run_sandbox(ctx).await,
        CheckCategory::Plugins => super::categories::plugins::run_plugins(ctx).await,
        CheckCategory::Skills => super::categories::skills::run_skills(ctx),
        CheckCategory::McpContracts => super::categories::mcp_contracts::run_mcp_contracts(ctx),
        CheckCategory::Governance => super::categories::governance::run_governance(ctx),
    }
}
