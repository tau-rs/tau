//! Request dispatcher: route inbound messages to method handlers,
//! enforce handshake + concurrency state.
//!
//! The dispatcher is single-task (one tokio task running this loop).
//! Per-request work is spawned into a `LocalSet` so that
//! non-`Send` Runtime streams can be polled across await points.

use super::cancel::CancelRegistry;
use super::error_codes;
use super::framing::Inbound;
use super::handshake::{Check, HandshakeState};
use super::methods;
use super::project::Project;
use super::protocol::{
    ErrorObject, ErrorResponse, Notification, Outbound, Request, RequestId, Response,
};
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// Shared dispatcher state. Cheap to clone (all `Arc`/clone-safe inner).
#[derive(Clone)]
pub struct Dispatcher {
    /// Loaded project. Shared, read-only.
    pub project: Arc<Project>,
    /// The tau runtime. Shared, used by run executors.
    pub runtime: Arc<tau_runtime_tokio::Runtime>,
    /// Atomic handshake state.
    pub handshake: HandshakeState,
    /// Cancel-token registry indexed by RequestId.
    pub cancel_reg: CancelRegistry,
    /// Max concurrent in-flight runtime.* calls.
    pub max_concurrent: usize,
    /// Channel sender to the writer task.
    pub out_tx: mpsc::Sender<Outbound>,
    /// Tripped (once, with a log) when an outbound send fails because the
    /// writer task is gone. The dispatch loop selects on it to begin shutdown.
    pub writer_gone: CancellationToken,
}

impl Dispatcher {
    /// Main dispatch loop. Runs until `in_rx` closes (EOF / shutdown).
    pub async fn run(self, mut in_rx: mpsc::Receiver<Inbound>) -> Result<()> {
        while let Some(frame) = in_rx.recv().await {
            match frame {
                Inbound::Eof => break,
                Inbound::ParseError(msg) => {
                    warn!(error = %msg, "parse error");
                    // Per JSON-RPC 2.0, parse errors carry a null id (the
                    // originating id can't be recovered from malformed input).
                    self.send_err(
                        RequestId::Null,
                        error_codes::PARSE_ERROR,
                        "Parse error".into(),
                        None,
                    )
                    .await;
                }
                Inbound::Json(value) => self.handle_one(value).await,
            }
        }
        Ok(())
    }

    async fn handle_one(&self, value: Value) {
        // Parse as Request.
        let req: Request = match serde_json::from_value(value) {
            Ok(r) => r,
            Err(e) => {
                // Invalid JSON-RPC object: the id is unknown, so per spec the
                // error response carries a null id.
                self.send_err(
                    RequestId::Null,
                    error_codes::INVALID_REQUEST,
                    format!("invalid request: {}", e),
                    None,
                )
                .await;
                return;
            }
        };

        // Handshake state check.
        let check = self.handshake.check(&req.method);
        match check {
            Check::HandshakeRequired => {
                self.send_err(
                    req.id,
                    error_codes::HANDSHAKE_REQUIRED,
                    "Handshake required".into(),
                    None,
                )
                .await;
                return;
            }
            Check::AlreadyHandshaken => {
                self.send_err(
                    req.id,
                    error_codes::ALREADY_HANDSHAKEN,
                    "Already handshaken".into(),
                    None,
                )
                .await;
                return;
            }
            Check::Allowed => {}
        }

        // Concurrency cap (only for runtime.run / runtime.run_streaming).
        let is_runtime_run =
            req.method == methods::RUNTIME_RUN || req.method == methods::RUNTIME_RUN_STREAMING;
        if is_runtime_run && self.cancel_reg.len() >= self.max_concurrent {
            self.send_err(
                req.id,
                error_codes::SERVER_BUSY,
                format!(
                    "Server busy: max_concurrent_runs={} reached",
                    self.max_concurrent
                ),
                Some(json!({"max_concurrent": self.max_concurrent})),
            )
            .await;
            return;
        }

        // Route.
        match req.method.as_str() {
            methods::META_HANDSHAKE => self.handle_handshake(req).await,
            methods::META_PING => self.handle_ping(req).await,
            methods::RUNTIME_RUN => self.spawn_run(req, /*streaming=*/ false),
            methods::RUNTIME_RUN_STREAMING => self.spawn_run(req, /*streaming=*/ true),
            methods::RUNTIME_CANCEL => self.handle_cancel(req).await,
            other => {
                self.send_err(
                    req.id,
                    error_codes::METHOD_NOT_FOUND,
                    format!("Method not found: {}", other),
                    None,
                )
                .await;
            }
        }
    }

    async fn handle_handshake(&self, req: Request) {
        let params = req.params.unwrap_or_else(|| json!({}));
        let client_proto = params
            .get("protocol_version")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if client_proto != 1 {
            self.send_err(
                req.id,
                error_codes::HANDSHAKE_MISMATCH,
                format!("protocol_version {} not supported", client_proto),
                Some(json!({"supported_versions": [1]})),
            )
            .await;
            return;
        }
        self.handshake.mark_handshaken();
        let result = json!({
            "server_name": "tau",
            "server_version": env!("CARGO_PKG_VERSION"),
            "protocol_version": 1,
            "project_path": self.project.root.display().to_string(),
            "agents": self.project.agent_ids(),
        });
        self.send_ok(req.id, result).await;
    }

    async fn handle_ping(&self, req: Request) {
        self.send_ok(req.id, json!({"ok": true})).await;
    }

    async fn handle_cancel(&self, req: Request) {
        let params = req.params.unwrap_or(json!({}));
        let target: RequestId = match params.get("id") {
            Some(v) => match serde_json::from_value(v.clone()) {
                Ok(id) => id,
                Err(_) => {
                    self.send_err(
                        req.id,
                        error_codes::INVALID_PARAMS,
                        "params.id must be int or string".into(),
                        None,
                    )
                    .await;
                    return;
                }
            },
            None => {
                self.send_err(
                    req.id,
                    error_codes::INVALID_PARAMS,
                    "params.id missing".into(),
                    None,
                )
                .await;
                return;
            }
        };
        let cancelled = self.cancel_reg.cancel(&target);
        self.send_ok(req.id, json!({"cancelled": cancelled})).await;
    }

    /// Spawn the per-request task. Runtime streams are non-`Send` so we must
    /// use `spawn_local`. Works within any active LocalSet on the current
    /// thread — `lifecycle::run` drives `Dispatcher::run` inside a LocalSet.
    fn spawn_run(&self, req: Request, streaming: bool) {
        let this = self.clone();
        tokio::task::spawn_local(async move {
            super::dispatch_run::execute(this, req, streaming).await;
        });
    }

    /// Record that the writer task is gone: log and trip the shutdown token.
    /// Idempotent — calls after the first are silent. The dispatcher and its
    /// per-request `spawn_local` tasks share one current-thread executor and
    /// this fn has no `.await`, so the `is_cancelled`→`cancel` window can't be
    /// interleaved: a dead writer yields exactly one warning, not one per
    /// dropped message.
    fn note_writer_gone(&self) {
        if !self.writer_gone.is_cancelled() {
            warn!("writer task gone; outbound message dropped, beginning shutdown");
            self.writer_gone.cancel();
        }
    }

    /// Send a successful JSON-RPC 2.0 response to the writer task.
    pub async fn send_ok(&self, id: RequestId, result: Value) {
        if self
            .out_tx
            .send(Outbound::Response(Response {
                jsonrpc: "2.0".into(),
                id,
                result,
            }))
            .await
            .is_err()
        {
            self.note_writer_gone();
        }
    }

    /// Send an error JSON-RPC 2.0 response to the writer task.
    pub async fn send_err(&self, id: RequestId, code: i32, message: String, data: Option<Value>) {
        if self
            .out_tx
            .send(Outbound::Error(ErrorResponse {
                jsonrpc: "2.0".into(),
                id,
                error: ErrorObject {
                    code,
                    message,
                    data,
                },
            }))
            .await
            .is_err()
        {
            self.note_writer_gone();
        }
    }

    /// Send a server-initiated JSON-RPC 2.0 notification to the writer task.
    pub async fn send_notification(&self, method: &str, params: Value) {
        if self
            .out_tx
            .send(Outbound::Notification(Notification {
                jsonrpc: "2.0".into(),
                method: method.into(),
                params: Some(params),
            }))
            .await
            .is_err()
        {
            self.note_writer_gone();
        }
    }
}
