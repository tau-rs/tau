//! `McpClient` — live MCP server handle returned by `open`.
//!
//! Carries the `Arc<dyn Transport>` (a `McpStdioServer` in PR-2, a
//! Streamable HTTP transport in PR-3, possibly cassette-replay in
//! tests) and the captured `ServerContract` from the handshake.

use std::sync::Arc;
use std::time::Duration;

use tau_mcp::contract::ServerContract;
use tau_mcp::protocol::{
    jsonrpc::{JsonRpcMessage, JsonRpcRequest, JsonRpcResponse, RequestId, JSONRPC_VERSION},
    tools::{ToolsCallRequest, ToolsCallResponse},
};
use tau_mcp::transport::Transport;
use tau_mcp::McpError;
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::host_lifecycle::handshake::HandshakeOptions;

/// Options for an `McpClient` (handshake settings + tool-call defaults).
#[derive(Debug, Clone)]
pub struct McpClientOptions {
    /// Handshake settings (timeout, client info).
    pub handshake: HandshakeOptions,
    /// Default per-tool-call timeout.
    pub call_timeout: Duration,
}

impl Default for McpClientOptions {
    fn default() -> Self {
        Self {
            handshake: HandshakeOptions::default(),
            call_timeout: Duration::from_secs(60),
        }
    }
}

/// Live MCP server handle.
pub struct McpClient {
    transport: Arc<dyn Transport>,
    contract: ServerContract,
    options: McpClientOptions,
    next_id: Mutex<i64>,
}

impl McpClient {
    /// Construct from a live transport + already-completed handshake.
    pub fn new(
        transport: Arc<dyn Transport>,
        contract: ServerContract,
        options: McpClientOptions,
    ) -> Self {
        Self {
            transport,
            contract,
            options,
            next_id: Mutex::new(2),
        }
    }

    /// The captured server contract (initialize + tools/list snapshot).
    pub fn contract(&self) -> &ServerContract {
        &self.contract
    }

    /// Call a server-side tool by name with the given JSON args.
    pub async fn call_tool(
        &self,
        server_tool_name: &str,
        args: serde_json::Value,
    ) -> Result<ToolsCallResponse, McpError> {
        let id = {
            let mut next = self.next_id.lock().await;
            let id = *next;
            *next += 1;
            id
        };
        let req = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::Number(id),
            method: "tools/call".to_string(),
            params: Some(serde_json::to_value(ToolsCallRequest {
                name: server_tool_name.to_string(),
                arguments: Some(args),
            })?),
        });
        let call_timeout = self.options.call_timeout;
        let inner = async {
            self.transport.send_message(&req).await?;
            let resp = recv_response_for(&*self.transport, &RequestId::Number(id)).await?;
            let result: ToolsCallResponse = serde_json::from_value(resp)?;
            Ok::<_, McpError>(result)
        };
        timeout(call_timeout, inner)
            .await
            .map_err(|_| {
                McpError::Transport(format!(
                    "tools/call {server_tool_name:?} timed out after {}ms",
                    call_timeout.as_millis()
                ))
            })?
    }

    /// Borrow the live transport (PR-5 wires the inbound-dispatch task here).
    pub fn transport(&self) -> &Arc<dyn Transport> {
        &self.transport
    }
}

async fn recv_response_for(
    transport: &dyn Transport,
    expected_id: &RequestId,
) -> Result<serde_json::Value, McpError> {
    loop {
        let msg = transport
            .next_message()
            .await?
            .ok_or_else(|| McpError::Transport("peer closed mid-call".into()))?;
        match msg {
            JsonRpcMessage::Response(JsonRpcResponse { id, result, error, .. })
                if &id == expected_id =>
            {
                if let Some(e) = error {
                    return Err(McpError::Protocol(format!(
                        "server returned error code={} msg={}",
                        e.code, e.message
                    )));
                }
                return Ok(result.unwrap_or(serde_json::Value::Null));
            }
            _ => continue,
        }
    }
}
