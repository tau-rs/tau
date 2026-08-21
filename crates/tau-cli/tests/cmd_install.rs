//! Integration tests for `tau install`.
//!
//! Uses local `file://`-based git fixtures (bare repo + working repo
//! pattern from sub-project 3 / tau-pkg) so the suite has no network
//! requirement. Each test sets `TAU_HOME` to an isolated tempdir so
//! global-scope installs do not leak across tests or pollute the
//! developer's real `~/.tau`.
//!
//! Fixture builders live in `tests/common/mod.rs` (Task 15) so the
//! `tau list` and cross-cutting suites can share them.
//!
//! Fixes preserved from sub-project 3 Task 14:
//! - `git symbolic-ref HEAD refs/heads/main` on the bare repo so the
//!   downstream clone checks out the right branch regardless of the
//!   host's `init.defaultBranch` (CI runners default to `master`).
//! - tau-pkg's install path threads `protocol.file.allow=always` for
//!   CVE-2022-39253 mitigation; tests rely on that being plumbed.

mod common;

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn install_local_file_url_writes_to_global_scope() {
    let (fixture, url, _bare) = common::setup_local_package_fixture("hello-tool", "0.1.0");
    let global_dir = fixture.path().join("scope-global");
    std::fs::create_dir_all(&global_dir).unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args(["install", "--global", &url])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success()
        .stdout(predicate::str::contains("hello-tool"))
        .stdout(predicate::str::contains("0.1.0"))
        .stdout(predicate::str::contains("global"));

    // Verify the package directory exists at the canonical scope path.
    let pkg_dir = global_dir.join("packages/hello-tool/0.1.0");
    assert!(
        pkg_dir.exists(),
        "package not at expected scope path: {}",
        pkg_dir.display(),
    );
    // And that its tau.toml made it through.
    assert!(
        pkg_dir.join("tau.toml").is_file(),
        "tau.toml missing in installed package: {}",
        pkg_dir.display(),
    );
}

#[test]
fn install_dry_run_does_not_write() {
    let (fixture, url, _bare) = common::setup_local_package_fixture("hello-tool", "0.1.0");
    let global_dir = fixture.path().join("scope-global");
    std::fs::create_dir_all(&global_dir).unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args(["install", "--global", "--dry-run", &url])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success()
        .stderr(predicate::str::contains("[dry-run]"))
        .stderr(predicate::str::contains("would install"))
        .stderr(predicate::str::contains("hello-tool"))
        .stderr(predicate::str::contains("0.1.0"))
        .stderr(predicate::str::contains("kind:"))
        .stderr(predicate::str::contains("tool"))
        .stderr(predicate::str::contains("no changes written."));

    // Dry-run must NOT create the package directory.
    let pkg_dir = global_dir.join("packages/hello-tool/0.1.0");
    assert!(
        !pkg_dir.exists(),
        "dry-run should not write package dir: {}",
        pkg_dir.display(),
    );
    // And no lockfile entry.
    let lockfile = global_dir.join("tau-lock.toml");
    assert!(
        !lockfile.exists(),
        "dry-run should not write lockfile: {}",
        lockfile.display(),
    );
}

#[test]
fn install_bad_url_fails_with_exit_2() {
    let global_dir = tempfile::tempdir().unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args(["install", "--global", "not-a-url"])
        .env("TAU_HOME", global_dir.path())
        .assert()
        .failure()
        .code(2);
}

#[test]
fn install_json_output_includes_name_version_scope_path() {
    let (fixture, url, _bare) = common::setup_local_package_fixture("hello-tool", "0.1.0");
    let global_dir = fixture.path().join("scope-global");
    std::fs::create_dir_all(&global_dir).unwrap();

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["install", "--global", "--json", &url])
        .env("TAU_HOME", &global_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "tau install --json failed: stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("install --json should emit a JSON object");
    assert_eq!(parsed["name"], "hello-tool");
    assert_eq!(parsed["version"], "0.1.0");
    assert_eq!(parsed["scope"], "global");
    assert!(parsed["path"].is_string());
}
