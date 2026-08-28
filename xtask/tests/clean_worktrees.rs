//! Behavioural gate for `scripts/clean-worktrees.sh`.
//!
//! The script removes `target/` directories from sibling worktrees *outside*
//! its own repository, so the invariant that matters is not "did it free
//! space" but **"is the directory it promised to keep still there"**.
//!
//! That is not a theoretical concern. An ad-hoc version of this sweep, run by
//! hand during #719, printed `keeping <worktree>` for every branch and then
//! removed every `target/` anyway — including the ones it had just named. No
//! work was lost (a `target/` is build output) but the whole workspace paid an
//! unplanned rebuild. The bug was invisible because the script's *output* was
//! correct; only the filesystem disagreed. So these tests assert against the
//! filesystem after the fact, never against stdout.
//!
//! The script is bash rather than an `xtask` subcommand on purpose — it is the
//! recovery tool for a full disk, and `cargo build` is exactly what does not
//! work in that state. Driving it from a Rust test is how it still gets CI
//! coverage without taking on a build dependency at the moment of use.
//!
//! Fixtures are real `git init` repos: the script reads branches with
//! `git symbolic-ref`, and faking that with stub files would test nothing. No
//! commits are needed — `symbolic-ref` reports the branch of an unborn HEAD,
//! which is why the script uses it instead of `rev-parse --abbrev-ref`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A temp directory that cleans itself up, so a failing assertion does not
/// leak multi-worktree fixtures into `/tmp`. xtask is dependency-light
/// (anyhow + clap, no dev-dependencies), so this stands in for `tempfile`.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "tau-clean-worktrees-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create fixture root");
        Self { root }
    }

    /// A worktree on `branch` whose `target/` holds a sentinel file, with the
    /// `target/` mtime pinned to `stamp` (a `touch -t` timestamp) so the
    /// newest-copy rule is deterministic rather than racing on wall-clock.
    fn worktree(&self, name: &str, branch: &str, stamp: &str) -> PathBuf {
        let dir = self.root.join(name);
        fs::create_dir_all(dir.join("target")).expect("create worktree target");
        fs::write(dir.join("target").join("sentinel"), b"build output").expect("write sentinel");

        run_ok(
            Command::new("git")
                .args(["init", "-b", branch])
                .current_dir(&dir),
        );
        run_ok(
            Command::new("touch")
                .args(["-t", stamp])
                .arg(dir.join("target")),
        );
        dir
    }

    fn target_exists(&self, name: &str) -> bool {
        self.root
            .join(name)
            .join("target")
            .join("sentinel")
            .exists()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn run_ok(cmd: &mut Command) {
    let out = cmd.output().expect("spawn");
    assert!(
        out.status.success(),
        "fixture command failed: {:?}\nstderr: {}",
        cmd,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn repo_root() -> PathBuf {
    // xtask/ sits at the workspace root, so its parent is the repo root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a parent (the repo root)")
        .to_path_buf()
}

fn script() -> PathBuf {
    repo_root().join("scripts").join("clean-worktrees.sh")
}

/// Runs the sweep against a fixture root.
///
/// Both escape hatches are set: `SKIP_BUILD_GUARD` because this test itself
/// runs under cargo and would otherwise always trip the "a build is active"
/// refusal, and `SKIP_GH` so the result never depends on network or on the
/// live PR state of a real branch.
fn sweep(root: &Path, extra: &[&str]) -> Output {
    let mut cmd = Command::new(script());
    cmd.arg("--root")
        .arg(root)
        .args(extra)
        .env("TAU_CLEAN_SKIP_BUILD_GUARD", "1")
        .env("TAU_CLEAN_SKIP_GH", "1");
    cmd.output().expect("run clean-worktrees.sh")
}

#[test]
fn dry_run_is_the_default_and_removes_nothing() {
    let fx = Fixture::new("dry");
    fx.worktree("alpha-old", "feat/alpha", "202601010000");
    fx.worktree("alpha-new", "feat/alpha", "202606010000");
    fx.worktree("solo", "feat/solo", "202601010000");

    let out = sweep(&fx.root, &[]);
    assert!(out.status.success(), "dry run should exit 0");

    // The whole point: a default invocation is inert.
    for name in ["alpha-old", "alpha-new", "solo"] {
        assert!(
            fx.target_exists(name),
            "{name}/target must survive a dry run"
        );
    }
}

#[test]
fn apply_keeps_the_newest_copy_and_removes_the_duplicate() {
    let fx = Fixture::new("apply");
    fx.worktree("alpha-old", "feat/alpha", "202601010000");
    fx.worktree("alpha-new", "feat/alpha", "202606010000");
    fx.worktree("solo", "feat/solo", "202601010000");

    let out = sweep(&fx.root, &["--yes"]);
    assert!(
        out.status.success(),
        "sweep failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The regression this file exists for: the kept path is still on disk.
    assert!(
        fx.target_exists("alpha-new"),
        "the most recently built copy of a duplicated lane must be KEPT"
    );
    assert!(
        !fx.target_exists("alpha-old"),
        "the stale duplicate copy must be reclaimed"
    );
    // A lane with a single worktree is not a duplicate of anything.
    assert!(
        fx.target_exists("solo"),
        "a sole worktree on its branch must be KEPT"
    );
}

#[test]
fn refuses_to_sweep_the_home_directory() {
    // Exercises the real guard with zero blast radius: HOME is pointed at the
    // fixture, so `--root <fixture>` is the forbidden `$HOME` case. Passing a
    // literal $HOME instead would delete the developer's build caches if the
    // guard ever regressed.
    let fx = Fixture::new("home");
    fx.worktree("alpha-old", "feat/alpha", "202601010000");
    fx.worktree("alpha-new", "feat/alpha", "202606010000");

    let mut cmd = Command::new(script());
    cmd.arg("--root")
        .arg(&fx.root)
        .arg("--yes")
        .env("HOME", &fx.root)
        .env("TAU_CLEAN_SKIP_BUILD_GUARD", "1")
        .env("TAU_CLEAN_SKIP_GH", "1");
    let out = cmd.output().expect("run clean-worktrees.sh");

    assert_eq!(
        out.status.code(),
        Some(2),
        "sweeping $HOME must be refused with exit 2"
    );
    for name in ["alpha-old", "alpha-new"] {
        assert!(
            fx.target_exists(name),
            "{name}/target must survive a refused sweep"
        );
    }
}

#[test]
fn non_git_directories_are_ignored() {
    let fx = Fixture::new("nongit");
    // A bare directory that merely *looks* like a worktree. Sweeping it would
    // mean deleting something the script cannot reason about at all.
    fs::create_dir_all(fx.root.join("not-a-worktree").join("target")).expect("create decoy");
    fs::write(
        fx.root
            .join("not-a-worktree")
            .join("target")
            .join("sentinel"),
        b"build output",
    )
    .expect("write sentinel");

    let out = sweep(&fx.root, &["--yes"]);
    assert!(out.status.success());
    assert!(
        fx.target_exists("not-a-worktree"),
        "a directory with no .git must never be swept"
    );
}

#[test]
fn never_reclaims_the_worktree_it_was_invoked_from() {
    // A cleanup command that deletes the caller's own warm cache is a nasty
    // surprise, and the real workspace hits this: the worktree you are working
    // in is frequently the older copy of a duplicated lane, so the plain
    // newest-wins rule would target it. `self-copy` is deliberately given the
    // OLDER timestamp so it would be reclaimed if this rule regressed.
    let fx = Fixture::new("self");
    let me = fx.worktree("self-copy", "feat/dup", "202601010000");
    fx.worktree("sibling", "feat/dup", "202606010000");

    let scripts = me.join("scripts");
    fs::create_dir_all(&scripts).expect("create scripts dir");
    let copied = scripts.join("clean-worktrees.sh");
    fs::copy(script(), &copied).expect("copy script into fixture worktree");
    run_ok(Command::new("chmod").arg("+x").arg(&copied));

    let out = Command::new(&copied)
        .arg("--root")
        .arg(&fx.root)
        .arg("--yes")
        .env("TAU_CLEAN_SKIP_BUILD_GUARD", "1")
        .env("TAU_CLEAN_SKIP_GH", "1")
        .output()
        .expect("run copied clean-worktrees.sh");
    assert!(
        out.status.success(),
        "sweep failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        fx.target_exists("self-copy"),
        "the invoking worktree must never be reclaimed, even as the older duplicate"
    );
    assert!(
        !fx.target_exists("sibling"),
        "the other copy of the lane is still reclaimed"
    );
}

#[test]
fn script_is_executable() {
    // The justfile and the docs both invoke it directly; a lost exec bit turns
    // the documented recovery path into a permission error at the worst moment.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(script())
            .expect("stat clean-worktrees.sh")
            .permissions()
            .mode();
        assert!(
            mode & 0o111 != 0,
            "scripts/clean-worktrees.sh must be executable (mode {mode:o})"
        );
    }
}
