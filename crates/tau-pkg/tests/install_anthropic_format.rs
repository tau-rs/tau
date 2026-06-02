//! Integration tests for Skills-5 Anthropic-format auto-detection
//! in the tau-pkg install pipeline.
//!
//! These tests exercise the full install pipeline against file:// git fixtures.
//! Each test skips cleanly when no `git` binary is available on PATH.

mod fixtures;

use std::str::FromStr;

use tau_domain::PackageSource;
use tau_pkg::SynthesizedSource;
use tau_pkg::{install_with_options, InstallError, InstallOptions, LockFile, Scope};
use tempfile::TempDir;

/// Construct install options suitable for tests:
/// - skip_build (no cargo available / not needed for skill packages)
/// - skip_cross_check (stub skills won't pass full cross-check)
fn test_install_options() -> InstallOptions {
    let mut opts = InstallOptions::default();
    opts.skip_cross_check = true;
    opts.build.skip_build = true;
    opts
}

/// Create a bare git repo containing only a `SKILL.md` (Anthropic format).
/// The SKILL.md uses a simple body with no `${SKILL_DIR}` references.
fn make_anthropic_fixture_repo(parent: &std::path::Path, name: &str) -> std::path::PathBuf {
    let bare = fixtures::make_bare_repo(parent, name);
    let working = parent.join(format!("{name}-working"));
    std::fs::create_dir_all(&working).unwrap();

    fixtures::run_git_in(&working, &["init", "-q", "-b", "main"]);
    fixtures::run_git_in(&working, &["config", "user.email", "test@example.com"]);
    fixtures::run_git_in(&working, &["config", "user.name", "Test User"]);

    let skill_md = format!(
        "---\nname: {name}\ndescription: A synthesized test skill.\n---\nReview the draft carefully.\n"
    );
    std::fs::write(working.join("SKILL.md"), skill_md).unwrap();

    fixtures::run_git_in(&working, &["add", "SKILL.md"]);
    fixtures::run_git_in(&working, &["commit", "-q", "-m", "initial fixture commit"]);
    fixtures::run_git_in(
        &working,
        &["remote", "add", "origin", &bare.to_string_lossy()],
    );
    fixtures::run_git_in(&working, &["push", "-q", "origin", "main"]);

    bare
}

/// Create a bare git repo containing a `tau.toml` + `SKILL.md` (tau-native format).
/// The tau.toml source field matches the bare repo's file:// URL.
fn make_tau_skill_fixture_repo(parent: &std::path::Path, name: &str) -> std::path::PathBuf {
    let bare = fixtures::make_bare_repo(parent, name);
    let working = parent.join(format!("{name}-working"));
    std::fs::create_dir_all(&working).unwrap();

    fixtures::run_git_in(&working, &["init", "-q", "-b", "main"]);
    fixtures::run_git_in(&working, &["config", "user.email", "test@example.com"]);
    fixtures::run_git_in(&working, &["config", "user.name", "Test User"]);

    let source_url = fixtures::file_url(&bare);
    let tau_toml = format!(
        r#"name = "{name}"
version = "1.2.3"
description = "A tau-native test skill."
authors = []
source = "{source_url}"
kind = "skill"
dependencies = []
capabilities = []

[skill]
"#
    );
    std::fs::write(working.join("tau.toml"), &tau_toml).unwrap();

    let skill_md = format!(
        "---\nname: {name}\ndescription: A tau-native test skill.\n---\nReview the draft.\n"
    );
    std::fs::write(working.join("SKILL.md"), skill_md).unwrap();

    fixtures::run_git_in(&working, &["add", "tau.toml", "SKILL.md"]);
    fixtures::run_git_in(&working, &["commit", "-q", "-m", "initial fixture commit"]);
    fixtures::run_git_in(
        &working,
        &["remote", "add", "origin", &bare.to_string_lossy()],
    );
    fixtures::run_git_in(&working, &["push", "-q", "origin", "main"]);

    bare
}

/// Create a bare git repo with neither `tau.toml` nor `SKILL.md`.
fn make_invalid_fixture_repo(parent: &std::path::Path, name: &str) -> std::path::PathBuf {
    let bare = fixtures::make_bare_repo(parent, name);
    let working = parent.join(format!("{name}-working"));
    std::fs::create_dir_all(&working).unwrap();

    fixtures::run_git_in(&working, &["init", "-q", "-b", "main"]);
    fixtures::run_git_in(&working, &["config", "user.email", "test@example.com"]);
    fixtures::run_git_in(&working, &["config", "user.name", "Test User"]);

    std::fs::write(working.join("README.md"), "# Not a skill\n").unwrap();

    fixtures::run_git_in(&working, &["add", "README.md"]);
    fixtures::run_git_in(&working, &["commit", "-q", "-m", "initial fixture commit"]);
    fixtures::run_git_in(
        &working,
        &["remote", "add", "origin", &bare.to_string_lossy()],
    );
    fixtures::run_git_in(&working, &["push", "-q", "origin", "main"]);

    bare
}

// ─── Test 1 ──────────────────────────────────────────────────────────────────

/// Anthropic-format source auto-detects + synthesizes manifest.
/// Lockfile records `synthesized_from = Some(Anthropic)`, name = package
/// name from SKILL.md frontmatter, version = 0.1.0.
#[test]
fn install_anthropic_format_synthesizes_manifest_and_marks_provenance() {
    if !fixtures::git_available() {
        eprintln!("skipping: `git` not on PATH");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let scope_dir = tmp.path().join("tau-home");
    std::fs::create_dir_all(&scope_dir).unwrap();
    let scope = Scope::new_project(&scope_dir).unwrap();

    let bare = make_anthropic_fixture_repo(tmp.path(), "my-skill");
    let source = PackageSource::from_str(&fixtures::file_url(&bare)).unwrap();

    let installed = install_with_options(&source, &scope, test_install_options()).unwrap();
    assert_eq!(installed.name.as_str(), "my-skill");
    assert_eq!(installed.version.to_string(), "0.1.0");
    assert!(installed.installed_path.is_dir());

    // SKILL.md is present (no tau.toml was written by the pipeline).
    assert!(installed.installed_path.join("SKILL.md").is_file());

    // Lockfile records provenance.
    let lf = LockFile::load(&scope.lockfile_path()).unwrap();
    let pkg = lf
        .packages
        .iter()
        .find(|p| p.name.as_str() == "my-skill")
        .expect("package should be recorded in lockfile");

    assert_eq!(pkg.active_version.to_string(), "0.1.0");
    assert_eq!(
        pkg.synthesized_from,
        Some(SynthesizedSource::Anthropic),
        "Anthropic-format install should record synthesized_from = Some(Anthropic)"
    );
}

// ─── Test 2 ──────────────────────────────────────────────────────────────────

/// Tau-native format (tau.toml present) does NOT set synthesized_from.
/// The `synthesized_from` field should be `None` on the lockfile entry.
#[test]
fn install_tau_format_does_not_set_synthesized_from() {
    if !fixtures::git_available() {
        eprintln!("skipping: `git` not on PATH");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let scope_dir = tmp.path().join("tau-home");
    std::fs::create_dir_all(&scope_dir).unwrap();
    let scope = Scope::new_project(&scope_dir).unwrap();

    let bare = make_tau_skill_fixture_repo(tmp.path(), "native-skill");
    let source = PackageSource::from_str(&fixtures::file_url(&bare)).unwrap();

    let installed = install_with_options(&source, &scope, test_install_options()).unwrap();
    assert_eq!(installed.name.as_str(), "native-skill");
    assert_eq!(installed.version.to_string(), "1.2.3");

    let lf = LockFile::load(&scope.lockfile_path()).unwrap();
    let pkg = lf
        .packages
        .iter()
        .find(|p| p.name.as_str() == "native-skill")
        .expect("package should be recorded in lockfile");

    assert_eq!(
        pkg.synthesized_from, None,
        "tau-native format install should have synthesized_from = None"
    );
}

// ─── Test 3 ──────────────────────────────────────────────────────────────────

/// Directory with neither `tau.toml` nor `SKILL.md` errors with
/// `InstallError::NotASkillPackage`.
#[test]
fn install_invalid_workspace_errors_with_not_a_skill_package() {
    if !fixtures::git_available() {
        eprintln!("skipping: `git` not on PATH");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let scope_dir = tmp.path().join("tau-home");
    std::fs::create_dir_all(&scope_dir).unwrap();
    let scope = Scope::new_project(&scope_dir).unwrap();

    let bare = make_invalid_fixture_repo(tmp.path(), "not-a-skill");
    let source = PackageSource::from_str(&fixtures::file_url(&bare)).unwrap();

    let err = install_with_options(&source, &scope, test_install_options()).unwrap_err();
    assert!(
        matches!(err, InstallError::NotASkillPackage { .. }),
        "expected NotASkillPackage, got {err:?}"
    );
}

// ─── Test 4 (new) ────────────────────────────────────────────────────────────

/// After an Anthropic-format install, the synthesized `tau.toml` must be
/// written to `<install_dir>/tau.toml` so that `find_installed_skill` (and
/// thus `tau skill show` / `tau skill export`) can locate the manifest
/// without consulting the lockfile.
///
/// This test addresses the Skills-5 gap surfaced by T7's roundtrip tests.
#[test]
fn install_anthropic_writes_tau_toml_to_install_dir() {
    if !fixtures::git_available() {
        eprintln!("skipping: `git` not on PATH");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let scope_dir = tmp.path().join("tau-home");
    std::fs::create_dir_all(&scope_dir).unwrap();
    let scope = Scope::new_project(&scope_dir).unwrap();

    let bare = make_anthropic_fixture_repo(tmp.path(), "synthesized-skill");
    let source = PackageSource::from_str(&fixtures::file_url(&bare)).unwrap();

    let installed = install_with_options(&source, &scope, test_install_options()).unwrap();

    // The install dir must contain tau.toml.
    let tau_toml_path = installed.installed_path.join("tau.toml");
    assert!(
        tau_toml_path.exists(),
        "tau.toml must be written to the install dir after Anthropic-format install; \
         expected at {tau_toml_path:?}"
    );

    // The tau.toml content must reflect the synthesized manifest.
    let content = std::fs::read_to_string(&tau_toml_path).unwrap();
    assert!(
        content.contains("name = \"synthesized-skill\""),
        "synthesized tau.toml should contain the package name; got:\n{content}"
    );
    assert!(
        content.contains("version = \"0.1.0\""),
        "synthesized tau.toml should contain version 0.1.0; got:\n{content}"
    );

    // SKILL.md is still present alongside the new tau.toml.
    assert!(
        installed.installed_path.join("SKILL.md").exists(),
        "SKILL.md must still be present after Anthropic-format install"
    );
}

// ─── Test 5 (original test 4) ────────────────────────────────────────────────

/// Upgrade from a v5 lockfile: install an Anthropic-format package on top
/// of an existing v5 lockfile. The resulting lockfile should be v6, contain
/// both the original entry (synthesized_from = None) and the new entry
/// (synthesized_from = Some(Anthropic)).
#[test]
fn install_upgrades_v5_lockfile_to_v6_with_anthropic_provenance() {
    if !fixtures::git_available() {
        eprintln!("skipping: `git` not on PATH");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let scope_dir = tmp.path().join("tau-home");
    std::fs::create_dir_all(&scope_dir).unwrap();
    let scope = Scope::new_project(&scope_dir).unwrap();

    // Seed a v5 lockfile with one existing (tau-native) package entry.
    let v5_toml = r#"schema_version = 5
generated_by_tau_version = "0.0.0"
generated_at = "2026-05-12T10:00:00Z"

[[package]]
name = "old-tool"
active_version = "1.0.0"
source = "https://example.com/old-tool.git"

[[package.versions]]
version = "1.0.0"
resolved_commit = "0000000000000000000000000000000000000000"
sha256 = ""
installed_at = "2026-05-12T10:00:00Z"
"#;
    std::fs::write(scope.lockfile_path(), v5_toml).unwrap();

    // Also create the directory so tau-pkg's scope detection finds the
    // lock dir parent correctly.
    if let Some(parent) = scope.lockfile_path().parent() {
        std::fs::create_dir_all(parent).unwrap();
    }

    // Install an Anthropic-format skill on top.
    let bare = make_anthropic_fixture_repo(tmp.path(), "new-skill");
    let source = PackageSource::from_str(&fixtures::file_url(&bare)).unwrap();

    install_with_options(&source, &scope, test_install_options()).unwrap();

    // The lockfile on disk should now be v7.
    let disk_content = std::fs::read_to_string(scope.lockfile_path()).unwrap();
    assert!(
        disk_content.contains("schema_version = 7"),
        "lockfile should be upgraded to v7; disk content:\n{disk_content}"
    );

    let lf = LockFile::load(&scope.lockfile_path()).unwrap();
    assert_eq!(lf.schema_version, 7, "schema_version should be 7");
    assert_eq!(lf.packages.len(), 2, "both packages should be present");

    // Original entry retains synthesized_from = None.
    let old = lf
        .packages
        .iter()
        .find(|p| p.name.as_str() == "old-tool")
        .expect("old-tool should still be in lockfile");
    assert_eq!(
        old.synthesized_from, None,
        "v5-seeded package should have synthesized_from = None"
    );

    // New Anthropic entry records provenance.
    let new = lf
        .packages
        .iter()
        .find(|p| p.name.as_str() == "new-skill")
        .expect("new-skill should be in lockfile after install");
    assert_eq!(
        new.synthesized_from,
        Some(SynthesizedSource::Anthropic),
        "newly installed Anthropic skill should have synthesized_from = Some(Anthropic)"
    );
}
