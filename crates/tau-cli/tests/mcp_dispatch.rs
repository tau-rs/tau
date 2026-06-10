//! Smoke test: `tau mcp --help` and the 5 sub-verbs are dispatchable.

use assert_cmd::Command;
use assert_fs::prelude::*;
use predicates::prelude::*;

/// Write a synthetic pin file into a fresh tempdir with a minimal tau.toml.
///
/// Used by Phase 3 `show`, `refresh`, and `diff` tests that only need a
/// pre-existing pin on disk (not a live server probe).
fn setup_project_with_pin() -> assert_fs::TempDir {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    tmp.child("tau.toml")
        .write_str(
            r#"
[project]
name = "show-test"
version = "0.0.1"
"#,
        )
        .expect("write");
    let pin = serde_json::json!({
        "schema_version": 1,
        "url": "cassette:./fixtures/weather.jsonl",
        "contract_hash_hex": "0".repeat(64),
        "contract": {
            "protocol_version": "2025-03-26",
            "server_info": {"name": "weather", "version": "1.0"},
            "tools": [{
                "name": "get_forecast",
                "input_schema": {"type": "object"},
                "caps": [],
            }],
        }
    });
    tmp.child(".tau/mcp/weather.contract.json")
        .write_str(&serde_json::to_string(&pin).unwrap())
        .expect("write");
    tmp
}

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

// ─── Phase 3: show ──────────────────────────────────────────────────────────

#[test]
fn show_json_emits_full_contract() {
    let tmp = setup_project_with_pin();
    let mut cmd = assert_cmd::Command::cargo_bin("tau").expect("bin");
    cmd.current_dir(tmp.path())
        .args(["mcp", "show", "weather", "--json"])
        .assert()
        .success()
        .stdout(predicates::str::contains("\"server_info\""))
        .stdout(predicates::str::contains("\"tools\""));
}

#[test]
fn show_sarif_emits_valid_sarif_document() {
    let tmp = setup_project_with_pin();
    let mut cmd = assert_cmd::Command::cargo_bin("tau").expect("bin");
    let output = cmd
        .current_dir(tmp.path())
        .args(["mcp", "show", "weather", "--sarif"])
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(parsed["version"], "2.1.0");
    assert_eq!(parsed["runs"][0]["tool"]["driver"]["name"], "tau-mcp");
    assert_eq!(parsed["runs"][0]["results"].as_array().unwrap().len(), 0);
}

// ─── Phase 3: diff ──────────────────────────────────────────────────────────

#[test]
fn diff_unchanged_exits_zero() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    let cassette_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tau-mcp-tokio/tests/fixtures/weather_minimal_cassette.jsonl");
    tmp.child("fixtures/weather.jsonl")
        .write_binary(&std::fs::read(&cassette_src).expect("read"))
        .expect("write");
    tmp.child("tau.toml")
        .write_str(
            r#"
[project]
name = "diff-test"
version = "0.0.1"

[tools.weather]
mcp = "cassette:./fixtures/weather.jsonl"
"#,
        )
        .expect("write");

    assert_cmd::Command::cargo_bin("tau")
        .expect("bin")
        .current_dir(tmp.path())
        .args(["mcp", "pin", "weather"])
        .assert()
        .success();
    assert_cmd::Command::cargo_bin("tau")
        .expect("bin")
        .current_dir(tmp.path())
        .args(["mcp", "diff", "weather"])
        .assert()
        .success(); // exit 0
}

#[test]
fn diff_drift_exits_64() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    let cassette_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tau-mcp-tokio/tests/fixtures/weather_minimal_cassette.jsonl");
    tmp.child("fixtures/weather.jsonl")
        .write_binary(&std::fs::read(&cassette_src).expect("read"))
        .expect("write");
    tmp.child("tau.toml")
        .write_str(
            r#"
[project]
name = "diff-test"
version = "0.0.1"

[tools.weather]
mcp = "cassette:./fixtures/weather.jsonl"
"#,
        )
        .expect("write");

    assert_cmd::Command::cargo_bin("tau")
        .expect("bin")
        .current_dir(tmp.path())
        .args(["mcp", "pin", "weather"])
        .assert()
        .success();

    // Tamper: load the pin, mutate the server version, and re-derive the hash
    // so the pin is self-consistent (drift is between pin and live cassette).
    let pin_path = tmp.child(".tau/mcp/weather.contract.json");
    let bytes = std::fs::read(pin_path.path()).unwrap();
    let mut pinned: tau_mcp::contract::pinned::PinnedContract =
        serde_json::from_slice(&bytes).unwrap();
    pinned.contract.server_info.version = "99.0".to_string();
    let rebuilt = tau_mcp::contract::pinned::PinnedContract::from_parts(
        pinned.url.clone(),
        pinned.contract.clone(),
    )
    .unwrap();
    std::fs::write(
        pin_path.path(),
        serde_json::to_vec_pretty(&rebuilt).unwrap(),
    )
    .unwrap();

    assert_cmd::Command::cargo_bin("tau")
        .expect("bin")
        .current_dir(tmp.path())
        .args(["mcp", "diff", "weather"])
        .assert()
        .code(64);
}

// ─── Phase 3: refresh ───────────────────────────────────────────────────────

#[test]
fn refresh_overwrites_pin_file_and_reports_changed_false_on_no_drift() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    let cassette_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tau-mcp-tokio/tests/fixtures/weather_minimal_cassette.jsonl");
    tmp.child("fixtures/weather.jsonl")
        .write_binary(&std::fs::read(&cassette_src).expect("read"))
        .expect("write");
    tmp.child("tau.toml")
        .write_str(
            r#"
[project]
name = "refresh-test"
version = "0.0.1"

[tools.weather]
mcp = "cassette:./fixtures/weather.jsonl"
"#,
        )
        .expect("write");

    // First, pin it.
    assert_cmd::Command::cargo_bin("tau")
        .expect("bin")
        .current_dir(tmp.path())
        .args(["mcp", "pin", "weather"])
        .assert()
        .success();
    let first =
        std::fs::read_to_string(tmp.child(".tau/mcp/weather.contract.json").path()).unwrap();

    // Refresh against the same cassette → identical contract.
    assert_cmd::Command::cargo_bin("tau")
        .expect("bin")
        .current_dir(tmp.path())
        .args(["mcp", "refresh", "weather", "--json"])
        .assert()
        .success()
        .stdout(
            predicates::str::contains("\"changed\": false")
                .or(predicates::str::contains("\"changed\":false")),
        );
    let second =
        std::fs::read_to_string(tmp.child(".tau/mcp/weather.contract.json").path()).unwrap();
    assert_eq!(first, second);
}
