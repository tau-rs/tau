//! Smoke test: `tau mcp --help` and the 5 sub-verbs are dispatchable.

use assert_cmd::Command;
use assert_fs::prelude::*;
use predicates::prelude::*;

#[test]
fn mcp_help_lists_five_verbs() {
    let output = Command::cargo_bin("tau")
        .expect("binary")
        .args(["mcp", "--help"])
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for verb in ["pin", "ls", "show", "refresh", "diff"] {
        assert!(stdout.contains(verb), "expected `{verb}` in: {stdout}");
    }
}

#[test]
fn pin_writes_contract_file_for_cassette_tool() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    let cassette_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tau-mcp-tokio/tests/fixtures/weather_minimal_cassette.jsonl");
    tmp.child("fixtures/weather.jsonl")
        .write_binary(&std::fs::read(&cassette_src).expect("read fixture"))
        .expect("write fixture");
    tmp.child("tau.toml").write_str(r#"
[project]
name = "pin-test"
version = "0.0.1"

[tools.weather]
mcp = "cassette:./fixtures/weather.jsonl"
"#).expect("write tau.toml");

    let mut cmd = assert_cmd::Command::cargo_bin("tau").expect("bin");
    cmd.current_dir(tmp.path())
        .args(["mcp", "pin", "weather"])
        .assert()
        .success();

    let pinned = tmp.child(".tau/mcp/weather.contract.json");
    pinned.assert(predicates::path::is_file());
    let content = std::fs::read_to_string(pinned.path()).expect("read");
    assert!(content.contains("\"schema_version\":1") || content.contains("\"schema_version\": 1"), "got: {content}");
    assert!(content.contains("\"url\": \"cassette:") || content.contains("\"url\":\"cassette:"), "got: {content}");
    assert!(content.contains("contract_hash_hex"), "got: {content}");
}

#[test]
fn pin_with_from_override_uses_override_url() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    let cassette_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tau-mcp-tokio/tests/fixtures/weather_minimal_cassette.jsonl");
    tmp.child("fixtures/weather.jsonl")
        .write_binary(&std::fs::read(&cassette_src).expect("read"))
        .expect("write");
    tmp.child("tau.toml").write_str(r#"
[project]
name = "pin-test"
version = "0.0.1"

[tools.weather]
mcp = "stdio:nonexistent-binary"
"#).expect("write tau.toml");

    let override_url = "cassette:./fixtures/weather.jsonl";
    let mut cmd = assert_cmd::Command::cargo_bin("tau").expect("bin");
    cmd.current_dir(tmp.path())
        .args(["mcp", "pin", "weather", "--from", override_url])
        .assert()
        .success();
    let content = std::fs::read_to_string(tmp.child(".tau/mcp/weather.contract.json").path())
        .expect("read");
    assert!(content.contains(override_url), "got: {content}");
}

#[test]
fn ls_empty_project_returns_zero_pins() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    tmp.child("tau.toml").write_str(r#"
[project]
name = "ls-test"
version = "0.0.1"
"#).expect("write");

    let mut cmd = assert_cmd::Command::cargo_bin("tau").expect("bin");
    cmd.current_dir(tmp.path())
        .args(["mcp", "ls", "--json"])
        .assert()
        .success()
        .stdout(predicates::str::contains("\"pins\": []").or(predicates::str::contains("\"pins\":[]")));
}

#[test]
fn ls_lists_existing_pin_files() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    tmp.child("tau.toml").write_str(r#"
[project]
name = "ls-test"
version = "0.0.1"
"#).expect("write");
    let pin = serde_json::json!({
        "schema_version": 1,
        "url": "stdio:echo",
        "contract_hash_hex": "00".repeat(32),
        "contract": {
            "protocol_version": "2025-03-26",
            "server_info": {"name": "weather", "version": "1.0"},
            "tools": [],
        }
    });
    tmp.child(".tau/mcp/weather.contract.json")
        .write_str(&serde_json::to_string(&pin).unwrap())
        .expect("write");

    let mut cmd = assert_cmd::Command::cargo_bin("tau").expect("bin");
    cmd.current_dir(tmp.path())
        .args(["mcp", "ls"])
        .assert()
        .success()
        .stdout(predicates::str::contains("weather"));
}
