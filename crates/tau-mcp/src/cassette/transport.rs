//! `CassetteTransport` — wraps a `Replayer` so cassettes can drive
//! `tau_mcp::transport::Transport` directly. Gated on `with-std-adapters`.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::ToString;
use alloc::sync::Arc;
use core::future::Future;
use core::pin::Pin;

use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::stream::StreamExt;
use std::sync::Mutex;

use crate::cassette::message::{CassetteMessage, Direction, MessageKind};
use crate::cassette::replayer::{ReplayError, Replayer};
use crate::error::McpError;
use crate::protocol::jsonrpc::{
    JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse,
};
use crate::transport::Transport;

/// Live cassette-as-transport.
pub struct CassetteTransport {
    replayer: Mutex<Replayer>,
    inbound_rx: futures::lock::Mutex<UnboundedReceiver<JsonRpcMessage>>,
    inbound_tx: UnboundedSender<JsonRpcMessage>,
}

impl CassetteTransport {
    /// Construct from raw JSONL bytes.
    pub fn from_jsonl_bytes(bytes: &[u8]) -> Result<Arc<Self>, ReplayError> {
        let replayer = Replayer::from_jsonl_bytes(bytes)?;
        let (tx, rx) = mpsc::unbounded();
        Ok(Arc::new(Self {
            replayer: Mutex::new(replayer),
            inbound_rx: futures::lock::Mutex::new(rx),
            inbound_tx: tx,
        }))
    }

    /// Push any queued outbounds (notifications + server-initiated
    /// requests) into the inbound channel for the host to consume.
    fn drain_pending(&self) -> Result<(), McpError> {
        let mut replayer = self
            .replayer
            .lock()
            .map_err(|_| McpError::Transport("replayer mutex poisoned".to_string()))?;
        while let Some(rec) = replayer.next_pending_outbound() {
            let msg = cassette_record_to_jsonrpc(&rec)?;
            if self.inbound_tx.unbounded_send(msg).is_err() {
                return Err(McpError::Transport("inbound channel closed".to_string()));
            }
        }
        Ok(())
    }
}

impl Transport for CassetteTransport {
    fn send_message<'a>(
        &'a self,
        msg: &'a JsonRpcMessage,
    ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
        Box::pin(async move {
            match msg {
                JsonRpcMessage::Request(req) => {
                    // Match the request in the cassette; queue its
                    // response + any preceding notifications/server-
                    // initiated-requests for the host to read next.
                    let response = {
                        let mut replayer = self.replayer.lock().map_err(|_| {
                            McpError::Transport("replayer mutex poisoned".to_string())
                        })?;
                        let method = req.method.clone();
                        let args = req.params.clone().unwrap_or(serde_json::Value::Null);
                        replayer
                            .match_request(&method, &args)
                            .map_err(|e| McpError::Transport(format!("cassette: {e}")))?
                    };
                    // Drain pending_outbound (notifications +
                    // server-initiated requests recorded BETWEEN this
                    // request and its response).
                    self.drain_pending()?;
                    // Push the matched response.
                    let resp_msg = cassette_record_to_jsonrpc(&response)?;
                    self.inbound_tx
                        .unbounded_send(resp_msg)
                        .map_err(|_| McpError::Transport("inbound channel closed".to_string()))?;
                    Ok(())
                }
                JsonRpcMessage::Response(_) | JsonRpcMessage::Notification(_) => {
                    // Host responding to a server-initiated request or
                    // emitting a notification. The cassette's
                    // pending_outbound queue was already drained when
                    // the prior request matched; nothing more to do.
                    Ok(())
                }
            }
        })
    }

    fn next_message<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<JsonRpcMessage>, McpError>> + Send + 'a>> {
        Box::pin(async move {
            let mut rx = self.inbound_rx.lock().await;
            Ok(rx.next().await)
        })
    }
}

/// Convert a `CassetteMessage` (Direction::Out only) into a `JsonRpcMessage`
/// for the host to consume.
fn cassette_record_to_jsonrpc(rec: &CassetteMessage) -> Result<JsonRpcMessage, McpError> {
    if rec.dir != Direction::Out {
        return Err(McpError::Transport(format!(
            "cassette record direction must be Out for replay; got {:?}",
            rec.dir
        )));
    }
    match rec.kind {
        MessageKind::Response => {
            // result/error live in rec.payload (already split or as
            // {result: ..., error: ...}? — cassette format §11 says
            // payload IS the result/error directly for responses).
            // Parse defensively: if payload has top-level "error", treat
            // as error; else as result.
            let id = rec
                .id
                .clone()
                .ok_or_else(|| McpError::Transport("cassette response without id".to_string()))?;
            // The cassette stores the result directly as `payload` per
            // spec §11. We construct a JsonRpcResponse.
            // Per §11 example: payload for response is the inner result.
            let resp = JsonRpcResponse {
                jsonrpc: crate::protocol::jsonrpc::JSONRPC_VERSION.to_string(),
                id,
                result: Some(rec.payload.clone()),
                error: None,
            };
            Ok(JsonRpcMessage::Response(resp))
        }
        MessageKind::Notification => {
            let method = rec.method.clone().ok_or_else(|| {
                McpError::Transport("cassette notification without method".to_string())
            })?;
            let n = JsonRpcNotification {
                jsonrpc: crate::protocol::jsonrpc::JSONRPC_VERSION.to_string(),
                method,
                params: Some(rec.payload.clone()),
            };
            Ok(JsonRpcMessage::Notification(n))
        }
        MessageKind::Request => {
            // Server-initiated request — same shape as a regular
            // JsonRpcRequest. Host responds to it via send_message.
            let id = rec.id.clone().ok_or_else(|| {
                McpError::Transport("cassette server-initiated request without id".to_string())
            })?;
            let method = rec.method.clone().ok_or_else(|| {
                McpError::Transport("cassette server-initiated request without method".to_string())
            })?;
            let req = JsonRpcRequest {
                jsonrpc: crate::protocol::jsonrpc::JSONRPC_VERSION.to_string(),
                id,
                method,
                params: Some(rec.payload.clone()),
            };
            Ok(JsonRpcMessage::Request(req))
        }
    }
}
