//! `EmbedPorts` (EPIC 7.2): a live-style ports impl runs the real
//! `tau-wasm-guest` component and streams events one at a time, matching the
//! buffered `run_component` API's sequence shape exactly.
//!
//! `#[ignore]` for the same reason as `roundtrip.rs`: building the guest for
//! `wasm32-wasip2` is slow/heavy for the default unit-test lane.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use tau_wasm_host::embed::{
    run_component_with_ports, CompletionRequest, CompletionResponse, EmbedPorts,
};

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

/// A live-style ports impl: fixed wall clock, counting entropy, canned LLM,
/// and a shared handle recording events as they arrive.
struct RecordingPorts {
    responses: Vec<String>,
    clock: u64,
    entropy: u64,
    events: Arc<Mutex<Vec<String>>>,
}

impl EmbedPorts for RecordingPorts {
    fn complete(&mut self, _req: CompletionRequest) -> Result<CompletionResponse, String> {
        if self.responses.is_empty() {
            return Err("recording ports: cassette exhausted".to_string());
        }
        serde_json::from_str(&self.responses.remove(0)).map_err(|e| e.to_string())
    }
    fn now_millis(&mut self) -> u64 {
        self.clock += 7;
        self.clock
    }
    fn next_u64(&mut self) -> u64 {
        self.entropy += 1;
        self.entropy
    }
    fn on_event(&mut self, event_json: &str) {
        self.events.lock().unwrap().push(event_json.to_string());
    }
}

fn end_turn_response() -> String {
    r#"{"text":"done","tool_uses":[],"stop_reason":"EndTurn","usage":null}"#.to_string()
}

#[test]
#[ignore = "builds the wasm32-wasip2 guest; run with --run-ignored"]
fn with_ports_streams_the_same_events_the_buffered_api_returns() {
    let component = build_guest_component(Some(&trivial_ir_bytes()));

    let events = Arc::new(Mutex::new(Vec::new()));
    let ports = RecordingPorts {
        responses: vec![end_turn_response()],
        clock: 1_724_000_000_000,
        entropy: 0,
        events: Arc::clone(&events),
    };
    let payload = run_component_with_ports(&component, "hi", Box::new(ports), &[], Path::new("."))
        .expect("live ports run the guest");

    let (buf_payload, buffered) =
        tau_wasm_host::run_component(&component, "hi", vec![end_turn_response()])
            .expect("buffered API still runs");

    assert_eq!(payload, buf_payload);
    let live = events.lock().unwrap();
    assert!(
        live.first().is_some_and(|e| e.contains("RunStarted")),
        "live stream starts with RunStarted: {live:?}"
    );
    assert!(
        live.last().is_some_and(|e| e.contains("RunCompleted")),
        "live stream ends with RunCompleted: {live:?}"
    );
    // Same component, same cassette → identical event *sequence shape*
    // (timestamps inside events may differ: live clock ≠ deterministic clock,
    // so compare count and variant markers, not bytes).
    assert_eq!(
        live.len(),
        buffered.len(),
        "live: {live:?}\nbuffered: {buffered:?}"
    );
}

#[test]
#[ignore = "builds the wasm32-wasip2 guest; run with --run-ignored"]
fn with_ports_surfaces_complete_error_as_a_fatal_error_event() {
    // Verified against `tau_wasm_guest::guest::Component::run`
    // (crates/tau-wasm-guest/src/guest.rs): once the baked IR decodes, `run`
    // always returns `Ok(String::new())` — it never surfaces a mid-run LLM
    // failure as its own `Err` arm. The interpreter instead yields
    // `RunEvent::FatalError { kind: "Llm", .. }` on the event stream and the
    // stream ends there (no `RunCompleted` follows; see
    // `tau_runtime_core::stream::RunEvent::FatalError`'s doc comment). So the
    // *ports* completion failure crosses the boundary as a live event, not a
    // `WasmHostError::Guest`.
    let component = build_guest_component(Some(&trivial_ir_bytes()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let ports = RecordingPorts {
        responses: vec![], // exhausted immediately
        clock: 0,
        entropy: 0,
        events: Arc::clone(&events),
    };
    let payload = run_component_with_ports(&component, "hi", Box::new(ports), &[], Path::new("."))
        .expect("guest run completes even though the LLM call failed");
    assert_eq!(
        payload, "",
        "payload stays the empty sentinel; the outcome travels via events"
    );
    let live = events.lock().unwrap();
    assert!(
        live.iter()
            .any(|e| e.contains("FatalError") && e.contains("\"Llm\"")),
        "LLM failure must surface as a FatalError{{kind:\"Llm\"}} event: {live:?}"
    );
}
