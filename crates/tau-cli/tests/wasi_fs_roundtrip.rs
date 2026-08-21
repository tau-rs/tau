//! EPIC 3.6-b live host-enforcement round-trip: the REAL production guest,
//! built by `tau build wasm`, driven by real IR, issues a `Read` at an
//! UNGRANTED path — the host granted no preopen for it, so the guest holds no
//! descriptor and surfaces `FsAccessDenied`. Denial-only (offline; a granted
//! read needs seeded files, covered by the host-side `wasi_fs_enforcement.rs`
//! fs-probe test). Builds the wasm32-wasip2 guest, so it is #[ignore]d.

use std::path::PathBuf;
use std::process::Command;

use tau_cli::cmd::build_wasm::{lower_to_wasm_ir, wasm_world_for_project};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm-build")
        .join(name)
}

/// Build the guest for a fixture and return its component bytes. Copied from
/// `wasi_http_roundtrip.rs::build_guest`.
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

/// A cassette turn that calls the `read_file` tool at an ungranted path, then a
/// turn that ends. Field spellings mirror `tau_ports::llm` exactly (verified in
/// `wasi_http_roundtrip.rs`).
fn cassette() -> Vec<String> {
    let tool_use = serde_json::json!({
        "text": "",
        "tool_uses": [{
            "id": "call_1",
            "name": "read_file",
            "input": { "path": "/etc/secret" }
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
fn ungranted_path_is_denied_at_runtime_through_real_guest() {
    let wasm = build_guest("fs-read");

    // Grant fs.read on /data/** → the host preopens <sandbox>/data as guest
    // path "/data". The cassette reads "/etc/secret", for which the host
    // granted NO preopen, so the guest holds no descriptor. Constructed via
    // Capability's Deserialize impl (FsCapability::Read is #[non_exhaustive],
    // same manifest-authoring path used by wasi_http_roundtrip.rs / wasi_map).
    let caps = vec![serde_json::from_str::<tau_domain::Capability>(
        r#"{"kind":"fs.read","paths":["/data/**"]}"#,
    )
    .unwrap()];

    let sandbox = tempfile::tempdir().unwrap();
    let (_payload, emitted) =
        tau_wasm_host::run_component_with_caps(&wasm, "go", cassette(), &caps, sandbox.path())
            .expect("run completes: the denial is a tool-result error, not a host trap");

    // Ungranted-path denial is guest-observed ABSENCE (no host error-code exists
    // by construction — the guest never calls the host for a path it holds no
    // descriptor for), so the marker is the guest's exact `FsAccessDenied`, not
    // net's host `HttpRequestDenied`. See ADR-0066.
    assert!(
        emitted.iter().any(|e| e.contains("FsAccessDenied")),
        "ungranted path must be denied with FsAccessDenied; emitted events:\n{emitted:#?}"
    );
}
