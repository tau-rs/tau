//! `open(url, plan, gate, options)` — v0 entrypoint.
//!
//! Composes URL parse → spawn → framers → handshake → live `McpClient`.

use std::sync::Arc;

use tau_ports::CapabilityPlan;
use tau_runtime_tokio::process_gate::DynProcessCapabilityGate;
use tokio::process::Command;
use tracing::{info, instrument};

use crate::host_lifecycle::client::{McpClient, McpClientOptions};
use crate::host_lifecycle::error::{HandshakeError, LifecycleError};
use crate::host_lifecycle::handshake::drive_handshake;
use crate::host_lifecycle::url::{parse_url, McpUrl};
use crate::transport_stdio::{server::McpStdioServer, spawn};

/// Open a connection to an MCP server.
///
/// Returns a live `McpClient` once the MCP handshake has completed.
#[instrument(name = "mcp_open", skip(plan, gate, options), fields(url = url))]
pub async fn open(
    url: &str,
    plan: &CapabilityPlan,
    gate: Arc<dyn DynProcessCapabilityGate>,
    options: McpClientOptions,
) -> Result<McpClient, LifecycleError> {
    let parsed = parse_url(url)?;
    match parsed {
        McpUrl::Stdio { cmd } => {
            let mut command = Command::new(&cmd[0]);
            command.args(&cmd[1..]);

            info!(stdio_cmd = ?cmd, "spawning stdio MCP server");
            let child = spawn(command, gate, plan).await?;

            let transport = McpStdioServer::from_child(child).map_err(|e| {
                LifecycleError::Handshake(HandshakeError::Transport(format!("{e}")))
            })?;

            let contract = drive_handshake(&*transport, &options.handshake).await?;
            info!(
                server_name = %contract.server_info.name,
                tools_count = contract.tools.len(),
                "MCP handshake complete"
            );

            Ok(McpClient::new(transport, contract, options))
        }
    }
}
