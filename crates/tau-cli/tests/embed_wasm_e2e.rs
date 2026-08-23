//! EPIC 7.2 DoD (#414): a product runtime loads a built tau wasm component
//! and runs it. Builds the trivial fixture component, then drives the
//! `tau-wasm-embed-example` binary against it — the roadmap acceptance
//! ("example loads + runs") as a test.

use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm-build")
        .join(name)
}

/// Build the guest with the trivial fixture's IR baked in, via the same
/// lowering the CLI uses, and return the component bytes.
fn build_trivial_component() -> Vec<u8> {
    let (_module, bytes) =
        tau_cli::cmd::build_wasm::lower_to_wasm_ir(&fixture("trivial")).expect("lowers");

    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .to_path_buf();
    let target_dir = workspace_root.join("target/tau-embed-wasm-e2e-guest");

    let ir_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(ir_file.path(), &bytes).unwrap();

    let output = Command::new(env!("CARGO"))
        .current_dir(&workspace_root)
        .args([
            "build",
            "-p",
            "tau-wasm-guest",
            "--target",
            "wasm32-wasip2",
            "--release",
            "--message-format=json",
        ])
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &target_dir)
        .env("TAU_IR_BYTES", ir_file.path())
        .output()
        .expect("cargo spawn");
    assert!(
        output.status.success(),
        "guest build failed (is wasm32-wasip2 installed?):\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let wasm_path = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|m| m["reason"] == "compiler-artifact")
        .filter(|m| {
            m["target"]["name"]
                .as_str()
                .is_some_and(|n| n == "tau-wasm-guest" || n == "tau_wasm_guest")
        })
        .flat_map(|m| {
            m["filenames"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| f.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .find(|f| f.ends_with(".wasm"))
        .expect("a .wasm artifact for tau-wasm-guest");
    std::fs::read(wasm_path).unwrap()
}

#[test]
#[ignore = "builds a wasm component; run with --run-ignored"]
fn example_product_loads_and_runs_component() {
    let component = build_trivial_component();
    let wasm_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(wasm_file.path(), &component).unwrap();

    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .to_path_buf();

    let run = Command::new(env!("CARGO"))
        .current_dir(&workspace_root)
        .args(["run", "--quiet", "-p", "tau-wasm-embed-example", "--"])
        .arg(wasm_file.path())
        .arg("hi")
        .env("CARGO_INCREMENTAL", "0")
        .env(
            "CARGO_TARGET_DIR",
            workspace_root.join("target/tau-embed-wasm-e2e"),
        )
        .output()
        .expect("cargo spawn");

    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        run.status.success(),
        "example product failed\n--- stdout\n{stdout}\n--- stderr\n{stderr}"
    );
    assert!(
        stdout.contains("RunCompleted"),
        "expected a terminal RunCompleted event line:\n{stdout}"
    );
    assert!(
        stdout.contains("run completed:"),
        "expected the product's summary line:\n{stdout}"
    );
    assert!(
        !stderr.contains("unparseable RunEvent"),
        "every emitted event must parse as a typed RunEvent:\n{stderr}"
    );
}
