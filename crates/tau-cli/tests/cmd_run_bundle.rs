//! Integration tests for `tau run --bundle` (Phase 2 §C.3).
//!
//! Each test stands up an isolated scratch root (a project tempdir + a
//! sibling `TAU_HOME` tempdir so global-scope writes never touch the
//! developer's `~/.tau`), invokes the real `tau build` binary to produce
//! a sealed bundle, then exercises the `tau run --bundle` verify gate
//! end-to-end. Drift / missing-install / self-hash-mismatch must each
//! surface as exit code 3 with an actionable stderr substring; the clean
//! fixture must pass the gate (exit code != 3).
//!
//! The fixture shape mirrors `cmd_build.rs` (`write_minimal_project` +
//! `make_tau_home`): a `[project]` header plus one agent (`solo`) and an
//! empty schema-v6 lockfile (no packages → the build's install-state
//! check is a no-op and the bundle writes cleanly). For the
//! missing-install test we add one locked-but-installed package and then
//! delete its on-disk tree after building.
//!
//! `--dry-run` is used so no live LLM backend is needed: the verify gate
//! runs at the very top of `run()`, before the dry-run short-circuit, so
//! the gate is fully exercised regardless of what the run path would do
//! afterwards. The self-hash test does not strictly need `--dry-run`
//! (verify fails before anything else), but passes it for consistency.

#![allow(clippy::needless_raw_string_hashes)]

use assert_cmd::Command;
use predicates::prelude::*;

/// Minimal project fixture: a `[project]` header with `name` + `version`
/// plus one agent (`solo`) with an inline system prompt. Mirrors
/// `cmd_build.rs::write_minimal_project`.
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
        .args(["build"])
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
fn run_bundle_with_tau_toml_drift_exits_three() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("drift");
    std::fs::create_dir(&project).unwrap();
    write_minimal_project(&project, "drift");
    write_empty_lockfile(&project);
    let tau_home = make_tau_home(scratch.path());

    let bundle = build_bundle(&project, &tau_home);

    // Mutate tau.toml after sealing the bundle. The bundle recorded the
    // original tau.toml's sha256; verify step 6 recomputes it from the
    // cwd and rejects the drift. The bundle file itself is a sibling
    // `.tau` artifact, so editing tau.toml leaves it untouched.
    write_minimal_project(&project, "drift-RENAMED");

    Command::cargo_bin("tau")
        .unwrap()
        .args([
            "run",
            "--bundle",
            bundle.to_str().unwrap(),
            "solo",
            "--dry-run",
        ])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("tau.toml drift"));
}

#[test]
fn run_bundle_with_missing_install_exits_three() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("missing");
    std::fs::create_dir(&project).unwrap();
    write_minimal_project(&project, "missing");

    // Lockfile claims one package "ghost" at 0.1.0 with a matching
    // on-disk tree at `.tau/packages/ghost/0.1.0/`. The build records
    // the package's tree hash; we then delete the tree so verify step 7
    // (package install check) fails with PackageMissing → exit 3.
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
    let tau_home = make_tau_home(scratch.path());

    let bundle = build_bundle(&project, &tau_home);

    // Delete the installed-package tree the bundle recorded.
    std::fs::remove_dir_all(project.join(".tau/packages/ghost")).unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args([
            "run",
            "--bundle",
            bundle.to_str().unwrap(),
            "solo",
            "--dry-run",
        ])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("missing from"));
}

#[test]
fn run_bundle_self_hash_stale_exits_three() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("selfhash");
    std::fs::create_dir(&project).unwrap();
    write_minimal_project(&project, "selfhash");
    write_empty_lockfile(&project);
    let tau_home = make_tau_home(scratch.path());

    let bundle = build_bundle(&project, &tau_home);

    // Hand-edit one byte of the sealed bundle so the recorded self-hash
    // no longer matches the computed hash of the body. The bundle is a
    // TOML doc with a `name = "selfhash"` line; renaming the project
    // there flips the body without touching the `bundle.sha256` field,
    // so verify step 3 (self-hash) rejects it → exit 3.
    let body = std::fs::read_to_string(&bundle).unwrap();
    let tampered = body.replacen("selfhash", "selfhasX", 1);
    assert_ne!(
        body, tampered,
        "expected the bundle body to contain the project name"
    );
    std::fs::write(&bundle, tampered).unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args([
            "run",
            "--bundle",
            bundle.to_str().unwrap(),
            "solo",
            "--dry-run",
        ])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .code(3)
        .stderr(predicate::str::contains("self-hash"));
}

#[test]
fn run_bundle_clean_fixture_passes_verify_gate() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("clean");
    std::fs::create_dir(&project).unwrap();
    write_minimal_project(&project, "clean");
    write_empty_lockfile(&project);
    let tau_home = make_tau_home(scratch.path());

    let bundle = build_bundle(&project, &tau_home);

    // Run --bundle against the UNCHANGED tree. The verify gate (which
    // runs first, before the dry-run short-circuit) must pass: it never
    // exits 3 on a clean fixture. The run flow then takes over; the
    // strict invariant for this test is that the bundle gate did NOT
    // reject the tree (exit code != 3). Whether the subsequent dry-run
    // reaches a clean exit 0 depends on the agent package being
    // resolvable, which is out of scope for the gate test.
    let output = Command::cargo_bin("tau")
        .unwrap()
        .args([
            "run",
            "--bundle",
            bundle.to_str().unwrap(),
            "solo",
            "--dry-run",
        ])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_ne!(
        output.status.code(),
        Some(3),
        "clean fixture must pass the bundle verify gate (no exit 3); stderr was:\n{stderr}"
    );
    // Belt-and-suspenders: the verify-gate error messages must NOT appear.
    assert!(
        !stderr.contains("drift")
            && !stderr.contains("self-hash")
            && !stderr.contains("missing from")
            && !stderr.contains("does not match host"),
        "clean fixture must not surface any verify-gate refusal; stderr was:\n{stderr}"
    );
}
