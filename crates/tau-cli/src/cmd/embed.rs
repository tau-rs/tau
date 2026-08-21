//! `tau embed` — emit host-embedding glue for a target language
//! (Phase 2 §5.2). `--host js` (the `@tau/embed-js` scaffold) is
//! implemented; `rust`/`c` are accepted by `validate_host` but not yet
//! wired.

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

/// CLI entry point for `tau embed --host js|rust|c`.
pub async fn run(args: &EmbedArgs, output: &mut Output) -> Result<()> {
    validate_host(&args.host)?;

    if args.host != "js" {
        // TODO(EPIC 5.2 Task 4): wire the rust/c embed bodies.
        bail!("--host '{}': not yet wired", args.host);
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
