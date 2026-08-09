//! EPIC 3.3 acceptance: a component's network egress at runtime matches its
//! declared `net.http` caps. A request to a host the caps didn't authorize —
//! or with a method the caps didn't authorize — is rejected by the host's
//! `EgressPolicy` (`WasiHttpHooks::send_request`) BEFORE any socket is opened,
//! so the assertions hold offline. This closes the "unit-tested only" gap
//! noted in #536.
//!
//! `#[ignore]` by default: shells `cargo build --target wasm32-wasip2` for the
//! http-probe fixture (needs `rustup target add wasm32-wasip2`). Run with:
//!   cargo nextest run -p tau-wasm-host --run-ignored all

use std::path::{Path, PathBuf};
use std::process::Command;

use tau_domain::fixtures::cap_net_http;
use tau_wasm_host::{run_component_with_caps, WasmHostError};

/// Build the http-probe fixture for wasm32-wasip2 and return the component bytes.
fn build_http_probe() -> Vec<u8> {
    let manifest =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/http-probe/Cargo.toml");
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is two levels below the workspace root")
        .to_path_buf();
    let target_dir = workspace_root.join("target/wasm-http-probe-fixture");

    let output = Command::new(env!("CARGO"))
        .args([
            "build",
            "--manifest-path",
            manifest.to_str().unwrap(),
            "--target",
            "wasm32-wasip2",
            "--release",
            "--message-format=json",
        ])
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &target_dir)
        .output()
        .expect("failed to spawn cargo for http-probe");

    assert!(
        output.status.success(),
        "http-probe build failed (is wasm32-wasip2 installed?):\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("cargo json is utf-8");
    let wasm_path = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|m| m["reason"] == "compiler-artifact")
        .flat_map(|m| {
            m["filenames"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| f.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .find(|f| f.ends_with(".wasm"))
        .expect("no .wasm artifact for http-probe");

    std::fs::read(&wasm_path).expect("read http-probe component")
}

/// Assert that `run(input)` was denied by the egress *policy*: the guest
/// reports its `denied` arm carrying `HttpRequestDenied`, which the host
/// surfaces as `WasmHostError::Guest`.
///
/// Requiring the exact `HttpRequestDenied` code — not just the substring
/// "denied" — is what makes this an enforcement test. The `EgressPolicy`
/// returns that code from `send_request` before any socket opens; a generic
/// transport failure (e.g. a regression that let the request through and it
/// then failed to connect offline) would carry a *different* code, and must
/// fail this assertion instead of masquerading as a successful denial.
fn assert_policy_denied(result: Result<String, WasmHostError>, what: &str) {
    match result {
        Ok(payload) => panic!("{what} should have been denied, but succeeded: {payload}"),
        Err(WasmHostError::Guest(msg)) => {
            assert!(
                msg.contains("HttpRequestDenied"),
                "{what}: expected a policy denial (HttpRequestDenied), got: {msg}"
            )
        }
        Err(other) => panic!("{what}: expected a guest denial, got host error: {other:?}"),
    }
}

#[test]
#[ignore = "builds a wasm32-wasip2 fixture; run in the wasm lane"]
fn egress_is_denied_for_unauthorized_host_and_method() {
    let component = build_http_probe();
    // No preopens needed; the probe only touches wasi:http.
    let sandbox = tempfile::tempdir().expect("sandbox tempdir");

    // Authorize GET to exactly `allowed.example`.
    let caps = [cap_net_http(&["allowed.example"], &["GET"])];

    // HOST MISMATCH: a GET to a host the cap didn't authorize is rejected by
    // the EgressPolicy before any socket — safe to assert offline.
    let blocked_host = run_component_with_caps(
        &component,
        "GET blocked.invalid",
        vec![],
        &caps,
        sandbox.path(),
    );
    assert_policy_denied(blocked_host, "request to unauthorized host");

    // METHOD MISMATCH: POST to the authorized host is still denied — the cap
    // only granted GET. Again rejected before a socket opens.
    let blocked_method = run_component_with_caps(
        &component,
        "POST allowed.example",
        vec![],
        &caps,
        sandbox.path(),
    );
    assert_policy_denied(blocked_method, "request with unauthorized method");
}
