//! `tau mcp refresh <name>` — re-probe and overwrite the pin; report diff.

use anyhow::{Context, Result};

use crate::cli::McpRefreshArgs;
use crate::cmd::mcp::probe_and_pin;
use crate::output::Output;

/// Run `tau mcp refresh`.
pub async fn run(args: McpRefreshArgs, _output: &mut Output) -> Result<()> {
    let project_root = std::env::current_dir().context("get cwd")?;
    let (new_pin, prev_pin) = probe_and_pin(&args.name, None, &project_root).await?;

    let changed = match &prev_pin {
        Some(prev) => prev.contract_hash_hex != new_pin.contract_hash_hex,
        None => true, // no previous pin = first-time pin counts as "changed"
    };

    if args.json {
        let payload = serde_json::json!({
            "name": &args.name,
            "url": &new_pin.url,
            "changed": changed,
            "new_hash": &new_pin.contract_hash_hex,
            "prev_hash": prev_pin.as_ref().map(|p| p.contract_hash_hex.clone()),
            "tools_count": new_pin.contract.tools.len(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if changed {
        match &prev_pin {
            Some(prev) => println!(
                "refreshed `{}` (CHANGED): {} → {} (tools: {} → {})",
                args.name,
                &prev.contract_hash_hex[..16],
                &new_pin.contract_hash_hex[..16],
                prev.contract.tools.len(),
                new_pin.contract.tools.len(),
            ),
            None => println!(
                "refreshed `{}` (NEW): {} ({} tools)",
                args.name,
                &new_pin.contract_hash_hex[..16],
                new_pin.contract.tools.len(),
            ),
        }
    } else {
        println!(
            "refreshed `{}` (unchanged): {}",
            args.name,
            &new_pin.contract_hash_hex[..16]
        );
    }
    Ok(())
}
