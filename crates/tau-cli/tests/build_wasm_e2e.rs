//! β.7.5 PR-E2 DoD: `tau build wasm` of a trivial 1-agent cassette project
//! produces a component that runs in wasmtime and returns a typed RunEvent
//! stream. Requires `wasm32-wasip2` installed.

use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm-build")
        .join(name)
}

/// Build the guest with the trivial fixture's IR baked in, via the same
/// lowering the CLI uses, and return the component bytes.
fn build_trivial_component() -> Vec<u8> {
    let (_module, bytes) =
        tau_cli::cmd::build_wasm::lower_to_wasm_ir(&fixture("trivial")).expect("lowers");

    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .to_path_buf();
    let target_dir = workspace_root.join("target/tau-build-wasm-e2e");

    let ir_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(ir_file.path(), &bytes).unwrap();

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

#[test]
#[ignore = "builds a wasm component; run with --run-ignored"]
fn build_wasm_then_run_returns_typed_stream() {
    let component = build_trivial_component();
    let response =
        r#"{"text":"done","tool_uses":[],"stop_reason":"EndTurn","usage":null}"#.to_string();
    let (_out, emitted) =
        tau_wasm_host::run_component(&component, "hi", vec![response]).expect("runs");

    // Events now stream via `emit-event`, one JSON-encoded RunEvent per entry,
    // rather than being buffered into the `run` return payload.
    let events: Vec<tau_runtime_core::stream::RunEvent> = emitted
        .iter()
        .map(|e| serde_json::from_str(e).expect("each emitted entry is a RunEvent"))
        .collect();
    assert!(
        matches!(
            events.last(),
            Some(tau_runtime_core::stream::RunEvent::RunCompleted { .. })
        ),
        "stream must end with RunCompleted; got {:?}",
        events.last()
    );
}
