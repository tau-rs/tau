//! β.7.5 PR-F DoD: the simplified fan-monitor (read_temp → set_fan → end)
//! runs inside the wasm guest with its native tools sourced from
//! tau-native-tools. Requires wasm32-wasip2 installed.

use std::path::{Path, PathBuf};
use std::process::Command;

use tau_runtime_core::stream::RunEvent;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is two levels below the workspace root")
        .to_path_buf()
}

/// Build `tau-wasm-guest` for `wasm32-wasip2` (release) with the given IR
/// baked in, and return the component bytes.
///
/// Mirrors `roundtrip.rs`'s `build_guest_component` pattern — filters
/// compiler-artifact JSON by target name before `.ends_with(".wasm")` to
/// avoid picking up intermediate artifacts.
fn build_guest_with_ir(bytes: &[u8]) -> Vec<u8> {
    let root = workspace_root();

    // Dedicated target dir so the nested build never contends on the outer
    // test run's target lock (CLAUDE.md Rule 1/5).
    let guest_target_dir = root.join("target/wasm-guest-fan-monitor-simple");

    let ir_scratch = tempfile::NamedTempFile::new().expect("ir scratch");
    std::fs::write(ir_scratch.path(), bytes).expect("write ir scratch");

    let output = Command::new(env!("CARGO"))
        .current_dir(&root)
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
        .env("TAU_IR_BYTES", ir_scratch.path())
        .output()
        .expect("failed to spawn cargo to build the guest");

    assert!(
        output.status.success(),
        "guest build failed (is the wasm32-wasip2 target installed?):\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Parse the compiler-artifact stream for the guest's `.wasm` output.
    // Filter by target name first (same pattern as roundtrip.rs) to avoid
    // picking up any intermediate `.wasm` artifacts.
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

/// Lower the simplified fan-monitor fixture to canonical IR bytes.
fn simple_ir_bytes() -> Vec<u8> {
    let toml_path =
        workspace_root().join("crates/tau-conformance/fixtures/fan_monitor_simple/tau.toml");
    let toml = std::fs::read_to_string(&toml_path).expect("fixture tau.toml exists");
    let config = tau_pkg::project::ProjectConfig::parse_str(&toml).expect("fixture parses");
    let target: tau_ports::target::TargetTriple = "any-wasi-strict".parse().unwrap();
    let caches = tau_ir_lower::Caches {
        // Must be non-zero: [0u8;32] is the "unresolved sentinel" that
        // typecheck rejects (see tau-ir-lower/src/lower/typecheck.rs §3).
        native_tool: &|_| Some([1u8; 32]),
        mcp_contract: &|_| None,
        skill: &|_| None,
        prompt_file: &|_| Ok(Vec::new()),
    };
    let module = tau_ir_lower::lower_project(&config, &target, &caches)
        .expect("lowers")
        .module;
    tau_ir::to_canonical_bytes(&module)
}

/// The 3 cassette completions (CompletionResponse JSON) matching the fixture's
/// mock_llm.jsonl: read_temp tool_use → set_fan tool_use → end_turn.
fn cassette() -> Vec<String> {
    vec![
        r#"{"text":"","tool_uses":[{"id":"t0","name":"read_temp","input":{}}],"stop_reason":"ToolUse","usage":null}"#
            .to_string(),
        r#"{"text":"","tool_uses":[{"id":"t1","name":"set_fan","input":{"on":true}}],"stop_reason":"ToolUse","usage":null}"#
            .to_string(),
        r#"{"text":"Fan is on.","tool_uses":[],"stop_reason":"EndTurn","usage":null}"#.to_string(),
    ]
}

#[test]
#[ignore = "builds the wasm32-wasip2 guest; run with --run-ignored"]
fn simplified_fan_monitor_runs_in_guest() {
    let component = build_guest_with_ir(&simple_ir_bytes());
    let (_out, state) =
        tau_wasm_host::run_component(&component, "", cassette()).expect("guest runs");
    let events: Vec<RunEvent> = state
        .emitted
        .iter()
        .map(|e| serde_json::from_str(e).expect("each emitted entry is a RunEvent"))
        .collect();

    // Both native tools were dispatched in-guest, in order.
    let tool_completions: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            RunEvent::ToolCallCompleted { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        tool_completions,
        vec!["read_temp", "set_fan"],
        "expected read_temp then set_fan; got {tool_completions:?}"
    );
    assert!(matches!(events.last(), Some(RunEvent::RunCompleted { .. })));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, RunEvent::FatalError { .. })),
        "no fatal errors expected"
    );
}
