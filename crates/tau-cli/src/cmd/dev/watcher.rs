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

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
///
/// `project_file_path` is the original path supplied by the caller (may be
/// a `.ts` file or a directory). When it ends in `.ts`, the watcher tracks
/// the `.ts` file instead of `<root>/tau.toml`.
pub fn spawn(
    project_root: &Path,
    project_file_path: &Path,
    project: &ProjectConfig,
    pending_reload: Arc<AtomicBool>,
) -> Result<RecommendedWatcher> {
    let paths = resolve_watch_paths(project_root, project_file_path, project);

    // The set of files we actually care about, canonicalized so the callback's
    // path matching survives symlinked temp dirs (e.g. macOS `/var` ->
    // `/private/var`). This is the crux of correct filtering: some notify
    // backends (notably FSEvents) watch the *parent directory* of a watched
    // file and deliver events for sibling files too. Without this filter a
    // write to e.g. `tau-lock.toml` — a sibling of the watched `tau.toml` —
    // would spuriously set `pending_reload`. Match each event's paths against
    // this set so only genuine changes to watched files trigger a reload.
    let watched: HashSet<PathBuf> = paths
        .iter()
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
        .collect();

    // Content fingerprint of the watched files at boot. The watcher flips
    // `pending_reload` only when an event *actually changes* watched content
    // — not on metadata-only events (`Modify(Metadata)`), and not on the
    // historical `Create`/`Modify` events that FSEvents replays for the
    // watched file when the watch first registers. Those replayed events
    // otherwise race the boot sequence and cause spurious reloads.
    let last_hash = Arc::new(AtomicU64::new(hash_watched(&watched)));

    let pending_for_callback = pending_reload.clone();
    let watched_for_callback = watched.clone();
    let last_hash_for_callback = last_hash.clone();
    let mut watcher = RecommendedWatcher::new(
        move |res: std::result::Result<notify::Event, notify::Error>| {
            let Ok(event) = res else { return };
            if !matches!(
                event.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
            ) {
                return;
            }
            // Only consider events touching an actually-watched file. Some
            // backends (FSEvents) watch the parent directory and deliver
            // sibling-file events; canonicalize first (matches FSEvents'
            // `/private/…` paths), falling back to the raw path for removes.
            let touches_watched = event.paths.iter().any(|p| {
                let canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
                watched_for_callback.contains(&canon) || watched_for_callback.contains(p)
            });
            if !touches_watched {
                return;
            }
            // Content-based change detection: ignore metadata-only and
            // replayed events whose bytes match what we already have.
            let new_hash = hash_watched(&watched_for_callback);
            if new_hash != last_hash_for_callback.swap(new_hash, Ordering::AcqRel) {
                pending_for_callback.store(true, Ordering::Release);
            }
        },
        notify::Config::default(),
    )
    .context("create notify watcher")?;

    for path in &paths {
        if path.exists() {
            watcher
                .watch(path, RecursiveMode::NonRecursive)
                .with_context(|| format!("watch {}", path.display()))?;
        }
    }

    Ok(watcher)
}

/// Combined content fingerprint of all watched files, order-independent
/// (paths sorted for determinism). A missing file contributes a sentinel so
/// create/remove still register as a change. Used to suppress reloads from
/// metadata-only and replayed filesystem events whose content is unchanged.
fn hash_watched(watched: &HashSet<PathBuf>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut paths: Vec<&PathBuf> = watched.iter().collect();
    paths.sort();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for p in paths {
        p.hash(&mut hasher);
        match std::fs::read(p) {
            Ok(bytes) => bytes.hash(&mut hasher),
            Err(_) => (-1i8).hash(&mut hasher),
        }
    }
    hasher.finish()
}

/// Resolve the full set of paths to watch for this project:
///   - for `.ts` projects: `project_file_path` (the `.ts` source)
///   - for TOML projects: `<root>/tau.toml`
///   - `<root>/workflows/*.toml` (if directory exists)
///   - external prompt files declared as `prompt.system_file = "..."` in any agent
fn resolve_watch_paths(
    project_root: &Path,
    project_file_path: &Path,
    project: &ProjectConfig,
) -> Vec<PathBuf> {
    let ext = project_file_path.extension().and_then(|s| s.to_str());
    let manifest_path = if ext == Some("ts") {
        project_file_path.to_path_buf()
    } else {
        project_root.join("tau.toml")
    };
    let mut paths = vec![manifest_path];

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
