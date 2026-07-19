//! Integration tests for `tau verify --bundle` (Phase 2 §E).
//!
//! Each test stands up an isolated scratch root (a project tempdir + a
//! sibling `TAU_HOME` tempdir so global-scope writes never touch the
//! developer's `~/.tau`), invokes the real `tau build` binary to produce
//! a sealed bundle, then exercises `tau verify --bundle <path>` end-to-end.
//!
//! Unlike the `tau run --bundle` gate (cmd_run_bundle.rs), `tau verify
//! --bundle` *rebuilds* the bundle from the local tree and compares the
//! two manifests rather than running an agent — so a clean fixture yields
//! a true exit 0 with a "Reproducible" line on stdout.
//!
//! Output channels (from §E renderers):
//! - Reproducible (success): `✓ Reproducible …` → stdout, exit 0.
//! - NOT reproducible: `✗ NOT reproducible` + divergence body → stderr
//!   (plain, no `error:` prefix), exit 2.
//! - `--json` (global flag, after the subcommand): JSON object → stdout.
//! - Rebuild blocked by install state (missing/uninstalled package):
//!   `error: …` → stderr, exit 3.
//!
//! The fixture shape mirrors cmd_run_bundle.rs: a `[project]` header plus
//! one agent (`solo`) and a schema-v6 lockfile. The drift / not-installed
//! / json tests add one locked-and-installed package and mutate or delete
//! its on-disk tree after building.

#![allow(clippy::needless_raw_string_hashes)]

use assert_cmd::Command;
use predicates::prelude::*;

/// Minimal project fixture: a `[project]` header with `name` + `version`
/// plus one agent (`solo`) with an inline system prompt.
fn write_minimal_project(root: &std::path::Path, name: &str) {
    std::fs::write(
        root.join("tau.toml"),
        format!(
            r#"packages = ["anthropic"]

[project]
name = "{name}"
version = "0.1.0"

[models]
default = {{ backend = "anthropic", model = "claude-haiku-4-5" }}

[agents.solo]
display_name = "Solo"
package = "{name}@^0.1"
model = "default"

[agents.solo.prompt]
system = "hi"
"#
        ),
    )
    .unwrap();
}

/// Empty schema-v6 lockfile: no packages, so the build's install-state
/// check is a no-op and the bundle writes cleanly.
fn write_empty_lockfile(root: &std::path::Path) {
    std::fs::write(
        root.join("tau.lock"),
        r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"
"#,
    )
    .unwrap();
}

/// Write a project + lockfile that locks ONE installed package
/// (`ghost@0.1.0`) with a matching on-disk tree at
/// `.tau/packages/ghost/0.1.0/`. The build records the package's tree
/// hash; tests then mutate or delete the tree to provoke drift / a
/// not-installed error.
fn write_one_package_fixture(project: &std::path::Path, name: &str) {
    write_minimal_project(project, name);

    let pkg_dir = project.join(".tau/packages/ghost/0.1.0");
    std::fs::create_dir_all(pkg_dir.join("src")).unwrap();
    std::fs::write(pkg_dir.join("tau.toml"), "name = \"ghost\"\n").unwrap();
    std::fs::write(pkg_dir.join("src/lib.rs"), "// ghost\n").unwrap();

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
}

/// Build an isolated `TAU_HOME` tempdir under `scratch` with
/// `config.toml` pre-created so parallel tests don't race on its initial
/// write (see project memory `feedback_windows_tau_home_test_pattern`).
fn make_tau_home(scratch: &std::path::Path) -> std::path::PathBuf {
    let home = scratch.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let cfg = home.join("config.toml");
    if !cfg.exists() {
        std::fs::write(&cfg, "").unwrap();
    }
    home
}

/// Build a bundle in `project` by invoking the real `tau build` binary.
/// Returns the bundle path (read from `tau build`'s stdout).
fn build_bundle(project: &std::path::Path, tau_home: &std::path::Path) -> std::path::PathBuf {
    let out = Command::cargo_bin("tau")
        .unwrap()
        .args(["build", "--allow-ungoverned"])
        .current_dir(project)
        .env("TAU_HOME", tau_home)
        .assert()
        .success()
        .get_output()
        .clone();
    let path = String::from_utf8(out.stdout).unwrap().trim().to_string();
    std::path::PathBuf::from(path)
}

#[test]
fn verify_bundle_reproducible_exits_zero() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("repro");
    std::fs::create_dir(&project).unwrap();
    write_minimal_project(&project, "repro");
    write_empty_lockfile(&project);
    let tau_home = make_tau_home(scratch.path());

    let bundle = build_bundle(&project, &tau_home);

    // Verify against the UNCHANGED tree. verify --bundle rebuilds the
    // bundle and compares manifests; an untouched tree rebuilds to the
    // same self-hash → reproducible → exit 0, "Reproducible" on stdout.
    Command::cargo_bin("tau")
        .unwrap()
        .args(["verify", "--bundle", bundle.to_str().unwrap()])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .success()
        .stdout(predicate::str::contains("Reproducible"));
}

#[test]
fn verify_bundle_drift_exits_two() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("drift");
    std::fs::create_dir(&project).unwrap();
    write_one_package_fixture(&project, "drift");
    let tau_home = make_tau_home(scratch.path());

    let bundle = build_bundle(&project, &tau_home);

    // Mutate a file inside the installed package tree. The bundle
    // recorded the original tree_sha256; the rebuild recomputes a
    // different hash → a PackageField divergence naming the package →
    // not reproducible → exit 2 with the diagnosis on stderr.
    std::fs::write(
        project.join(".tau/packages/ghost/0.1.0/src/lib.rs"),
        "// ghost MUTATED\n",
    )
    .unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args(["verify", "--bundle", bundle.to_str().unwrap()])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("NOT reproducible"))
        .stderr(predicate::str::contains("ghost"));
}

#[test]
fn verify_bundle_not_installed_exits_three() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("notinstalled");
    std::fs::create_dir(&project).unwrap();
    write_one_package_fixture(&project, "notinstalled");
    let tau_home = make_tau_home(scratch.path());

    let bundle = build_bundle(&project, &tau_home);

    // Delete the installed package tree the bundle recorded. The rebuild
    // can no longer hash the package install → BuildError indicating
    // incomplete install state → exit 3.
    std::fs::remove_dir_all(project.join(".tau/packages/ghost")).unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args(["verify", "--bundle", bundle.to_str().unwrap()])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .code(3);
}

#[test]
fn verify_bundle_json_emits_structured_result() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("json");
    std::fs::create_dir(&project).unwrap();
    write_one_package_fixture(&project, "json");
    let tau_home = make_tau_home(scratch.path());

    let bundle = build_bundle(&project, &tau_home);

    // Mutate the package tree to force a non-reproducible result, then
    // request JSON. `--json` is the GLOBAL flag, so it goes after the
    // subcommand+args: `tau verify --bundle <path> --json`.
    std::fs::write(
        project.join(".tau/packages/ghost/0.1.0/src/lib.rs"),
        "// ghost MUTATED FOR JSON\n",
    )
    .unwrap();

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["verify", "--bundle", bundle.to_str().unwrap(), "--json"])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .output()
        .unwrap();

    // The JSON object is emitted on stdout regardless of the exit code.
    let obj: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "verify --bundle --json must emit a JSON object on stdout: {e}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        )
    });

    assert_eq!(
        obj["reproducible"],
        serde_json::Value::Bool(false),
        "mutated package tree must report reproducible=false; obj was {obj}"
    );
    let diffs = obj["diffs"]
        .as_array()
        .expect("`diffs` must be a JSON array");
    assert!(
        !diffs.is_empty(),
        "a non-reproducible result must list at least one diff; obj was {obj}"
    );
}
