//! `tau embed` — emit host-embedding glue for a target language
//! (Phase 2 §5.2). Only `--host js` (the `@tau/embed-js` scaffold) is
//! supported today.

use anyhow::{bail, Result};

use crate::cli::EmbedArgs;
use crate::output::Output;

/// CLI entry point for `tau embed --host js`.
pub async fn run(args: &EmbedArgs, output: &mut Output) -> Result<()> {
    if args.host != "js" {
        bail!("unsupported --host '{}': only 'js' is supported", args.host);
    }

    let out_root = args
        .output
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let rendered = tau_sdk_codegen::embed_js::render_embed_js();
    for (rel, contents) in &rendered {
        // Rendered paths are repo-relative under sdk/embed-js/; write beneath out_root.
        let path = out_root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, contents)?;
    }

    let _ = output.human(&format!(
        "emitted @tau/embed-js ({} files) under {}",
        rendered.len(),
        out_root.display()
    ));
    Ok(())
}
