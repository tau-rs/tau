//! Integration tests for `tau list`.
//!
//! Mirrors the local-`file://` git fixture pattern from `cmd_install.rs`
//! so the suite has no network requirement, and isolates each test's
//! global scope under a tempdir via `TAU_HOME`.
//!
//! Fixture builders live in `tests/common/mod.rs` (Task 15).

mod common;

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn list_packages_empty_scope_says_no_packages() {
    let global_dir = tempfile::tempdir().unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args(["list", "--global"])
        .env("TAU_HOME", global_dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("no packages installed"));
}

#[test]
fn list_packages_after_install_shows_row() {
    let (fixture, url, _bare) = common::setup_local_package_fixture("hello-tool", "0.1.0");
    let global_dir = fixture.path().join("scope-global");
    std::fs::create_dir_all(&global_dir).unwrap();

    // Install first.
    Command::cargo_bin("tau")
        .unwrap()
        .args(["install", "--global", &url])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success();

    // Then list.
    Command::cargo_bin("tau")
        .unwrap()
        .args(["list", "--global"])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success()
        .stdout(predicate::str::contains("hello-tool"))
        .stdout(predicate::str::contains("0.1.0"))
        .stdout(predicate::str::contains("global"));
}

#[test]
fn list_packages_json_output_is_array_with_expected_fields() {
    let (fixture, url, _bare) = common::setup_local_package_fixture("hello-tool", "0.1.0");
    let global_dir = fixture.path().join("scope-global");
    std::fs::create_dir_all(&global_dir).unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args(["install", "--global", &url])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success();

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["list", "--global", "--json"])
        .env("TAU_HOME", &global_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "tau list --json failed: stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("list --json should emit a JSON array");
    assert!(parsed.is_array(), "expected JSON array, got: {parsed}");
    let rows = parsed.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "hello-tool");
    assert_eq!(rows[0]["version"], "0.1.0");
    assert_eq!(rows[0]["scope"], "global");
}

#[test]
fn list_agents_with_no_project_tau_toml_fails_with_init_hint() {
    let dir = tempfile::tempdir().unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args(["list", "agents"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path())
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("tau init"));
}

#[test]
fn list_agents_reads_project_tau_toml() {
    let dir = tempfile::tempdir().unwrap();
    let toml_str = r#"packages = ["anthropic"]

[project]
name = "demo"

[models]
default = { backend = "anthropic", model = "claude-haiku-4-5" }

[agents.reviewer]
display_name = "Code Reviewer"
package      = "code-reviewer@^0.1"
model        = "default"

[agents.committer]
display_name = "Code Committer"
package      = "code-committer@^0.1"
model        = "default"
"#;
    std::fs::write(dir.path().join("tau.toml"), toml_str).unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args(["list", "agents"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("reviewer"))
        .stdout(predicate::str::contains("committer"))
        .stdout(predicate::str::contains("Code Reviewer"));
}

#[test]
fn list_agents_json_output_is_array_with_expected_fields() {
    let dir = tempfile::tempdir().unwrap();
    let toml_str = r#"packages = ["anthropic"]

[project]
name = "demo"

[models]
default = { backend = "anthropic", model = "claude-haiku-4-5" }

[agents.reviewer]
display_name = "Code Reviewer"
package      = "code-reviewer@^0.1"
model        = "default"
"#;
    std::fs::write(dir.path().join("tau.toml"), toml_str).unwrap();

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["list", "agents", "--json"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "tau list agents --json failed: stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("list agents --json should emit a JSON array");
    assert!(parsed.is_array());
    let rows = parsed.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], "reviewer");
    assert_eq!(rows[0]["display_name"], "Code Reviewer");
    assert_eq!(rows[0]["package"], "code-reviewer@^0.1");
    assert_eq!(rows[0]["llm_backend"], "anthropic");
}

#[test]
fn list_dry_run_rejected_with_read_only_message() {
    let global_dir = tempfile::tempdir().unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args(["list", "--dry-run"])
        .env("TAU_HOME", global_dir.path())
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("dry-run").and(predicate::str::contains("read-only")));
}

#[test]
fn list_global_and_all_mutually_exclusive() {
    Command::cargo_bin("tau")
        .unwrap()
        .args(["list", "--global", "--all"])
        .assert()
        .failure();
}

#[test]
fn list_agents_without_capabilities_flag_omits_effective_caps() {
    let dir = tempfile::tempdir().unwrap();
    let toml_str = r#"packages = ["anthropic"]

[project]
name = "demo"

[models]
default = { backend = "anthropic", model = "claude-haiku-4-5" }

[agents.reviewer]
display_name = "Code Reviewer"
package      = "code-reviewer@^0.1"
model        = "default"
"#;
    std::fs::write(dir.path().join("tau.toml"), toml_str).unwrap();

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["list", "agents"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The "effective_capabilities" header / token should NOT appear.
    assert!(
        !stdout.contains("effective_capabilities"),
        "expected --capabilities-off to skip effective_caps column; got: {stdout}"
    );
}

#[test]
fn list_agents_with_capabilities_flag_renders_package_not_installed() {
    // The agent's package isn't installed in this temp scope, so the row
    // should render with "(package not installed)" — non-fatal, exit 0.
    let dir = tempfile::tempdir().unwrap();
    let toml_str = r#"packages = ["anthropic"]

[project]
name = "demo"

[models]
default = { backend = "anthropic", model = "claude-haiku-4-5" }

[agents.reviewer]
display_name = "Code Reviewer"
package      = "code-reviewer@^0.1"
model        = "default"
"#;
    std::fs::write(dir.path().join("tau.toml"), toml_str).unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args(["list", "agents", "--capabilities"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("reviewer"))
        .stdout(predicate::str::contains("package not installed"));
}

#[test]
fn list_agents_with_capabilities_json_emits_field() {
    let dir = tempfile::tempdir().unwrap();
    let toml_str = r#"packages = ["anthropic"]

[project]
name = "demo"

[models]
default = { backend = "anthropic", model = "claude-haiku-4-5" }

[agents.reviewer]
display_name = "Code Reviewer"
package      = "code-reviewer@^0.1"
model        = "default"
"#;
    std::fs::write(dir.path().join("tau.toml"), toml_str).unwrap();

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["list", "agents", "--capabilities", "--json"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let rows: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let arr = rows.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    // effective_capabilities is omitted when None; it should not appear
    // for an uninstalled package.
    assert!(arr[0].get("id").is_some());
    assert!(arr[0].get("effective_capabilities").is_none());
}

#[test]
fn list_agents_capabilities_flag_does_not_affect_package_listing() {
    // --capabilities is silently ignored on `tau list packages`.
    let dir = tempfile::tempdir().unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args(["list", "packages", "--capabilities"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path())
        .assert()
        .success();
}
