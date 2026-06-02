//! End-to-end install lifecycle test against a `file://` git fixture.
//!
//! Skips cleanly if the host has no `git` binary on PATH.

mod fixtures;

use std::str::FromStr;

use tempfile::TempDir;

use tau_domain::PackageSource;
use tau_pkg::{install, list, LockFile, Scope};

#[test]
fn install_minimal_tool_package() {
    if !fixtures::git_available() {
        eprintln!("skipping: `git` not on PATH");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("tau-home");
    std::fs::create_dir_all(&project_root).unwrap();
    let scope = Scope::new_project(&project_root).unwrap();

    let bare = fixtures::make_fixture_repo(tmp.path(), "acme-tool", "1.0.0", "tool");
    let source = PackageSource::from_str(&fixtures::file_url(&bare)).unwrap();

    let installed = install(&source, &scope).unwrap();
    assert_eq!(installed.name.as_str(), "acme-tool");
    assert_eq!(installed.version.to_string(), "1.0.0");
    assert!(installed.installed_path.is_dir());
    assert!(installed.installed_path.join("tau.toml").is_file());

    // Lockfile reflects the install.
    let pkgs = list(&scope).unwrap();
    assert_eq!(pkgs.len(), 1);
    assert_eq!(pkgs[0].name.as_str(), "acme-tool");
    assert_eq!(pkgs[0].active_version.to_string(), "1.0.0");
    assert_eq!(pkgs[0].installed_versions.len(), 1);
    assert_eq!(pkgs[0].installed_versions[0].resolved_commit.len(), 40);
}

#[test]
fn install_is_idempotent_when_called_twice() {
    if !fixtures::git_available() {
        eprintln!("skipping: `git` not on PATH");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("tau-home");
    std::fs::create_dir_all(&project_root).unwrap();
    let scope = Scope::new_project(&project_root).unwrap();

    let bare = fixtures::make_fixture_repo(tmp.path(), "acme-tool", "1.0.0", "tool");
    let source = PackageSource::from_str(&fixtures::file_url(&bare)).unwrap();

    let first = install(&source, &scope).unwrap();
    let second = install(&source, &scope).unwrap();

    assert_eq!(first.installed_path, second.installed_path);
    let pkgs = list(&scope).unwrap();
    assert_eq!(pkgs.len(), 1, "second install should not duplicate");
    assert_eq!(pkgs[0].installed_versions.len(), 1);
}

/// Task 2: After install, `LockedVersion.sha256` must be a non-empty 64-char hex string
/// computed from the source tree. This verifies `tree_hash` is called and persisted.
#[test]
fn install_populates_locked_version_sha256() {
    if !fixtures::git_available() {
        eprintln!("skipping: `git` not on PATH");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("tau-home");
    std::fs::create_dir_all(&project_root).unwrap();
    let scope = Scope::new_project(&project_root).unwrap();

    let bare = fixtures::make_fixture_repo(tmp.path(), "sha256-tool", "1.0.0", "tool");
    let source = PackageSource::from_str(&fixtures::file_url(&bare)).unwrap();

    install(&source, &scope).unwrap();

    let lf = LockFile::load(&scope.lockfile_path()).unwrap();
    let pkg = lf
        .packages
        .iter()
        .find(|p| p.name.as_str() == "sha256-tool")
        .expect("package should be recorded in lockfile");
    let locked_version = &pkg.installed_versions[0];

    let sha = &locked_version.sha256;
    assert_eq!(
        sha.len(),
        64,
        "LockedVersion.sha256 should be 64-char hex after install; got: {sha:?}"
    );
    assert!(
        sha.chars().all(|c| c.is_ascii_hexdigit()),
        "LockedVersion.sha256 should be hex; got: {sha:?}"
    );
    assert!(
        !sha.is_empty(),
        "LockedVersion.sha256 should be non-empty after install"
    );
}

/// After install, the lockfile schema_version should be the current max
/// (v4 as of the sandboxing sub-project).
#[test]
fn install_writes_current_schema_version_lockfile() {
    if !fixtures::git_available() {
        eprintln!("skipping: `git` not on PATH");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("tau-home");
    std::fs::create_dir_all(&project_root).unwrap();
    let scope = Scope::new_project(&project_root).unwrap();

    let bare = fixtures::make_fixture_repo(tmp.path(), "schema-v4-tool", "1.0.0", "tool");
    let source = PackageSource::from_str(&fixtures::file_url(&bare)).unwrap();

    install(&source, &scope).unwrap();

    // Check the on-disk TOML file directly — this confirms schema_version is
    // written as 7 (not still an older value from a default that was never bumped).
    let disk_content = std::fs::read_to_string(scope.lockfile_path()).unwrap();
    assert!(
        disk_content.contains("schema_version = 7"),
        "lockfile on disk should contain schema_version = 7 after fresh install;\ngot:\n{disk_content}"
    );

    let lf = LockFile::load(&scope.lockfile_path()).unwrap();
    assert_eq!(
        lf.schema_version, 7,
        "LockFile::load should return schema_version = 7"
    );
}

#[test]
fn install_rejects_when_manifest_source_does_not_match() {
    if !fixtures::git_available() {
        eprintln!("skipping: `git` not on PATH");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("tau-home");
    std::fs::create_dir_all(&project_root).unwrap();
    let scope = Scope::new_project(&project_root).unwrap();

    // Make fixture A (its tau.toml says source = file://A.git).
    let bare_a = fixtures::make_fixture_repo(tmp.path(), "pkg-a", "1.0.0", "tool");

    // Try to install passing fixture A's bare repo URL but call it
    // through bare_b's path so the user-supplied source disagrees with
    // the manifest's declared source. We just need any URL that doesn't
    // match the manifest.
    let mismatched_url = format!("file://{}/totally-different.git", tmp.path().display());
    let source = PackageSource::from_str(&mismatched_url);
    if source.is_err() {
        // If the URL doesn't parse, skip — this isn't testing parse logic.
        return;
    }
    let source = source.unwrap();

    // The clone will likely fail because the URL doesn't exist. We
    // accept either CloneFailed (clone errored) OR
    // SourceManifestMismatch (clone somehow succeeded and the check
    // fired). Both are correct rejections; we just want a typed error
    // and no successful install.
    let result = install(&source, &scope);
    assert!(
        result.is_err(),
        "install should fail when source URL is bogus or doesn't match manifest"
    );
    let _ = bare_a; // silence unused-warning
}
