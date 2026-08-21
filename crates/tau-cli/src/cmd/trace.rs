//! `tau trace` — open the execution-trace waterfall TUI (M1) on a run's
//! `.tau/runs/<id>.jsonl` file.
//!
//! [`resolve_run`] is the pure (I/O-only, no terminal) path-resolution
//! half — it is what makes `--last` unit-testable without driving
//! [`run_tui`], which owns raw-mode/alternate-screen terminal state and is
//! blocking. [`run`] is the thin impure shell: resolve, then hand the
//! resolved path to [`run_tui`] via [`TraceSource::File`].

use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::cli::TraceArgs;
use crate::tui::{run_tui, TraceSource};

/// Directory (relative to cwd) `tau run` writes JSONL trace logs under —
/// mirrors `tau_runtime_tokio::orchestration::persistence::run_log_path`'s
/// `<scope_root>/.tau/runs/<run_id>.jsonl` layout, rooted at the cwd the
/// same way `tau run`'s scope resolution is (project scope walks up from
/// cwd to find `tau.toml`; `tau trace` mirrors the project-scope default
/// rather than re-resolving the full `tau_pkg::Scope` for a read-only
/// trace viewer).
const RUNS_DIR: &str = ".tau/runs";

/// Run `tau trace`: resolve the run id / `--last` to a path under
/// `./.tau/runs`, then open the TUI on it.
///
/// `run_tui` is synchronous/blocking (owns the terminal's raw-mode read
/// loop); `tau trace` has no concurrent work to interleave it with (unlike
/// `tau run --tui`, which must not stall the tokio reactor), so it is
/// called directly rather than via `spawn_blocking`.
pub async fn run(args: &TraceArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("resolving current directory")?;
    let runs_dir = cwd.join(RUNS_DIR);
    let path = resolve_run(&runs_dir, args.run_id.as_deref(), args.last)?;
    run_tui(TraceSource::File(path))
}

/// Resolve `run_id` / `--last` to a concrete `.jsonl` path under
/// `runs_dir`.
///
/// - `run_id = Some(id)` → `<runs_dir>/<id>.jsonl`; errors (listing
///   available ids) if that exact file does not exist.
/// - `run_id = None, last = true` → the `*.jsonl` file under `runs_dir`
///   with the newest mtime; errors if `runs_dir` has none.
/// - `run_id = None, last = false` → neither was given; errors with a
///   hint to pass a run id or `--last` (clap's `conflicts_with` on
///   `TraceArgs` already rules out both being set at once).
pub(crate) fn resolve_run(
    runs_dir: &Path,
    run_id: Option<&str>,
    last: bool,
) -> anyhow::Result<PathBuf> {
    match (run_id, last) {
        (Some(id), _) => {
            let path = runs_dir.join(format!("{id}.jsonl"));
            if path.is_file() {
                Ok(path)
            } else {
                Err(anyhow::anyhow!(
                    "no run {id:?} found ({} does not exist)\n{}",
                    path.display(),
                    available_runs_hint(runs_dir)
                ))
            }
        }
        (None, true) => newest_run_file(runs_dir).ok_or_else(|| {
            anyhow::anyhow!(
                "--last requested but no run files exist under {}\n{}",
                runs_dir.display(),
                available_runs_hint(runs_dir)
            )
        }),
        (None, false) => Err(anyhow::anyhow!(
            "tau trace requires a run id or --last\n{}",
            available_runs_hint(runs_dir)
        )),
    }
}

/// The `*.jsonl` file directly under `runs_dir` with the newest mtime, or
/// `None` if `runs_dir` doesn't exist or has no `.jsonl` files.
fn newest_run_file(runs_dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(runs_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

/// A `available runs: a, b, c` (or "no run files found ...") hint appended
/// to resolution errors, so a typo'd id or an empty `.tau/runs` tells the
/// operator what actually exists instead of just failing silently.
fn available_runs_hint(runs_dir: &Path) -> String {
    let mut ids: Vec<String> = std::fs::read_dir(runs_dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(|e| e.to_str()) == Some("jsonl"))
                .then(|| path.file_stem().and_then(|s| s.to_str()).map(String::from))
                .flatten()
        })
        .collect();
    ids.sort();
    if ids.is_empty() {
        format!("no run files found under {}", runs_dir.display())
    } else {
        format!("available runs: {}", ids.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    /// Write `names` (each a bare `<id>.jsonl` filename) into a fresh
    /// tempdir, each with a distinct mtime spaced 1s apart in the given
    /// order (last name = newest) — `File::set_modified` gives exact
    /// control so the test doesn't depend on real wall-clock write timing
    /// or filesystem mtime resolution.
    fn tempdir_with_runs(names: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let base = SystemTime::now() - Duration::from_secs(names.len() as u64 + 1);
        for (i, name) in names.iter().enumerate() {
            let path = dir.path().join(name);
            std::fs::write(&path, "{}").unwrap();
            let mtime = base + Duration::from_secs(i as u64 + 1);
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(mtime).unwrap();
        }
        dir
    }

    #[test]
    fn last_resolves_newest_run_file() {
        let dir = tempdir_with_runs(&["01A.jsonl", "01B.jsonl"]); // 01B newer
        let resolved = resolve_run(dir.path(), None, true).unwrap();
        assert_eq!(resolved.file_name().unwrap(), "01B.jsonl");
    }

    #[test]
    fn last_picks_newest_regardless_of_write_order() {
        // Same two files, but 01A is given the later mtime this time —
        // proves the resolver sorts by mtime, not filename or write order.
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("01A.jsonl");
        let b = dir.path().join("01B.jsonl");
        std::fs::write(&b, "{}").unwrap();
        std::fs::write(&a, "{}").unwrap();
        let now = SystemTime::now();
        std::fs::File::options()
            .write(true)
            .open(&b)
            .unwrap()
            .set_modified(now - Duration::from_secs(10))
            .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&a)
            .unwrap()
            .set_modified(now)
            .unwrap();

        let resolved = resolve_run(dir.path(), None, true).unwrap();
        assert_eq!(resolved.file_name().unwrap(), "01A.jsonl");
    }

    #[test]
    fn explicit_run_id_resolves_to_its_jsonl_path() {
        let dir = tempdir_with_runs(&["01A.jsonl", "01B.jsonl"]);
        let resolved = resolve_run(dir.path(), Some("01A"), false).unwrap();
        assert_eq!(resolved, dir.path().join("01A.jsonl"));
    }

    #[test]
    fn missing_explicit_run_id_errors_with_available_hint() {
        let dir = tempdir_with_runs(&["01A.jsonl"]);
        let err = resolve_run(dir.path(), Some("nope"), false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nope"), "got: {msg}");
        assert!(msg.contains("01A"), "got: {msg}");
    }

    #[test]
    fn last_on_empty_dir_errors_with_no_runs_hint() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve_run(dir.path(), None, true).unwrap_err();
        assert!(err.to_string().contains("no run files"), "got: {err}");
    }

    #[test]
    fn neither_id_nor_last_errors() {
        let dir = tempdir_with_runs(&["01A.jsonl"]);
        let err = resolve_run(dir.path(), None, false).unwrap_err();
        assert!(
            err.to_string().contains("requires a run id or --last"),
            "got: {err}"
        );
    }

    #[test]
    fn non_jsonl_files_are_ignored_by_last_and_the_hint() {
        let dir = tempdir_with_runs(&["01A.jsonl"]);
        std::fs::write(dir.path().join("notes.txt"), "hi").unwrap();
        let resolved = resolve_run(dir.path(), None, true).unwrap();
        assert_eq!(resolved.file_name().unwrap(), "01A.jsonl");

        let err = resolve_run(dir.path(), Some("nope"), false).unwrap_err();
        assert!(!err.to_string().contains("notes"), "got: {err}");
    }
}
