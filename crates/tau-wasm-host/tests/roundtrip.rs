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

use std::path::{Path, PathBuf};
use std::process::Command;

use tau_wasm_host::run_component;

/// Build `tau-wasm-guest` for `wasm32-wasip2` (release) and return the bytes
/// of the emitted component, locating the artifact from cargo's JSON output
/// rather than guessing the target-dir layout.
///
/// When `ir_bytes` is `Some`, writes them to a temp file and passes the path
/// via `TAU_IR_BYTES` so the guest `build.rs` bakes the IR into the component.
/// When `None`, no env-var is set and the guest builds with empty baked IR.
fn build_guest_component(ir_bytes: Option<&[u8]>) -> Vec<u8> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is two levels below the workspace root")
        .to_path_buf();

    // Dedicated target dir for the guest fixture so the nested build never
    // contends on the outer test run's target lock (CLAUDE.md Rule 1/5).
    let guest_target_dir = workspace_root.join("target/wasm-guest-fixture");

    let mut cmd = Command::new(env!("CARGO"));
    cmd.current_dir(&workspace_root)
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
        .env("CARGO_TARGET_DIR", &guest_target_dir);

    let _scratch; // keep the tempfile alive across the build
    if let Some(bytes) = ir_bytes {
        let f = tempfile::NamedTempFile::new().expect("ir scratch");
        std::fs::write(f.path(), bytes).expect("write ir scratch");
        cmd.env("TAU_IR_BYTES", f.path());
        _scratch = Some(f);
    } else {
        _scratch = None;
    }

    let output = cmd
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

/// Lower the trivial fixture to canonical IR bytes (mirrors `tau build wasm`).
fn trivial_ir_bytes() -> Vec<u8> {
    let toml = r#"
packages = ["anthropic"]

[project]
name = "trivial-wasm"
version = "0.1.0"

[models.claude]
backend = "anthropic"
model = "claude-sonnet-4-6"

[agents.main]
display_name = "Main"
package = "trivial-wasm@^0.1"
model = "claude"

[agents.main.prompt]
system = "You are a trivial test agent. Reply and stop."
"#;
    let config = tau_pkg::project::ProjectConfig::parse_str(toml).expect("fixture parses");
    let target: tau_ports::target::TargetTriple = "any-wasi-strict".parse().unwrap();
    let caches = tau_ir_lower::Caches {
        native_tool: &|_| Some([0u8; 32]),
        mcp_contract: &|_| None,
        skill: &|_| None,
        prompt_file: &|_| Ok(Vec::new()),
    };
    let module = tau_ir_lower::lower_project(&config, &target, &caches)
        .expect("lowers")
        .module;
    tau_ir::to_canonical_bytes(&module)
}

#[test]
#[ignore = "builds the wasm32-wasip2 guest; run with --run-ignored"]
fn guest_with_no_baked_ir_errors() {
    let component = build_guest_component(None);
    let err = run_component(&component, "hi", vec![]).unwrap_err();
    // Empty baked IR → guest returns its error arm.
    assert!(
        matches!(err, tau_wasm_host::WasmHostError::Guest(_)),
        "got: {err:?}"
    );
}

#[test]
#[ignore = "builds the wasm32-wasip2 guest; run with --run-ignored"]
fn guest_decodes_baked_ir_and_starts_run() {
    // The guest now drives `run_ir_streaming`. Feed one end-turn response so the
    // agent loop completes rather than hanging waiting for an LLM call.
    let component = build_guest_component(Some(&trivial_ir_bytes()));
    let (_out, state) =
        run_component(&component, "hi", vec![end_turn_response()]).expect("runs with cassette");
    // Events now stream via `emit-event`; the presence of "RunStarted" in the
    // buffer proves the IR was decoded and the interpreter was entered.
    assert!(
        state.emitted.iter().any(|e| e.contains("RunStarted")),
        "guest should stream a RunEvent stream via emit-event; got: {:?}",
        state.emitted
    );
}

/// A minimal valid CompletionResponse that ends the turn immediately
/// (no tool calls) — the cassette for a 1-agent reply-and-stop scenario.
fn end_turn_response() -> String {
    r#"{"text":"done","tool_uses":[],"stop_reason":"EndTurn","usage":null}"#.to_string()
}

#[test]
#[ignore = "builds the wasm32-wasip2 guest; run with --run-ignored"]
fn guest_drives_ir_and_returns_typed_stream() {
    let component = build_guest_component(Some(&trivial_ir_bytes()));
    let (_out, state) = tau_wasm_host::run_component(&component, "hi", vec![end_turn_response()])
        .expect("guest runs the baked IR");

    // Events now stream via `emit-event`, one JSON-encoded RunEvent per entry.
    let events: Vec<tau_runtime_core::stream::RunEvent> = state
        .emitted
        .iter()
        .map(|e| serde_json::from_str(e).expect("each emitted entry is a RunEvent"))
        .collect();

    assert!(
        matches!(
            events.first(),
            Some(tau_runtime_core::stream::RunEvent::RunStarted)
        ),
        "stream must start with RunStarted; got {:?}",
        events.first()
    );
    assert!(
        matches!(
            events.last(),
            Some(tau_runtime_core::stream::RunEvent::RunCompleted { .. })
        ),
        "stream must end with RunCompleted; got {:?}",
        events.last()
    );
}

#[test]
#[ignore = "builds the wasm32-wasip2 guest; run with --run-ignored"]
fn host_guest_roundtrip_is_deterministic() {
    let ir = trivial_ir_bytes();
    let wasm = build_guest_component(Some(&ir));
    // Provide one cassette response so both runs complete without hanging.
    let (first_out, first_state) =
        run_component(&wasm, "hello", vec![end_turn_response()]).expect("first run");
    let (second_out, second_state) =
        run_component(&wasm, "hello", vec![end_turn_response()]).expect("second run");
    assert_eq!(
        first_out, second_out,
        "same inputs must yield byte-identical output payload"
    );
    assert_eq!(
        first_state.emitted, second_state.emitted,
        "same inputs must yield byte-identical streamed events"
    );
}
