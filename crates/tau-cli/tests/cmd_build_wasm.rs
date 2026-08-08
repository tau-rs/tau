//! `tau build wasm` pipeline tests (β.7.5 PR-E2).
//!
//! These tests verify the lowering + capability-fit logic only — they do NOT
//! shell `cargo build -p tau-wasm-guest` (a wasm build takes 60–90 s and
//! belongs in the Task 4 e2e test).

use std::path::PathBuf;

use tau_cli::cmd::build_wasm::{lower_to_wasm_ir, wasm_governance_gate, wasm_world_for_project};
use tau_cli::cmd::check::GovernanceFlags;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm-build")
        .join(name)
}

#[test]
fn trivial_project_lowers_to_wasm_ir() {
    let (module, bytes) = lower_to_wasm_ir(&fixture("trivial")).expect("trivial lowers");
    assert_eq!(module.ir_format.0, tau_ir::IrFormatVersion::CURRENT);
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

#[test]
fn project_using_control_flow_is_refused() {
    // A `Parallel` pipeline is control-flow; wasm guests drive run_ir_streaming,
    // not run_pipeline, so `tau build wasm` must refuse it (feature-fit, EPIC 4.2).
    let err = lower_to_wasm_ir(&fixture("needs-control-flow")).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("feature-fit") && msg.contains("control-flow") && msg.contains("Parallel"),
        "expected a feature-fit control-flow refusal naming Parallel, got: {msg}"
    );
}

#[test]
fn trivial_project_generates_host_only_world() {
    let world = wasm_world_for_project(&fixture("trivial")).expect("trivial world");
    assert!(world.contains("import host;"));
    assert!(
        !world.contains("wasi:"),
        "trivial should grant no wasi surface:\n{world}"
    );
}

#[test]
fn net_http_project_generates_http_world() {
    let world = wasm_world_for_project(&fixture("net-http")).expect("net-http world");
    assert!(
        world.contains("import wasi:http/outgoing-handler@0.2.3;"),
        "{world}"
    );
    assert!(world.contains("import wasi:io/streams@0.2.3;"), "{world}");
}

#[tokio::test]
async fn ungoverned_project_is_refused_on_wasm_path() {
    // `trivial` declares no `[allow]` ceiling → GOV000 unless opted out.
    let err = wasm_governance_gate(&fixture("trivial"), GovernanceFlags::default())
        .await
        .expect_err("ungoverned must be refused");
    assert!(err.contains("GOV000"), "expected GOV000, got: {err}");
}

#[tokio::test]
async fn allow_ungoverned_flag_lets_it_proceed() {
    let flags = GovernanceFlags {
        allow_ungoverned: true,
        no_governance: false,
    };
    wasm_governance_gate(&fixture("trivial"), flags)
        .await
        .expect("--allow-ungoverned proceeds");
}

#[tokio::test]
async fn over_reaching_project_is_refused_on_wasm_path() {
    // `over-reach` declares a `[allow]` ceiling of `net.http` scoped to
    // `example.com`, but its `fetch` tool actually requires
    // `api.anthropic.com` — a ceiling violation (`tau.governance.over_reach`),
    // not an absent-ceiling GOV000. Proves Approach B's headline behavior
    // (tool ⊆ agent-effective ⊆ root ceiling) is enforced on the wasm path too.
    let err = wasm_governance_gate(&fixture("over-reach"), GovernanceFlags::default())
        .await
        .expect_err("over-reaching project must be refused");
    assert!(
        err.contains("tau.governance.over_reach"),
        "expected an over_reach ceiling violation, got: {err}"
    );
}
