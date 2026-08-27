//! File watcher for `tau dev` — wraps `notify::RecommendedWatcher`.
//!
//! Watches:
//!   - `tau.toml`
//!   - `workflows/*.toml`
//!   - every external prompt file referenced by `[agents.X.prompt] system_file`
//!   - every `[dirs]` root (`agents`/`tools`), recursively
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
    let dir_roots = resolve_watch_dirs(project_root, project);

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

    // `[dirs]` roots, canonicalized the same way as `watched` above. These
    // are matched by prefix rather than exact equality — any path under a
    // watched root (including nested subdirectories and moved/renamed
    // files, which notify reports as a remove+add pair) is in scope.
    let watched_dirs: Vec<PathBuf> = dir_roots
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

    // `[dirs]`-subtree analogue of `last_hash` above, for the same reason:
    // FSEvents replays historical Create/Modify events for files that
    // already existed under a root when the recursive watch registers, and
    // under load (nextest parallelism) those replays race the boot
    // sequence — see `hash_watched`'s doc comment / commit 6d1a8552 (#363),
    // which fixed the identical race for the flat file watchlist. Real
    // projects have pre-existing files under `agents/`/`tools/` at
    // registration time, so this is the common case, not an edge case.
    // Fingerprint is `(path, len, mtime)` per file, not file bytes: cheap
    // enough to recompute on every dir event (a `stat`, not a `read`, per
    // file) while still detecting adds/removes/edits (mtime and/or length
    // change on write).
    let last_dir_hash = Arc::new(AtomicU64::new(hash_watched_dirs(&watched_dirs)));

    let pending_for_callback = pending_reload.clone();
    let watched_for_callback = watched.clone();
    let watched_dirs_for_callback = watched_dirs.clone();
    let last_hash_for_callback = last_hash.clone();
    let last_dir_hash_for_callback = last_dir_hash.clone();
    let mut watcher = RecommendedWatcher::new(
        move |res: std::result::Result<notify::Event, notify::Error>| {
            let Ok(event) = res else { return };
            if !matches!(
                event.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
            ) {
                return;
            }
            // Canonicalize first (matches FSEvents' `/private/…` paths),
            // falling back to the raw path for removes (the file is gone,
            // so canonicalize fails).
            let canon_paths: Vec<PathBuf> = event
                .paths
                .iter()
                .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
                .collect();

            // Case 1: an exact watched file (tau.toml, workflows/*.toml,
            // an external prompt file). Some backends (FSEvents) watch the
            // parent directory and deliver sibling-file events too, so this
            // exact-match filter is load-bearing — without it a write to
            // e.g. `tau-lock.toml` (a sibling of watched `tau.toml`) would
            // spuriously set `pending_reload`.
            let touches_watched_file = event.paths.iter().zip(&canon_paths).any(|(p, canon)| {
                watched_for_callback.contains(canon) || watched_for_callback.contains(p)
            });

            // Case 2: any path under a watched `[dirs]` root. A move/rename
            // inside the root surfaces as a remove+add pair; both events'
            // paths land under the root, so both branches reach here and
            // both flip `pending_reload`.
            let touches_watched_dir = canon_paths.iter().any(|canon| {
                watched_dirs_for_callback
                    .iter()
                    .any(|d| canon.starts_with(d))
            });

            if !touches_watched_file && !touches_watched_dir {
                return;
            }

            if touches_watched_dir {
                // Metadata-based change detection, same purpose as the
                // content-hash check below but scoped to `[dirs]`
                // subtrees: ignore the FSEvents at-registration replay of
                // Create events for files that already existed at boot
                // (their (path, len, mtime) triple is unchanged from the
                // initial snapshot), while still catching real adds,
                // removes, edits, and renames (renames change the set of
                // present paths, so the fingerprint changes regardless of
                // which half of the remove+add pair is processed first).
                let new_dir_hash = hash_watched_dirs(&watched_dirs_for_callback);
                if new_dir_hash != last_dir_hash_for_callback.swap(new_dir_hash, Ordering::AcqRel) {
                    pending_for_callback.store(true, Ordering::Release);
                }
                return;
            }

            // Content-based change detection for exact-file matches: ignore
            // metadata-only and replayed events whose bytes match what we
            // already have.
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

    for dir in &dir_roots {
        if dir.exists() {
            watcher
                .watch(dir, RecursiveMode::Recursive)
                .with_context(|| format!("watch {}", dir.display()))?;
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

/// Combined `(path, len, mtime)` fingerprint of every file under the given
/// `[dirs]` roots, order-independent (paths sorted for determinism).
///
/// This is the `[dirs]`-subtree analogue of `hash_watched`: same boot-race
/// purpose (see the call site's comment), but hashing file *metadata*
/// rather than *bytes* — reading every file under `agents/`/`tools/` on
/// every fs event would be needless I/O for a category whose purpose is
/// just "did anything under this subtree change", and metadata already
/// answers that (a write changes length and/or mtime; an add/remove/rename
/// changes which paths are present at all).
fn hash_watched_dirs(dirs: &[PathBuf]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut entries: Vec<(PathBuf, u64, Option<std::time::Duration>)> = Vec::new();
    for root in dirs {
        collect_dir_snapshot(root, &mut entries);
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (path, len, mtime) in entries {
        path.hash(&mut hasher);
        len.hash(&mut hasher);
        mtime.hash(&mut hasher);
    }
    hasher.finish()
}

/// Recursively collect `(path, len, mtime-since-epoch)` for every file
/// under `dir`, appending to `out`. Unreadable directories/entries are
/// silently skipped (mirrors `resolve_watch_paths`' `.flatten()` pattern
/// elsewhere in this file) — a transient stat failure just means that
/// entry doesn't contribute to the fingerprint this round, which is no
/// worse than the file not existing yet.
fn collect_dir_snapshot(dir: &Path, out: &mut Vec<(PathBuf, u64, Option<std::time::Duration>)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_dir_snapshot(&path, out);
        } else if file_type.is_file() {
            if let Ok(meta) = entry.metadata() {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
                out.push((path, meta.len(), mtime));
            }
        }
    }
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

/// `[dirs]` roots to watch recursively (empty when the project has none).
///
/// Unlike [`resolve_watch_paths`] these are directories, watched with
/// [`RecursiveMode::Recursive`] so any file added, removed, or edited
/// anywhere under an `agents`/`tools` root triggers a reload — including
/// newly created subdirectories, which a flat file-watch list can't express.
fn resolve_watch_dirs(project_root: &Path, project: &ProjectConfig) -> Vec<PathBuf> {
    let Some(dirs) = &project.dirs else {
        return Vec::new();
    };
    [dirs.agents.as_ref(), dirs.tools.as_ref()]
        .into_iter()
        .flatten()
        .map(|rel| project_root.join(rel))
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_paths_include_dirs_roots() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::write(
            root.join("tau.toml"),
            "[project]\nname = \"p\"\n[dirs]\nagents = \"agents\"\n",
        )
        .expect("write tau.toml");
        std::fs::create_dir_all(root.join("agents")).expect("mkdir agents");

        let project =
            ProjectConfig::from_path(root.join("tau.toml")).expect("project must validate");
        assert!(project.dirs.is_some(), "expected [dirs] to be populated");

        let dirs = resolve_watch_dirs(root, &project);
        assert_eq!(dirs, vec![root.join("agents")]);
    }

    #[test]
    fn watch_paths_include_both_dirs_roots_when_both_declared() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::write(
            root.join("tau.toml"),
            "[project]\nname = \"p\"\n[dirs]\nagents = \"agents\"\ntools = \"defs/tools\"\n",
        )
        .expect("write tau.toml");
        std::fs::create_dir_all(root.join("agents")).expect("mkdir agents");
        std::fs::create_dir_all(root.join("defs/tools")).expect("mkdir defs/tools");

        let project =
            ProjectConfig::from_path(root.join("tau.toml")).expect("project must validate");

        let dirs = resolve_watch_dirs(root, &project);
        assert_eq!(
            dirs,
            vec![root.join("agents"), root.join("defs/tools")],
            "both declared roots must be watched"
        );
    }

    #[test]
    fn watch_paths_empty_when_no_dirs_declared() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::write(root.join("tau.toml"), "[project]\nname = \"p\"\n").expect("write tau.toml");

        let project =
            ProjectConfig::from_path(root.join("tau.toml")).expect("project must validate");
        assert!(project.dirs.is_none());

        let dirs = resolve_watch_dirs(root, &project);
        assert!(dirs.is_empty(), "no [dirs] means nothing to watch");
    }
}
