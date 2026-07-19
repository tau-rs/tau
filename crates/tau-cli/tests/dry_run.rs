//! Cross-cutting: `--dry-run` invariants per supported subcommand.
//!
//! For every subcommand that supports the flag (`init`, `install`,
//! `run`, `chat`), this file asserts:
//!
//! 1. Exit code `0` on a would-succeed path.
//! 2. `[dry-run]` prefix appears on stderr.
//! 3. No on-disk state change (snapshot of relevant directory entries
//!    before/after).
//!
//! Some assertions overlap with what's already in the per-command
//! suites (e.g. `cmd_install.rs::install_dry_run_does_not_write`).
//! That overlap is intentional: this file is a regression net for the
//! `--dry-run` *contract* and runs alongside the per-command coverage.

mod common;

use std::collections::BTreeSet;
use std::path::Path;

use assert_cmd::Command as AssertCmd;
use predicates::prelude::*;

/// Snapshot the immediate-children file/directory names of `path`.
/// Used to compare before/after for "no state change" assertions.
fn snapshot_dir(path: &Path) -> BTreeSet<String> {
    if !path.exists() {
        return BTreeSet::new();
    }
    std::fs::read_dir(path)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

// ---- init -------------------------------------------------------------------

#[test]
fn init_dry_run_no_disk_change() {
    let dir = common::temp_project();
    let before = snapshot_dir(dir.path());

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["init", "--dry-run"])
        .current_dir(dir.path())
        .assert()
        .success()
        .code(0)
        .stderr(predicate::str::contains("[dry-run]"));

    let after = snapshot_dir(dir.path());
    assert_eq!(
        before, after,
        "init --dry-run must not change the cwd; before={before:?} after={after:?}"
    );
    assert!(
        !dir.path().join("tau.toml").exists(),
        "init --dry-run must not write tau.toml"
    );
}

// ---- install ----------------------------------------------------------------

#[test]
fn install_dry_run_no_disk_change() {
    let (fixture, url, _bare) = common::setup_local_package_fixture("hello-tool", "0.1.0");
    let global_dir = fixture.path().join("scope-global");
    std::fs::create_dir_all(&global_dir).unwrap();

    // Tickle the scope first so `tau-pkg` writes its default
    // `config.toml`; otherwise the dry-run install would create that
    // file as a side-effect of `Scope::global()` and trip the
    // before/after snapshot. The before/after invariant we care about
    // here is that the install step itself doesn't create a
    // `packages/` tree or a `tau-lock.toml`.
    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["list", "--global"])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success();

    let before = snapshot_dir(&global_dir);

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["install", "--global", "--dry-run", &url])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success()
        .code(0)
        .stderr(predicate::str::contains("[dry-run]"));

    let after = snapshot_dir(&global_dir);
    assert_eq!(
        before, after,
        "install --dry-run must not write to the scope; before={before:?} after={after:?}"
    );
    assert!(
        !global_dir.join("packages").exists(),
        "install --dry-run must not create packages/ tree"
    );
    assert!(
        !global_dir.join("tau-lock.toml").exists(),
        "install --dry-run must not write the lockfile"
    );
}

// ---- run --------------------------------------------------------------------

#[test]
fn run_dry_run_no_disk_change() {
    // --dry-run short-circuits before plugin loading, but we still
    // synthesize a real echo-plugin fixture so the project / lockfile
    // shapes are realistic for the no-disk-change snapshot.
    let dir = common::setup_echo_project("echo", "canned_text = \"unused\"\n", &[]);
    let before = snapshot_dir(dir.path());
    let lockfile_before = std::fs::read_to_string(dir.path().join("tau-lock.toml")).unwrap();

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["run", "echo", "hi", "--dry-run", "--no-governance"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .assert()
        .success()
        .code(0)
        .stderr(predicate::str::contains("[dry-run]"));

    let after = snapshot_dir(dir.path());
    assert_eq!(
        before, after,
        "run --dry-run must not change project root; before={before:?} after={after:?}"
    );
    let lockfile_after = std::fs::read_to_string(dir.path().join("tau-lock.toml")).unwrap();
    assert_eq!(
        lockfile_before, lockfile_after,
        "run --dry-run must not mutate the lockfile"
    );
}

// ---- chat -------------------------------------------------------------------

#[test]
fn chat_dry_run_no_disk_change() {
    let dir = common::setup_echo_project("echo", "canned_text = \"unused\"\n", &[]);
    let before = snapshot_dir(dir.path());
    let global_dir = dir.path().join("global");

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["chat", "echo", "--dry-run"])
        .current_dir(dir.path())
        .env("TAU_HOME", &global_dir)
        .assert()
        .success()
        .code(0)
        .stderr(predicate::str::contains("[dry-run]"));

    let after = snapshot_dir(dir.path());
    assert_eq!(
        before, after,
        "chat --dry-run must not change project root; before={before:?} after={after:?}"
    );
}

// ---- would-fail dry-run -----------------------------------------------------

#[test]
fn install_dry_run_would_fail_url_is_two() {
    // `--dry-run` doesn't suppress validation: a bogus URL still fails
    // at parse-time and the would-fail dry-run exits 2 (not 0).
    let global_dir = tempfile::tempdir().unwrap();

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["install", "--global", "--dry-run", "not-a-url"])
        .env("TAU_HOME", global_dir.path())
        .assert()
        .failure()
        .code(2);
}
