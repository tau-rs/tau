//! EPIC 5.1: `tau build --target rust-lib` emits the no_std embedding crate.
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm-build")
        .join(name)
}

#[test]
fn rust_lib_emits_shape_checked_crate() {
    let out = tempfile::tempdir().unwrap();
    let dir = out.path().join("gen");
    // Public seam used by the CLI dispatch — same as the wasm path's seams.
    tau_cli::cmd::build::emit_rust_lib_to(&fixture("trivial"), &dir).unwrap();

    for f in ["Cargo.toml", "src/lib.rs", "tau.wit", "README.md"] {
        assert!(dir.join(f).exists(), "missing {f}");
    }
    let lib = std::fs::read_to_string(dir.join("src/lib.rs")).unwrap();
    assert!(lib.contains("#![no_std]"));
    assert!(lib.contains("pub const TAU_IR: &[u8] = &["));
    assert!(lib.contains("pub use tau_runtime_core::run_ir"));
}
