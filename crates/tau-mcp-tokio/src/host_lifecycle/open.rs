//! `open(url, plan, gate, options)` — v0 entrypoint.
//!
//! Composes URL parse → spawn (stdio) or dial (HTTP) → handshake →
//! live `McpClient`.

use std::sync::Arc;

use tau_ports::CapabilityPlan;
use tau_runtime_tokio::process_gate::DynProcessCapabilityGate;
use tokio::process::Command;
use tracing::{info, instrument};

use crate::host_lifecycle::client::{McpClient, McpClientOptions};
use crate::host_lifecycle::error::{HandshakeError, LifecycleError};
use crate::host_lifecycle::handshake::drive_handshake;
use crate::host_lifecycle::url::{parse_url, McpUrl};
use crate::transport_http::dial::{dial as http_dial, HttpDialOptions};
use crate::transport_stdio::{server::McpStdioServer, spawn as stdio_spawn};

/// Open a connection to an MCP server.
#[instrument(name = "mcp_open", skip(plan, gate, options), fields(url = url))]
pub async fn open(
    url: &str,
    plan: &CapabilityPlan,
    gate: Arc<dyn DynProcessCapabilityGate>,
    options: McpClientOptions,
) -> Result<McpClient, LifecycleError> {
    let parsed = parse_url(url)?;
    match parsed {
        McpUrl::Stdio { cmd } => open_stdio(cmd, plan, gate, options).await,
        McpUrl::Http { url } => open_http(url, options).await,
        McpUrl::Https { url } => open_http(url, options).await,
    }
}

async fn open_stdio(
    cmd: Vec<String>,
    plan: &CapabilityPlan,
    gate: Arc<dyn DynProcessCapabilityGate>,
    options: McpClientOptions,
) -> Result<McpClient, LifecycleError> {
    let mut command = Command::new(&cmd[0]);
    command.args(&cmd[1..]);
    info!(stdio_cmd = ?cmd, "spawning stdio MCP server");
    let child = stdio_spawn(command, gate, plan).await?;
    let transport = McpStdioServer::from_child(child)
        .map_err(|e| LifecycleError::Handshake(HandshakeError::Transport(format!("{e}"))))?;
    let contract = drive_handshake(&*transport, &options.handshake).await?;
    info!(
        server_name = %contract.server_info.name,
        tools_count = contract.tools.len(),
        "MCP handshake complete (stdio)"
    );
    Ok(McpClient::new(transport, contract, options))
}

async fn open_http(url: url::Url, options: McpClientOptions) -> Result<McpClient, LifecycleError> {
    info!(http_url = %url, "dialing HTTP MCP server");
    let transport = http_dial(url, HttpDialOptions::default())?;
    let contract = drive_handshake(&*transport, &options.handshake).await?;
    info!(
        server_name = %contract.server_info.name,
        tools_count = contract.tools.len(),
        "MCP handshake complete (HTTP)"
    );
    Ok(McpClient::new(transport, contract, options))
}
