//! Host↔guest round-trip: build the real `tau-wasm-guest` component, load it
//! in `tau-wasm-host`, drive its exported `run`, and prove the value comes
//! back across the wasm boundary.
//!
//! These tests are `#[ignore]` by default because they shell `cargo build`
//! for the `wasm32-wasip2` target, which:
//!   - requires `rustup target add wasm32-wasip2`, and
//!   - is too slow/heavy for the default unit-test lane.
//!
//! Run locally (or in the dedicated wasm CI lane) with:
//!
//! ```text
//! rustup target add wasm32-wasip2
//! CARGO_TARGET_DIR=target/agent-impl \
//!   cargo nextest run -p tau-wasm-host --run-ignored all
//! ```
//!
//! The PR-C guest currently exports `run(_) -> Ok("{}")` (hardcoded), so the
//! round-trip asserts `"{}"`; PR-E swaps the body for a real IR-driven
//! observable without changing this test's wiring.

use std::path::{Path, PathBuf};
use std::process::Command;

use tau_wasm_host::run_component;

/// Build `tau-wasm-guest` for `wasm32-wasip2` (release) and return the bytes
/// of the emitted component, locating the artifact from cargo's JSON output
/// rather than guessing the target-dir layout.
fn build_guest_component() -> Vec<u8> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is two levels below the workspace root")
        .to_path_buf();

    // Dedicated target dir for the guest fixture so the nested build never
    // contends on the outer test run's target lock (CLAUDE.md Rule 1/5).
    let guest_target_dir = workspace_root.join("target/wasm-guest-fixture");

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
        .env("CARGO_TARGET_DIR", &guest_target_dir)
        .output()
        .expect("failed to spawn cargo to build the guest");

    assert!(
        output.status.success(),
        "guest build failed (is the wasm32-wasip2 target installed?):\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Parse the compiler-artifact stream for the guest's `.wasm` output.
    let stdout = String::from_utf8(output.stdout).expect("cargo json output is utf-8");
    let wasm_path = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|msg| msg["reason"] == "compiler-artifact")
        .filter(|msg| {
            msg["target"]["name"]
                .as_str()
                .is_some_and(|name| name == "tau-wasm-guest" || name == "tau_wasm_guest")
        })
        .flat_map(|msg| {
            msg["filenames"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| f.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .find(|f| f.ends_with(".wasm"))
        .expect("no .wasm artifact in cargo output for tau-wasm-guest");

    std::fs::read(&wasm_path).expect("failed to read the built guest component")
}

#[test]
#[ignore = "builds the wasm32-wasip2 guest; run with --run-ignored"]
fn host_guest_roundtrip_returns_guest_ok_value() {
    let wasm = build_guest_component();
    let result = run_component(&wasm, "hello", vec![])
        .expect("run_component should drive the guest's run successfully");
    // PR-C guest hard-codes Ok("{}").
    assert_eq!(result, "{}");
}

#[test]
#[ignore = "builds the wasm32-wasip2 guest; run with --run-ignored"]
fn host_guest_roundtrip_is_deterministic() {
    let wasm = build_guest_component();
    let first = run_component(&wasm, "hello", vec![]).expect("first run");
    let second = run_component(&wasm, "hello", vec![]).expect("second run");
    assert_eq!(
        first, second,
        "same inputs must yield byte-identical output"
    );
}
