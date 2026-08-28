//! EPIC 7.1 DoD: the generated rust-lib + embed-rust crates COMPILE and RUN
//! against this workspace (no more string-only checking). Shells out to
//! cargo; slow (cold-builds futures/serde_json/tau-runtime-core once per
//! tempdir) but CI-runnable.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm-build")
        .join(name)
}

/// Workspace checkout root (two levels up from crates/tau-cli).
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/tau-cli has a workspace root")
        .to_path_buf()
}

#[test]
fn generated_embed_rust_compiles_and_runs() {
    let out = tempfile::tempdir().unwrap();
    let root = workspace_root().display().to_string().replace('\\', "/");
    let dep = tau_sdk_codegen::TauDep::Path(&root);

    // rust-lib at tempdir root; embed-rust/ beside it (path dep "..").
    tau_cli::cmd::build::emit_rust_lib_to(&fixture("trivial"), out.path(), dep).unwrap();
    tau_cli::cmd::embed::emit_host_to("rust", &fixture("trivial"), out.path(), dep).unwrap();

    let target = out.path().join("e2e-target");
    let run = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .current_dir(out.path().join("embed-rust"))
        .env("CARGO_TARGET_DIR", &target)
        .env("CARGO_INCREMENTAL", "0")
        .output()
        .expect("cargo is on PATH");

    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        run.status.success(),
        "generated embed-rust failed to build/run\n--- stdout\n{stdout}\n--- stderr\n{stderr}"
    );
    assert!(
        stdout.contains("RunCompleted"),
        "expected a terminal RunCompleted event on stdout:\n{stdout}\n--- stderr\n{stderr}"
    );
    assert!(
        stdout.contains("embed-rust scaffold reply"),
        "echo backend text should appear in emitted events:\n{stdout}"
    );
}
