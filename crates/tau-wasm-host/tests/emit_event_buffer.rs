//! Proves `RunEvent`s stream to the host one at a time via the `emit-event`
//! host import, rather than being buffered into a single JSON blob returned
//! from `run`.
//!
//! Reuses the exact fixture-component + canned-completion harness as
//! `roundtrip.rs` (build the real `tau-wasm-guest` component for
//! `wasm32-wasip2`, bake a trivial one-agent IR, and feed one end-turn
//! cassette response so the interpreter completes without hanging).
//!
//! `#[ignore]` by default for the same reason as `roundtrip.rs`: it shells
//! `cargo build` for the `wasm32-wasip2` target. Run locally (or in the
//! dedicated wasm CI lane) with:
//!
//! ```text
//! rustup target add wasm32-wasip2
//! CARGO_TARGET_DIR=target/agent-impl2 \
//!   cargo nextest run -p tau-wasm-host --run-ignored all
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

use tau_wasm_host::run_component;

/// Build `tau-wasm-guest` for `wasm32-wasip2` (release) and return the bytes
/// of the emitted component. Verbatim copy of `roundtrip.rs`'s
/// `build_guest_component` — same target-dir isolation rationale (CLAUDE.md
/// Rule 1/5) and same JSON-artifact-parsing approach.
fn build_guest_component(ir_bytes: Option<&[u8]>) -> Vec<u8> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is two levels below the workspace root")
        .to_path_buf();

    // Dedicated target dir so this nested build never contends on the outer
    // test run's target lock (CLAUDE.md Rule 1/5).
    let guest_target_dir = workspace_root.join("target/wasm-guest-fixture-emit-event");

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

/// Lower the trivial fixture to canonical IR bytes (mirrors `tau build wasm`
/// and `roundtrip.rs`'s `trivial_ir_bytes`).
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

/// A minimal valid CompletionResponse that ends the turn immediately
/// (no tool calls) — the cassette for a 1-agent reply-and-stop scenario.
fn end_turn_response() -> String {
    r#"{"text":"done","tool_uses":[],"stop_reason":"EndTurn","usage":null}"#.to_string()
}

#[test]
#[ignore = "builds the wasm32-wasip2 guest; run with --run-ignored"]
fn events_stream_via_emit_event_not_the_run_payload() {
    let component = build_guest_component(Some(&trivial_ir_bytes()));
    let (out, state) =
        run_component(&component, "hi", vec![end_turn_response()]).expect("runs with cassette");

    // `run`'s own payload is now an empty sentinel — events flow via
    // `emit-event` instead (design D2).
    assert_eq!(out, "", "run payload must be the empty sentinel");

    assert!(
        !state.emitted.is_empty(),
        "no events streamed via emit-event"
    );

    // Every entry must deserialize as a RunEvent (well-formed JSON, one
    // event per emit-event call rather than one buffered blob).
    for entry in &state.emitted {
        let _: tau_runtime_core::stream::RunEvent =
            serde_json::from_str(entry).unwrap_or_else(|e| {
                panic!("emitted entry is not a valid RunEvent: {e}\nentry: {entry}")
            });
    }

    // RunStarted is a bare externally-tagged unit variant → serializes as
    // the plain JSON string "RunStarted".
    let first: serde_json::Value = serde_json::from_str(&state.emitted[0]).unwrap();
    assert_eq!(first, serde_json::json!("RunStarted"));

    assert!(
        state.emitted.last().unwrap().contains("RunCompleted"),
        "last streamed event must be RunCompleted; got {:?}",
        state.emitted.last()
    );
}
