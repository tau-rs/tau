//! `tau mcp <subcommand>` — manage MCP server contracts.
//!
//! See spec at `docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md`
//! §10 (CLI surface) and ADR-0038.

pub mod diff;
pub mod ls;
pub mod pin;
pub mod refresh;
pub mod show;

use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use tau_mcp::contract::pinned::PinnedContract;
use tau_mcp_tokio::host_lifecycle::client::McpClientOptions;
use tau_mcp_tokio::host_lifecycle::open::open;
use tau_pkg::project::project::{ProjectConfig, ToolBody};
use tau_ports::CapabilityPlan;
use tau_runtime_tokio::process_gate::passthrough::PassthroughSandbox;

use crate::cli::McpSubcommand;
use crate::output::Output;

/// Output format selector. Used by `show`; `pin`/`refresh` have their
/// own bool flags that funnel through `from_flags` indirectly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// Human-readable terminal output.
    Human,
    /// Canonical JSON.
    Json,
    /// SARIF 2.1.0 document.
    Sarif,
}

impl OutputFormat {
    /// Build from `--json` / `--sarif` flag pair (mutually exclusive at clap layer).
    pub fn from_flags(json: bool, sarif: bool) -> Self {
        match (json, sarif) {
            (true, false) => Self::Json,
            (false, true) => Self::Sarif,
            _ => Self::Human,
        }
    }
}

/// Render an arbitrary serializable payload as a SARIF 2.1.0 document.
/// Single tool ("tau-mcp"), single rule (the verb name), zero results.
pub fn render_sarif(rule_id: &str, embedded_payload: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "tau-mcp",
                    "informationUri": "https://github.com/LEBOCQTitouan/tau",
                    "rules": [{ "id": rule_id }],
                }
            },
            "results": [],
            "properties": { "embedded": embedded_payload },
        }],
    })
}

/// Probe a live MCP server and return a freshly-built `PinnedContract`
/// **without** writing anything to disk.
///
/// Shared by `pin` (which then writes) and `diff` (which stays read-only).
pub(super) async fn probe_only(
    name: &str,
    override_url: Option<String>,
    project_root: &Path,
) -> Result<PinnedContract> {
    let url = resolve_url(name, override_url, project_root)?;

    let gate: Arc<dyn tau_runtime_tokio::process_gate::DynProcessCapabilityGate> =
        Arc::new(PassthroughSandbox::new());
    let plan = CapabilityPlan::new(vec![], None, None);
    let client = open(&url, &plan, gate, McpClientOptions::default())
        .await
        .with_context(|| format!("open MCP server at {url}"))?;
    let contract = client.contract().clone();
    PinnedContract::from_parts(url, contract).map_err(|e| anyhow!("build pinned contract: {e}"))
}

/// Probe a live MCP server, write the new pin to disk, and return
/// `(new_pin, previous_pin)`.  The previous pin is `None` if this is
/// the first time we are pinning `name`.
///
/// Shared by `pin` and `refresh`.
pub(super) async fn probe_and_pin(
    name: &str,
    override_url: Option<String>,
    project_root: &Path,
) -> Result<(PinnedContract, Option<PinnedContract>)> {
    // Load the old pin (if any) before we overwrite it.
    let pin_dir = project_root.join(".tau/mcp");
    let pin_path = pin_dir.join(format!("{name}.contract.json"));
    let prev_pin: Option<PinnedContract> = if pin_path.exists() {
        let bytes = std::fs::read(&pin_path)
            .with_context(|| format!("read existing pin {}", pin_path.display()))?;
        Some(
            serde_json::from_slice(&bytes)
                .with_context(|| format!("parse existing pin {}", pin_path.display()))?,
        )
    } else {
        None
    };

    let new_pin = probe_only(name, override_url, project_root).await?;

    std::fs::create_dir_all(&pin_dir)
        .with_context(|| format!("create {}", pin_dir.display()))?;
    let bytes = serde_json::to_vec_pretty(&new_pin).context("serialize pinned contract")?;
    std::fs::write(&pin_path, &bytes)
        .with_context(|| format!("write {}", pin_path.display()))?;

    Ok((new_pin, prev_pin))
}

/// Resolve the MCP URL for `name`: override wins, else `[tools.<name>] mcp`.
fn resolve_url(name: &str, override_url: Option<String>, project_root: &Path) -> Result<String> {
    if let Some(url) = override_url {
        return Ok(url);
    }
    let toml_path = project_root.join("tau.toml");
    let toml_str = std::fs::read_to_string(&toml_path)
        .with_context(|| format!("read {}", toml_path.display()))?;
    let config = ProjectConfig::parse_str(&toml_str)
        .with_context(|| format!("parse {}", toml_path.display()))?;
    let tool = config
        .tools
        .get(name)
        .ok_or_else(|| anyhow!("tool {name:?} not found in tau.toml [tools] section"))?;
    match &tool.body {
        ToolBody::Mcp(url) => Ok(url.clone()),
        ToolBody::Native(_) => anyhow::bail!("tool {name:?} is a native tool, not an MCP tool"),
        ToolBody::Subflow(_) => anyhow::bail!("tool {name:?} is a subflow, not an MCP tool"),
        _ => anyhow::bail!("tool {name:?} has an unrecognized body kind (not an MCP tool)"),
    }
}

/// Route `tau mcp <subcommand>` to its impl.
pub async fn dispatch(sub: McpSubcommand, output: &mut Output) -> anyhow::Result<()> {
    match sub {
        McpSubcommand::Pin(args) => pin::run(args, output).await,
        McpSubcommand::Ls(args) => ls::run(args, output).await,
        McpSubcommand::Show(args) => show::run(args, output).await,
        McpSubcommand::Refresh(args) => refresh::run(args, output).await,
        McpSubcommand::Diff(args) => diff::run(args, output).await,
    }
}
