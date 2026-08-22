//! EPIC 7.1: the committed embed-native workflow-lib stays byte-identical
//! to a fresh `tau build --target rust-lib` of its source project.
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    // crates/tau-cli -> crates -> repo root
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn embed_native_lib_matches_fresh_render() {
    let root = repo_root();
    let project = root.join("examples/embed-native/workflow");
    let committed =
        std::fs::read_to_string(root.join("examples/embed-native/workflow-lib/src/lib.rs"))
            .expect("committed workflow-lib/src/lib.rs");

    let tmp = tempfile::tempdir().unwrap();
    let gen = tmp.path().join("gen");
    tau_cli::cmd::build::emit_rust_lib_to(&project, &gen).expect("emit_rust_lib_to");
    let fresh = std::fs::read_to_string(gen.join("src/lib.rs")).expect("fresh src/lib.rs");

    assert_eq!(
        committed, fresh,
        "examples/embed-native/workflow-lib/src/lib.rs is stale; regenerate:\n  \
         tau build --target rust-lib --allow-ungoverned \
         -o examples/embed-native/workflow-lib examples/embed-native/workflow"
    );
}
