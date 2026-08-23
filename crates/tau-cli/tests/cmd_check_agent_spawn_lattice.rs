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
//! `tau-pkg/src/project/allow.rs`'s `agent_spawn_key_rejected`). L1 in
//! `governance.rs` therefore *excludes* spawn caps from its raw-ceiling
//! subset check (governing them via the spawn link, L3, instead), so a
//! spawn-capable manifest no longer collaterally trips
//! `package_exceeds_allow`. `clean_positive_agent_spawn_passes` below
//! asserts that clean path; the over-reach fixtures here only assert their
//! spawn-link finding is *present*, which the exclusion leaves intact.

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

/// Positive case (the point of the L1 spawn-cap exemption): a package whose
/// manifest declares `agent.spawn` alongside a real `fs.read` grant, with a
/// fitting `[agent.kinds.worker]`, must pass `tau check governance` CLEANLY.
///
/// Before the exemption this was impossible — the manifest's `agent.spawn`
/// cap has no matching key in root `[allow]` (it structurally can't;
/// `ALLOW_CEILING_KINDS` excludes it), so L1's raw-ceiling subset check
/// tripped `package_exceeds_allow` for *every* spawn-capable agent. L1 now
/// excludes spawn caps (they're governed by the spawn link, L3), so the
/// non-spawn cap (`fs.read ⊆ root`) passes L1 and the spawn kind
/// (`fs.read ⊆ agent.effective`) passes L3 → no findings.
#[test]
fn clean_positive_agent_spawn_passes() {
    check_common::ensure_tau_home();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    setup(
        root,
        r#"[{ kind = "fs.read", paths = ["/proj/**"] }, { kind = "agent.spawn", allowed_kinds = ["worker"] }]"#,
        r#"
packages = ["demo"]

[project]
name = "demo"

[allow]
"fs.read" = { paths = ["/proj/**"] }

[allow.models.fast]
backend = "demo"
model = "m-1"

[agent.kinds.worker]
capabilities = { "fs.read" = { paths = ["/proj/**"] } }

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
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    assert!(
        output.status.success(),
        "a spawn-capable agent with a fitting kind must pass tau check cleanly\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );

    // Neither the L1 collateral trip nor any spawn-link violation may appear.
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v["type"] != "check_finished" {
            continue;
        }
        for f in v["findings"].as_array().into_iter().flatten() {
            let rule = f["rule_id"].as_str().unwrap_or("");
            assert!(
                rule != "tau.governance.package_exceeds_allow"
                    && rule != "tau.governance.spawn_exceeds_agent"
                    && rule != "tau.governance.unknown_spawn_kind",
                "unexpected governance finding '{rule}' in clean positive case:\n{stdout}"
            );
        }
    }
}

/// The spawn-cap exemption is surgical: an `agent.spawn` cap must NOT mask a
/// real *non-spawn* over-reach. Here the manifest declares `agent.spawn`
/// (exempt from L1) plus an `fs.read` outside the root ceiling — the latter
/// must still trip `package_exceeds_allow` at L1.
#[test]
fn agent_spawn_does_not_mask_nonspawn_overreach() {
    check_common::ensure_tau_home();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    setup(
        root,
        r#"[{ kind = "fs.read", paths = ["/etc/**"] }, { kind = "agent.spawn", allowed_kinds = ["worker"] }]"#,
        r#"
packages = ["demo"]

[project]
name = "demo"

[allow]
"fs.read" = { paths = ["/proj/**"] }

[allow.models.fast]
backend = "demo"
model = "m-1"

[agent.kinds.worker]
capabilities = { "fs.read" = { paths = ["/proj/**"] } }

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
        "a non-spawn cap outside the root ceiling must still fail L1\nstdout: {}\nstderr: {}",
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
                .any(|f| f["rule_id"] == "tau.governance.package_exceeds_allow")
    });
    assert!(
        found,
        "expected tau.governance.package_exceeds_allow for the fs.read over-reach, got:\n{stdout}"
    );
}
