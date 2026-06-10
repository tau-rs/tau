//! `tau mcp diff` — diff a pinned contract vs. live probe. Stub; implemented in Phase 3.

use crate::cli::McpDiffArgs;
use crate::output::Output;

/// Run `tau mcp diff`.
pub async fn run(_args: McpDiffArgs, _output: &mut Output) -> anyhow::Result<()> {
    anyhow::bail!("not yet implemented")
}
