//! Integration tests for `tau verify`.
//!
//! Uses local `file://`-based git fixtures (bare repo + working repo
//! pattern) so the suite has no network requirement. Each test sets
//! `TAU_HOME` to an isolated tempdir so global-scope operations do not
//! leak across tests or pollute the developer's real `~/.tau`.
//!
//! Exit code contract (per ADR-0007 §7):
//! - 0: all packages Ok or Unverified.
//! - 2: any TreeDrift, BinaryDrift, or Missing.

mod common;

use std::path::PathBuf;

use assert_cmd::Command;
use predicates::prelude::*;

/// Test 1: after a clean install, `tau verify` reports all packages ok and exits 0.
#[test]
fn cmd_verify_clean_install_exits_0() {
    let (fixture, url, _bare) = common::setup_local_package_fixture("verify-clean-pkg", "1.0.0");
    let global_dir = fixture.path().join("scope-global");
    std::fs::create_dir_all(&global_dir).unwrap();

    // Install the package (real install computes sha256).
    Command::cargo_bin("tau")
        .unwrap()
        .args(["install", "--global", &url])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success();

    // Verify all: should be exit 0 with "ok" in output.
    Command::cargo_bin("tau")
        .unwrap()
        .args(["verify", "--global"])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success()
        .stdout(predicate::str::contains("ok"));
}

/// Test 2: after tampering with a file in the install tree, `tau verify` exits 2
/// and reports tree drift.
#[test]
fn cmd_verify_tampered_file_exits_2() {
    let (fixture, url, _bare) = common::setup_local_package_fixture("verify-tamper-pkg", "1.0.0");
    let global_dir = fixture.path().join("scope-global");
    std::fs::create_dir_all(&global_dir).unwrap();

    // Install the package.
    Command::cargo_bin("tau")
        .unwrap()
        .args(["install", "--global", &url])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success();

    // Tamper: write an extra file into the installed package tree.
    let pkg_dir = global_dir.join("packages/verify-tamper-pkg/1.0.0");
    assert!(
        pkg_dir.exists(),
        "install dir should exist: {}",
        pkg_dir.display()
    );
    std::fs::write(
        pkg_dir.join("tampered.txt"),
        b"this file was not there at install time",
    )
    .unwrap();

    // Verify: should exit 2 with drift info.
    Command::cargo_bin("tau")
        .unwrap()
        .args(["verify", "--global"])
        .env("TAU_HOME", &global_dir)
        .assert()
        .failure()
        .stdout(predicate::str::contains("drift (tree)"));
}

/// Test 3: if the install directory is removed after install, `tau verify`
/// exits 2 and reports the package as missing.
#[test]
fn cmd_verify_missing_install_dir_exits_2() {
    let (fixture, url, _bare) = common::setup_local_package_fixture("verify-missing-pkg", "1.0.0");
    let global_dir = fixture.path().join("scope-global");
    std::fs::create_dir_all(&global_dir).unwrap();

    // Install the package.
    Command::cargo_bin("tau")
        .unwrap()
        .args(["install", "--global", &url])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success();

    // Remove the install directory to simulate a corrupted/missing install.
    let pkg_dir = global_dir.join("packages/verify-missing-pkg/1.0.0");
    assert!(
        pkg_dir.exists(),
        "install dir should exist: {}",
        pkg_dir.display()
    );
    std::fs::remove_dir_all(&pkg_dir).unwrap();

    // Verify: should exit 2 with missing drift info.
    Command::cargo_bin("tau")
        .unwrap()
        .args(["verify", "--global"])
        .env("TAU_HOME", &global_dir)
        .assert()
        .failure()
        .stdout(predicate::str::contains("drift (missing)"));
}

/// Test 4: a v2-leftover lockfile entry (empty sha256) with an existing install
/// dir results in `Unverified` status and exit 0 — this is NOT drift.
#[test]
fn cmd_verify_v2_leftover_unverified_exits_0() {
    let global_dir = tempfile::tempdir().unwrap();
    let global_path = global_dir.path();

    // Hand-author a lockfile with empty sha256 (v2-leftover format).
    let now_rfc3339 = "2026-04-28T00:00:00Z";
    let resolved_commit = "0".repeat(40);
    let lockfile_contents = format!(
        r#"schema_version = 1
generated_by_tau_version = "0.0.0"
generated_at = "{now_rfc3339}"

[[package]]
name = "leftover-pkg"
active_version = "2.0.0"
source = "https://example.com/leftover-pkg.git"

[[package.versions]]
version = "2.0.0"
resolved_commit = "{resolved_commit}"
sha256 = ""
installed_at = "{now_rfc3339}"
"#
    );
    std::fs::write(global_path.join("tau-lock.toml"), lockfile_contents).unwrap();

    // Create the install dir so it's "present" (sha256 empty → Unverified, not Missing).
    let pkg_dir = global_path.join("packages/leftover-pkg/2.0.0");
    std::fs::create_dir_all(&pkg_dir).unwrap();
    std::fs::write(
        pkg_dir.join("tau.toml"),
        b"name = \"leftover-pkg\"\nversion = \"2.0.0\"\n",
    )
    .unwrap();

    // Verify: should exit 0 with "unverified" in output.
    Command::cargo_bin("tau")
        .unwrap()
        .args(["verify", "--global"])
        .env("TAU_HOME", global_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("unverified"));
}

// ---------------------------------------------------------------------------
// Helper for anthropic_strict tests
// ---------------------------------------------------------------------------

/// Build a minimal skill lockfile entry (schema_version = 6) with a
/// [package.skill] section so that verify_all_with_options treats it as
/// a skill package and applies the Anthropic conformance check.
fn skill_lockfile_toml(name: &str, version: &str) -> String {
    format!(
        "schema_version = 6\n\
         generated_by_tau_version = \"0.0.0\"\n\
         generated_at = \"2026-05-12T10:00:00Z\"\n\n\
         [[package]]\n\
         name = \"{name}\"\n\
         active_version = \"{version}\"\n\
         source = \"https://example.com/{name}.git\"\n\
         \n\
         [package.skill]\n\
         content_sha256 = \"\"\n\
         [package.skill.frontmatter]\n\
         name = \"{name}\"\n\
         description = \"Test skill.\"\n\
         \n\
         [[package.versions]]\n\
         version = \"{version}\"\n\
         resolved_commit = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n\
         sha256 = \"\"\n\
         installed_at = \"2026-05-12T10:00:00Z\"\n"
    )
}

/// Write a minimal tau.toml + SKILL.md into `pkg_dir`.
/// The `skill_md` content is written verbatim.
fn write_skill_package(pkg_dir: &std::path::Path, name: &str, version: &str, skill_md: &str) {
    std::fs::create_dir_all(pkg_dir).unwrap();
    // Write a minimal tau.toml so show / verify can parse the manifest.
    let tau_toml = format!(
        "name = \"{name}\"\nversion = \"{version}\"\ndescription = \"Test skill.\"\n\
         authors = []\nsource = \"https://example.com/{name}.git\"\nkind = \"skill\"\n\
         dependencies = []\ncapabilities = []\n\n[skill]\n"
    );
    std::fs::write(pkg_dir.join("tau.toml"), tau_toml).unwrap();
    std::fs::write(pkg_dir.join("SKILL.md"), skill_md).unwrap();
}

/// Test 6: `tau verify --anthropic-strict` exits 0 for a conformant skill.
///
/// A skill with a well-formed frontmatter, non-empty description, and
/// non-empty body must not produce any AnthropicConformance entries.
#[test]
fn verify_anthropic_strict_passes_for_conformant_skill() {
    let global_dir = tempfile::tempdir().unwrap();
    let global_path = global_dir.path();

    // Write the lockfile.
    std::fs::write(
        global_path.join("tau-lock.toml"),
        skill_lockfile_toml("good-skill", "1.0.0"),
    )
    .unwrap();

    // Write the package dir with a conformant SKILL.md.
    let pkg_dir = global_path.join("packages/good-skill/1.0.0");
    write_skill_package(
        &pkg_dir,
        "good-skill",
        "1.0.0",
        "---\nname: good-skill\ndescription: Does something useful.\n---\nDo the thing.\n",
    );

    // Run tau verify --anthropic-strict --global.
    Command::cargo_bin("tau")
        .unwrap()
        .args(["verify", "--global", "--anthropic-strict"])
        .env("TAU_HOME", global_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("ok").or(predicate::str::contains("unverified")));
}

/// Test 7: `tau verify --anthropic-strict` exits 2 for a skill with an
/// empty description field.
///
/// Validates that the `MissingDescription` conformance issue is raised and
/// renders as "AnthropicConformance" in the output.
#[test]
fn verify_anthropic_strict_fails_for_missing_description() {
    let global_dir = tempfile::tempdir().unwrap();
    let global_path = global_dir.path();

    // Write the lockfile.
    std::fs::write(
        global_path.join("tau-lock.toml"),
        skill_lockfile_toml("bad-skill", "1.0.0"),
    )
    .unwrap();

    // Write the package dir with a SKILL.md that has an empty description.
    let pkg_dir = global_path.join("packages/bad-skill/1.0.0");
    write_skill_package(
        &pkg_dir,
        "bad-skill",
        "1.0.0",
        // description is empty (whitespace only); body is non-empty.
        "---\nname: bad-skill\ndescription: \"\"\n---\nDo the thing.\n",
    );

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["verify", "--global", "--anthropic-strict"])
        .env("TAU_HOME", global_path)
        .output()
        .expect("tau verify ran");

    assert!(
        !output.status.success(),
        "expected non-zero exit when description is empty"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("AnthropicConformance"),
        "expected 'AnthropicConformance' in output for missing description; got: {stdout}"
    );
}

/// Test 7b: `tau verify --anthropic-strict` exits 2 for a skill whose
/// SKILL.md body (after the closing `---`) is empty / whitespace-only.
/// Validates the `AnthropicConformanceIssue::EmptyBody` variant.
#[test]
fn verify_anthropic_strict_fails_for_empty_body() {
    let global_dir = tempfile::tempdir().unwrap();
    let global_path = global_dir.path();

    std::fs::write(
        global_path.join("tau-lock.toml"),
        skill_lockfile_toml("empty-body-skill", "1.0.0"),
    )
    .unwrap();

    let pkg_dir = global_path.join("packages/empty-body-skill/1.0.0");
    write_skill_package(
        &pkg_dir,
        "empty-body-skill",
        "1.0.0",
        // Frontmatter is valid; the body after `---` is whitespace-only.
        "---\nname: empty-body-skill\ndescription: A skill with no body.\n---\n   \n\n",
    );

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["verify", "--global", "--anthropic-strict"])
        .env("TAU_HOME", global_path)
        .output()
        .expect("tau verify ran");

    assert!(
        !output.status.success(),
        "expected non-zero exit when SKILL.md body is empty"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("AnthropicConformance"),
        "expected 'AnthropicConformance' in output for empty body; got: {stdout}"
    );
}

/// Test 7c: `tau verify --anthropic-strict` exits 2 for a skill whose
/// SKILL.md frontmatter is malformed (missing closing `---`).
/// Validates the `AnthropicConformanceIssue::MalformedFrontmatter` variant.
#[test]
fn verify_anthropic_strict_fails_for_malformed_frontmatter() {
    let global_dir = tempfile::tempdir().unwrap();
    let global_path = global_dir.path();

    std::fs::write(
        global_path.join("tau-lock.toml"),
        skill_lockfile_toml("broken-frontmatter-skill", "1.0.0"),
    )
    .unwrap();

    let pkg_dir = global_path.join("packages/broken-frontmatter-skill/1.0.0");
    // SKILL.md begins with `---` but never closes the frontmatter block.
    // The frontmatter parser must reject this with MalformedFrontmatter
    // rather than silently treat the entire file as body.
    write_skill_package(
        &pkg_dir,
        "broken-frontmatter-skill",
        "1.0.0",
        "---\nname: broken-frontmatter-skill\ndescription: missing closing fence\nDo the thing.\n",
    );

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["verify", "--global", "--anthropic-strict"])
        .env("TAU_HOME", global_path)
        .output()
        .expect("tau verify ran");

    assert!(
        !output.status.success(),
        "expected non-zero exit when SKILL.md frontmatter is malformed"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("AnthropicConformance"),
        "expected 'AnthropicConformance' in output for malformed frontmatter; got: {stdout}"
    );
}

/// Test 8: `tau verify` (without `--anthropic-strict`) exits 0 even for a
/// skill that would fail the strict conformance check.
///
/// Validates that the flag is required to trigger Anthropic conformance
/// checks — the default verify must not surface AnthropicConformance.
#[test]
fn verify_without_flag_does_not_check_anthropic_conformance() {
    let global_dir = tempfile::tempdir().unwrap();
    let global_path = global_dir.path();

    // Write the lockfile.
    std::fs::write(
        global_path.join("tau-lock.toml"),
        skill_lockfile_toml("lax-skill", "1.0.0"),
    )
    .unwrap();

    // Write the package dir with a SKILL.md that would fail strict.
    let pkg_dir = global_path.join("packages/lax-skill/1.0.0");
    write_skill_package(
        &pkg_dir,
        "lax-skill",
        "1.0.0",
        // description is whitespace-only; would fail --anthropic-strict.
        "---\nname: lax-skill\ndescription: \"   \"\n---\nDo the thing.\n",
    );

    // Run without --anthropic-strict: should exit 0 (only sha256 drift is
    // checked, and we seeded sha256 = "" which gives Unverified, not drift).
    Command::cargo_bin("tau")
        .unwrap()
        .args(["verify", "--global"])
        .env("TAU_HOME", global_path)
        .assert()
        .success();

    // Re-run with JSON mode and assert no "anthropic_conformance" entries.
    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["--json", "verify", "--global"])
        .env("TAU_HOME", global_path)
        .output()
        .expect("tau --json verify ran");

    assert!(
        output.status.success(),
        "expected exit 0 without --anthropic-strict"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("anthropic_conformance"),
        "expected no 'anthropic_conformance' entries without --anthropic-strict; got:\n{stdout}"
    );
}

/// Test 5 (original): `tau verify --json` emits one JSON object per line on stdout,
/// each with an "event" field.
#[test]
fn cmd_verify_json_mode_emits_one_event_per_line() {
    let (fixture, url, _bare) = common::setup_local_package_fixture("verify-json-pkg", "1.0.0");
    let global_dir = fixture.path().join("scope-global");
    std::fs::create_dir_all(&global_dir).unwrap();

    // Install the package so there is something to verify.
    Command::cargo_bin("tau")
        .unwrap()
        .args(["install", "--global", &url])
        .env("TAU_HOME", &global_dir)
        .assert()
        .success();

    // Run verify in JSON mode.
    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["--json", "verify", "--global"])
        .env("TAU_HOME", &global_dir)
        .output()
        .expect("tau --json verify ran");

    // Exit 0 (clean install → all ok).
    assert!(
        output.status.success(),
        "expected exit 0 for clean install; got: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // Every non-empty stdout line must be valid JSON with an "event" field.
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let mut line_count = 0usize;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(trimmed)
            .unwrap_or_else(|e| panic!("line is not valid JSON: {trimmed:?}\nerror: {e}"));
        assert!(
            v.get("event").is_some(),
            "JSON line missing \"event\" field: {trimmed}"
        );
        line_count += 1;
    }

    assert!(
        line_count >= 3,
        "expected at least 3 JSON lines (started + package + completed), got {line_count}\nstdout: {stdout}",
    );
}

// ---------------------------------------------------------------------------
// `tau verify --wasm <project> --wit <path>` (EPIC 3.5)
// ---------------------------------------------------------------------------

/// Absolute path to a committed wasm-build fixture project.
fn wasm_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm-build")
        .join(name)
}

/// Test W1: a `.wit` produced by re-deriving the project's own world verifies
/// reproducible and exits 0.
#[test]
fn verify_wasm_matching_wit_exits_0() {
    let project = wasm_fixture("net-http");
    let world = tau_cli::cmd::build_wasm::wasm_world_for_project(&project).unwrap();
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), &world).unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args([
            "verify",
            "--wasm",
            project.to_str().unwrap(),
            "--wit",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("reproducible"));
}

/// Test W2: a tampered `.wit` (extra import line injected) exits 2 and names
/// the differing line.
#[test]
fn verify_wasm_tampered_wit_exits_2() {
    let project = wasm_fixture("net-http");
    let world = tau_cli::cmd::build_wasm::wasm_world_for_project(&project).unwrap();
    let tampered = world.replace(
        "world runner {",
        "world runner {\n    import wasi:sockets/instance-network@0.2.3;",
    );
    assert_ne!(tampered, world, "tamper must change the world text");
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), &tampered).unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args([
            "verify",
            "--wasm",
            project.to_str().unwrap(),
            "--wit",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("wasi:sockets"));
}

/// Test W3: empty-cap invariant — a host-only project re-derives to a world
/// byte-equal to the committed `wit-baseline/runner.wit`, so verifying against
/// that baseline exits 0. Guards the frozen baseline against generator drift.
#[test]
fn verify_wasm_host_only_matches_committed_baseline() {
    let project = wasm_fixture("trivial");
    let baseline = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tau-wasm-guest/wit-baseline/runner.wit");

    Command::cargo_bin("tau")
        .unwrap()
        .args([
            "verify",
            "--wasm",
            project.to_str().unwrap(),
            "--wit",
            baseline.to_str().unwrap(),
        ])
        .assert()
        .success();
}

/// Test W4: a project that cannot target wasm (declares a process-exec tool)
/// exits 1 (operational error), NOT 2 (drift).
#[test]
fn verify_wasm_not_buildable_exits_1_not_2() {
    let project = wasm_fixture("needs-exec");
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), "irrelevant\n").unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args([
            "verify",
            "--wasm",
            project.to_str().unwrap(),
            "--wit",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .code(1);
}

/// Test W5: a missing `--wit` file exits 1 with a clear read error.
#[test]
fn verify_wasm_missing_wit_exits_1() {
    let project = wasm_fixture("trivial");

    Command::cargo_bin("tau")
        .unwrap()
        .args([
            "verify",
            "--wasm",
            project.to_str().unwrap(),
            "--wit",
            "/nonexistent/does-not-exist.wit",
        ])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("does-not-exist.wit"));
}
