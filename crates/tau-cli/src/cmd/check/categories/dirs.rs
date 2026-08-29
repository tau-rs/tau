//! `tau check dirs` — flag `[dirs]` definition files that are gitignored.
//!
//! A definition file under a `[dirs]` root that matches a `.gitignore`
//! pattern builds fine locally but is silently absent from clones and CI
//! (nothing tracks it). This category re-scans the declared roots and
//! reports one warning per gitignored definition file.

use std::path::{Path, PathBuf};

use crate::cmd::check::result::{
    CheckCategory, CheckFinding, CheckResult, CheckStatus, FindingLocation, Severity,
};
use crate::cmd::check::runner::CheckCtx;
use serde_json::json;
use tau_pkg::project::dirs::definition_files;

/// Run the `dirs` category. `Ok`-status (findings may still be non-empty —
/// gitignored definitions are warnings, not failures) when the project has
/// no `[dirs]`, git is unavailable, or nothing is ignored. When tau.toml
/// can't be parsed/scanned at all, this category didn't actually run, so
/// it reports `Skipped` — matching sibling categories (`packages`,
/// `sandbox`, `governance`) — rather than a misleading `Ok`. The `config`
/// category owns reporting the parse error itself; we don't double-report
/// the error text, only the fact that we skipped.
pub fn run_dirs(ctx: &CheckCtx) -> CheckResult {
    const MALFORMED: &str = "tau.toml malformed (see config check)";
    let ok_empty = || CheckResult {
        category: CheckCategory::Dirs,
        status: CheckStatus::Ok,
        findings: Vec::new(),
        duration: std::time::Duration::ZERO,
    };
    let skipped = |reason: &str| CheckResult {
        category: CheckCategory::Dirs,
        status: CheckStatus::Skipped {
            reason: reason.to_string(),
        },
        findings: Vec::new(),
        duration: std::time::Duration::ZERO,
    };

    // `ctx.project` is `None` when tau.toml failed to parse.
    let Some(project) = &ctx.project else {
        return skipped(MALFORMED);
    };
    let Some(dirs) = &project.dirs else {
        return ok_empty();
    };

    let files = match definition_files(&ctx.project_root, dirs) {
        Ok(files) => files,
        // Scanning can fail for the same reasons `ProjectConfig::from_path`
        // would (hygiene violations etc.); `config` already surfaces the
        // error text, but this category still didn't run, so: Skipped.
        Err(_) => return skipped(MALFORMED),
    };
    if files.is_empty() {
        return ok_empty();
    }

    let ignored = gitignored_files(&ctx.project_root, &files);
    let findings = ignored
        .into_iter()
        .map(|path| CheckFinding {
            category: CheckCategory::Dirs,
            severity: Severity::Warning,
            rule_id: "tau.dirs.gitignored",
            summary: format!(
                "definition file is gitignored: builds locally but is absent from clones/CI: {}",
                path.display()
            ),
            detail: None,
            location: Some(FindingLocation {
                path: path.clone(),
                line: None,
                column: None,
            }),
            remediation: Some("remove the .gitignore rule or relocate the file".into()),
            structured: json!({"path": path.to_string_lossy()}),
        })
        .collect();

    CheckResult {
        category: CheckCategory::Dirs,
        status: CheckStatus::Ok,
        findings,
        duration: std::time::Duration::ZERO,
    }
}

/// Which of `files` (project-root-relative) are gitignored. Empty when git
/// is unavailable or the root is not a repository (lint silently skips).
fn gitignored_files(project_root: &Path, files: &[PathBuf]) -> Vec<PathBuf> {
    use std::io::Write;
    let mut child = match std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["check-ignore", "--stdin", "-z"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let input: Vec<u8> = files
        .iter()
        .flat_map(|f| {
            f.to_string_lossy()
                .into_owned()
                .into_bytes()
                .into_iter()
                .chain([0u8])
        })
        .collect();
    if child
        .stdin
        .take()
        .and_then(|mut s| s.write_all(&input).ok())
        .is_none()
    {
        return Vec::new();
    }
    let out = match child.wait_with_output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    // exit 0 = some ignored, 1 = none, 128 = not a repo / error → skip
    out.stdout
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| PathBuf::from(String::from_utf8_lossy(s).into_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gitignored_definitions_flagged() {
        let t = tempfile::TempDir::new().unwrap();
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(t.path())
            .arg("init")
            .arg("-q")
            .status();
        if !ok.map(|s| s.success()).unwrap_or(false) {
            eprintln!("SKIP: git unavailable");
            return;
        }
        std::fs::write(t.path().join(".gitignore"), "agents/scratch.md\n").unwrap();
        std::fs::create_dir_all(t.path().join("agents")).unwrap();
        let files = [
            std::path::PathBuf::from("agents/scratch.md"),
            std::path::PathBuf::from("agents/kept.md"),
        ];
        let ignored = gitignored_files(t.path(), &files);
        assert_eq!(ignored, vec![std::path::PathBuf::from("agents/scratch.md")]);
    }

    #[test]
    fn no_git_repo_skips_silently() {
        let t = tempfile::TempDir::new().unwrap();
        let files = [std::path::PathBuf::from("agents/a.md")];
        let ignored = gitignored_files(t.path(), &files);
        assert!(ignored.is_empty());
    }

    #[test]
    fn run_dirs_ok_when_no_dirs_table() {
        let t = tempfile::TempDir::new().unwrap();
        std::fs::write(
            t.path().join("tau.toml"),
            "[project]\nname = \"p\"\n[allow]\n",
        )
        .unwrap();
        let ctx = test_ctx(t.path());
        let result = run_dirs(&ctx);
        assert_eq!(result.status, CheckStatus::Ok);
        assert!(result.findings.is_empty());
    }

    #[test]
    fn run_dirs_skipped_when_project_failed_to_parse() {
        use tau_pkg::Scope;
        let t = tempfile::TempDir::new().unwrap();
        // No tau.toml on disk at all — simulates a malformed/missing
        // project file. Built directly (not via `test_ctx`, which now
        // panics loudly on an unparseable fixture) since `ctx.project =
        // None` is exactly the state under test here, not a fixture bug.
        // config category owns reporting the error text, but dirs itself
        // did not run, so it must report Skipped (not a misleading Ok) —
        // matches packages/sandbox/governance precedent.
        let ctx = CheckCtx {
            project_root: t.path().to_path_buf(),
            scope: Scope::resolve(t.path()).expect("resolve scope"),
            project: None,
            fast: false,
            target: None,
        };
        let result = run_dirs(&ctx);
        assert_eq!(
            result.status,
            CheckStatus::Skipped {
                reason: "tau.toml malformed (see config check)".into()
            }
        );
        assert!(result.findings.is_empty());
    }

    #[test]
    fn run_dirs_warns_on_gitignored_definition() {
        let t = tempfile::TempDir::new().unwrap();
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(t.path())
            .arg("init")
            .arg("-q")
            .status();
        if !ok.map(|s| s.success()).unwrap_or(false) {
            eprintln!("SKIP: git unavailable");
            return;
        }
        std::fs::write(t.path().join(".gitignore"), "agents/scratch.md\n").unwrap();
        std::fs::create_dir_all(t.path().join("agents")).unwrap();
        // `model: fast` is required: full `ProjectConfig::validate()` (unlike
        // the bare `scan_dirs` used in tau-pkg's own tests) rejects an agent
        // with no resolvable model alias (`MissingAgentModel`), so the
        // fixture's `tau.toml` below must declare `[models] fast = { .. }`
        // with `backend` matching this agent's `package` name (`p`).
        std::fs::write(
            t.path().join("agents/scratch.md"),
            "---\ndisplay_name: A\npackage: p@^1\nmodel: fast\n---\nbody\n",
        )
        .unwrap();
        std::fs::write(
            t.path().join("agents/kept.md"),
            "---\ndisplay_name: A\npackage: p@^1\nmodel: fast\n---\nbody\n",
        )
        .unwrap();
        // With `[allow]` present, models must live under `[allow.models]`
        // rather than a top-level `[models]` table (ADR-0057).
        std::fs::write(
            t.path().join("tau.toml"),
            "[project]\nname = \"p\"\n[allow]\n[allow.models.fast]\nbackend = \"p\"\nmodel = \"model-v1\"\n[dirs]\nagents = \"agents\"\n",
        )
        .unwrap();
        let ctx = test_ctx(t.path());
        let result = run_dirs(&ctx);
        assert_eq!(result.status, CheckStatus::Ok);
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].severity, Severity::Warning);
        assert_eq!(result.findings[0].rule_id, "tau.dirs.gitignored");
        // The summary embeds a natively-joined path, so build the expected
        // rendering the same way rather than hardcoding a separator — on
        // Windows this is `agents\scratch.md`. See #741.
        let expected = std::path::Path::new("agents").join("scratch.md");
        assert!(
            result.findings[0]
                .summary
                .contains(&expected.display().to_string()),
            "{}",
            result.findings[0].summary
        );
    }

    /// Builds a `CheckCtx` from a real `tau.toml` on disk. Fails loudly
    /// (`panic!` with the real `ProjectConfigError`) rather than silently
    /// degrading to `ctx.project = None` on a bad fixture — a prior version
    /// used `.ok()` here, which turned an invalid test fixture into a
    /// silent `None` and cost a full debugging cycle chasing a
    /// `left: 0, right: 1` failure that was actually "the fixture doesn't
    /// parse." Callers that specifically want the `None` case (simulating a
    /// malformed tau.toml) construct `CheckCtx` directly instead of going
    /// through this helper — see `run_dirs_skipped_when_project_failed_to_parse`.
    fn test_ctx(project_root: &Path) -> CheckCtx {
        use tau_pkg::{project::ProjectConfig, Scope};
        let tau_toml = project_root.join("tau.toml");
        let project = Some(
            ProjectConfig::from_path(&tau_toml)
                .unwrap_or_else(|e| panic!("test fixture failed to parse: {e}")),
        );
        let scope = Scope::resolve(project_root).expect("resolve scope");
        CheckCtx {
            project_root: project_root.to_path_buf(),
            scope,
            project,
            fast: false,
            target: None,
        }
    }
}
