//! Integration tests: `tau check` exit-code contract.
//!
//! Mandated by the check design spec
//! (`docs/superpowers/specs/2026-05-18-tau-check-design.md` §9, test-file
//! table) but never written. The taxonomy under test, from spec §8:
//!
//! | Exit | Meaning |
//! |---|---|
//! | `0`  | All selected checks passed |
//! | `2`  | At least one `Severity::Error` finding |
//! | `3`  | Only `Severity::NeedsSetup` findings (missing packages); run `tau resolve` and retry |
//! | `64` | Usage error (sysexits E_USAGE) |
//! | `70` | Internal error / runner panic (sysexits E_SOFTWARE) |
//!
//! Individual codes are also asserted in the per-category test files
//! (`cmd_check_config.rs` → 2, `cmd_check_packages.rs` → 3,
//! `cmd_check_target.rs` → 64). What lives here and nowhere else is the
//! **precedence** rule the spec calls out at §8:
//!
//! > `Severity::Error` beats `NeedsSetup` — surface real bugs to the
//! > developer before they get masked by a "needs setup" wall.
//!
//! Exit 70 is deliberately not covered: it fires only on a runner-level
//! panic, which no fixture can provoke without `panic!`-injection the
//! production code does not expose. `cmd_check_target.rs:70` asserts the
//! negative (a normal run never returns 70), which is the reachable half.

#[path = "check_common.rs"]
mod check_common;

use assert_cmd::Command;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/check")
        .join(name)
}

/// Copy a fixture's `tau.toml` into a fresh tempdir project.
fn project_from_fixture(tmp: &TempDir, name: &str) -> PathBuf {
    let proj = tmp.path().join("proj");
    std::fs::create_dir(&proj).unwrap();
    std::fs::copy(fixture(name).join("tau.toml"), proj.join("tau.toml")).unwrap();
    proj
}

/// Pin the sandbox tier so `tau check`'s sandbox category resolves
/// deterministically regardless of what adapters the host offers.
fn write_scope_config(root: &Path) {
    std::fs::create_dir_all(root.join(".tau")).unwrap();
    std::fs::write(
        root.join(".tau").join("config.toml"),
        "schema_version = 3\nkind = \"project\"\ncreated_at = \"2026-06-19T00:00:00Z\"\n\
         created_by_tau_version = \"0.0.0\"\n\n[sandbox]\nrequired_tier = \"none\"\n",
    )
    .unwrap();
}

fn run_check(proj: &Path, args: &[&str]) -> std::process::Output {
    Command::cargo_bin("tau")
        .unwrap()
        .args(args)
        .current_dir(proj)
        .output()
        .unwrap()
}

fn describe(out: &std::process::Output) -> String {
    format!(
        "\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    )
}

#[test]
fn clean_project_exits_0() {
    check_common::ensure_tau_home();
    let tmp = TempDir::new().unwrap();
    let proj = project_from_fixture(&tmp, "clean-project");
    write_scope_config(&proj);

    let out = run_check(&proj, &["check"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "clean fixture must exit 0{}",
        describe(&out)
    );
}

#[test]
fn error_finding_exits_2() {
    check_common::ensure_tau_home();
    let tmp = TempDir::new().unwrap();
    let proj = project_from_fixture(&tmp, "bad-config-project");

    let out = run_check(&proj, &["check"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "an Error finding must exit 2{}",
        describe(&out)
    );
}

#[test]
fn needs_setup_only_exits_3() {
    check_common::ensure_tau_home();
    let tmp = TempDir::new().unwrap();
    let proj = project_from_fixture(&tmp, "missing-package-project");

    let out = run_check(&proj, &["check", "packages"]);
    assert_eq!(
        out.status.code(),
        Some(3),
        "a NeedsSetup-only run must exit 3{}",
        describe(&out)
    );
}

/// The spec's precedence rule, and the reason this file exists: when a
/// single run produces BOTH severities, Error wins and the exit is 2 —
/// a real bug must not be masked behind a "needs setup" wall.
///
/// Asserting the exit code alone would be weak (a config-only failure
/// also exits 2), so this parses `--json` and proves both severities were
/// actually present in the same run before checking the code.
#[test]
fn error_beats_needs_setup_exits_2() {
    check_common::ensure_tau_home();
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir(&proj).unwrap();
    write_scope_config(&proj);

    // `[tools.fetch]` reaches outside the `[allow]` ceiling  → governance
    //     emits Severity::Error.
    // `agents.thing` requires a tool that is in no lockfile → packages
    //     emits Severity::NeedsSetup.
    // Both categories run off the parsed tau.toml alone, so no lockfile or
    // installed package is needed to stage the collision.
    std::fs::write(
        proj.join("tau.toml"),
        r#"
[project]
name = "precedence-demo"

[allow]
"fs.read" = { paths = ["/proj/**"] }

# With an [allow] ceiling declared, model aliases must live under
# [allow.models] — a bare [models] table leaves the alias unresolvable and
# fails config validation, which would collapse this fixture to a single
# Error finding and defeat the point of the test.
[allow.models.default]
backend = "echo-llm"
model   = "claude-haiku-4-5"

[allow.tools.fetch]
native = "Fetch"

[tools.fetch]
native = "Fetch"
capabilities = [{ kind = "fs.read", paths = ["/etc/**"] }]

[agents.thing]
display_name  = "agent needing a missing tool"
package       = "echo-llm@^0.1"
model         = "default"
prompt.system = "test"

[[agents.thing.requires.tools]]
name    = "never-installed"
source  = "https://example.com/never-installed.git"
version = "^0.1"
"#,
    )
    .unwrap();

    let out = run_check(&proj, &["check", "--json"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    let summary = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|v| v["type"] == "run_finished")
        .unwrap_or_else(|| panic!("no run_finished line in --json output{}", describe(&out)));

    let errors = summary["summary"]["by_severity"]["error"].as_u64().unwrap();
    let setup = summary["summary"]["by_severity"]["needs-setup"]
        .as_u64()
        .unwrap();

    assert!(
        errors >= 1 && setup >= 1,
        "fixture must stage BOTH severities to test precedence, \
         got error={errors} needs-setup={setup}{}",
        describe(&out)
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "Error must beat NeedsSetup (spec §8){}",
        describe(&out)
    );
}

#[test]
fn usage_error_exits_64() {
    check_common::ensure_tau_home();
    let tmp = TempDir::new().unwrap();
    let proj = project_from_fixture(&tmp, "clean-project");

    let out = run_check(
        &proj,
        &["check", "sandbox", "--target", "bogus-bogus-bogus"],
    );
    assert_eq!(
        out.status.code(),
        Some(64),
        "an unparseable --target triple is a usage error{}",
        describe(&out)
    );
}

/// A freshly scaffolded project exits **2**, and that is intentional.
///
/// `tau init` writes an example agent with empty `package` / `model` for
/// the user to fill in; those blanks fail `ProjectConfig` validation, so
/// the `config` category emits `Severity::Error`. Exit 3 would be wrong:
/// spec §8 scopes it to "missing packages; run `tau resolve` and retry",
/// i.e. conditions a *command* resolves. Nothing but the user editing the
/// file resolves a blank field.
///
/// A 2026-08-23 QA sweep filed this as a defect. It is not one — this test
/// pins the behaviour so it does not get "fixed" by a later reader.
/// `crates/tau-cli/src/cmd/init.rs`'s `scaffold_template_validates_via_project_config`
/// pins the same decision one layer down.
#[test]
fn fresh_scaffold_exits_2_by_design() {
    check_common::ensure_tau_home();
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("scaffold-demo");
    std::fs::create_dir(&proj).unwrap();

    let init = Command::cargo_bin("tau")
        .unwrap()
        .arg("init")
        .current_dir(&proj)
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "tau init must succeed{}",
        describe(&init)
    );

    let out = run_check(&proj, &["check"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "a fresh scaffold exits 2 by design (spec §8){}",
        describe(&out)
    );
}
