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

/// Point `$TAU_HOME` at a process-local tempdir so the governance gate can
/// resolve a global scope.
///
/// The wasm-build fixtures carry no `.tau/`, so `CheckCtx::load` walks up,
/// finds none, and falls back to `Scope::global()`, which reads
/// `$TAU_HOME`/`$XDG_DATA_HOME`/`$HOME`. On Windows CI none of those are set,
/// so the gate fails with `HomeNotFound` instead of the expected governance
/// verdict. We give it a dedicated tempdir initialized once, with
/// `config.toml` pre-created so parallel tests don't race writing it. Mirrors
/// `e2e_common::ensure_home_env` (added in #545). See #530.
fn ensure_home_env() {
    use std::sync::OnceLock;
    static TAU_HOME_INIT: OnceLock<()> = OnceLock::new();
    TAU_HOME_INIT.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("tau-wasm-build-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create tau_home tempdir");
        let cfg = dir.join("config.toml");
        if !cfg.exists() {
            let _ = std::fs::write(&cfg, "");
        }
        std::env::set_var("TAU_HOME", dir);
    });
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
    assert!(world.contains("import tau:host/host@0.1.0;"));
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

#[test]
fn emitted_world_is_deterministic_and_matches_generator() {
    let a = wasm_world_for_project(&fixture("net-http")).unwrap();
    let b = wasm_world_for_project(&fixture("net-http")).unwrap();
    assert_eq!(a, b, "world generation must be byte-deterministic");
    // The net-http fixture grants net → the world imports wasi:http, not wasi:filesystem.
    assert!(
        a.contains("import wasi:http/outgoing-handler@0.2.3;"),
        "{a}"
    );
    assert!(
        !a.contains("wasi:filesystem"),
        "net-only must not grant fs:\n{a}"
    );
}

#[tokio::test]
async fn ungoverned_project_is_refused_on_wasm_path() {
    ensure_home_env();
    // `trivial` declares no `[allow]` ceiling → GOV000 unless opted out.
    let err = wasm_governance_gate(&fixture("trivial"), GovernanceFlags::default())
        .await
        .expect_err("ungoverned must be refused");
    assert!(err.contains("GOV000"), "expected GOV000, got: {err}");
}

#[tokio::test]
async fn allow_ungoverned_flag_lets_it_proceed() {
    ensure_home_env();
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
    ensure_home_env();
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
