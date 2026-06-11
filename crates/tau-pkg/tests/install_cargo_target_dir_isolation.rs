//! Regression test for D6 (audit/design.md): the build-on-install path
//! for `kind = "rust-cargo"` plugins must locate the produced binary
//! deterministically, **regardless of an inherited `CARGO_TARGET_DIR`**
//! in the parent environment.
//!
//! Background: this workspace's CLAUDE.md requires every cargo
//! invocation to set `CARGO_TARGET_DIR` to a per-agent path. That env
//! var is inherited by child processes, so the inner `cargo build` that
//! `tau-pkg` spawns to compile a plugin would build into the *outer*
//! target dir while `tau-pkg`'s binary-path lookup expected
//! `<package_dir>/target/release/` — a "binary not found after build"
//! foot-gun. The install path must pin the spawned build's target dir to
//! a single source of truth so the produced binary and the lookup can
//! never drift.
//!
//! This test lives in its own integration-test binary and is the only
//! test in it, so the process-global `std::env::set_var` below cannot
//! race other tests (each integration-test file is a separate process).
//!
//! Like the other build-on-install tests, this shells out to `cargo
//! build` against a synthetic fixture and is slow; it skips cleanly when
//! `git` or `cargo` is missing from PATH.

mod fixtures;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use tau_domain::PackageSource;
use tau_pkg::{install_with_options, InstallOptions, Scope};
use tempfile::TempDir;

/// Returns true if both `git --version` and `cargo --version` succeed.
fn cargo_and_git_available() -> bool {
    let git_ok = Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let cargo_ok = Command::new("cargo")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    git_ok && cargo_ok
}

/// Build a bare git repo holding a minimal rust-cargo plugin package
/// (`tau.toml` with a `[plugin]` table, a standalone `Cargo.toml`, and a
/// trivial `src/main.rs`). Returns the bare repo path. Mirrors the
/// fixture builder in `install_builds_rust_cargo_plugin.rs`.
fn make_plugin_fixture_repo(parent: &Path, name: &str, version: &str) -> PathBuf {
    let bare = fixtures::make_bare_repo(parent, name);
    let working = parent.join(format!("{name}-working"));
    std::fs::create_dir_all(&working).unwrap();

    fixtures::run_git_in(&working, &["init", "-q", "-b", "main"]);
    fixtures::run_git_in(&working, &["config", "user.email", "test@example.com"]);
    fixtures::run_git_in(&working, &["config", "user.name", "Test User"]);

    let source_url = fixtures::file_url(&bare);
    let manifest = format!(
        r#"name = "{name}"
version = "{version}"
description = "Synthetic fixture for CARGO_TARGET_DIR isolation test"
authors = ["Test <test@example.com>"]
source = "{source_url}"
kind = "tool"
dependencies = []
capabilities = []

[plugin]
provides = "tool"
kind     = "rust-cargo"
bin      = "{name}"
"#
    );
    std::fs::write(working.join("tau.toml"), manifest).unwrap();

    let cargo_toml = format!(
        r#"[package]
name = "{name}"
version = "{version}"
edition = "2021"

[[bin]]
name = "{name}"
path = "src/main.rs"

[dependencies]
"#
    );
    std::fs::write(working.join("Cargo.toml"), cargo_toml).unwrap();

    std::fs::create_dir_all(working.join("src")).unwrap();
    std::fs::write(working.join("src").join("main.rs"), "fn main() {}\n").unwrap();

    fixtures::run_git_in(&working, &["add", "."]);
    fixtures::run_git_in(&working, &["commit", "-q", "-m", "initial fixture commit"]);
    fixtures::run_git_in(
        &working,
        &["remote", "add", "origin", &bare.to_string_lossy()],
    );
    fixtures::run_git_in(&working, &["push", "-q", "origin", "main"]);

    bare
}

fn install_options_skip_cross_check() -> InstallOptions {
    let mut opts = InstallOptions::default();
    opts.skip_cross_check = true;
    opts
}

/// With a polluted parent `CARGO_TARGET_DIR`, installing a rust-cargo
/// plugin still records the binary under `<package_dir>/target/release/`
/// — not under the inherited outer target dir — and the binary exists.
#[test]
fn install_build_ignores_inherited_cargo_target_dir() {
    if !cargo_and_git_available() {
        eprintln!("skipping: `git` or `cargo` not on PATH");
        return;
    }

    let tmp = TempDir::new().unwrap();

    // Pollute the parent environment exactly as CLAUDE.md's cargo rules
    // would: an absolute, agent-style target dir well away from the
    // installed package tree. If the spawned build inherited this, the
    // binary would land at `<outer>/release/` and tau-pkg's lookup at
    // `<package_dir>/target/release/` would fail with "binary not found".
    let outer_target = tmp.path().join("outer-cargo-target");
    std::fs::create_dir_all(&outer_target).unwrap();
    // SAFETY/race note: this integration-test file contains exactly one
    // test, so it owns its process — no other thread observes this env.
    std::env::set_var("CARGO_TARGET_DIR", &outer_target);

    let project_root = tmp.path().join("tau-home");
    std::fs::create_dir_all(&project_root).unwrap();
    let scope = Scope::new_project(&project_root).unwrap();

    let bare = make_plugin_fixture_repo(tmp.path(), "leak-fixture-plugin", "0.1.0");
    let source = PackageSource::from_str(&fixtures::file_url(&bare)).unwrap();

    let installed =
        install_with_options(&source, &scope, install_options_skip_cross_check()).unwrap();

    let lockfile = tau_pkg::LockFile::load(&scope.lockfile_path()).unwrap();
    let entry = lockfile
        .packages
        .iter()
        .find(|p| p.name.as_str() == "leak-fixture-plugin")
        .expect("lockfile should record leak-fixture-plugin");
    let locked_plugin = entry
        .plugin
        .as_ref()
        .expect("LockedPlugin should be Some for a rust-cargo plugin install");

    let bin_path = &locked_plugin.binary_path;
    assert!(
        bin_path.exists(),
        "binary {} should exist on disk",
        bin_path.display(),
    );

    // The recorded binary must live under the per-package target dir,
    // NOT under the inherited outer CARGO_TARGET_DIR.
    let expected_parent = installed
        .installed_path
        .join("target")
        .join("release")
        .canonicalize()
        .expect("<pkg>/target/release should be canonicalizable");
    assert_eq!(
        bin_path.parent().unwrap(),
        expected_parent,
        "binary should live under <pkg>/target/release/, not the inherited CARGO_TARGET_DIR",
    );

    let canonical_outer = outer_target
        .canonicalize()
        .expect("outer target dir should be canonicalizable");
    assert!(
        !bin_path.starts_with(&canonical_outer),
        "binary {} leaked into the inherited CARGO_TARGET_DIR {}",
        bin_path.display(),
        canonical_outer.display(),
    );
}
