//! Host-side [`crate::builder::DynLlmBackend`] implementation backed by
//! a spawned plugin subprocess.
//!
//! Thin RPC dispatcher: each `complete()` call allocates a msgid,
//! registers a `oneshot::Sender` in the shared `in_flight_responses`
//! map, sends a single `Frame::Request` over the writer, and awaits the
//! response.
//!
//! See spec §7.4 (per-port adapters) and §7.3 (PluginProcess + read
//! loop). Streaming (`llm.stream`) is wired alongside `llm.complete`:
//! [`IpcLlmBackend::stream`] dispatches an `llm.stream` request,
//! registers both the response oneshot and a chunk mpsc keyed on the
//! same msgid, and hands the assembled stream off to
//! [`crate::plugin_host::stream_router`].

use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tau_plugin_protocol::Frame;
use tau_ports::{CompletionRequest, CompletionResponse, CompletionStream, LlmError};
use tokio::sync::{mpsc, oneshot};

use crate::builder::DynLlmBackend;

use super::process::{PluginProcess, RpcResult};

/// Wire method name for the non-streaming LLM completion.
const LLM_COMPLETE_METHOD: &str = "llm.complete";

/// Wire method name for the streaming LLM completion. Per the plan-
/// erratum carryover from Task 9, the SDK runner dispatches
/// `llm.stream` (NOT `llm.complete_streaming`); both ends of the wire
/// must agree.
const LLM_STREAM_METHOD: &str = "llm.stream";

/// Bound on the per-stream chunk channel. Sized to absorb a short burst
/// of provider-side chunks without forcing the read loop to await an
/// idle consumer (which would block the loop from servicing other
/// in-flight calls on the same plugin process). 64 is a starting
/// guess — adjust if production traces show backpressure.
const STREAM_CHUNK_CHANNEL_CAPACITY: usize = 64;

/// IPC-backed [`DynLlmBackend`] adapter. Holds an `Arc<PluginProcess>`
/// shared with the read loop and any other adapters bound to the same
/// plugin instance.
///
/// Constructed by [`crate::plugin_host::load_llm_backend`] after the
/// handshake validates that the plugin advertises `llm.complete`. The
/// type is `pub` so the integration-test `__internals` re-exports
/// (gated by `feature = "test-support"`) can build instances without
/// spawning a real subprocess; production code only sees an
/// `Arc<dyn DynLlmBackend>` and never the concrete type.
pub struct IpcLlmBackend {
    pub(crate) name: String,
    pub(crate) process: Arc<PluginProcess>,
}

impl IpcLlmBackend {
    /// Construct an adapter from a plugin name (host-visible identity)
    /// and a shared [`PluginProcess`]. The name is what the kernel uses
    /// to resolve `agent_def.llm_backend` against the registry, and is
    /// expected to match the plugin's [`tau_pkg::LockedPlugin`] manifest
    /// name.
    pub fn new(name: String, process: Arc<PluginProcess>) -> Self {
        Self { name, process }
    }
}

impl DynLlmBackend for IpcLlmBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn complete<'a>(
        &'a self,
        req: CompletionRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<CompletionResponse, LlmError>> + 'a>> {
        let process = self.process.clone();
        Box::pin(async move {
            // Dispatch via the shared msgid+oneshot pattern (spec §7.4).
            let id = process.next_msgid.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = oneshot::channel::<RpcResult>();
            {
                let mut map = process.in_flight_responses.lock().await;
                map.insert(id, tx);
            }
            // Wire shape: params is a 1-element array `[CompletionRequest]`,
            // matching `tau_plugin_sdk::runners::llm_backend::dispatch_llm`.
            let params_bytes = rmp_serde::to_vec(&vec![&req]).map_err(|e| LlmError::Internal {
                message: format!("rmp encode CompletionRequest: {e}"),
            })?;
            let frame = Frame::Request {
                id,
                method: LLM_COMPLETE_METHOD.to_string(),
                params: params_bytes,
            };
            let frame_bytes = frame.encode().map_err(|e| LlmError::Internal {
                message: format!("frame encode: {e}"),
            })?;
            process
                .send_frame(&frame_bytes)
                .await
                .map_err(|e| LlmError::Internal {
                    message: format!("write frame: {e}"),
                })?;
            let result = rx.await.map_err(|_| LlmError::Internal {
                message: "in-flight response sender dropped (plugin crashed?)".to_string(),
            })?;
            match result {
                Ok(bytes) => rmp_serde::from_slice::<CompletionResponse>(&bytes).map_err(|e| {
                    LlmError::Internal {
                        message: format!("rmp decode CompletionResponse: {e}"),
                    }
                }),
                Err(envelope) => Err(LlmError::Internal {
                    message: format!(
                        "plugin error code {} message {}",
                        envelope.code, envelope.message
                    ),
                }),
            }
        })
    }

    fn stream<'a>(
        &'a self,
        req: CompletionRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<CompletionStream, LlmError>> + 'a>> {
        let process = self.process.clone();
        Box::pin(async move {
            // Allocate the msgid that ties together the request, the
            // response oneshot, and the per-stream chunk channel. The
            // read loop in `process::read_loop` keys both maps on this
            // id when routing inbound `Frame::Response` and
            // `stream.chunk` notifications.
            let id = process.next_msgid.fetch_add(1, Ordering::Relaxed);

            // Set up both delivery channels *before* sending the
            // request: chunks could land before the host's `await`
            // resumes, so the receivers must already be installed in
            // the in-flight maps. Same registration ordering as the
            // non-streaming path.
            let (chunk_tx, chunk_rx) = mpsc::channel(STREAM_CHUNK_CHANNEL_CAPACITY);
            let (final_tx, final_rx) = oneshot::channel::<RpcResult>();
            {
                let mut streams = process.in_flight_streams.lock().await;
                streams.insert(id, chunk_tx);
            }
            {
                let mut responses = process.in_flight_responses.lock().await;
                responses.insert(id, final_tx);
            }

            // Wire shape: params is a 1-element array `[CompletionRequest]`,
            // matching `tau_plugin_sdk::runners::llm_backend`'s
            // `llm.stream` dispatch.
            let params_bytes = rmp_serde::to_vec(&vec![&req]).map_err(|e| LlmError::Internal {
                message: format!("rmp encode CompletionRequest (stream): {e}"),
            })?;
            let frame = Frame::Request {
                id,
                method: LLM_STREAM_METHOD.to_string(),
                params: params_bytes,
            };
            let frame_bytes = frame.encode().map_err(|e| LlmError::Internal {
                message: format!("frame encode (stream): {e}"),
            })?;
            process
                .send_frame(&frame_bytes)
                .await
                .map_err(|e| LlmError::Internal {
                    message: format!("write frame (stream): {e}"),
                })?;

            Ok(crate::plugin_host::stream_router::assemble(
                chunk_rx, final_rx,
            ))
        })
    }
}
