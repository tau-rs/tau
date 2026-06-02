//! `McpHttpServer` — Streamable HTTP MCP server handle.
//!
//! Implements `tau_mcp::transport::Transport` by translating each
//! outbound `send_message(JsonRpcMessage)` into an HTTP POST that
//! goes through `HttpClientGuard`, then demuxing the SSE response
//! stream into an inbound mpsc that `next_message` reads from.
//!
//! Design note on the SSE response pump: `Transport::send_message`
//! takes `&self`, not `&Arc<Self>`. We avoid threading an `Arc<Self>`
//! into the pump by capturing `inbound_tx.clone()` (cheap; mpsc senders
//! are `Clone`) and setting any `Mcp-Session-Id` response header on
//! `self.session` SYNCHRONOUSLY before spawning the streaming task.
//! The task itself only needs the sender.

use std::pin::Pin;
use std::sync::Arc;

use futures::stream::StreamExt;
use tau_mcp::protocol::JsonRpcMessage;
use tau_mcp::transport::Transport;
use tau_mcp::McpError;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;
use tracing::debug;
use url::Url;

use crate::transport_http::error::HttpTransportError;
use crate::transport_http::guard::HttpClientGuard;
use crate::transport_http::session::{SessionState, MCP_SESSION_ID_HEADER};
use crate::transport_http::sse::SseFramer;

/// Live Streamable HTTP MCP server.
pub struct McpHttpServer {
    guard: HttpClientGuard,
    session: SessionState,
    url: Url,
    inbound_rx: Mutex<UnboundedReceiver<Result<JsonRpcMessage, HttpTransportError>>>,
    inbound_tx: UnboundedSender<Result<JsonRpcMessage, HttpTransportError>>,
}

impl McpHttpServer {
    /// Construct from a guard + URL. Caller (`dial`) is responsible
    /// for having validated `url`'s host matches `guard.pinned_host()`.
    pub fn new(guard: HttpClientGuard, url: Url) -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            guard,
            session: SessionState::new(),
            url,
            inbound_rx: Mutex::new(rx),
            inbound_tx: tx,
        })
    }

    /// Server URL (for diagnostics).
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Borrow the session state (mainly for tests).
    pub fn session(&self) -> &SessionState {
        &self.session
    }

    /// Spawn an async pump that streams `response.bytes_stream()`
    /// through `SseFramer` and pushes decoded messages to `inbound_tx`.
    /// Captures the `Mcp-Session-Id` response header SYNCHRONOUSLY
    /// before spawning so the task only needs the sender.
    fn start_pump(&self, response: reqwest::Response) {
        if let Some(value) = response.headers().get(MCP_SESSION_ID_HEADER) {
            if let Ok(s) = value.to_str() {
                self.session.set(s.to_string());
            }
        }
        let inbound_tx = self.inbound_tx.clone();
        tokio::spawn(async move {
            let mut framer = SseFramer::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = inbound_tx.send(Err(HttpTransportError::Send(format!("{e}"))));
                        return;
                    }
                };
                match framer.feed_bytes(&chunk) {
                    Ok(messages) => {
                        for m in messages {
                            if inbound_tx.send(Ok(m)).is_err() {
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = inbound_tx.send(Err(e));
                        return;
                    }
                }
            }
            match framer.flush() {
                Ok(Some(m)) => {
                    let _ = inbound_tx.send(Ok(m));
                }
                Ok(None) => {}
                Err(e) => {
                    let _ = inbound_tx.send(Err(e));
                }
            }
            debug!("HTTP SSE stream ended cleanly");
        });
    }
}

impl Transport for McpHttpServer {
    fn send_message<'a>(
        &'a self,
        msg: &'a JsonRpcMessage,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), McpError>> + Send + 'a>> {
        Box::pin(async move {
            let body = serde_json::to_vec(msg)
                .map_err(|e| McpError::Serde(format!("encode JSON-RPC: {e}")))?;
            let mut builder = self
                .guard
                .post(self.url.clone())
                .header("Content-Type", "application/json")
                .header("Accept", "text/event-stream, application/json")
                .body(body);
            if let Some(sid) = self.session.get() {
                builder = builder.header(MCP_SESSION_ID_HEADER, sid);
            }
            let request = builder
                .build()
                .map_err(|e| McpError::Transport(format!("build HTTP request: {e}")))?;
            let response = self
                .guard
                .send(request)
                .await
                .map_err(convert_transport_error)?;
            if !response.status().is_success() {
                let status = response.status().as_u16();
                let body = response
                    .text()
                    .await
                    .unwrap_or_else(|e| format!("<body read failed: {e}>"));
                return Err(convert_transport_error(HttpTransportError::Status {
                    status,
                    body,
                }));
            }
            self.start_pump(response);
            Ok(())
        })
    }

    fn next_message<'a>(
        &'a self,
    ) -> Pin<
        Box<dyn std::future::Future<Output = Result<Option<JsonRpcMessage>, McpError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let mut rx = self.inbound_rx.lock().await;
            match rx.recv().await {
                Some(Ok(msg)) => Ok(Some(msg)),
                Some(Err(e)) => Err(convert_transport_error(e)),
                None => Ok(None), // Channel closed — EOF.
            }
        })
    }
}

fn convert_transport_error(e: HttpTransportError) -> McpError {
    match e {
        HttpTransportError::JsonDecode(s) => McpError::Serde(s),
        HttpTransportError::Send(s)
        | HttpTransportError::SseParse(s)
        | HttpTransportError::Channel(s) => McpError::Transport(s),
        HttpTransportError::Status { status, body } => {
            McpError::Transport(format!("HTTP {status}: {body}"))
        }
        HttpTransportError::HostPinViolation { actual, pinned } => McpError::Transport(format!(
            "host-pin violation: actual={actual} pinned={pinned}"
        )),
    }
}
