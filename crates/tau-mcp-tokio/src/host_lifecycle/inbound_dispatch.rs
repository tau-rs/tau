//! Inbound-dispatch task for `McpClient`.
//!
//! PR-3 shipped `McpClient` with outbound-only `call_tool` semantics:
//! the inbound side of `transport.next_message()` was drained ad-hoc
//! by `call_tool`'s response loop. PR-5 adds the server-initiated
//! request side: a tokio task that loops on `transport.next_message`,
//! routes `sampling/createMessage` + `roots/list` requests through
//! `HostHandlers`, and writes responses back via `transport.send_message`.
//!
//! Cancellation propagation (PR-5.1) is NOT in this PR — the pump
//! exits cleanly when the transport closes (next_message returns None
//! or McpError::Transport), and the parent can `shutdown()` the handle
//! to abort the task.

use std::sync::Arc;

use tau_mcp::host::handlers::{HostHandlers, InboundError};
use tau_mcp::protocol::jsonrpc::{
    JsonRpcError, JsonRpcMessage, JsonRpcRequest, JsonRpcResponse, RequestId, JSONRPC_VERSION,
};
use tau_mcp::protocol::roots::RootsListResponse;
use tau_mcp::protocol::sampling::SamplingCreateMessageRequest;
use tau_mcp::transport::Transport;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

/// JSON-RPC custom error code for refused inbound requests.
///
/// Per MCP spec rev 2025-03-26: `-32000` to `-32099` is the custom
/// server-error range. We use `-32000` for HostHandlers refusals.
pub const INBOUND_REFUSED_ERROR_CODE: i32 = -32000;

/// Handle returned by `McpClient::start_inbound_dispatch`. Drop or
/// call `shutdown()` to abort the pump task.
#[must_use = "drop the handle or call shutdown() to abort the inbound pump"]
pub struct InboundDispatchHandle {
    task: JoinHandle<()>,
}

impl InboundDispatchHandle {
    /// Construct from a spawned task.
    pub(crate) fn new(task: JoinHandle<()>) -> Self {
        Self { task }
    }

    /// Abort the pump task. Idempotent.
    pub fn shutdown(self) {
        self.task.abort();
    }
}

/// Spawn the inbound-dispatch task for a given transport + handlers.
///
/// The task loops on `transport.next_message()`, routes server-initiated
/// requests through `handlers`, writes the response back via
/// `transport.send_message`. Exits cleanly on EOF or transport error
/// (logged at warn level).
pub fn spawn_inbound_dispatch(
    transport: Arc<dyn Transport>,
    handlers: Arc<dyn HostHandlers>,
) -> InboundDispatchHandle {
    let task = tokio::spawn(async move {
        loop {
            let msg = match transport.next_message().await {
                Ok(Some(m)) => m,
                Ok(None) => {
                    debug!("inbound-dispatch: transport closed cleanly");
                    return;
                }
                Err(e) => {
                    warn!(error = %e, "inbound-dispatch: transport error; exiting");
                    return;
                }
            };
            // Only server-initiated REQUESTS need routing here.
            // Responses to our outbound calls are consumed by call_tool's
            // own recv loop in PR-3. Notifications are TODO (β.3.1 wires
            // progress + log tracing).
            let JsonRpcMessage::Request(req) = msg else {
                continue;
            };
            if let Err(e) = route_request(&*transport, &*handlers, req).await {
                warn!(error = %e, "inbound-dispatch: route_request failed");
            }
        }
    });
    InboundDispatchHandle::new(task)
}

async fn route_request(
    transport: &dyn Transport,
    handlers: &dyn HostHandlers,
    req: JsonRpcRequest,
) -> Result<(), tau_mcp::McpError> {
    let id = req.id.clone();
    let response_payload: Result<serde_json::Value, tau_mcp::McpError> = match req.method.as_str() {
        "sampling/createMessage" => {
            let parsed: SamplingCreateMessageRequest =
                match serde_json::from_value(req.params.unwrap_or(serde_json::Value::Null)) {
                    Ok(p) => p,
                    Err(e) => {
                        return send_error(transport, id, format!("decode sampling: {e}")).await
                    }
                };
            match handlers.sampling(parsed).await {
                Ok(resp) => match serde_json::to_value(resp) {
                    Ok(v) => Ok(v),
                    Err(e) => {
                        return send_error(transport, id, format!("encode sampling: {e}")).await
                    }
                },
                Err(e) => return send_inbound_error(transport, id, e).await,
            }
        }
        "roots/list" => match handlers.roots().await {
            Ok(roots) => {
                let resp = RootsListResponse { roots };
                match serde_json::to_value(resp) {
                    Ok(v) => Ok(v),
                    Err(e) => return send_error(transport, id, format!("encode roots: {e}")).await,
                }
            }
            Err(e) => return send_inbound_error(transport, id, e).await,
        },
        other => {
            return send_error(
                transport,
                id,
                format!("unsupported server-initiated method: {other}"),
            )
            .await;
        }
    };
    let result = response_payload?;
    let msg = JsonRpcMessage::Response(JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id,
        result: Some(result),
        error: None,
    });
    transport.send_message(&msg).await
}

async fn send_inbound_error(
    transport: &dyn Transport,
    id: RequestId,
    e: InboundError,
) -> Result<(), tau_mcp::McpError> {
    send_error(transport, id, format!("{e}")).await
}

async fn send_error(
    transport: &dyn Transport,
    id: RequestId,
    message: String,
) -> Result<(), tau_mcp::McpError> {
    let msg = JsonRpcMessage::Response(JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id,
        result: None,
        error: Some(JsonRpcError {
            code: INBOUND_REFUSED_ERROR_CODE,
            message,
            data: None,
        }),
    });
    transport.send_message(&msg).await
}
