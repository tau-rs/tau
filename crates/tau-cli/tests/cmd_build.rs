//! Integration tests for `tau build` (Phase 2 §C.2).
//!
//! Each test stands up an isolated scratch root: a project tempdir
//! containing a minimal `tau.toml` (and, when relevant, a `tau.lock`
//!   plus installed-package trees), plus a sibling `TAU_HOME` tempdir
//! so global-scope writes never touch the developer's `~/.tau`. The
//! `TAU_HOME` tempdir pre-creates `config.toml` to defeat the
//! parallel-write race surfaced on Windows runners (see project
//! memory `feedback_windows_tau_home_test_pattern`).

#![allow(clippy::needless_raw_string_hashes)]

mod common;

use assert_cmd::Command;
use predicates::prelude::*;

/// Canonical project fixture used by the build tests. Mirrors the
/// shape from `crates/tau-pkg/tests/bundle_build_e2e.rs`: a `[project]`
/// header with `name` + `version`, plus one minimal agent declaring
/// `package = "<name>@^0.1"`, `llm_backend`, and an inline
/// `[agents.<id>.prompt].system` body.
fn write_minimal_project(root: &std::path::Path, name: &str) {
    std::fs::write(
        root.join("tau.toml"),
        format!(
            r#"
[project]
name = "{name}"
version = "0.1.0"

[agents.solo]
display_name = "Solo"
package = "{name}@^0.1"
llm_backend = "anthropic"

[agents.solo.prompt]
system = "hi"
"#
        ),
    )
    .unwrap();
}

/// Build an isolated `TAU_HOME` tempdir under `scratch` with the
/// `config.toml` pre-created so parallel tests don't race on its
/// initial write.
fn make_tau_home(scratch: &std::path::Path) -> std::path::PathBuf {
    let home = scratch.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let cfg = home.join("config.toml");
    if !cfg.exists() {
        std::fs::write(&cfg, "").unwrap();
    }
    home
}

#[test]
fn build_emits_bundle_path_on_stdout_for_clean_fixture() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    write_minimal_project(&project, "smoke");

    // Empty schema-v6 lockfile: no packages → step 3 (verify install)
    // is a no-op, and the build proceeds through to writing a bundle.
    std::fs::write(
        project.join("tau.lock"),
        r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"
"#,
    )
    .unwrap();

    let tau_home = make_tau_home(scratch.path());

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["build"])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stdout_path = stdout.trim();
    assert!(
        stdout_path.ends_with("smoke-0.1.0.tau"),
        "stdout was: {stdout_path:?}\nstderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        std::path::Path::new(stdout_path).exists(),
        "bundle file should exist at {stdout_path:?}",
    );
}

#[test]
fn build_without_lockfile_exits_three_with_remediation_hint() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("noLock");
    std::fs::create_dir(&project).unwrap();
    write_minimal_project(&project, "x");
    // Deliberately no tau.lock → BuildError::MissingLockfile → exit 3,
    // stderr carries the `tau install` remediation hint baked into the
    // BuildError display impl.

    let tau_home = make_tau_home(scratch.path());

    Command::cargo_bin("tau")
        .unwrap()
        .args(["build"])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("tau install"));
}

#[test]
fn build_with_missing_install_exits_three_naming_the_package() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("noInstall");
    std::fs::create_dir(&project).unwrap();
    write_minimal_project(&project, "x");

    // Lockfile claims one package "ghost" at 0.1.0, but the matching
    // `.tau/packages/ghost/0.1.0/` directory does not exist on disk
    // → BuildError::PackageNotInstalled { name: "ghost", .. } → exit 3.
    std::fs::write(
        project.join("tau.lock"),
        r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"

[[package]]
name = "ghost"
active_version = "0.1.0"
source = "https://example.com/ghost.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0000000000000000000000000000000000000001"
installed_at = "2024-01-01T00:00:00Z"
"#,
    )
    .unwrap();

    let tau_home = make_tau_home(scratch.path());

    Command::cargo_bin("tau")
        .unwrap()
        .args(["build"])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("ghost"));
}

/// Empty schema-v6 lockfile body shared by the §C.2.1 flag tests: no
/// packages, so the install-verify step is a no-op and the build runs
/// through to writing a bundle.
const EMPTY_V6_LOCK: &str = r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"
"#;

#[test]
fn build_with_output_flag_writes_to_custom_path() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    write_minimal_project(&project, "ocustom");
    std::fs::write(project.join("tau.lock"), EMPTY_V6_LOCK).unwrap();
    let out_path = project.join("custom.tau");

    let tau_home = make_tau_home(scratch.path());

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["build", "-o", out_path.to_str().unwrap()])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .success()
        .get_output()
        .clone();

    assert!(out_path.exists(), "bundle written to -o path");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        out_path.display().to_string(),
    );
}

#[test]
fn build_with_json_emits_artifact_object() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    write_minimal_project(&project, "jbuild");
    std::fs::write(project.join("tau.lock"), EMPTY_V6_LOCK).unwrap();

    let tau_home = make_tau_home(scratch.path());

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["build", "--json"])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .success()
        .get_output()
        .clone();

    let v: serde_json::Value = serde_json::from_slice(&output.stdout).expect("stdout is JSON");
    assert!(
        v["path"].as_str().unwrap().ends_with("jbuild-0.1.0.tau"),
        "path={:?}",
        v["path"],
    );
    assert_eq!(v["sha256"].as_str().unwrap().len(), 64);
    assert!(v["size_bytes"].as_u64().unwrap() > 0);
}

#[test]
fn build_with_invalid_target_exits_two() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    write_minimal_project(&project, "badtgt");
    std::fs::write(project.join("tau.lock"), EMPTY_V6_LOCK).unwrap();

    let tau_home = make_tau_home(scratch.path());

    Command::cargo_bin("tau")
        .unwrap()
        .args(["build", "--target", "not-a-real-triple"])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not-a-real-triple"));
}

#[test]
fn build_with_available_target_succeeds() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    write_minimal_project(&project, "avtgt");
    std::fs::write(project.join("tau.lock"), EMPTY_V6_LOCK).unwrap();

    let tau_home = make_tau_home(scratch.path());

    // `passthrough` is Available on every host. (Don't use `host()`:
    // on Windows it is `windows-native-strict`, which is Reserved and
    // would be rejected by the --target Available gate.)
    Command::cargo_bin("tau")
        .unwrap()
        .args(["build", "--target", "passthrough"])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .success();
}
