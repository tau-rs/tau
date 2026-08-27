//! Integration tests for `tau resolve --check-sandbox`.
//!
//! Pattern: assert_cmd + tempfile-based project + hand-authored lockfiles
//! with `[package.plugin]` entries. We use the mock sandbox adapter
//! (always available, supports the 5 standard shapes) so tests pass on
//! every platform without Docker or Linux kernel requirements.
//!
//! The "no adapter" test configures `required_tier = "strict"` WITHOUT
//! `TAU_TESTING_ALLOW_MOCK_SANDBOX=1`, so the real registry runs and
//! finds no suitable adapter on macOS (native is Linux-only, container
//! requires Docker which is absent in CI). That gives a reliable exit 2.
//!
//! When `required_tier = "none"`, --check-sandbox skips passthrough and
//! calls `resolve_strict_for_validation()`. When
//! `TAU_TESTING_ALLOW_MOCK_SANDBOX=1` is set, that function returns mock,
//! so tests remain platform-independent.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

mod common;

/// Write a minimal tau.toml project file into `dir`.
fn write_tau_toml(dir: &std::path::Path) {
    std::fs::write(
        dir.join("tau.toml"),
        r#"packages = ["anthropic"]

[project]
name = "demo"

[models]
default = { backend = "anthropic", model = "claude-haiku-4-5" }

[agents.reviewer]
display_name = "Reviewer"
package      = "demo@^0.1"
model        = "default"
"#,
    )
    .unwrap();
}

/// Write a v3 scope config.toml with `required_tier = "none"`.
///
/// When paired with `TAU_TESTING_ALLOW_MOCK_SANDBOX=1`, the
/// `resolve_strict_for_validation()` helper returns the mock adapter, so
/// validation still runs against a real (mock) adapter rather than
/// passthrough.
fn write_mock_sandbox_config(scope_dir: &std::path::Path) {
    std::fs::create_dir_all(scope_dir).unwrap();
    std::fs::write(
        scope_dir.join("config.toml"),
        r#"schema_version = 3
kind = "project"
created_at = "2026-05-04T00:00:00Z"
created_by_tau_version = "0.0.0"

[sandbox]
required_tier = "none"
"#,
    )
    .unwrap();
}

/// Write a lockfile with a plugin entry whose capabilities are declared in
/// a tau.toml at `<scope>.tau/packages/<name>/<version>/tau.toml`.
///
/// The plugin manifest uses standard capabilities (fs.read) that the
/// mock adapter supports.
fn write_plugin_fixture_with_standard_caps(root: &std::path::Path, name: &str, version: &str) {
    let pkg_dir = root.join(".tau").join("packages").join(name).join(version);
    std::fs::create_dir_all(&pkg_dir).unwrap();

    // Package tau.toml — declares fs.read capability (supported by mock).
    std::fs::write(
        pkg_dir.join("tau.toml"),
        format!(
            r#"name = "{name}"
version = "{version}"
description = "test plugin"
authors = []
source = "https://example.com/{name}.git"
kind = "tool"
dependencies = []

[[capabilities]]
kind = "fs.read"
paths = ["/tmp/**"]

[plugin]
provides = "tool"
kind = "rust-cargo"
bin = "{name}"
"#
        ),
    )
    .unwrap();

    // Lockfile entry.
    let now = "2026-05-01T00:00:00Z";
    let zero_sha = "0".repeat(40);
    // Note: binary_path doesn't need to exist for --check-sandbox
    // (we never spawn the plugin).
    let lockfile_content = format!(
        r#"schema_version = 4
generated_by_tau_version = "0.0.0"
generated_at = "{now}"

[[package]]
name = "{name}"
active_version = "{version}"
source = "https://example.com/{name}.git"

[[package.versions]]
version = "{version}"
resolved_commit = "{zero_sha}"
sha256 = ""
installed_at = "{now}"

[package.plugin]
binary_path = "/nonexistent/{name}"
built_at = "{now}"

[package.plugin.manifest]
provides = "tool"
kind = "rust-cargo"
bin = "{name}"
"#
    );
    std::fs::write(root.join("tau-lock.toml"), lockfile_content).unwrap();
}

/// Write a lockfile with a plugin entry whose capabilities include a
/// Custom capability (not supported by the mock adapter — always triggers
/// rejection).
fn write_plugin_fixture_with_custom_cap(root: &std::path::Path, name: &str, version: &str) {
    let pkg_dir = root.join(".tau").join("packages").join(name).join(version);
    std::fs::create_dir_all(&pkg_dir).unwrap();

    // Package tau.toml — declares a Custom (mcp.tool.use) capability.
    // MockSandbox does NOT support CapabilityShape::Custom.
    std::fs::write(
        pkg_dir.join("tau.toml"),
        format!(
            r#"name = "{name}"
version = "{version}"
description = "test plugin with custom cap"
authors = []
source = "https://example.com/{name}.git"
kind = "tool"
dependencies = []

[[capabilities]]
kind = "custom.mcp.tool.use"
tool = "some-tool"

[plugin]
provides = "tool"
kind = "rust-cargo"
bin = "{name}"
"#
        ),
    )
    .unwrap();

    let now = "2026-05-01T00:00:00Z";
    let zero_sha = "0".repeat(40);
    let lockfile_content = format!(
        r#"schema_version = 4
generated_by_tau_version = "0.0.0"
generated_at = "{now}"

[[package]]
name = "{name}"
active_version = "{version}"
source = "https://example.com/{name}.git"

[[package.versions]]
version = "{version}"
resolved_commit = "{zero_sha}"
sha256 = ""
installed_at = "{now}"

[package.plugin]
binary_path = "/nonexistent/{name}"
built_at = "{now}"

[package.plugin.manifest]
provides = "tool"
kind = "rust-cargo"
bin = "{name}"
"#
    );
    std::fs::write(root.join("tau-lock.toml"), lockfile_content).unwrap();
}

// ---- Test 1: mock adapter accepts standard capabilities -------------------

#[test]
fn mock_chain_accepts_standard_capabilities() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    // Set up project.
    std::fs::create_dir_all(root.join(".tau")).unwrap();
    write_tau_toml(root);
    write_mock_sandbox_config(&root.join(".tau"));
    write_plugin_fixture_with_standard_caps(root, "fs-plugin", "0.1.0");

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["resolve", "--check-sandbox"])
        .current_dir(root)
        .env("TAU_HOME", root.join(".tau-global"))
        .env("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "expected exit 0 for standard caps; stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains("✓ fs-plugin"),
        "expected '✓ fs-plugin' in stdout; got: {stdout}"
    );
    assert!(
        stdout.contains("1 plugins checked: 1 ok, 0 errors"),
        "expected summary line in stdout; got: {stdout}"
    );
}

// ---- Test 2: mock adapter rejects custom capability -----------------------

#[test]
fn mock_chain_rejects_custom_capability() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    std::fs::create_dir_all(root.join(".tau")).unwrap();
    write_tau_toml(root);
    write_mock_sandbox_config(&root.join(".tau"));
    write_plugin_fixture_with_custom_cap(root, "mcp-plugin", "0.1.0");

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["resolve", "--check-sandbox"])
        .current_dir(root)
        .env("TAU_HOME", root.join(".tau-global"))
        .env("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "expected exit 2 for custom cap rejection; stdout={stdout:?} stderr={stderr:?}"
    );
    assert_eq!(
        output.status.code(),
        Some(2),
        "exit code must be 2 (not 1); stdout={stdout:?}"
    );
    assert!(
        stdout.contains("✗ mcp-plugin"),
        "expected '✗ mcp-plugin' in stdout; got: {stdout}"
    );
    assert!(
        stdout.contains("1 plugins checked: 0 ok, 1 errors"),
        "expected summary line in stdout; got: {stdout}"
    );
}

// ---- Test 3: --json emits one JSON object per line -----------------------

#[test]
fn json_output_emits_per_line_events() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    std::fs::create_dir_all(root.join(".tau")).unwrap();
    write_tau_toml(root);
    write_mock_sandbox_config(&root.join(".tau"));
    write_plugin_fixture_with_standard_caps(root, "fs-plugin", "0.1.0");

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["--json", "resolve", "--check-sandbox"])
        .current_dir(root)
        .env("TAU_HOME", root.join(".tau-global"))
        .env("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "expected exit 0 in JSON mode; stdout={stdout:?} stderr={stderr:?}"
    );

    // Every non-empty output line must be valid JSON.
    let mut found_check_event = false;
    let mut found_summary_event = false;
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let obj: serde_json::Value =
            serde_json::from_str(line).expect("each line must be valid JSON");
        let event = obj["event"].as_str().unwrap_or("");
        match event {
            "sandbox_check" => {
                found_check_event = true;
                assert_eq!(obj["plugin_id"].as_str(), Some("fs-plugin"));
                assert_eq!(obj["status"].as_str(), Some("ok"));
            }
            "summary" => {
                found_summary_event = true;
                assert_eq!(obj["ok"].as_u64(), Some(1));
                assert_eq!(obj["errors"].as_u64(), Some(0));
            }
            _ => panic!("unexpected event type {event:?} in line: {line}"),
        }
    }

    assert!(
        found_check_event,
        "expected at least one sandbox_check event; stdout: {stdout}"
    );
    assert!(
        found_summary_event,
        "expected a summary event; stdout: {stdout}"
    );
}

// ---- Test 4: REMOVED (2026-08-23) ---------------------------------------
//
// `no_adapter_emits_clear_error` asserted the `required_tier = "strict"` +
// no-strict-capable-adapter error path. It was `#[ignore]`d pending a
// "sub-project D e2e CI" job that was never built, and no GitHub-hosted
// runner has that host shape: ubuntu-latest probes the Linux native
// (Landlock) adapter Available, macos-latest probes the darwin
// sandbox-exec adapter Available, windows-latest has Docker. On every one
// of them the resolver succeeds and the test fails (verified on macOS —
// `✓ fs-plugin / 1 plugins checked: 1 ok, 0 errors`). Rather than leave a
// test alive but permanently unrun, it is deleted. What remains covered:
// `ResolutionError::NoAdapterMatches`'s Display is unit-tested in
// `tau-runtime-tokio::process_gate::resolution_error`, and the resolver
// tests there accept `NoAdapterMatches` host-adaptively. The CLI's
// error-to-exit-2 mapping on that path (`cmd/resolve.rs`) is NOT covered —
// it was not covered before either, since this test never ran.

// ---- Test 5: empty lockfile (no plugins) reports 0 checked ---------------

#[test]
fn empty_lockfile_reports_zero_checked() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    std::fs::create_dir_all(root.join(".tau")).unwrap();
    write_tau_toml(root);
    write_mock_sandbox_config(&root.join(".tau"));
    // Write a lockfile with no plugin entries (data-only package).
    let now = "2026-05-01T00:00:00Z";
    let zero_sha = "0".repeat(40);
    std::fs::write(
        root.join("tau-lock.toml"),
        format!(
            r#"schema_version = 4
generated_by_tau_version = "0.0.0"
generated_at = "{now}"

[[package]]
name = "data-tool"
active_version = "1.0.0"
source = "https://example.com/data-tool.git"

[[package.versions]]
version = "1.0.0"
resolved_commit = "{zero_sha}"
sha256 = ""
installed_at = "{now}"
"#
        ),
    )
    .unwrap();

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["resolve", "--check-sandbox"])
        .current_dir(root)
        .env("TAU_HOME", root.join(".tau-global"))
        .env("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "expected exit 0 for empty plugin set; stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("0 plugins checked: 0 ok, 0 errors"),
        "expected '0 plugins checked' summary; stdout: {stdout}"
    );
}

// ---- Test 6: flag present in --help output --------------------------------

#[test]
fn check_sandbox_flag_appears_in_help() {
    Command::cargo_bin("tau")
        .unwrap()
        .args(["resolve", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--check-sandbox"));
}

// ---- Test 7: --check-sandbox skips passthrough and validates against mock -

/// When `required_tier = "none"`, the runtime would resolve to passthrough.
/// `--check-sandbox` must NOT validate against passthrough (every plan
/// trivially passes). Instead it calls `resolve_strict_for_validation()`,
/// which returns the mock adapter when `TAU_TESTING_ALLOW_MOCK_SANDBOX=1`.
/// Standard capabilities still pass against mock, so exit 0 is expected.
#[test]
fn check_sandbox_skips_passthrough_to_native_or_container() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    std::fs::create_dir_all(root.join(".tau")).unwrap();
    write_tau_toml(root);
    // required_tier = "none" → passthrough at runtime, but --check-sandbox
    // skips to the next-strict adapter (mock via env var here).
    write_mock_sandbox_config(&root.join(".tau"));
    write_plugin_fixture_with_standard_caps(root, "fs-plugin", "0.1.0");

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["resolve", "--check-sandbox"])
        .current_dir(root)
        .env("TAU_HOME", root.join(".tau-global"))
        .env("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "expected exit 0: required_tier=none + standard caps should pass against mock; stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains("✓ fs-plugin"),
        "expected '✓ fs-plugin' in stdout; got: {stdout}"
    );
    assert!(
        stdout.contains("1 plugins checked: 1 ok, 0 errors"),
        "expected summary line; got: {stdout}"
    );
}

// ---- Test 8: REMOVED (2026-08-23) ---------------------------------------
//
// `check_sandbox_errors_when_only_passthrough_available` asserted the
// `required_tier = "none"` + only-passthrough-available error path. Deleted
// for the same reason as Test 4 above: it was `#[ignore]`d for a host shape
// (no Docker, no Linux native, no darwin sandbox-exec) that no CI runner
// has, gated behind a "sub-project D e2e CI" job that does not exist, so it
// never ran anywhere. Verified failing on macOS before removal.

// ---- Test 9: plugin with custom cap triggers rejection (tier-none config) --

/// Variant of `mock_chain_rejects_custom_capability` that exercises the
/// passthrough-skip path: `required_tier = "none"` + mock env var
/// → `resolve_strict_for_validation()` returns mock → custom cap rejected.
///
/// This also doubles as a plugin-tier-mismatch surface test: if a plugin's
/// declared capability is unsupported by the strictest available adapter,
/// --check-sandbox reports ✗ with a clear reason.
#[test]
fn check_sandbox_surfaces_plugin_tier_mismatch() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    std::fs::create_dir_all(root.join(".tau")).unwrap();
    write_tau_toml(root);
    // required_tier = "none" → passthrough-skip → resolve_strict_for_validation
    // returns mock → custom cap is rejected because mock doesn't support Custom.
    write_mock_sandbox_config(&root.join(".tau"));
    write_plugin_fixture_with_custom_cap(root, "mcp-plugin", "0.1.0");

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["resolve", "--check-sandbox"])
        .current_dir(root)
        .env("TAU_HOME", root.join(".tau-global"))
        .env("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "expected exit 2: custom cap rejected by mock adapter; stdout={stdout:?} stderr={stderr:?}"
    );
    assert_eq!(
        output.status.code(),
        Some(2),
        "exit code must be 2; stdout={stdout:?}"
    );
    assert!(
        stdout.contains("✗ mcp-plugin"),
        "expected '✗ mcp-plugin' in stdout (plugin's capability is unsupported); got: {stdout}"
    );
    assert!(
        stdout.contains("1 plugins checked: 0 ok, 1 errors"),
        "expected summary with 1 error; got: {stdout}"
    );
}
