//! `tau mcp show` — show a pinned contract. Stub; implemented in Phase 3.

use crate::cli::McpShowArgs;
use crate::output::Output;

/// Run `tau mcp show`.
pub async fn run(_args: McpShowArgs, _output: &mut Output) -> anyhow::Result<()> {
    anyhow::bail!("not yet implemented")
}
