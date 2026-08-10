//! EPIC 3.6 live host-enforcement round-trip: the REAL production guest,
//! built by `tau build wasm`, driven by real IR, issues a `Fetch` through
//! wasi:http at an UNGRANTED host — the host `EgressPolicy` must deny it at
//! the WasiCtx before any socket, and the exact `HttpRequestDenied` code must
//! surface through the guest. Denial-only (offline; a granted host cannot open
//! a socket without a network, same as the `http-probe` positive case). Builds
//! the wasm32-wasip2 guest, so it is #[ignore]d (run with --run-ignored).

use std::path::PathBuf;
use std::process::Command;

use tau_cli::cmd::build_wasm::{lower_to_wasm_ir, wasm_world_for_project};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm-build")
        .join(name)
}

/// Build the `net-http` guest and return its component bytes. Mirrors the
/// build recipe in `build_wasm_world_dod.rs` (EPIC 3.2 DoD).
fn build_guest(fixture_name: &str) -> Vec<u8> {
    let (_module, ir_bytes) = lower_to_wasm_ir(&fixture(fixture_name)).unwrap();
    let world = wasm_world_for_project(&fixture(fixture_name)).unwrap();
    let ir = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(ir.path(), &ir_bytes).unwrap();
    let wit = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(wit.path(), world.as_bytes()).unwrap();

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let out = Command::new(env!("CARGO"))
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
        .env("CARGO_TARGET_DIR", root.join("target/tau-build-wasm"))
        .env("TAU_IR_BYTES", ir.path())
        .env("TAU_WORLD_WIT", wit.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "guest build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
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
        .unwrap();
    std::fs::read(&wasm_path).unwrap()
}

/// A cassette turn that calls the `fetch` tool at an ungranted host, then a
/// turn that ends. Field names/`stop_reason` spelling mirror
/// `tau_ports::llm::{CompletionResponse, ToolUse, StopReason}` exactly
/// (verified against `crates/tau-wasm-host/src/lib.rs`'s `canned_response()`).
fn cassette() -> Vec<String> {
    let tool_use = serde_json::json!({
        "text": "",
        "tool_uses": [{
            "id": "call_1",
            "name": "fetch",
            "input": { "url": "https://blocked.invalid/", "method": "GET" }
        }],
        "stop_reason": "ToolUse",
        "usage": null
    });
    let end = serde_json::json!({
        "text": "done",
        "tool_uses": [],
        "stop_reason": "EndTurn",
        "usage": null
    });
    vec![tool_use.to_string(), end.to_string()]
}

#[test]
#[ignore = "builds the wasm32-wasip2 guest; run with --run-ignored"]
fn ungranted_host_is_denied_at_runtime_through_real_guest() {
    let wasm = build_guest("net-http");

    // Grant net.http to a DIFFERENT host than the cassette fetches, so
    // `blocked.invalid` is denied by the host `EgressPolicy`. Constructed via
    // `Capability`'s custom `Deserialize` impl (the manifest-authoring path;
    // `NetCapability::Http` is `#[non_exhaustive]` so it cannot be struct-
    // literal-constructed outside `tau-domain` — same pattern already used in
    // `hostset_divergence.rs`).
    let caps = vec![serde_json::from_str::<tau_domain::Capability>(
        r#"{"kind":"net.http","hosts":["api.anthropic.com"]}"#,
    )
    .unwrap()];

    let sandbox = tempfile::tempdir().unwrap();
    let (_payload, emitted) =
        tau_wasm_host::run_component_with_caps(&wasm, "go", cassette(), &caps, sandbox.path())
            .expect("run completes: the denial is a tool-result error, not a host trap");

    // The exact wasi:http ErrorCode the EgressPolicy returns before any socket
    // (Debug-formatted as `ErrorCode::HttpRequestDenied`; the substring check
    // pins the exact variant name, not a bare "denied"). #546 lesson: assert
    // the exact code.
    assert!(
        emitted.iter().any(|e| e.contains("HttpRequestDenied")),
        "ungranted host must be denied with HttpRequestDenied at the host \
         WasiCtx; emitted events:\n{emitted:#?}"
    );
}
