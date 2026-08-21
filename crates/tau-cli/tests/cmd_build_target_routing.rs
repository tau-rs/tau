//! EPIC 5.1: `--target wasm-guest` routes to the wasm AOT pipeline. This asserts
//! the *routing* (resolve → wasm path selection) without the 60–90s `.wasm`
//! build; the full component build is covered by build_wasm_e2e / _world_dod.
#[test]
fn wasm_guest_keyword_selects_wasm_pipeline() {
    assert_eq!(
        tau_cli::cmd::build::classify_target_for_test(Some("wasm-guest")),
        "wasm-guest"
    );
    assert_eq!(
        tau_cli::cmd::build::classify_target_for_test(Some("rust-lib")),
        "rust-lib"
    );
    assert_eq!(
        tau_cli::cmd::build::classify_target_for_test(None),
        "bundle"
    );
    assert_eq!(
        tau_cli::cmd::build::classify_target_for_test(Some("passthrough")),
        "bundle"
    );
    assert_eq!(
        tau_cli::cmd::build::classify_target_for_test(Some("not a triple!!!")),
        "invalid"
    );
}
