//! Integration tests covering the build-on-install path for
//! `kind = "rust-cargo"` plugin packages.
//!
//! Three scenarios:
//!
//! 1. Success path — installing a fixture rust-cargo plugin runs
//!    `cargo build --release --bin <bin>` and records the resulting
//!    [`tau_pkg::LockedPlugin`] in the lockfile.
//! 2. Compile failure path — a deliberately broken fixture surfaces
//!    [`tau_pkg::InstallError::BuildFailed`] with cargo's exit status
//!    and the tail of cargo's stderr.
//! 3. Lockfile v1 → v2 auto-upgrade — a hand-written v1 lockfile is
//!    loaded, augmented by a non-plugin install, and re-saved as v2
//!    with `plugin = None` on legacy entries.
//!
//! These tests shell out to `cargo build` against a workspace fixture
//! and are slow (5-30 s each). They skip cleanly when `git` isn't on
//! PATH so contributors without git installed can still run the suite.

mod fixtures;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use tau_domain::{PackageSource, PluginKind, PortKind};
use tau_pkg::{install_with_options, InstallError, InstallOptions, LockFile, Scope};
use tempfile::TempDir;

/// Build [`InstallOptions`] with `skip_cross_check = true` for tests
/// that build stub plugin binaries that don't implement the
/// `meta.handshake` protocol. Without this, sub-project B's Layer 2
/// cross-check at install step 8.7 fails with "EOF before handshake
/// response".
fn install_options_skip_cross_check() -> InstallOptions {
    let mut opts = InstallOptions::default();
    opts.skip_cross_check = true;
    opts
}

/// Run `git ARGS` in `cwd`, panicking on failure with a helpful message.
fn run_git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?} spawn failure: {e}"));
    assert!(
        output.status.success(),
        "git {args:?} in {cwd:?} failed:\nstderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );
}

/// Initialize a bare repo at `<parent>/<name>.git` with HEAD on `main`.
fn make_bare_repo(parent: &Path, name: &str) -> PathBuf {
    let bare = parent.join(format!("{name}.git"));
    std::fs::create_dir_all(&bare).unwrap();
    run_git(&bare, &["init", "--bare", "-q"]);
    run_git(&bare, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    bare
}

/// Build a `file://` URL from a local path. Mirrors `tests/fixtures/mod.rs`
/// but is duplicated here so this test file stays self-contained
/// alongside its lifecycle helpers.
fn file_url(path: &Path) -> String {
    let forward_slashed = path
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    if forward_slashed.starts_with('/') {
        format!("file://{forward_slashed}")
    } else {
        format!("file:///{forward_slashed}")
    }
}

/// Create a bare git repo populated with a single commit containing a
/// minimal **rust-cargo plugin** package: `tau.toml` (with a `[plugin]`
/// table), `Cargo.toml` (standalone, no workspace inheritance), and a
/// trivial `src/main.rs`. Returns the bare repo path.
///
/// The optional `main_rs_override` lets tests inject deliberately
/// broken source (used by the BuildFailed scenario).
fn make_plugin_fixture_repo(
    parent: &Path,
    name: &str,
    version: &str,
    main_rs_override: Option<&str>,
) -> PathBuf {
    let bare = make_bare_repo(parent, name);
    let working = parent.join(format!("{name}-working"));
    std::fs::create_dir_all(&working).unwrap();

    run_git(&working, &["init", "-q", "-b", "main"]);
    run_git(&working, &["config", "user.email", "test@example.com"]);
    run_git(&working, &["config", "user.name", "Test User"]);

    let source_url = file_url(&bare);
    let manifest = format!(
        r#"name = "{name}"
version = "{version}"
description = "Synthetic fixture for rust-cargo plugin install tests"
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

    // Standalone Cargo.toml — no `[workspace]` inheritance, since the
    // cloned package lives outside the tau workspace at install time.
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
    let main_rs = main_rs_override.unwrap_or("fn main() {}\n");
    std::fs::write(working.join("src").join("main.rs"), main_rs).unwrap();

    run_git(&working, &["add", "."]);
    run_git(&working, &["commit", "-q", "-m", "initial fixture commit"]);
    run_git(
        &working,
        &["remote", "add", "origin", &bare.to_string_lossy()],
    );
    run_git(&working, &["push", "-q", "origin", "main"]);

    bare
}

/// Returns true if `git --version` and `cargo --version` both succeed.
/// Tests that build rust-cargo plugins skip cleanly when either binary
/// is missing.
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

#[test]
fn install_runs_cargo_build_for_rust_cargo_plugin() {
    if !cargo_and_git_available() {
        eprintln!("skipping: `git` or `cargo` not on PATH");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("tau-home");
    std::fs::create_dir_all(&project_root).unwrap();
    let scope = Scope::new_project(&project_root).unwrap();

    let bare = make_plugin_fixture_repo(tmp.path(), "fixture-plugin", "0.1.0", None);
    let source = PackageSource::from_str(&file_url(&bare)).unwrap();

    let installed =
        install_with_options(&source, &scope, install_options_skip_cross_check()).unwrap();

    assert_eq!(installed.name.as_str(), "fixture-plugin");
    assert!(installed.installed_path.is_dir());

    // The lockfile should record a LockedPlugin with the resolved binary path.
    let lockfile = LockFile::load(&scope.lockfile_path()).unwrap();
    assert_eq!(lockfile.schema_version, 7);

    let entry = lockfile
        .packages
        .iter()
        .find(|p| p.name.as_str() == "fixture-plugin")
        .expect("lockfile should record fixture-plugin");
    let locked_plugin = entry
        .plugin
        .as_ref()
        .expect("LockedPlugin should be Some for a rust-cargo plugin install");

    assert_eq!(locked_plugin.manifest.provides, PortKind::Tool);
    assert_eq!(locked_plugin.manifest.kind, PluginKind::RustCargo);
    assert_eq!(locked_plugin.manifest.bin, "fixture-plugin");

    let bin_path = &locked_plugin.binary_path;
    assert!(
        bin_path.exists(),
        "binary {} should exist on disk",
        bin_path.display(),
    );
    // Binary should sit under <installed_path>/target/release/.
    let expected_parent = installed.installed_path.join("target").join("release");
    let canonical_expected = expected_parent
        .canonicalize()
        .expect("target/release should be canonicalizable");
    assert_eq!(
        bin_path.parent().unwrap(),
        canonical_expected,
        "binary should live under <pkg>/target/release/",
    );
    // Also verify the binary file name is the plugin's bin name (modulo
    // the platform extension, which on POSIX is empty).
    assert!(
        bin_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .starts_with("fixture-plugin"),
        "binary file name should start with the plugin bin name; got: {}",
        bin_path.display(),
    );
}

#[test]
fn install_surfaces_compile_error_as_build_failed() {
    if !cargo_and_git_available() {
        eprintln!("skipping: `git` or `cargo` not on PATH");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("tau-home");
    std::fs::create_dir_all(&project_root).unwrap();
    let scope = Scope::new_project(&project_root).unwrap();

    // Deliberate type error: assigning a string literal to an i32.
    let broken_main = "fn main() {\n    let _x: i32 = \"this is not an i32\";\n}\n";
    let bare = make_plugin_fixture_repo(tmp.path(), "broken-plugin", "0.1.0", Some(broken_main));
    let source = PackageSource::from_str(&file_url(&bare)).unwrap();

    let result = install_with_options(&source, &scope, install_options_skip_cross_check());
    let err = result.expect_err("expected install to fail with BuildFailed");

    let InstallError::BuildFailed {
        exit_status,
        stderr_tail,
    } = err
    else {
        panic!("expected InstallError::BuildFailed, got {err:?}");
    };

    assert!(
        !exit_status.success(),
        "BuildFailed should carry a non-success exit status; got {exit_status:?}",
    );
    assert!(
        stderr_tail.contains("error[E0") || stderr_tail.contains("error:"),
        "stderr_tail should include a cargo/rustc compile error marker; got:\n{stderr_tail}",
    );
    // `stderr_tail` is bounded to ~4 KiB. We allow a small overhead
    // because `String::from_utf8_lossy` may emit U+FFFD for invalid
    // bytes, which expands the byte count beyond the original 4096.
    // 4 KiB of valid UTF-8 plus replacement-char expansion stays well
    // under 16 KiB in pathological cases; in practice cargo emits
    // valid UTF-8 and the tail is exactly the slice length.
    assert!(
        stderr_tail.len() <= 16 * 1024,
        "stderr_tail should be bounded; got {} bytes",
        stderr_tail.len(),
    );
}

#[test]
fn lockfile_v1_auto_upgrades_to_v2_on_next_install() {
    if !cargo_and_git_available() {
        eprintln!("skipping: `git` or `cargo` not on PATH");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("tau-home");
    std::fs::create_dir_all(&project_root).unwrap();
    let scope = Scope::new_project(&project_root).unwrap();

    let lock_path = scope.lockfile_path();

    // Hand-write a v1 lockfile (note: schema_version = 1, no `plugin`
    // field on the legacy entry).
    let v1_lockfile = r#"schema_version = 1
generated_by_tau_version = "0.0.0"
generated_at = "2026-04-27T10:00:00Z"

[[package]]
name = "legacy-pkg"
active_version = "0.1.0"
source = "https://example.com/legacy/pkg.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0123456789abcdef0123456789abcdef01234567"
sha256 = ""
installed_at = "2026-04-27T10:00:00Z"
"#;
    std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
    std::fs::write(&lock_path, v1_lockfile).unwrap();

    // Sanity-check the file is parseable as v1 (auto-upgraded in memory).
    let pre = LockFile::load(&lock_path).unwrap();
    assert_eq!(
        pre.schema_version, 7,
        "load should auto-upgrade v1 to v7 in memory",
    );
    assert_eq!(pre.packages.len(), 1);
    assert_eq!(pre.packages[0].name.as_str(), "legacy-pkg");
    assert!(
        pre.packages[0].plugin.is_none(),
        "legacy v1 entry should auto-populate plugin = None",
    );

    // Install a *non-plugin* fixture so the build step is skipped (the
    // fixture has no `[plugin]` table). This still triggers the
    // lockfile-write path, which should persist `schema_version = 7`.
    let bare = fixtures::make_fixture_repo(tmp.path(), "data-only-pkg", "0.1.0", "tool");
    let source = PackageSource::from_str(&fixtures::file_url(&bare)).unwrap();
    install_with_options(&source, &scope, install_options_skip_cross_check()).unwrap();

    // Verify the file on disk is now v7.
    let updated = std::fs::read_to_string(&lock_path).unwrap();
    assert!(
        updated.contains("schema_version = 7"),
        "lockfile on disk should be schema_version = 7 after auto-upgrade install; got:\n{updated}",
    );
    assert!(
        !updated.contains("schema_version = 1"),
        "lockfile on disk should not still claim schema_version = 1; got:\n{updated}",
    );

    let post = LockFile::load(&lock_path).unwrap();
    assert_eq!(post.schema_version, 7);
    // Both the legacy entry and the freshly installed data-only package
    // should have plugin = None (data-only).
    for pkg in &post.packages {
        assert!(
            pkg.plugin.is_none(),
            "{} should have plugin = None (data-only fixture / legacy entry)",
            pkg.name.as_str(),
        );
    }
    let names: Vec<&str> = post.packages.iter().map(|p| p.name.as_str()).collect();
    assert!(
        names.contains(&"legacy-pkg"),
        "legacy v1 entry should survive auto-upgrade; got names: {names:?}",
    );
    assert!(
        names.contains(&"data-only-pkg"),
        "freshly installed data-only-pkg should be present; got names: {names:?}",
    );
}
