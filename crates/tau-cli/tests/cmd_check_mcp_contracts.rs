//! Integration tests: `tau check mcp-contracts`
//!
//! Verifies that the phase passes when pin hashes match the lockfile, and
//! exits with code 2 when there is lockfile drift.

#[path = "check_common.rs"]
mod check_common;

use assert_cmd::Command;
use std::collections::BTreeMap;
use tau_mcp::contract::pinned::PinnedContract;
use tau_mcp::contract::server_contract::{ContractTool, ServerContract};
use tau_mcp::protocol::initialize::ServerInfo;
use tau_mcp::protocol::tools::McpToolInputSchema;
use tau_pkg::lockfile::{LockedMcpEntry, LockedMcpExpandedTool};
use tau_pkg::LockFile;
use tempfile::TempDir;


/// Build a minimal `PinnedContract` for a "weather" server.
fn build_pin(url: &str) -> PinnedContract {
    let contract = ServerContract {
        protocol_version: "2025-03-26".to_string(),
        server_info: ServerInfo {
            name: "weather".to_string(),
            version: "1.0".to_string(),
            additional: BTreeMap::new(),
        },
        tools: vec![ContractTool {
            name: "get_forecast".to_string(),
            description: None,
            input_schema: McpToolInputSchema(serde_json::json!({"type": "object"})),
            caps: vec![],
        }],
    };
    PinnedContract::from_parts(url.to_string(), contract).expect("build pin")
}

/// Write a minimal tau.toml and a valid Tau.lock + pin file into `dir`.
/// The lockfile `contract_hash` is taken from `pin.contract_hash_hex`.
fn setup_project_with_pin(dir: &TempDir, pin: &PinnedContract) {
    // tau.toml
    std::fs::write(
        dir.path().join("tau.toml"),
        r#"
[project]
name = "check-mcp-test"
version = "0.0.1"
"#,
    )
    .expect("write tau.toml");

    // Pin file at .tau/mcp/weather.contract.json
    let pin_dir = dir.path().join(".tau").join("mcp");
    std::fs::create_dir_all(&pin_dir).expect("create .tau/mcp");
    std::fs::write(
        pin_dir.join("weather.contract.json"),
        serde_json::to_string(pin).expect("serialize pin"),
    )
    .expect("write pin file");

    // Build lockfile programmatically
    let mut lf = LockFile::default();
    lf.mcp_entries.push(LockedMcpEntry::new(
        "weather".to_string(),
        pin.url.clone(),
        pin.contract_hash_hex.clone(),
        Some(".tau/mcp/weather.contract.json".to_string()),
        vec![LockedMcpExpandedTool::new(
            "get_forecast".to_string(),
            vec![],
            // schema_hash is not validated by this check — use a placeholder
            "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
        )],
    ));
    lf.save(&dir.path().join("tau-lock.toml"))
        .expect("save lockfile");
}

#[test]
fn check_mcp_contracts_passes_when_pin_matches_lockfile() {
    check_common::ensure_tau_home();
    let tmp = TempDir::new().unwrap();
    let pin = build_pin("cassette:./fixtures/weather.jsonl");
    setup_project_with_pin(&tmp, &pin);

    Command::cargo_bin("tau")
        .unwrap()
        .current_dir(tmp.path())
        .args(["check", "mcp-contracts"])
        .assert()
        .success();
}

#[test]
fn check_mcp_contracts_fails_when_lockfile_hash_drifted() {
    check_common::ensure_tau_home();
    let tmp = TempDir::new().unwrap();
    let pin = build_pin("cassette:./fixtures/weather.jsonl");
    setup_project_with_pin(&tmp, &pin);

    // Build a second (drifted) pin with a different server version.
    let drifted_pin = {
        let contract = ServerContract {
            protocol_version: "2025-03-26".to_string(),
            server_info: ServerInfo {
                name: "weather".to_string(),
                version: "99.0".to_string(), // bumped — hash will differ
                additional: BTreeMap::new(),
            },
            tools: vec![ContractTool {
                name: "get_forecast".to_string(),
                description: None,
                input_schema: McpToolInputSchema(serde_json::json!({"type": "object"})),
                caps: vec![],
            }],
        };
        PinnedContract::from_parts(pin.url.clone(), contract).expect("build drifted pin")
    };

    // Rewrite the pin file with the drifted pin.
    // Its self-hash is internally consistent, but its contract_hash_hex
    // no longer matches the lockfile's contract_hash (which came from the
    // original pin).
    std::fs::write(
        tmp.path().join(".tau/mcp/weather.contract.json"),
        serde_json::to_string(&drifted_pin).expect("serialize drifted"),
    )
    .expect("rewrite pin file");

    let out = Command::cargo_bin("tau")
        .unwrap()
        .current_dir(tmp.path())
        .args(["check", "mcp-contracts"])
        .output()
        .unwrap();

    assert_eq!(
        out.status.code(),
        Some(2),
        "expected exit 2 for lockfile-drift\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}
