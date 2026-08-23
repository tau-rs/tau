//! Live MCP contract resolver — async, performs handshakes upfront and
//! populates a sync cache that tau-ir's lowering stage reads through.

use std::collections::BTreeMap;
use std::sync::Arc;

use tau_mcp::contract::canonical::canonical_hash;
use tau_mcp::contract::pinned::PinnedContract;
use tau_mcp::contract::resolver::{resolved_from_server_contract, ResolvedMcpContract};
use tau_mcp::McpError;
use tau_ports::CapabilityPlan;
use thiserror::Error;
use tracing::{info, instrument};

use crate::host_lifecycle::{open, McpClientOptions};

/// Errors from the live resolver.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LiveResolverError {
    /// `host_lifecycle::open` failed.
    #[error("entry {entry:?} url {url:?}: open failed: {reason}")]
    OpenFailed {
        /// `[tools.<entry>]` name.
        entry: String,
        /// MCP URL.
        url: String,
        /// `LifecycleError` rendered.
        reason: String,
    },
    /// Canonical hash or pinning failed.
    #[error("MCP contract error: {0}")]
    Hash(McpError),
}

/// Resolved contract + pin payload (caller writes pin to disk).
pub struct LiveResolved {
    /// Tau-ir-shaped resolved contract (cache key in tau-cli is the URL).
    pub resolved: ResolvedMcpContract,
    /// The pinned-contract payload (caller writes to `.tau/mcp/<entry>.contract.json`).
    pub pinned: PinnedContract,
}

/// One author-side `[tools.<entry>] mcp = "..."` to dial.
pub struct McpEntryInput {
    /// `[tools.<entry>]` name.
    pub entry: String,
    /// MCP URL.
    pub url: String,
    /// Capability plan from author's `capabilities` field.
    pub plan: CapabilityPlan,
}

/// Resolve all `entries` sequentially (one handshake per URL).
///
/// Returns a map keyed by URL so tau-ir's `Caches::mcp_contract` closure
/// can be `|url| map.get(url).cloned()`. Also returns per-entry pinned
/// contracts so tau-cli can write `.tau/mcp/<entry>.contract.json`.
#[instrument(skip(entries))]
pub async fn resolve_all(
    entries: Vec<McpEntryInput>,
) -> Result<BTreeMap<String, LiveResolved>, LiveResolverError> {
    let mut out = BTreeMap::new();
    for input in entries {
        info!(entry = %input.entry, url = %input.url, "live MCP resolve");
        let client = open(
            &input.url,
            &input.plan,
            passthrough_gate(),
            McpClientOptions::default(),
        )
        .await
        .map_err(|e| LiveResolverError::OpenFailed {
            entry: input.entry.clone(),
            url: input.url.clone(),
            reason: format!("{e}"),
        })?;
        let contract = client.contract();
        let hash = canonical_hash(contract).map_err(LiveResolverError::Hash)?;
        let pinned = PinnedContract::from_parts(input.url.clone(), contract.clone())
            .map_err(LiveResolverError::Hash)?;
        let resolved = resolved_from_server_contract(hash, contract);
        out.insert(input.url.clone(), LiveResolved { resolved, pinned });
    }
    Ok(out)
}

fn passthrough_gate() -> Arc<dyn tau_ports::DynProcessGate> {
    Arc::new(tau_ports::PassthroughGate::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_entries_returns_empty_map() {
        let result = resolve_all(Vec::new())
            .await
            .expect("empty resolve succeeds");
        assert!(result.is_empty());
    }
}
