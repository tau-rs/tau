//! EPIC 5.2 Task 4: `tau embed --host rust|c` derives IR + WIT from the
//! project (like the EPIC 5.1 build path) and writes the renderer output.
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm-build")
        .join(name)
}

#[test]
fn embed_host_rust_writes_native_host_crate() {
    let out = tempfile::tempdir().unwrap();
    let art = tau_cli::cmd::embed::emit_host_to(
        "rust",
        &fixture("trivial"),
        out.path(),
        tau_sdk_codegen::TauDep::Version(env!("CARGO_PKG_VERSION")),
    )
    .unwrap();
    assert_eq!(art.kind, "embed-rust");
    assert!(art.ir_hash.is_some());
    for f in [
        "embed-rust/Cargo.toml",
        "embed-rust/src/main.rs",
        "embed-rust/tau.wit",
    ] {
        assert!(out.path().join(f).exists(), "missing {f}");
    }
    let main = std::fs::read_to_string(out.path().join("embed-rust/src/main.rs")).unwrap();
    assert!(main.contains("run_ir_streaming("));
    assert!(main.contains("impl ToolDispatcher for ScaffoldDispatcher"));
    let wit = std::fs::read_to_string(out.path().join("embed-rust/tau.wit")).unwrap();
    assert!(wit.contains("world runner"));
}

#[test]
fn embed_host_c_writes_wasmtime_host_stub() {
    let out = tempfile::tempdir().unwrap();
    let art = tau_cli::cmd::embed::emit_host_to(
        "c",
        &fixture("trivial"),
        out.path(),
        tau_sdk_codegen::TauDep::Version(env!("CARGO_PKG_VERSION")),
    )
    .unwrap();
    assert_eq!(art.kind, "embed-c");
    let header = std::fs::read_to_string(out.path().join("embed-c/tau_embed.h")).unwrap();
    assert!(header.contains("tau_host_complete"));
    assert!(header.contains("tau_embed_run"));
    let wit = std::fs::read_to_string(out.path().join("embed-c/tau.wit")).unwrap();
    assert!(wit.contains("world runner"));
}
