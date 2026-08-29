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

/// Does this component still link the regex engine? (#689)
///
/// Reaching `__tau::goal::matches` pulls in `regex-automata`, `regex_syntax`
/// and regex's Unicode tables — ~770 KiB of a ~2.8 MB component. The guest
/// cfg-gates the predicate registry on what the baked IR can reach, and
/// wasm-ld then garbage-collects the engine; this is how a test observes
/// whether that actually happened.
///
/// Detection is a substring search over the component's name section, the
/// same signal `#679`'s per-crate size attribution used. That makes the
/// NEGATIVE result (no hits) meaningless on its own — a build that stripped
/// names would report "no regex" for every component, including one that
/// links it. Every caller asserting absence must therefore be paired with a
/// caller asserting PRESENCE, so a stripped name section fails the pair
/// instead of silently passing the suite;
/// `north_star_wasm_guest_executes_same_workflow_same_terminal_outcome` is
/// that positive control.
pub fn links_regex_engine(component: &[u8]) -> bool {
    [b"regex_automata".as_slice(), b"regex_syntax".as_slice()]
        .iter()
        .any(|needle| {
            component
                .windows(needle.len())
                .any(|window| window == *needle)
        })
}
