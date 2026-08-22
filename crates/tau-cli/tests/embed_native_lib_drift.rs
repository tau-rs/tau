//! EPIC 7.1: the committed embed-native workflow-lib stays byte-identical
//! to a fresh `tau build --target rust-lib` of its source project.
//!
//! `emit_rust_lib_to` writes four files: `Cargo.toml`, `src/lib.rs`,
//! `tau.wit`, `README.md`. `Cargo.toml` bakes `env!("CARGO_PKG_VERSION")`
//! (the tau crate version), so comparing it here would false-positive on
//! every tau version bump — it is deliberately excluded. `src/lib.rs`,
//! `tau.wit`, and `README.md` bake only the crate name, the canonical IR
//! bytes, and the IR content hash (`tau_ir::compute_hash`), none of which
//! depend on the tau version, so all three are guarded.
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    // crates/tau-cli -> crates -> repo root
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const REGENERATE_CMD: &str = "tau build --target rust-lib --allow-ungoverned \
     -o examples/embed-native/workflow-lib examples/embed-native/workflow";

#[test]
fn embed_native_lib_matches_fresh_render() {
    let root = repo_root();
    let project = root.join("examples/embed-native/workflow");
    let lib_dir = root.join("examples/embed-native/workflow-lib");

    let tmp = tempfile::tempdir().unwrap();
    let gen = tmp.path().join("gen");
    tau_cli::cmd::build::emit_rust_lib_to(&project, &gen).expect("emit_rust_lib_to");

    for rel in ["src/lib.rs", "tau.wit", "README.md"] {
        let committed = std::fs::read_to_string(lib_dir.join(rel))
            .unwrap_or_else(|e| panic!("committed workflow-lib/{rel}: {e}"));
        let fresh =
            std::fs::read_to_string(gen.join(rel)).unwrap_or_else(|e| panic!("fresh {rel}: {e}"));

        assert_eq!(
            committed, fresh,
            "examples/embed-native/workflow-lib/{rel} is stale; regenerate:\n  {REGENERATE_CMD}"
        );
    }
}
