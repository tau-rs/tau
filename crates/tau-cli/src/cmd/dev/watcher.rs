//! File watcher for `tau dev` — wraps `notify::RecommendedWatcher`.
//!
//! Watches:
//!   - `tau.toml`
//!   - `workflows/*.toml`
//!   - every external prompt file referenced by `[agents.X.prompt] system_file`
//!
//! On any `Modify`, `Create`, or `Remove` event for a watched path,
//! `pending_reload` is set to `true`. The caller must hold the returned
//! [`notify::RecommendedWatcher`] alive — dropping it stops the watcher.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tau_pkg::project::project::{AgentEntry, ProjectConfig, PromptEntry};

/// Spawn a watcher over the project's relevant paths.
///
/// Returns the watcher handle — the caller **MUST** hold it alive.
/// Dropping it stops all file watching.
///
/// On any modify/create/remove event for a watched path,
/// `pending_reload` is set to `true` (Acquire/Release ordering).
pub fn spawn(
    project_root: &Path,
    project: &ProjectConfig,
    pending_reload: Arc<AtomicBool>,
) -> Result<RecommendedWatcher> {
    let paths = resolve_watch_paths(project_root, project);

    let pending_for_callback = pending_reload.clone();
    let mut watcher = RecommendedWatcher::new(
        move |res: std::result::Result<notify::Event, notify::Error>| {
            let Ok(event) = res else { return };
            if matches!(
                event.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
            ) {
                pending_for_callback.store(true, Ordering::Release);
            }
        },
        notify::Config::default(),
    )
    .context("create notify watcher")?;

    for path in paths {
        if path.exists() {
            watcher
                .watch(&path, RecursiveMode::NonRecursive)
                .with_context(|| format!("watch {}", path.display()))?;
        }
    }

    Ok(watcher)
}

/// Resolve the full set of paths to watch for this project:
///   - `<root>/tau.toml`
///   - `<root>/workflows/*.toml` (if directory exists)
///   - external prompt files declared as `prompt.system_file = "..."` in any agent
fn resolve_watch_paths(project_root: &Path, project: &ProjectConfig) -> Vec<PathBuf> {
    let mut paths = vec![project_root.join("tau.toml")];

    // workflows/*.toml
    let workflows_dir = project_root.join("workflows");
    if workflows_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&workflows_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|s| s.to_str()) == Some("toml") {
                    paths.push(p);
                }
            }
        }
    }

    // External prompt files (PromptEntry::File variant).
    // If an agent uses `prompt.system = "..."` (inline), no extra path is added.
    for agent in project.agents.values() {
        if let Some(file_path) = agent_prompt_file(agent) {
            let resolved = if file_path.is_absolute() {
                file_path
            } else {
                project_root.join(file_path)
            };
            paths.push(resolved);
        }
    }

    paths
}

/// Extract the external prompt file path from an agent's `PromptEntry`, if any.
///
/// Returns `None` for `PromptEntry::None` and `PromptEntry::Inline` (the
/// watcher doesn't need to track inline strings). The trailing wildcard arm
/// handles any future variants added to the `#[non_exhaustive]` enum.
fn agent_prompt_file(agent: &AgentEntry) -> Option<PathBuf> {
    match &agent.prompt {
        PromptEntry::File(p) => Some(p.clone()),
        PromptEntry::Inline(_) | PromptEntry::None => None,
        // Forward-compat: future PromptEntry variants default to no file.
        _ => None,
    }
}
