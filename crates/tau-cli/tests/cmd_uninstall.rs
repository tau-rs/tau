//! Integration tests for `tau uninstall`.
//!
//! Uses local `file://`-based git fixtures (bare repo + working repo
//! pattern) so the suite has no network requirement. Each test sets
//! `TAU_HOME` to an isolated tempdir so global-scope installs/uninstalls
//! do not leak across tests or pollute the developer's real `~/.tau`.

mod common;

use assert_cmd::Command;
use predicates::prelude::*;

/// Test 1: uninstalling a package with no `--version` flag removes all versions,
/// removes the package directory, clears the lockfile entry, and prints the
/// remediation hint pointing users at `[[agents.<id>.requires.tools]]`.
#[test]
fn cmd_uninstall_removes_all_versions() {
    let (fixture, url, _bare) = common::setup_local_package_fixture("remove-me-tool", "1.0.0");
    let global_dir = fixture.path().join("scope-global");
    std::fs::create_dir_all(&global_dir).unwrap();

    // Install the package first.
    Command::cargo_bin("tau")
        .unwrap()
        .args(["install", "--global", &url])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success();

    // Confirm it installed.
    let pkg_dir = global_dir.join("packages/remove-me-tool");
    assert!(pkg_dir.exists(), "package dir should exist after install");

    let lockfile = global_dir.join("tau-lock.toml");
    assert!(lockfile.exists(), "lockfile should exist after install");
    let lockfile_before = std::fs::read_to_string(&lockfile).unwrap();
    assert!(
        lockfile_before.contains("remove-me-tool"),
        "lockfile should contain package entry before uninstall",
    );

    // Uninstall all versions.
    Command::cargo_bin("tau")
        .unwrap()
        .args(["uninstall", "--global", "remove-me-tool"])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success()
        .stdout(predicate::str::contains("Uninstalled remove-me-tool"))
        .stdout(predicate::str::contains("[[agents.<id>.requires.tools]]"));

    // Package directory should be gone.
    assert!(
        !pkg_dir.exists(),
        "package dir should be gone after uninstall: {}",
        pkg_dir.display(),
    );

    // Lockfile entry should be removed (file may still exist but without the entry).
    if lockfile.exists() {
        let lockfile_after = std::fs::read_to_string(&lockfile).unwrap();
        assert!(
            !lockfile_after.contains("remove-me-tool"),
            "lockfile should not contain package entry after full uninstall",
        );
    }
}

/// Test 2: uninstalling a specific version leaves other versions intact and
/// promotes a new active version.
#[test]
fn cmd_uninstall_with_version_keeps_other_versions() {
    // Set up two separate git fixtures for the same package at two versions.
    let (fixture1, url1, _bare1) = common::setup_local_package_fixture("multi-ver-tool", "1.0.0");
    let global_dir = fixture1.path().join("scope-global");
    std::fs::create_dir_all(&global_dir).unwrap();

    // Build a second fixture at 1.1.0. We reuse the fixture1 tempdir so
    // global_dir lives as long as both fixtures do.
    let bare2 = fixture1.path().join("multi-ver-tool-v2.git");
    std::fs::create_dir_all(&bare2).unwrap();
    common::run_git(&bare2, &["init", "--bare", "-q"]);
    common::run_git(&bare2, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    let url2 = common::file_url(&bare2);

    let work2 = fixture1.path().join("multi-ver-tool-v2-work");
    std::fs::create_dir_all(&work2).unwrap();
    common::run_git(&work2, &["init", "-q", "-b", "main"]);
    common::run_git(&work2, &["config", "user.email", "test@example.com"]);
    common::run_git(&work2, &["config", "user.name", "Test User"]);

    let manifest2 = format!(
        r#"name = "multi-ver-tool"
version = "1.1.0"
description = "test fixture v2"
authors = ["Test <test@example.com>"]
source = "{url2}"
kind = "tool"
dependencies = []
capabilities = []
"#
    );
    std::fs::write(work2.join("tau.toml"), &manifest2).unwrap();
    common::run_git(&work2, &["add", "tau.toml"]);
    common::run_git(&work2, &["commit", "-q", "-m", "initial"]);
    common::run_git(
        &work2,
        &["remote", "add", "origin", &bare2.to_string_lossy()],
    );
    common::run_git(&work2, &["push", "-q", "origin", "main"]);

    // Install v1.0.0.
    Command::cargo_bin("tau")
        .unwrap()
        .args(["install", "--global", &url1])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success();

    // Install v1.1.0.
    Command::cargo_bin("tau")
        .unwrap()
        .args(["install", "--global", &url2])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success();

    // Verify both versions are installed.
    let v1_dir = global_dir.join("packages/multi-ver-tool/1.0.0");
    let v2_dir = global_dir.join("packages/multi-ver-tool/1.1.0");
    assert!(
        v1_dir.exists(),
        "v1.0.0 should be installed: {}",
        v1_dir.display()
    );
    assert!(
        v2_dir.exists(),
        "v1.1.0 should be installed: {}",
        v2_dir.display()
    );

    // Uninstall only v1.0.0.
    Command::cargo_bin("tau")
        .unwrap()
        .args([
            "uninstall",
            "--global",
            "multi-ver-tool",
            "--version",
            "1.0.0",
        ])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success()
        .stdout(predicate::str::contains("Uninstalled multi-ver-tool"));

    // v1.0.0 directory should be gone.
    assert!(
        !v1_dir.exists(),
        "v1.0.0 dir should be gone after targeted uninstall: {}",
        v1_dir.display(),
    );

    // v1.1.0 directory should still be present.
    assert!(
        v2_dir.exists(),
        "v1.1.0 dir should still be present: {}",
        v2_dir.display(),
    );

    // Lockfile should still reference the package (with remaining version).
    let lockfile = global_dir.join("tau-lock.toml");
    let lockfile_content = std::fs::read_to_string(&lockfile).unwrap();
    assert!(
        lockfile_content.contains("multi-ver-tool"),
        "lockfile should still reference multi-ver-tool after partial uninstall",
    );
    assert!(
        lockfile_content.contains("1.1.0"),
        "lockfile should still reference v1.1.0",
    );
    assert!(
        !lockfile_content.contains("\"1.0.0\"") && {
            // Also check without surrounding quotes for TOML bare value
            // The lockfile stores the version as a TOML string, e.g. version = "1.0.0"
            // Just confirm 1.0.0 no longer appears as an installed version.
            // Since 1.1.0 contains "1.0.0" as a substring, we need to be careful.
            // Instead check the packages dir directly, which we already did.
            true
        },
        "sanity check",
    );

    // Verify active_version was promoted to 1.1.0 after uninstalling 1.0.0.
    assert!(
        lockfile_content.contains(r#"active_version = "1.1.0""#),
        "lockfile should have active_version promoted to 1.1.0 after uninstalling the old active version",
    );
}

/// Test 3: attempting to uninstall a package that was never installed
/// exits with code 2 and prints an informative error.
#[test]
fn cmd_uninstall_unknown_package_exits_2() {
    let global_dir = tempfile::tempdir().unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args(["uninstall", "--global", "never-installed-package"])
        .env("TAU_HOME", global_dir.path())
        .assert()
        .failure()
        .code(2)
        .stderr(
            predicate::str::contains("not installed")
                .or(predicate::str::contains("never-installed-package")),
        );
}
