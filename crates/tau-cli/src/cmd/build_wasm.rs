//! `tau build wasm <project>` — IR-to-wasm AOT compiler (β.7.5).
//!
//! Pipeline: load + validate the project, lower its IR for `any-wasi-strict`
//! (capability-fit refuses `process-exec`/`agent-spawn` here), serialize the
//! canonical IR bytes, bake them into the `tau-wasm-guest` crate via the
//! `TAU_IR_BYTES` env handshake, shell `cargo build … --target wasm32-wasip2`,
//! and emit the produced `.wasm` plus its IR hash.
//!
//! The guest source lives in *this* workspace; the command shells cargo
//! against it (located via `CARGO_MANIFEST_DIR`). Shipping the guest for a
//! user-facing `tau run --wasm` is a γ concern.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context as _, Result};

use crate::cli::BuildWasmArgs;
use crate::cmd::build::{hex_lower, native_tool_hash};
use crate::cmd::project_load::load_project;
use crate::output::Output;

/// The triple every wasm build lowers for.
const WASM_TARGET: &str = "any-wasi-strict";

/// Load + validate the project and lower its IR for `any-wasi-strict`.
///
/// Returns the lowered module and its canonical bytes. Capability-fit is
/// enforced inside `lower_project`; a `process-exec`/`agent-spawn` project
/// surfaces `LowerError::CapabilityFitFailed` here (no bypass flag).
///
/// This function is also the entry point for the Task 4 e2e integration test
/// so it must remain `pub`.
pub fn lower_to_wasm_ir(project: &Path) -> Result<(tau_ir::IrModule, Vec<u8>)> {
    let loaded = load_project(project)
        .with_context(|| format!("loading project from {}", project.display()))?;

    let target: tau_ports::target::TargetTriple = WASM_TARGET
        .parse()
        .expect("any-wasi-strict is a registered triple");
    if tau_ports::target::lookup(&target).is_none() {
        bail!("internal: target `{WASM_TARGET}` not found in the registry");
    }

    let caches = tau_ir_lower::Caches {
        native_tool: &|name: &str| native_tool_hash(name),
        mcp_contract: &|_url| None,
        skill: &|_name| None,
    };

    let module = tau_ir_lower::lower_project(&loaded.project, &target, &caches)
        .map_err(|e| anyhow::anyhow!("lowering for {WASM_TARGET} failed: {e}"))?;
    let bytes = tau_ir::to_canonical_bytes(&module);
    Ok((module, bytes))
}

/// Locate the tau source workspace root at compile time.
///
/// The guest source lives in the same Cargo workspace. We locate the workspace
/// root at compile time so the command works regardless of the working
/// directory at runtime (e.g. inside a tmp project directory).
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("tau-cli is two levels below the workspace root")
        .to_path_buf()
}

/// Shell `cargo build -p tau-wasm-guest` with the baked IR and return the
/// produced `.wasm` bytes.
///
/// The IR path is injected via `TAU_IR_BYTES` env var. A dedicated
/// `CARGO_TARGET_DIR` (`<workspace>/target/tau-build-wasm`) ensures this
/// build never contends with the main agent's target dir (CLAUDE.md Rule 1).
fn build_guest_with_ir(ir_path: &Path) -> Result<Vec<u8>> {
    let root = workspace_root();
    let target_dir = root.join("target/tau-build-wasm");

    let output = Command::new(env!("CARGO"))
        .current_dir(&root)
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
        .env("TAU_IR_BYTES", ir_path)
        .output()
        .context("failed to spawn cargo for the guest build")?;

    if !output.status.success() {
        bail!(
            "guest build failed (is wasm32-wasip2 installed?):\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8(output.stdout).context("cargo json output is utf-8")?;
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
        .context("no .wasm artifact in cargo output for tau-wasm-guest")?;

    std::fs::read(&wasm_path).with_context(|| format!("reading built component {wasm_path}"))
}

/// CLI entry point for `tau build wasm`.
pub async fn run(args: &BuildWasmArgs, output: &mut Output) -> Result<()> {
    let project = args
        .project
        .clone()
        .unwrap_or_else(|| std::env::current_dir().expect("cwd is readable"));

    let (module, bytes) = lower_to_wasm_ir(&project)?;
    let ir_hash = hex_lower(&tau_ir::compute_hash(&module));

    // Bake the IR bytes into a tempfile the guest build reads via TAU_IR_BYTES.
    // The file must stay alive until after the cargo call returns.
    let ir_file = tempfile::NamedTempFile::new().context("creating IR scratch file")?;
    std::fs::write(ir_file.path(), &bytes).context("writing IR scratch bytes")?;

    let wasm = build_guest_with_ir(ir_file.path())?;
    drop(ir_file); // bytes are consumed; safe to remove the scratch file now.

    let out_path = args
        .output
        .clone()
        .unwrap_or_else(|| project.join(format!("{}.wasm", project_stem(&project))));
    std::fs::write(&out_path, &wasm).with_context(|| format!("writing {}", out_path.display()))?;

    let _ = output.human(&format!(
        "built {} ({} bytes, ir {})",
        out_path.display(),
        wasm.len(),
        ir_hash
    ));
    Ok(())
}

fn project_stem(project: &Path) -> String {
    project
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string()
}
