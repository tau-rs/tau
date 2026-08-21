//! Integration: `tau check governance` on an over-reaching EPIC 4.4 dynamic
//! region (Task 8 conformance Fixture B — the end-to-end anchor of the
//! slice).
//!
//! Mirrors `cmd_check_governance.rs`/`cmd_check_lattice.rs`'s harness
//! (project scope `.tau/config.toml` + `tau.toml`, no lockfile needed since
//! the region owns no real agent) and reuses the exact greedy-kind fixture
//! from `governance.rs::over_reaching_spawn_in_region_fails_check`: root
//! `[allow]` and `[agent.kinds.greedy]` both grant `net.http hosts = "any"`,
//! but the dynamic region's own ceiling only permits `api.crawler.test` —
//! greedy's grant ⊄ region ceiling.

#[path = "check_common.rs"]
mod check_common;

use assert_cmd::Command;
use tempfile::TempDir;

fn write_scope(root: &std::path::Path) {
    std::fs::create_dir_all(root.join(".tau")).unwrap();
    std::fs::write(
        root.join(".tau").join("config.toml"),
        "schema_version = 3\nkind = \"project\"\ncreated_at = \"2026-08-21T00:00:00Z\"\ncreated_by_tau_version = \"0.0.0\"\n\n[sandbox]\nrequired_tier = \"none\"\n",
    )
    .unwrap();
}

#[test]
fn over_reaching_spawn_in_region_fails_check_exit_2() {
    check_common::ensure_tau_home();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_scope(root);
    std::fs::write(
        root.join("tau.toml"),
        r#"
[project]
name = "demo"

[allow]
"net.http" = { hosts = "any" }

[agent.kinds.greedy]
capabilities = { "net.http" = { hosts = "any" } }

[[pipeline.steps]]
id = "fanout"

[pipeline.steps.dynamic]
spawns = ["greedy"]
ceiling = { "net.http" = { hosts = ["api.crawler.test"] } }
max_spawns = 4
max_concurrency = 2
"#,
    )
    .unwrap();

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["check", "governance", "--json"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "over-reaching spawn must fail tau check\nstdout: {}\nstderr: {}",
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
                .any(|f| f["rule_id"] == "tau.governance.spawn_exceeds_region")
    });
    assert!(
        found,
        "expected a tau.governance.spawn_exceeds_region finding in JSON output, got:\n{stdout}"
    );
}
