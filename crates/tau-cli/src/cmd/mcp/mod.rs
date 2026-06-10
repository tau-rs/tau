//! `tau mcp <subcommand>` — manage MCP server contracts.
//!
//! See spec at `docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md`
//! §10 (CLI surface) and ADR-0038.

pub mod diff;
pub mod ls;
pub mod pin;
pub mod refresh;
pub mod show;

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
