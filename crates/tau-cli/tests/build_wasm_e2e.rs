//! β.7.5 PR-E2 DoD: `tau build wasm` of a trivial 1-agent cassette project
//! produces a component that runs in wasmtime and returns a typed RunEvent
//! stream. Requires `wasm32-wasip2` installed.

use std::path::PathBuf;

mod common;

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
    common::wasm_component::build_component_with_ir(&bytes)
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

/// #621 PR-2: a linear (agent → agent) pipeline already passes wasm
/// feature-fit today, but until `run_pipeline` landed in-guest, such a
/// build produced a component that errored at runtime with "pipelines are
/// not yet executed in-wasm". This proves the gap is closed end to end:
/// the guest drives the whole pipeline and the `run` export payload is the
/// LAST leaf step's rendered output (not the first agent's).
#[test]
#[ignore = "builds a wasm component; run with --run-ignored"]
fn build_wasm_linear_pipeline_runs_in_guest_and_returns_last_leaf() {
    let (_module, bytes) =
        tau_cli::cmd::build_wasm::lower_to_wasm_ir(&fixture("pipeline")).expect("lowers");
    let component = common::wasm_component::build_component_with_ir(&bytes);
    let response = |text: &str| {
        format!(r#"{{"text":"{text}","tool_uses":[],"stop_reason":"EndTurn","usage":null}}"#)
    };
    let (payload, _events) = tau_wasm_host::run_component(
        &component,
        "hello",
        vec![response("the draft"), response("the polished reply")],
    )
    .expect("guest runs the pipeline");
    assert_eq!(
        payload, "the polished reply",
        "payload must be the LAST leaf step's rendered output"
    );
}
