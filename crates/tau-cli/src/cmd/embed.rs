//! `tau embed` — emit host-embedding glue for a target language
//! (Phase 2 §5.2). `--host js` emits the project-independent `@tau/embed-js`
//! scaffold; `--host rust|c` derive IR + WIT from `--project` (defaulting to
//! the CWD) exactly like the EPIC 5.1 `tau build --target rust-lib`/`wasm-guest`
//! paths, then render the native/C host scaffolds.

use anyhow::{bail, Result};

use crate::cli::EmbedArgs;
use crate::output::Output;

/// Accept only the three supported hosts, with a message that names them.
pub(crate) fn validate_host(host: &str) -> Result<()> {
    match host {
        "js" | "rust" | "c" => Ok(()),
        other => bail!("unsupported --host '{other}': expected one of js, rust, c"),
    }
}

/// Result of an embed emission (for human/JSON output + tests).
pub struct EmbedArtifact {
    /// `"embed-js" | "embed-rust" | "embed-c"`.
    pub kind: &'static str,
    /// Directory the scaffold was written beneath.
    pub out_root: std::path::PathBuf,
    /// Number of files written.
    pub files: usize,
    /// Lowercase-hex IR module hash baked into the scaffold. `Some` for
    /// `rust`/`c` (IR-derived); `None` for `js` (project-independent).
    pub ir_hash: Option<String>,
}

/// Render the selected host scaffold and write it under `out_root`. Test
/// seam used by both the CLI dispatch and integration tests: derives IR +
/// WIT from `project` for `rust`/`c` (mirroring `build::emit_rust_lib_to`),
/// runs no governance, and touches no `Output`. `project` is ignored for
/// `js` (that scaffold is project-independent).
pub fn emit_host_to(
    host: &str,
    project: &std::path::Path,
    out_root: &std::path::Path,
    tau_dep: tau_sdk_codegen::TauDep,
) -> Result<EmbedArtifact> {
    use crate::cmd::build::{hex_lower, sanitize_crate_name};
    use crate::cmd::build_wasm::{lower_to_wasm_ir, world_from_module};

    let (kind, rendered, ir_hash) = match host {
        "js" => (
            "embed-js",
            tau_sdk_codegen::embed_js::render_embed_js(),
            None,
        ),
        "rust" => {
            let (module, _bytes) = lower_to_wasm_ir(project)?;
            let ir_hash = hex_lower(&tau_ir::compute_hash(&module));
            let wit = world_from_module(&module)?;
            let stem = project
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("workflow");
            let lib = sanitize_crate_name(stem);
            let host_name = format!("{lib}_host");
            let map = tau_sdk_codegen::render_embed_rust(tau_sdk_codegen::EmbedRustInput {
                crate_name: &host_name,
                lib_crate_name: &lib,
                ir_hash: &ir_hash,
                wit: &wit,
                tau_dep,
            });
            ("embed-rust", map, Some(ir_hash))
        }
        "c" => {
            let (module, _bytes) = lower_to_wasm_ir(project)?;
            let ir_hash = hex_lower(&tau_ir::compute_hash(&module));
            let wit = world_from_module(&module)?;
            let stem = project
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("workflow");
            let base = sanitize_crate_name(stem);
            let map = tau_sdk_codegen::render_embed_c(tau_sdk_codegen::EmbedCInput {
                base_name: &base,
                ir_hash: &ir_hash,
                wit: &wit,
            });
            ("embed-c", map, Some(ir_hash))
        }
        other => bail!("unsupported --host '{other}': expected one of js, rust, c"),
    };

    let mut files = 0usize;
    for (rel, contents) in &rendered {
        let path = out_root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, contents)?;
        files += 1;
    }
    Ok(EmbedArtifact {
        kind,
        out_root: out_root.to_path_buf(),
        files,
        ir_hash,
    })
}

/// CLI entry point for `tau embed --host js|rust|c`.
pub async fn run(args: &EmbedArgs, output: &mut Output) -> Result<()> {
    validate_host(&args.host)?;

    let out_root = args
        .output
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let project = args
        .project
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    let dep_path; // owned String kept alive for the borrow below
    let tau_dep = match &args.tau_dep_path {
        Some(p) => {
            dep_path = p.display().to_string().replace('\\', "/");
            tau_sdk_codegen::TauDep::Path(&dep_path)
        }
        None => tau_sdk_codegen::TauDep::Version(env!("CARGO_PKG_VERSION")),
    };
    let artifact = emit_host_to(&args.host, &project, &out_root, tau_dep)?;

    if output.is_json() {
        let mut obj = serde_json::json!({
            "kind": artifact.kind,
            "path": artifact.out_root.display().to_string(),
            "files": artifact.files,
        });
        if let Some(h) = &artifact.ir_hash {
            obj["ir_hash"] = serde_json::json!(h);
        }
        let _ = output.json(&obj);
    } else {
        let _ = output.human(&format!(
            "emitted {} ({} files) under {}",
            artifact.kind,
            artifact.files,
            artifact.out_root.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_host;

    #[test]
    fn validate_host_accepts_the_three_hosts_and_rejects_others() {
        for h in ["js", "rust", "c"] {
            assert!(validate_host(h).is_ok(), "{h} should be valid");
        }
        let err = validate_host("go").unwrap_err().to_string();
        assert!(err.contains("unsupported --host 'go'"), "{err}");
        assert!(err.contains("js"), "{err}");
        assert!(err.contains("rust"), "{err}");
        assert!(err.contains("c"), "{err}");
    }
}
