//! MCP handshake driver.
//!
//! Sends `initialize` + `tools/list` over a Transport, captures the
//! responses, and builds a `ServerContract` from them.

use std::time::Duration;

use tau_mcp::contract::ServerContract;
use tau_mcp::protocol::{
    initialize::{ClientInfo, InitializeRequest, InitializeResponse},
    jsonrpc::{JsonRpcMessage, JsonRpcRequest, JsonRpcResponse, RequestId, JSONRPC_VERSION},
    tools::{ToolsListRequest, ToolsListResponse},
};
use tau_mcp::transport::Transport;
use tokio::time::timeout;
use tracing::{debug, instrument};

use crate::host_lifecycle::error::HandshakeError;

/// MCP protocol version tau speaks.
pub const MCP_PROTOCOL_VERSION: &str = "2025-03-26";

/// Handshake options (timeout, client info).
#[derive(Debug, Clone)]
pub struct HandshakeOptions {
    /// Timeout for the entire handshake (initialize + tools/list).
    pub handshake_timeout: Duration,
    /// Client name reported to the server.
    pub client_name: String,
    /// Client version reported to the server.
    pub client_version: String,
}

impl Default for HandshakeOptions {
    fn default() -> Self {
        Self {
            handshake_timeout: Duration::from_secs(30),
            client_name: "tau".to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// Drive the MCP handshake. Returns a `ServerContract` capturing the
/// server's reported info + tools/list snapshot.
#[instrument(name = "mcp_handshake", skip(transport, options), fields(
    client_name = %options.client_name,
    handshake_timeout_ms = options.handshake_timeout.as_millis() as u64,
))]
pub async fn drive_handshake(
    transport: &dyn Transport,
    options: &HandshakeOptions,
) -> Result<ServerContract, HandshakeError> {
    let inner = async {
        // 1. initialize
        let init_req = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::Number(0),
            method: "initialize".to_string(),
            params: Some(
                serde_json::to_value(InitializeRequest {
                    protocol_version: MCP_PROTOCOL_VERSION.to_string(),
                    client_info: ClientInfo {
                        name: options.client_name.clone(),
                        version: options.client_version.clone(),
                        additional: Default::default(),
                    },
                    capabilities: None,
                })
                .map_err(|e| HandshakeError::Malformed(format!("encode initialize: {e}")))?,
            ),
        });
        send(transport, &init_req).await?;
        let init_resp = recv_response_for(transport, &RequestId::Number(0)).await?;
        let init_result: InitializeResponse = serde_json::from_value(init_resp)
            .map_err(|e| HandshakeError::Malformed(format!("decode initialize response: {e}")))?;
        debug!(
            server_name = %init_result.server_info.name,
            server_version = %init_result.server_info.version,
            "initialize response decoded"
        );

        // 2. tools/list
        let list_req = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::Number(1),
            method: "tools/list".to_string(),
            params: Some(serde_json::to_value(ToolsListRequest::default()).unwrap_or_default()),
        });
        send(transport, &list_req).await?;
        let list_resp = recv_response_for(transport, &RequestId::Number(1)).await?;
        let list_result: ToolsListResponse = serde_json::from_value(list_resp)
            .map_err(|e| HandshakeError::Malformed(format!("decode tools/list response: {e}")))?;
        debug!(
            tools_count = list_result.tools.len(),
            "tools/list response decoded"
        );

        let contract = ServerContract::from_handshake(init_result, list_result, |_| Vec::new());
        Ok::<_, HandshakeError>(contract)
    };

    timeout(options.handshake_timeout, inner)
        .await
        .map_err(|_| HandshakeError::Timeout {
            millis: options.handshake_timeout.as_millis() as u64,
        })?
}

async fn send(transport: &dyn Transport, msg: &JsonRpcMessage) -> Result<(), HandshakeError> {
    transport
        .send_message(msg)
        .await
        .map_err(|e| HandshakeError::Transport(format!("{e}")))
}

/// Receive messages until we see a response matching `expected_id`.
/// Ignores notifications and out-of-order responses (logs but skips).
async fn recv_response_for(
    transport: &dyn Transport,
    expected_id: &RequestId,
) -> Result<serde_json::Value, HandshakeError> {
    loop {
        let msg = transport
            .next_message()
            .await
            .map_err(|e| HandshakeError::Transport(format!("{e}")))?
            .ok_or_else(|| HandshakeError::Transport("peer closed mid-handshake".into()))?;
        match msg {
            JsonRpcMessage::Response(JsonRpcResponse {
                id, result, error, ..
            }) if &id == expected_id => {
                if let Some(e) = error {
                    return Err(HandshakeError::ServerError {
                        code: e.code,
                        message: e.message,
                    });
                }
                return Ok(result.unwrap_or(serde_json::Value::Null));
            }
            other => {
                debug!(
                    received_kind = ?std::mem::discriminant(&other),
                    expected_id = ?expected_id,
                    "skipping unexpected handshake message"
                );
            }
        }
    }
}
