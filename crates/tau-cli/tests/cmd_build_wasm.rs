//! `tau build wasm` pipeline tests (β.7.5 PR-E2).
//!
//! These tests verify the lowering + capability-fit logic only — they do NOT
//! shell `cargo build -p tau-wasm-guest` (a wasm build takes 60–90 s and
//! belongs in the Task 4 e2e test).

use std::path::PathBuf;

use tau_cli::cmd::build_wasm::lower_to_wasm_ir;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm-build")
        .join(name)
}

#[test]
fn trivial_project_lowers_to_wasm_ir() {
    let (module, bytes) = lower_to_wasm_ir(&fixture("trivial")).expect("trivial lowers");
    assert_eq!(module.ir_format.0, "v2.0.0");
    assert!(!bytes.is_empty(), "canonical IR bytes must be non-empty");
    // Re-decoding the bytes yields an equal module (round-trip sanity).
    let decoded = tau_ir::from_canonical_bytes(&bytes).expect("bytes decode");
    assert_eq!(decoded.ir_format.0, module.ir_format.0);
}

#[test]
fn project_needing_process_exec_is_refused() {
    let err = lower_to_wasm_ir(&fixture("needs-exec")).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("capability") || msg.contains("CapabilityFit"),
        "expected a capability-fit refusal, got: {msg}"
    );
}
