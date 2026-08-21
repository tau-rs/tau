//! Integration tests for `tau update`.
//!
//! Uses local `file://`-based git fixtures with multiple tagged versions,
//! so the suite has no network requirement. Each test sets `TAU_HOME` to
//! an isolated tempdir so global-scope operations do not leak across
//! tests or pollute the developer's real `~/.tau`.
//!
//! Exit code contract (per ADR-0007 §7):
//! - 0: success.
//! - 2: any UpdateError (e.g. version not found, package not installed).
//!
//! Output is a post-call result-summary (the library function is synchronous
//! and returns after all work is done; intermediate progress events would
//! require hooks into the library, deferred).

mod common;

use std::process::Command as StdCommand;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

// ── Multi-version repo fixture ───────────────────────────────────────────────

/// Detect whether `git` is on PATH.
fn git_available() -> bool {
    StdCommand::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Create a bare git repo containing multiple versions of a package,
/// each tagged with a semver tag (v1.0.0, v1.1.0, etc.).
///
/// Returns `(tempdir, bare_url)`.
/// Each tagged commit's tau.toml declares the versioned `file://...#v<version>`
/// source URL, satisfying the source/manifest match check in `install_with_options`.
fn make_multi_version_repo(name: &str, versions: &[&str]) -> (TempDir, String) {
    let dir = TempDir::new().unwrap();
    let parent = dir.path();

    let bare = parent.join(format!("{name}.git"));
    std::fs::create_dir_all(&bare).unwrap();
    common::run_git(&bare, &["init", "--bare", "-q"]);
    common::run_git(&bare, &["symbolic-ref", "HEAD", "refs/heads/main"]);

    let working = parent.join(format!("{name}-working"));
    std::fs::create_dir_all(&working).unwrap();
    common::run_git(&working, &["init", "-q", "-b", "main"]);
    common::run_git(&working, &["config", "user.email", "test@example.com"]);
    common::run_git(&working, &["config", "user.name", "Test User"]);
    common::run_git(
        &working,
        &["remote", "add", "origin", &bare.to_string_lossy()],
    );

    let source_url = common::file_url(&bare);

    for &version in versions {
        let versioned_source_url = format!("{source_url}#v{version}");
        let manifest = format!(
            r#"name = "{name}"
version = "{version}"
description = "Multi-version fixture for update CLI tests"
authors = ["Test <test@example.com>"]
source = "{versioned_source_url}"
kind = "tool"
dependencies = []
capabilities = []
"#
        );
        std::fs::write(working.join("tau.toml"), &manifest).unwrap();
        common::run_git(&working, &["add", "tau.toml"]);
        common::run_git(
            &working,
            &["commit", "-q", "-m", &format!("bump to {version}")],
        );
        common::run_git(&working, &["tag", &format!("v{version}")]);
    }

    // Push all commits + tags.
    common::run_git(&working, &["push", "-q", "origin", "main"]);
    common::run_git(&working, &["push", "-q", "--tags", "origin"]);

    (dir, source_url)
}

/// Install a package at `version` from a multi-version bare repo.
///
/// Returns the `global_dir` path under the tempdir (caller borrows the
/// tempdir's lifetime). The `_dir` return keeps the multi-version tempdir alive.
fn install_version_from_multi_repo(
    pkg_name: &str,
    version: &str,
    versions: &[&str],
) -> (TempDir, TempDir, String) {
    let (repo_dir, bare_url) = make_multi_version_repo(pkg_name, versions);

    let scope_dir = TempDir::new().unwrap();
    let global_dir = scope_dir.path().join("scope-global");
    std::fs::create_dir_all(&global_dir).unwrap();

    // Install the requested version explicitly via a rev pin.
    let versioned_url = format!("{bare_url}#v{version}");
    Command::cargo_bin("tau")
        .unwrap()
        .args(["install", "--global", &versioned_url])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success();

    (repo_dir, scope_dir, bare_url)
}

// ── Tests ────────────────────────────────────────────────────────────────────

/// Test 1: install v1.0.0, then `tau update <pkg>` (no --version).
/// Assert:
///   - exit 0.
///   - stdout contains "Updated update-latest-tool: 1.0.0 → 1.1.0".
///   - new active version = 1.1.0 (verified via `tau list --global`).
///   - old v1.0.0 directory still present (no --prune).
#[test]
fn cmd_update_to_latest_tag() {
    if !git_available() {
        eprintln!("skipping cmd_update_to_latest_tag: `git` not on PATH");
        return;
    }

    let pkg_name = "update-latest-tool";
    let (_repo_dir, scope_dir, _bare_url) =
        install_version_from_multi_repo(pkg_name, "1.0.0", &["1.0.0", "1.1.0"]);
    let global_dir = scope_dir.path().join("scope-global");

    // Run update without a version pin — should pick 1.1.0.
    Command::cargo_bin("tau")
        .unwrap()
        .args(["update", "--global", pkg_name])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success()
        .stdout(predicate::str::contains("Updated update-latest-tool"));

    // Verify old version dir still present (no --prune).
    let v1_dir = global_dir.join("packages/update-latest-tool/1.0.0");
    assert!(
        v1_dir.exists(),
        "v1.0.0 dir should still exist after update without --prune: {}",
        v1_dir.display(),
    );

    // Verify new active version = 1.1.0 via lockfile.
    let lockfile_path = global_dir.join("tau-lock.toml");
    let lockfile = std::fs::read_to_string(&lockfile_path).unwrap();
    assert!(
        lockfile.contains("active_version = \"1.1.0\""),
        "lockfile should record active_version 1.1.0 after update, got:\n{lockfile}",
    );
}

/// Test 2: install v1.0.0, then `tau update <pkg> --version 1.2.0`.
/// Assert:
///   - exit 0.
///   - stdout contains "Updated update-pinned-tool".
///   - new active version = 1.2.0.
#[test]
fn cmd_update_to_specific_version() {
    if !git_available() {
        eprintln!("skipping cmd_update_to_specific_version: `git` not on PATH");
        return;
    }

    let pkg_name = "update-pinned-tool";
    let (_repo_dir, scope_dir, _bare_url) =
        install_version_from_multi_repo(pkg_name, "1.0.0", &["1.0.0", "1.1.0", "1.2.0"]);
    let global_dir = scope_dir.path().join("scope-global");

    Command::cargo_bin("tau")
        .unwrap()
        .args(["update", "--global", pkg_name, "--version", "1.2.0"])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success()
        .stdout(predicate::str::contains("Updated update-pinned-tool"));

    // Verify active_version in lockfile.
    let lockfile = std::fs::read_to_string(global_dir.join("tau-lock.toml")).unwrap();
    assert!(
        lockfile.contains("active_version = \"1.2.0\""),
        "lockfile should record active_version 1.2.0 after pinned update, got:\n{lockfile}",
    );
}

/// Test 3: install v1.0.0, then `tau update <pkg> --prune`.
/// Assert:
///   - exit 0.
///   - stdout contains "Pruned: 1.0.0".
///   - old v1.0.0 directory is gone.
///   - lockfile no longer contains v1.0.0 installed_versions entry.
#[test]
fn cmd_update_with_prune_removes_old() {
    if !git_available() {
        eprintln!("skipping cmd_update_with_prune_removes_old: `git` not on PATH");
        return;
    }

    let pkg_name = "update-prune-tool";
    let (_repo_dir, scope_dir, _bare_url) =
        install_version_from_multi_repo(pkg_name, "1.0.0", &["1.0.0", "1.1.0"]);
    let global_dir = scope_dir.path().join("scope-global");

    Command::cargo_bin("tau")
        .unwrap()
        .args(["update", "--global", pkg_name, "--prune"])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success()
        .stdout(predicate::str::contains("Pruned: 1.0.0"));

    // Old version directory must be gone.
    let v1_dir = global_dir.join("packages/update-prune-tool/1.0.0");
    assert!(
        !v1_dir.exists(),
        "v1.0.0 directory should have been pruned: {}",
        v1_dir.display(),
    );

    // New active version should be present.
    let lockfile = std::fs::read_to_string(global_dir.join("tau-lock.toml")).unwrap();
    assert!(
        lockfile.contains("active_version = \"1.1.0\""),
        "lockfile should record active_version 1.1.0 after prune update, got:\n{lockfile}",
    );

    // v1.0.0 version entry should not appear in installed_versions.
    // The lockfile records [[package.versions]] entries with the version field.
    // After prune there should be no "1.0.0" version block remaining.
    assert!(
        !lockfile.contains("version = \"1.0.0\""),
        "lockfile should not contain v1.0.0 version entry after --prune, got:\n{lockfile}",
    );
}

/// Test 4: install v1.0.0, then `tau update <pkg> --version 9.9.9`.
/// Assert:
///   - exit 2 (UpdateError from library maps to ExitCode::Error).
///   - stderr contains an error mentioning the version or "not found" / "resolv".
#[test]
fn cmd_update_unreachable_version_exits_2() {
    if !git_available() {
        eprintln!("skipping cmd_update_unreachable_version_exits_2: `git` not on PATH");
        return;
    }

    let pkg_name = "update-missing-tool";
    let (_repo_dir, scope_dir, _bare_url) =
        install_version_from_multi_repo(pkg_name, "1.0.0", &["1.0.0", "1.1.0"]);
    let global_dir = scope_dir.path().join("scope-global");

    Command::cargo_bin("tau")
        .unwrap()
        .args(["update", "--global", pkg_name, "--version", "9.9.9"])
        .env("TAU_HOME", &global_dir)
        .assert()
        .failure()
        // ExitCode::Error == 2.
        .code(2)
        // The error message should contain some indication of resolution failure.
        .stderr(predicate::str::contains("9.9.9").or(predicate::str::contains("resolv")));
}
