//! Integration tests for the `tau build` governance gate (ADR-0059).
//!
//! `tau build` enforces the root `[allow]` constitution: a project with no
//! `[allow]` section is refused (exit 2, "ungoverned build"); a tool whose
//! declared capabilities exceed the `[allow]` ceiling is an over-reach
//! violation (exit 2, `tau.governance.over_reach`); widening the ceiling to
//! cover the tool clears the violation; and `--no-governance` skips the gate
//! with a stderr warning.
//!
//! Each test stands up an isolated scratch root (a project tempdir + a
//! sibling `TAU_HOME` tempdir with `config.toml` pre-created so global-scope
//! writes never race on Windows runners — matching the pattern in
//! `cmd_build.rs` / `feedback_windows_tau_home_test_pattern`), and an empty
//! schema-v6 lockfile so the build reaches the gate.

#![allow(clippy::needless_raw_string_hashes)]

use assert_cmd::Command;
use predicates::prelude::*;

/// Empty schema-v6 lockfile: no packages → the install-verify step is a
/// no-op and the build proceeds to the governance gate.
const EMPTY_V6_LOCK: &str = r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"
"#;

/// Build an isolated `TAU_HOME` tempdir under `scratch` with `config.toml`
/// pre-created so parallel tests don't race on its initial write.
fn make_tau_home(scratch: &std::path::Path) -> std::path::PathBuf {
    let home = scratch.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let cfg = home.join("config.toml");
    if !cfg.exists() {
        std::fs::write(&cfg, "").unwrap();
    }
    home
}

/// A governed, agent-less project whose `[tools.fetch]` declares `fs.read`
/// on `/etc/**` while both the root `[allow]` ceiling and the
/// `[allow.tools.fetch]` registration only permit the given `ceiling_paths`.
/// When `ceiling_paths` omits `/etc/**` this is an over-reach violation.
///
/// The fixture deliberately declares no agents (hence no `[allow.models]`):
/// a bare `[project]` + `[allow]` + `[tools.fetch]` is enough for `tau build`
/// to lower and reach the governance gate, and it keeps the test focused on
/// the tool-over-reach lattice check.
fn write_fetch_project(root: &std::path::Path, name: &str, ceiling_paths: &str) {
    std::fs::write(
        root.join("tau.toml"),
        format!(
            r#"[project]
name = "{name}"
version = "0.1.0"

[allow]
"fs.read" = {{ paths = [{ceiling_paths}] }}

[allow.tools.fetch]
native = "Fetch"
"fs.read" = {{ paths = [{ceiling_paths}] }}

[tools.fetch]
native = "Fetch"
capabilities = [{{ kind = "fs.read", paths = ["/etc/**"] }}]
"#
        ),
    )
    .unwrap();
    std::fs::write(root.join("tau.lock"), EMPTY_V6_LOCK).unwrap();
}

#[test]
fn build_refuses_when_tool_caps_exceed_ceiling() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    // Ceiling permits only /proj/** but the tool asks for /etc/** → over-reach.
    write_fetch_project(&project, "overreach", r#""/proj/**""#);
    let tau_home = make_tau_home(scratch.path());

    Command::cargo_bin("tau")
        .unwrap()
        .args(["build"])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("over_reach"))
        .stderr(predicate::str::contains("/etc/**"));
}

#[test]
fn build_succeeds_when_ceiling_widened() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    // Widen both ceilings to cover /etc/** → the tool is within the lattice.
    write_fetch_project(&project, "widened", r#""/proj/**", "/etc/**""#);
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
    assert!(
        stdout.trim().ends_with(".tau"),
        "stdout should be the bundle path ending in .tau; got {stdout:?}\nstderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn build_with_no_governance_skips_and_warns() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    // Same violating fixture as the refuse test, but --no-governance skips
    // the gate entirely.
    write_fetch_project(&project, "skipped", r#""/proj/**""#);
    let tau_home = make_tau_home(scratch.path());

    Command::cargo_bin("tau")
        .unwrap()
        .args(["build", "--no-governance"])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .success()
        .stderr(predicate::str::contains("--no-governance"));
}

#[test]
fn build_succeeds_for_governed_project_with_agent() {
    // Regression guard for the `[allow.models]` lowering/plugin-load gap
    // (ADR-0059): when a constitution is declared, the model alias map is the
    // sole home under `[allow.models]` and `[models]` cannot coexist. A
    // governed project *with an agent* must therefore build WITHOUT
    // `--no-governance`. Before the fix, `resolve_model_ref` read only
    // `[models]` and lowering failed with "model alias `default` not in
    // [models]" — making governed-by-default unusable for any real project.
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(
        project.join("tau.toml"),
        r#"packages = ["anthropic"]

[project]
name = "governed"
version = "0.1.0"

[allow]
"fs.read" = { paths = ["/proj/**"] }

[allow.models.default]
backend = "anthropic"
model = "claude-haiku-4-5"

[agents.solo]
display_name = "Solo"
package = "governed@^0.1"
model = "default"

[agents.solo.prompt]
system = "hi"
"#,
    )
    .unwrap();
    std::fs::write(project.join("tau.lock"), EMPTY_V6_LOCK).unwrap();
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
    assert!(
        stdout.trim().ends_with(".tau"),
        "governed agent project should build without --no-governance; \
         stdout={stdout:?} stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn build_refuses_ungoverned_project() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    // A valid minimal project with NO [allow] section → ungoverned build.
    std::fs::write(
        project.join("tau.toml"),
        r#"packages = ["anthropic"]

[project]
name = "ungoverned"
version = "0.1.0"

[models]
default = { backend = "anthropic", model = "claude-haiku-4-5" }

[agents.solo]
display_name = "Solo"
package = "ungoverned@^0.1"
model = "default"

[agents.solo.prompt]
system = "hi"
"#,
    )
    .unwrap();
    std::fs::write(project.join("tau.lock"), EMPTY_V6_LOCK).unwrap();
    let tau_home = make_tau_home(scratch.path());

    Command::cargo_bin("tau")
        .unwrap()
        .args(["build"])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("ungoverned build"));
}
