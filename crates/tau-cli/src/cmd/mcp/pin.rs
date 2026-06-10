//! `tau mcp pin <name> [--from URL]` — probe a server and write its
//! contract to `.tau/mcp/<name>.contract.json`.
//!
//! See spec at `docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md`
//! §10 (CLI surface) and ADR-0038.

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use tau_mcp::contract::pinned::PinnedContract;
use tau_mcp_tokio::host_lifecycle::client::McpClientOptions;
use tau_mcp_tokio::host_lifecycle::open::open;
use tau_pkg::project::project::{ProjectConfig, ToolBody};
use tau_ports::CapabilityPlan;
use tau_runtime_tokio::process_gate::passthrough::PassthroughSandbox;

use crate::cli::McpPinArgs;
use crate::output::Output;

/// Run `tau mcp pin`.
pub async fn run(args: McpPinArgs, _output: &mut Output) -> Result<()> {
    let project_root = std::env::current_dir().context("get cwd")?;

    // Resolve the URL: --from override wins, else the tau.toml tool block.
    let url = if let Some(override_url) = args.from {
        override_url
    } else {
        let toml_path = project_root.join("tau.toml");
        let toml_str = std::fs::read_to_string(&toml_path)
            .with_context(|| format!("read {}", toml_path.display()))?;
        let config = ProjectConfig::parse_str(&toml_str)
            .with_context(|| format!("parse {}", toml_path.display()))?;
        let tool = config
            .tools
            .get(&args.name)
            .ok_or_else(|| anyhow!("tool {:?} not found in tau.toml [tools] section", args.name))?;
        // Extract the MCP URL from the tool body.
        match &tool.body {
            ToolBody::Mcp(url) => url.clone(),
            ToolBody::Native(_) => {
                anyhow::bail!(
                    "tool {:?} is a native tool, not an MCP tool",
                    args.name
                )
            }
            ToolBody::Subflow(_) => {
                anyhow::bail!(
                    "tool {:?} is a subflow, not an MCP tool",
                    args.name
                )
            }
            _ => anyhow::bail!(
                "tool {:?} has an unrecognized body kind (not an MCP tool)",
                args.name
            ),
        }
    };

    // Build a passthrough gate — cassette replay never invokes the gate;
    // stdio/http callers can pass --no-sandbox explicitly if needed.
    let gate: Arc<dyn tau_runtime_tokio::process_gate::DynProcessCapabilityGate> =
        Arc::new(PassthroughSandbox::new());

    let plan = CapabilityPlan::new(vec![], None, None);
    let client = open(&url, &plan, gate, McpClientOptions::default())
        .await
        .with_context(|| format!("open MCP server at {url}"))?;
    let contract = client.contract().clone();
    let pinned = PinnedContract::from_parts(url.clone(), contract)
        .map_err(|e| anyhow!("build pinned contract: {e}"))?;

    let pin_dir = project_root.join(".tau/mcp");
    std::fs::create_dir_all(&pin_dir)
        .with_context(|| format!("create {}", pin_dir.display()))?;
    let pin_path = pin_dir.join(format!("{}.contract.json", &args.name));
    let bytes = serde_json::to_vec_pretty(&pinned).context("serialize pinned contract")?;
    std::fs::write(&pin_path, &bytes)
        .with_context(|| format!("write {}", pin_path.display()))?;

    if args.json {
        let payload = serde_json::json!({
            "ok": true,
            "name": args.name,
            "path": pin_path,
            "url": url,
            "contract_hash_hex": pinned.contract_hash_hex,
            "tools_count": pinned.contract.tools.len(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!(
            "pinned `{}` from {} → {} ({} tools, hash {})",
            args.name,
            url,
            pin_path.display(),
            pinned.contract.tools.len(),
            &pinned.contract_hash_hex[..16],
        );
    }
    Ok(())
}
