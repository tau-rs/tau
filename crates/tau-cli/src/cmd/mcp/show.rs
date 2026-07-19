//! `tau mcp show <name>` — show a pinned MCP contract.

use anyhow::{Context, Result};
use tau_mcp::contract::pinned::PinnedContract;

use crate::cli::McpShowArgs;
use crate::cmd::mcp::{render_sarif, OutputFormat};
use crate::output::Output;

/// Run `tau mcp show`.
pub async fn run(args: McpShowArgs, _output: &mut Output) -> Result<()> {
    let project_root = std::env::current_dir().context("get cwd")?;
    let pin_path = project_root
        .join(".tau/mcp")
        .join(format!("{}.contract.json", args.name));
    let bytes = std::fs::read(&pin_path)
        .with_context(|| format!("no pin file at {}", pin_path.display()))?;
    let pinned: PinnedContract =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", pin_path.display()))?;

    match OutputFormat::from_flags(args.json, args.sarif) {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&pinned)?);
        }
        OutputFormat::Sarif => {
            let payload = serde_json::to_value(&pinned)?;
            let sarif = render_sarif("tau-mcp/show", payload);
            println!("{}", serde_json::to_string_pretty(&sarif)?);
        }
        OutputFormat::Human => {
            println!("name:       {}", args.name);
            println!("url:        {}", pinned.url);
            println!(
                "server:     {} v{}",
                pinned.contract.server_info.name, pinned.contract.server_info.version
            );
            println!("hash:       {}", pinned.contract_hash_hex);
            println!("tools:      {}", pinned.contract.tools.len());
            for t in &pinned.contract.tools {
                println!("  - {}", t.name);
            }
        }
    }
    Ok(())
}
