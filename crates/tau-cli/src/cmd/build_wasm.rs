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
use tau_ir_lower::LowerError;
use tau_ports::target::wit_world::generate_world;

use crate::cli::BuildWasmArgs;
use crate::cmd::build::{hex_lower, native_tool_hash};
use crate::cmd::check::{
    evaluate_governance, render_no_constitution, render_violations, CheckCtx, GovernanceFlags,
    GovernanceOutcome,
};
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
        prompt_file: &|p: &std::path::Path| {
            tau_pkg::bundle::read_prompt_file(p, project)
                .map_err(|e| tau_ir_lower::PromptFileError(e.to_string()))
        },
    };

    let out =
        tau_ir_lower::lower_project(&loaded.project, &target, &caches).map_err(|e| match e {
            LowerError::CapabilityFitFailed {
                ref missing,
                ref tools,
            } => anyhow::anyhow!(
                "capability-fit refused for {WASM_TARGET}: \
                 the following capability shape(s) are not supported by wasm guests: {missing:?}. \
                 Offending tools: {tools:?}. \
                 Remove or replace tools that require ProcessExec or AgentSpawn \
                 before building for wasm.",
            ),
            LowerError::FeatureUnsupported { ref missing, .. } => anyhow::anyhow!(
                "feature-fit refused for {WASM_TARGET}: wasm guests cannot execute \
                 control-flow pipeline steps {missing:?} — the guest drives run_ir_streaming, \
                 which has no run_pipeline path. Flatten the pipeline (remove \
                 Branch/Parallel/Loop steps) before building for wasm.",
            ),
            other => anyhow::anyhow!("lowering for {WASM_TARGET} failed: {other}"),
        })?;
    // NOTE(D6-B PR3): embedding the content-addressed asset store into the
    // wasm component (so the guest can resolve `PromptSource::Asset` prompts)
    // lands with the wasm/WIT lane. Until then, warn rather than silently ship
    // a wasm module whose prompt assets the guest cannot resolve.
    if !out.assets.is_empty() {
        tracing::warn!(
            asset_count = out.assets.len(),
            "wasm build: {WASM_TARGET} does not yet embed the prompt asset store (D6-B PR3); \
             agents using `system_file` prompts will not resolve them in the wasm guest yet"
        );
    }
    let module = out.module;
    let bytes = tau_ir::to_canonical_bytes(&module);
    Ok((module, bytes))
}

/// Aggregate a lowered module's used capabilities and generate the guest's WIT
/// world. The used caps come from every tool's `declared` set in the IR
/// capability table; canonicalized so cap order and duplicates never affect the
/// world. After the governance gate proceeds these caps are provably within
/// `[allow]` (tool ⊆ agent-effective ⊆ root ceiling), so the generated world is
/// the `[allow]`-bounded set — no redundant `meet`.
pub fn world_from_module(module: &tau_ir::IrModule) -> Result<String> {
    let used: Vec<tau_domain::Capability> = module
        .workflow
        .capability_table
        .0
        .values()
        .flat_map(|req| req.declared.iter().cloned())
        .collect();
    let caps = tau_domain::canon_caps(&used);
    generate_world(&caps).map_err(|e| anyhow::anyhow!("wasm WIT-world generation failed: {e}"))
}

/// Lower a project and generate its WIT world. Test seam so world generation is
/// exercisable without shelling the 60-90s wasm build.
pub fn wasm_world_for_project(project: &Path) -> Result<String> {
    let (module, _bytes) = lower_to_wasm_ir(project)?;
    world_from_module(&module)
}

/// Governed-by-default gate for the wasm build path (ADR-0057 / D2), reusing
/// the `tau check governance` engine. Returns `Ok(())` to proceed or
/// `Err(diagnostic)` — the caller prints the diagnostic and exits 2. `tau build
/// wasm` produces no bundle, so the `GovernanceVerdict` is not stamped.
pub async fn wasm_governance_gate(
    project_path: &Path,
    flags: GovernanceFlags,
) -> std::result::Result<(), String> {
    let target: tau_ports::target::TargetTriple = WASM_TARGET
        .parse()
        .expect("any-wasi-strict is a registered triple");
    let ctx = CheckCtx::load(project_path.to_path_buf(), false, Some(target))
        .await
        .map_err(|e| format!("cannot evaluate governance: {e}"))?;
    let Some(project) = &ctx.project else {
        // Unparseable project — let the lowering path surface the precise error.
        return Ok(());
    };
    match evaluate_governance(project, &ctx, flags) {
        GovernanceOutcome::Proceed(_) => Ok(()),
        GovernanceOutcome::NoConstitution => Err(render_no_constitution()),
        GovernanceOutcome::Violations(findings) => Err(render_violations(&findings)),
    }
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
/// The IR path is injected via `TAU_IR_BYTES` env var and the cap-derived WIT
/// world via `TAU_WORLD_WIT`. A dedicated `CARGO_TARGET_DIR`
/// (`<workspace>/target/tau-build-wasm`) ensures this build never contends
/// with the main agent's target dir (CLAUDE.md Rule 1).
fn build_guest_with_ir(ir_path: &Path, world_path: &Path) -> Result<Vec<u8>> {
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
        .env("TAU_WORLD_WIT", world_path)
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

    // Governed-by-default gate (ADR-0057 / D2) — refuse an ungoverned or
    // over-reaching project before doing any build work.
    let flags = GovernanceFlags {
        allow_ungoverned: args.allow_ungoverned,
        no_governance: args.no_governance,
    };
    if let Err(diag) = wasm_governance_gate(&project, flags).await {
        let _ = output.diagnostic(diag);
        std::process::exit(2);
    }

    let (module, bytes) = lower_to_wasm_ir(&project)?;
    let ir_hash = hex_lower(&tau_ir::compute_hash(&module));

    // Generate the cap-derived WIT world BEFORE the expensive guest build so an
    // unsupported-on-wasm capability fails fast (exit 2) instead of after a
    // 60-90s build that would leave an orphan `.wasm` with no `.wit`. Computed
    // from the already-lowered `module` — no second lowering.
    let world = match world_from_module(&module) {
        Ok(w) => w,
        Err(e) => {
            let _ = output.error(format!("{e}"));
            std::process::exit(2);
        }
    };

    // Bake the IR bytes into a tempfile the guest build reads via TAU_IR_BYTES.
    // The file must stay alive until after the cargo call returns.
    let ir_file = tempfile::NamedTempFile::new().context("creating IR scratch file")?;
    std::fs::write(ir_file.path(), &bytes).context("writing IR scratch bytes")?;

    // Bake the cap-derived world into a tempfile the guest build reads via
    // TAU_WORLD_WIT. Must also stay alive until after the cargo call returns.
    let world_file = tempfile::NamedTempFile::new().context("creating world scratch file")?;
    std::fs::write(world_file.path(), world.as_bytes()).context("writing world scratch bytes")?;

    let wasm = build_guest_with_ir(ir_file.path(), world_file.path())?;
    drop(ir_file); // bytes are consumed; safe to remove the scratch file now.
    drop(world_file);

    let out_path = args
        .output
        .clone()
        .unwrap_or_else(|| project.join(format!("{}.wasm", project_stem(&project))));
    std::fs::write(&out_path, &wasm).with_context(|| format!("writing {}", out_path.display()))?;

    // Write the (already-generated) WIT world next to the component.
    let wit_path = out_path.with_extension("wit");
    std::fs::write(&wit_path, &world).with_context(|| format!("writing {}", wit_path.display()))?;

    let _ = output.human(&format!(
        "built {} ({} bytes, ir {}) + {}",
        out_path.display(),
        wasm.len(),
        ir_hash,
        wit_path.display()
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
