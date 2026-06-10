//! `tau mcp ls` — list pinned MCP contracts in the current project.
//!
//! See spec at `docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md`
//! §10 (CLI surface) and ADR-0038.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Serialize;
use tau_mcp::contract::pinned::PinnedContract;

use crate::cli::McpLsArgs;
use crate::output::Output;

#[derive(Serialize)]
struct PinSummary {
    name: String,
    url: String,
    server_name: String,
    tools_count: usize,
    contract_hash_hex: String,
    path: PathBuf,
}

/// Run `tau mcp ls`.
pub async fn run(args: McpLsArgs, _output: &mut Output) -> Result<()> {
    let project_root = std::env::current_dir().context("get cwd")?;
    let pin_dir = project_root.join(".tau/mcp");
    let mut pins = Vec::new();
    if pin_dir.is_dir() {
        for entry in std::fs::read_dir(&pin_dir)
            .with_context(|| format!("read {}", pin_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let Some(name) = path
                .file_name()
                .and_then(|s| s.to_str())
                .and_then(|s| s.strip_suffix(".contract.json"))
                .map(String::from)
            else {
                continue;
            };
            let bytes = std::fs::read(&path)
                .with_context(|| format!("read {}", path.display()))?;
            let pinned: PinnedContract = serde_json::from_slice(&bytes)
                .with_context(|| format!("parse {}", path.display()))?;
            pins.push(PinSummary {
                name,
                url: pinned.url.clone(),
                server_name: pinned.contract.server_info.name.clone(),
                tools_count: pinned.contract.tools.len(),
                contract_hash_hex: pinned.contract_hash_hex.clone(),
                path,
            });
        }
        pins.sort_by(|a, b| a.name.cmp(&b.name));
    }

    if args.json {
        let payload = serde_json::json!({ "pins": pins });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if pins.is_empty() {
        println!("no pinned MCP contracts (run `tau mcp pin <name>`)");
    } else {
        for p in &pins {
            println!(
                "{:24} {:8} {} → {} ({} tools, hash {})",
                p.name,
                "MCP",
                p.url,
                p.server_name,
                p.tools_count,
                &p.contract_hash_hex[..16],
            );
        }
    }
    Ok(())
}
