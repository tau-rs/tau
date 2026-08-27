//! Shared wasm-component build helper for `tau-cli` integration tests.
//!
//! Extracted from `build_wasm_e2e.rs`'s `build_trivial_component` (Task 6,
//! #621 PR-2) so Task 9's north-star wasm leg can reuse it against a
//! different fixture's already-lowered IR bytes.

#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;

/// Build the `tau-wasm-guest` crate for `wasm32-wasip2` with `ir_bytes`
/// baked in via the `TAU_IR_BYTES` handshake, and return the resulting
/// component's bytes.
///
/// Shares `CARGO_TARGET_DIR=target/tau-build-wasm-e2e` across every caller
/// (cargo's own lock file serializes concurrent invocations against that
/// directory, so parallel test binaries don't stomp on each other).
pub fn build_component_with_ir(ir_bytes: &[u8]) -> Vec<u8> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .to_path_buf();
    let target_dir = workspace_root.join("target/tau-build-wasm-e2e");

    let ir_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(ir_file.path(), ir_bytes).unwrap();

    let output = Command::new(env!("CARGO"))
        .current_dir(&workspace_root)
        .args([
            "build",
            "-p",
            "tau-wasm-guest",
            "--target",
            "wasm32-wasip2",
            "--release",
            "--message-format=json",
        ])
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &target_dir)
        .env("TAU_IR_BYTES", ir_file.path())
        .output()
        .expect("cargo spawn");
    assert!(
        output.status.success(),
        "guest build failed (is wasm32-wasip2 installed?):\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let wasm_path = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|m| m["reason"] == "compiler-artifact")
        .filter(|m| {
            m["target"]["name"]
                .as_str()
                .is_some_and(|n| n == "tau-wasm-guest" || n == "tau_wasm_guest")
        })
        .flat_map(|m| {
            m["filenames"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| f.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .find(|f| f.ends_with(".wasm"))
        .expect("a .wasm artifact for tau-wasm-guest");
    std::fs::read(wasm_path).unwrap()
}
