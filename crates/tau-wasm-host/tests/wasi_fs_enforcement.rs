//! EPIC 3.3 acceptance: a component's filesystem authority at runtime matches
//! its declared caps. A granted path (inside a preopen) reads; an un-granted
//! path (no preopen) is NOT reachable from the guest.
//!
//! `#[ignore]` by default: shells `cargo build --target wasm32-wasip2` for the
//! fs-probe fixture (needs `rustup target add wasm32-wasip2`). Run with:
//!   cargo nextest run -p tau-wasm-host --run-ignored all

use std::path::{Path, PathBuf};
use std::process::Command;

use tau_domain::fixtures::cap_fs_read;
use tau_wasm_host::{run_component_with_caps, WasmHostError};

/// Build the fs-probe fixture for wasm32-wasip2 and return the component bytes.
fn build_fs_probe() -> Vec<u8> {
    let manifest =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fs-probe/Cargo.toml");
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is two levels below the workspace root")
        .to_path_buf();
    let target_dir = workspace_root.join("target/wasm-fs-probe-fixture");

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
        .expect("failed to spawn cargo for fs-probe");

    assert!(
        output.status.success(),
        "fs-probe build failed (is wasm32-wasip2 installed?):\n{}",
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
        .expect("no .wasm artifact for fs-probe");

    std::fs::read(&wasm_path).expect("read fs-probe component")
}

#[test]
#[ignore = "builds a wasm32-wasip2 fixture; run in the wasm lane"]
fn granted_path_is_readable_ungranted_path_is_not() {
    let component = build_fs_probe();
    let sandbox = tempfile::tempdir().expect("sandbox tempdir");

    // Grant read on `/data/**`. wasi_grants maps this to a preopen at
    // host `<sandbox>/data` seen by the guest as `/data`.
    let caps = [cap_fs_read(&["/data/**"])];

    // Seed a file inside the granted dir.
    std::fs::create_dir_all(sandbox.path().join("data")).unwrap();
    std::fs::write(sandbox.path().join("data/ok.txt"), b"hello").unwrap();

    // GRANTED: reading /data/ok.txt succeeds.
    let ok = run_component_with_caps(&component, "/data/ok.txt", vec![], &caps, sandbox.path());
    assert!(
        matches!(&ok, Ok(msg) if msg.contains("read 5 bytes")),
        "granted path should read, got: {ok:?}"
    );

    // A real file that exists on the host OUTSIDE the granted `/data` preopen.
    // If the preopen scope were wrongly broadened to the sandbox root, the guest
    // could reach `/other/secret.txt`; with the tight `/data`-only preopen it cannot.
    std::fs::create_dir_all(sandbox.path().join("other")).unwrap();
    std::fs::write(sandbox.path().join("other/secret.txt"), b"top secret").unwrap();

    let sibling = run_component_with_caps(
        &component,
        "/other/secret.txt",
        vec![],
        &caps,
        sandbox.path(),
    );
    match sibling {
        Ok(payload) => {
            panic!("un-granted sibling file was reachable (preopen scope too broad): {payload}")
        }
        Err(WasmHostError::Guest(msg)) => assert!(msg.contains("denied"), "got: {msg}"),
        Err(WasmHostError::Trap(_)) => {}
        Err(other) => panic!("unexpected host error: {other:?}"),
    }

    // UN-GRANTED: /etc/secret has no preopen — the guest cannot reach it.
    let denied = run_component_with_caps(&component, "/etc/secret", vec![], &caps, sandbox.path());
    match denied {
        Ok(payload) => panic!("un-granted path was reachable: {payload}"),
        // The guest's `run` returned its Err arm ("denied: ...") — WASI gave
        // the guest no descriptor for a non-preopened path.
        Err(WasmHostError::Guest(msg)) => assert!(msg.contains("denied"), "got: {msg}"),
        // A trap is also acceptable evidence of non-reachability.
        Err(WasmHostError::Trap(_)) => {}
        Err(other) => panic!("unexpected host error: {other:?}"),
    }
}
