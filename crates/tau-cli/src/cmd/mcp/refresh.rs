//! `tau mcp refresh` — re-probe and overwrite pin. Stub; implemented in Phase 3.

use crate::cli::McpRefreshArgs;
use crate::output::Output;

/// Run `tau mcp refresh`.
pub async fn run(_args: McpRefreshArgs, _output: &mut Output) -> anyhow::Result<()> {
    anyhow::bail!("not yet implemented")
}
