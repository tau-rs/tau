//! Integration: governance lattice L3 (`agent ⊇ spawn`) with a real
//! installed package manifest (Task 8 conformance Fixture D).
//!
//! Carry-forward from Task 6 (`governance.rs`'s `Deviation 2`): the L3
//! `spawn_exceeds_agent` path is only reachable via `AgentCaps::Resolved`,
//! which needs a real lockfile + installed package directory — the unit
//! tests in `governance.rs` use a lockfile-less `CheckCtx` and so can't
//! reach it. This file reuses `cmd_check_lattice.rs`'s exact
//! lockfile/installed-manifest harness ("governance lattice L1/L2 with an
//! installed package manifest") to close that gap at the CLI/E2E layer.
//!
//! The installed package's manifest declares
//! `{ kind = "agent.spawn", allowed_kinds = ["greedy"] }` — the agent's
//! *entire* granted capability set is "may spawn kind greedy", nothing
//! else. `[agent.kinds.greedy]` then declares a `net.http` capability,
//! which is NOT a subset of the agent's effective grant (which has no
//! `net.http` at all) → `tau.governance.spawn_exceeds_agent`.
//!
//! Note: the root `[allow]` ceiling structurally cannot admit
//! `agent.spawn` as a raw ceiling key (`agent.spawn` "flows through the
//! lattice's spawn link, not a raw ceiling entry" — see
//! `tau-pkg/src/project/allow.rs`'s `agent_spawn_key_rejected`), so this
//! fixture also collaterally trips L1 (`package_exceeds_allow`) for the
//! same package. That's expected and harmless: this test only asserts
//! `spawn_exceeds_agent` is *present* among the findings, not that it is
//! the sole one.

#[path = "check_common.rs"]
mod check_common;

use assert_cmd::Command;
use tempfile::TempDir;

/// Write project scope + lockfile + an installed package manifest with the
/// given capabilities, plus the project tau.toml. Copied from
/// `cmd_check_lattice.rs::setup` (not shared — that file's helper is
/// private) to keep this fixture's harness byte-identical to the proven
/// L1/L2 mechanism.
fn setup(root: &std::path::Path, pkg_caps: &str, project_toml: &str) {
    std::fs::create_dir_all(root.join(".tau")).unwrap();
    std::fs::write(
        root.join(".tau").join("config.toml"),
        "schema_version = 3\nkind = \"project\"\ncreated_at = \"2026-08-21T00:00:00Z\"\ncreated_by_tau_version = \"0.0.0\"\n\n[sandbox]\nrequired_tier = \"none\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tau-lock.toml"),
        format!(
            "schema_version = 4\ngenerated_by_tau_version = \"0.0.0\"\ngenerated_at = \"2026-08-21T00:00:00Z\"\n\n[[package]]\nname = \"demo\"\nactive_version = \"0.1.0\"\nsource = \"https://example.com/demo.git\"\n\n[[package.versions]]\nversion = \"0.1.0\"\nresolved_commit = \"{zero}\"\nsha256 = \"\"\ninstalled_at = \"2026-08-21T00:00:00Z\"\n",
            zero = "0".repeat(40)
        ),
    )
    .unwrap();
    let inst = root
        .join(".tau")
        .join("packages")
        .join("demo")
        .join("0.1.0");
    std::fs::create_dir_all(&inst).unwrap();
    std::fs::write(
        inst.join("tau.toml"),
        format!(
            "name = \"demo\"\nversion = \"0.1.0\"\ndescription = \"d\"\nauthors = []\nsource = \"https://example.com/demo.git\"\nkind = \"tool\"\ndependencies = []\ncapabilities = {pkg_caps}\n"
        ),
    )
    .unwrap();
    std::fs::write(root.join("tau.toml"), project_toml).unwrap();
}

#[test]
fn spawn_exceeds_agent_via_installed_package_fails_exit_2() {
    check_common::ensure_tau_home();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    setup(
        root,
        r#"[{ kind = "agent.spawn", allowed_kinds = ["greedy"] }]"#,
        r#"
packages = ["demo"]

[project]
name = "demo"

[allow]
"fs.read" = { paths = ["/proj/**"] }

[allow.models.fast]
backend = "demo"
model = "m-1"

[agent.kinds.greedy]
capabilities = { "net.http" = { hosts = ["evil.example"] } }

[agents.solo]
display_name = "Solo"
package = "demo@^0.1"
model = "fast"
"#,
    );

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["check", "governance", "--json"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "spawn kind exceeding the agent's effective grant must fail tau check\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let found = stdout.lines().any(|line| {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            return false;
        };
        v["type"] == "check_finished"
            && v["findings"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|f| f["rule_id"] == "tau.governance.spawn_exceeds_agent")
    });
    assert!(
        found,
        "expected a tau.governance.spawn_exceeds_agent finding in JSON output, got:\n{stdout}"
    );
}

/// Companion coverage for the agent-path `unknown_spawn_kind` branch (no
/// `[agent.kinds.*]` defined for the kind the package's manifest lists as
/// spawnable). Both branches of L3 live inside the same
/// `AgentCaps::Resolved` match arm, so this still needs the same
/// installed-package harness as the `spawn_exceeds_agent` test above.
#[test]
fn unknown_spawn_kind_via_installed_package_fails_exit_2() {
    check_common::ensure_tau_home();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    setup(
        root,
        r#"[{ kind = "agent.spawn", allowed_kinds = ["ghost"] }]"#,
        r#"
packages = ["demo"]

[project]
name = "demo"

[allow]
"fs.read" = { paths = ["/proj/**"] }

[allow.models.fast]
backend = "demo"
model = "m-1"

[agents.solo]
display_name = "Solo"
package = "demo@^0.1"
model = "fast"
"#,
    );

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["check", "governance", "--json"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "spawning an undefined kind must fail tau check\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let found = stdout.lines().any(|line| {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            return false;
        };
        v["type"] == "check_finished"
            && v["findings"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|f| f["rule_id"] == "tau.governance.unknown_spawn_kind")
    });
    assert!(
        found,
        "expected a tau.governance.unknown_spawn_kind finding in JSON output, got:\n{stdout}"
    );
}
