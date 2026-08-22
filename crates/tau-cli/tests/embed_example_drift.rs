//! Keeps `crates/tau-embed-example/fixtures/trivial.ir.json` byte-equal to
//! lowering its `project/tau.toml` — the example's baked IR can't silently
//! drift from its source (same pattern as the SDK byte-equal tests).

use std::path::{Path, PathBuf};

fn example_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tau-embed-example")
}

#[test]
fn example_ir_fixture_matches_lowered_project() {
    let (_module, bytes) =
        tau_cli::cmd::build_wasm::lower_to_wasm_ir(&example_dir().join("project")).unwrap();
    // To REGENERATE after editing project/tau.toml:
    // std::fs::write(example_dir().join("fixtures/trivial.ir.json"), &bytes).unwrap();
    let committed = std::fs::read(example_dir().join("fixtures/trivial.ir.json")).unwrap();
    assert_eq!(
        bytes, committed,
        "example IR fixture drifted — uncomment the write line above, rerun, re-comment"
    );
}
