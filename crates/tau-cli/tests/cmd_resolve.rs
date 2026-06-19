//! Integration tests for `tau resolve`.
//!
//! Pattern: assert_cmd + tempfile-based project tau.toml + `file://`
//! URLs to local git fixtures so tests run offline.

use assert_cmd::Command;
use predicates::prelude::*;
use std::process::Command as StdCommand;
use tempfile::TempDir;

mod common;

/// Set up a local bare-git fixture for a tool with a single tagged version.
/// Mirrors the bare-repo pattern in `common::setup_local_package_fixture`.
///
/// The manifest's `source` field is set to the bare repo's `file://` URL
/// so tau-pkg's source/manifest match check passes.
///
/// Returns (tempdir guard, file:// URL of the bare repo).
fn make_tool_fixture(name: &str, version: &str) -> (TempDir, String) {
    let tempdir = TempDir::new().unwrap();

    // Bare repo (clone target).
    let bare = tempdir.path().join(format!("{name}.git"));
    std::fs::create_dir_all(&bare).unwrap();
    let run_git = |cwd: &std::path::Path, args: &[&str]| {
        let out = StdCommand::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        if !out.status.success() {
            panic!(
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    };
    run_git(&bare, &["init", "--bare", "-q"]);
    run_git(&bare, &["symbolic-ref", "HEAD", "refs/heads/main"]);

    // Build the file:// URL for the bare repo — used as the `source` in the
    // manifest so tau-pkg's source/manifest match check passes.
    let forward = bare
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    let url = if forward.starts_with('/') {
        format!("file://{forward}")
    } else {
        format!("file:///{forward}")
    };

    // Working repo where we author the initial commit.
    let work = tempdir.path().join(format!("{name}-work"));
    std::fs::create_dir_all(&work).unwrap();
    run_git(&work, &["init", "-q", "-b", "main"]);
    run_git(&work, &["config", "user.email", "test@example.com"]);
    run_git(&work, &["config", "user.name", "Test"]);

    let manifest_body = format!(
        r#"name = "{name}"
version = "{version}"
description = "fixture"
authors = []
source = "{url}"
kind = "tool"
dependencies = []
capabilities = []
"#
    );
    std::fs::write(work.join("tau.toml"), manifest_body).unwrap();
    run_git(&work, &["add", "tau.toml"]);
    run_git(&work, &["commit", "-q", "-m", "fixture"]);
    run_git(&work, &["tag", &format!("v{version}")]);
    run_git(&work, &["remote", "add", "origin", &bare.to_string_lossy()]);
    // Push both the branch and all tags so `git ls-remote --tags` on the
    // bare URL returns the version tag.
    run_git(&work, &["push", "-q", "origin", "main"]);
    run_git(&work, &["push", "-q", "origin", "--tags"]);

    (tempdir, url)
}

/// Build a project tau.toml in `proj_dir` with one agent declaring the
/// given requires.tools entries.
fn write_project_toml(proj_dir: &std::path::Path, tools: &[(&str, &str, Option<&str>)]) {
    let mut tools_block = String::new();
    for (name, source, version) in tools {
        tools_block.push_str(&format!(
            "\n[[agents.reviewer.requires.tools]]\nname = \"{name}\"\nsource = \"{source}\"\n"
        ));
        if let Some(v) = version {
            tools_block.push_str(&format!("version = \"{v}\"\n"));
        }
    }
    let toml = format!(
        r#"packages = ["anthropic"]

[project]
name = "demo"

[models]
default = {{ backend = "anthropic", model = "claude-haiku-4-5" }}

[agents.reviewer]
display_name = "Reviewer"
package      = "demo@^0.1"
model        = "default"
{tools_block}"#
    );
    std::fs::write(proj_dir.join("tau.toml"), toml).unwrap();
}

#[test]
fn resolve_with_no_project_fails_with_init_hint() {
    let dir = TempDir::new().unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args(["resolve"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("tau init"));
}

#[test]
fn resolve_dry_run_prints_plan_without_fetching() {
    let work = TempDir::new().unwrap();
    let proj = work.path().join("proj");
    std::fs::create_dir(&proj).unwrap();
    let (_tool_tempdir, tool_url) = make_tool_fixture("missing-tool", "0.1.0");
    write_project_toml(&proj, &[("missing-tool", &tool_url, Some("^0.1"))]);

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["resolve", "--dry-run"])
        .current_dir(&proj)
        .env("TAU_HOME", &proj)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "dry-run should succeed; stderr: {stderr}"
    );
    assert!(
        stderr.contains("[plan]") || stderr.contains("[resolve]"),
        "expected dry-run plan output; stderr was: {stderr}"
    );
    // Confirm no install actually happened: the lockfile under
    // <proj>/.tau/ should be absent or empty.
    let lockfile = proj.join(".tau/tau-lock.toml");
    if lockfile.exists() {
        let body = std::fs::read_to_string(&lockfile).unwrap_or_default();
        assert!(
            !body.contains("missing-tool"),
            "dry-run must not modify the lockfile; lockfile contained:\n{body}"
        );
    }
}

#[test]
fn resolve_no_install_hints_when_deps_missing() {
    let work = TempDir::new().unwrap();
    let proj = work.path().join("proj");
    std::fs::create_dir(&proj).unwrap();
    let (_tool_tempdir, tool_url) = make_tool_fixture("missing-tool", "0.1.0");
    write_project_toml(&proj, &[("missing-tool", &tool_url, Some("^0.1"))]);

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["resolve", "--no-install"])
        .current_dir(&proj)
        .env("TAU_HOME", &proj)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "--no-install should exit non-zero when deps missing; stderr: {stderr}"
    );
    assert!(
        stderr.contains("tau install"),
        "expected `tau install` hint in stderr; was: {stderr}"
    );
}

#[test]
fn resolve_full_install_path_succeeds_against_local_fixture() {
    let work = TempDir::new().unwrap();
    let proj = work.path().join("proj");
    std::fs::create_dir(&proj).unwrap();
    let (_tool_tempdir, tool_url) = make_tool_fixture("missing-tool", "0.1.0");
    write_project_toml(&proj, &[("missing-tool", &tool_url, Some("^0.1"))]);

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["resolve"])
        .current_dir(&proj)
        .env("TAU_HOME", &proj)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "full install path should succeed against local fixture; stderr: {stderr}"
    );
    assert!(
        stderr.contains("[install]"),
        "expected install progress in stderr; was: {stderr}"
    );
}

#[test]
fn resolve_idempotent_on_already_installed_deps() {
    let work = TempDir::new().unwrap();
    let proj = work.path().join("proj");
    std::fs::create_dir(&proj).unwrap();
    let (_tool_tempdir, tool_url) = make_tool_fixture("missing-tool", "0.1.0");
    write_project_toml(&proj, &[("missing-tool", &tool_url, Some("^0.1"))]);

    // First run: install.
    let first = Command::cargo_bin("tau")
        .unwrap()
        .args(["resolve"])
        .current_dir(&proj)
        .env("TAU_HOME", &proj)
        .output()
        .unwrap();
    assert!(first.status.success());

    // Second run: should reuse the lockfile, install nothing.
    let second = Command::cargo_bin("tau")
        .unwrap()
        .args(["resolve"])
        .current_dir(&proj)
        .env("TAU_HOME", &proj)
        .output()
        .unwrap();
    let second_stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        second.status.success(),
        "second resolve should succeed; stderr: {second_stderr}"
    );
    // "0 to fetch" or absence of [install] line both indicate reuse.
    assert!(
        second_stderr.contains("0 to fetch") || !second_stderr.contains("[install]"),
        "second resolve should reuse from lockfile; stderr: {second_stderr}"
    );
}
