//! `tau mcp diff <name>` — read-only diff between pin file and live probe.
//! Exit 0 if no drift, exit 64 if drift detected.

use anyhow::{Context, Result};
use tau_mcp::contract::pinned::PinnedContract;

use crate::cli::McpDiffArgs;
use crate::cmd::mcp::probe_only;
use crate::output::Output;

/// Run `tau mcp diff`.
pub async fn run(args: McpDiffArgs, _output: &mut Output) -> Result<()> {
    let project_root = std::env::current_dir().context("get cwd")?;
    let pin_path = project_root
        .join(".tau/mcp")
        .join(format!("{}.contract.json", &args.name));
    let bytes = std::fs::read(&pin_path)
        .with_context(|| format!("no pin file at {}", pin_path.display()))?;
    let pinned: PinnedContract = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse {}", pin_path.display()))?;

    let live = probe_only(&args.name, None, &project_root)
        .await
        .context("probe live MCP server")?;

    let drift = pinned.contract_hash_hex != live.contract_hash_hex;

    if args.json {
        let payload = serde_json::json!({
            "name": &args.name,
            "drift": drift,
            "pin_hash": &pinned.contract_hash_hex,
            "live_hash": &live.contract_hash_hex,
            "pin_tools": pinned.contract.tools.len(),
            "live_tools": live.contract.tools.len(),
            "pin_server_version": &pinned.contract.server_info.version,
            "live_server_version": &live.contract.server_info.version,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if !drift {
        println!("no drift: `{}` matches live server", args.name);
    } else {
        println!("DRIFT detected for `{}`:", args.name);
        println!("  pin hash:  {}", &pinned.contract_hash_hex);
        println!("  live hash: {}", &live.contract_hash_hex);
        println!(
            "  pin tools / live tools: {} / {}",
            pinned.contract.tools.len(),
            live.contract.tools.len()
        );
        println!(
            "  pin server / live server: {} v{} / {} v{}",
            pinned.contract.server_info.name,
            pinned.contract.server_info.version,
            live.contract.server_info.name,
            live.contract.server_info.version,
        );
    }

    if drift {
        std::process::exit(64);
    }
    Ok(())
}
