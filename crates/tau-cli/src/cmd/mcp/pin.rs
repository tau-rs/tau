//! `tau mcp pin <name> [--from URL]` — probe a server and write its
//! contract to `.tau/mcp/<name>.contract.json`.
//!
//! See spec at `docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md`
//! §10 (CLI surface) and ADR-0038.

use anyhow::{Context, Result};

use crate::cli::McpPinArgs;
use crate::cmd::mcp::probe_and_pin;
use crate::output::Output;

/// Run `tau mcp pin`.
pub async fn run(args: McpPinArgs, _output: &mut Output) -> Result<()> {
    let project_root = std::env::current_dir().context("get cwd")?;
    let pin_path = project_root
        .join(".tau/mcp")
        .join(format!("{}.contract.json", args.name));

    let (pinned, _prev) = probe_and_pin(&args.name, args.from, &project_root).await?;

    if args.json {
        let payload = serde_json::json!({
            "ok": true,
            "name": args.name,
            "path": pin_path,
            "url": pinned.url,
            "contract_hash_hex": pinned.contract_hash_hex,
            "tools_count": pinned.contract.tools.len(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!(
            "pinned `{}` from {} → {} ({} tools, hash {})",
            args.name,
            pinned.url,
            pin_path.display(),
            pinned.contract.tools.len(),
            &pinned.contract_hash_hex[..16],
        );
    }
    Ok(())
}
