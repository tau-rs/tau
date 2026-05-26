//! Host-side [`crate::builder::DynTool`] implementation backed by a
//! spawned plugin subprocess.
//!
//! The kernel's run loop drives `init` → `invoke` → `teardown` per
//! tool_use; on the IPC side, the SDK's tool runner collapses that
//! into a single `tool.call` RPC carrying `(SessionContext, Value)`
//! and returning a [`tau_ports::ToolResult`]. This adapter therefore
//! treats `init` / `teardown` as no-ops, builds the
//! `(SessionContext, Value)` tuple in `invoke`, and dispatches one RPC
//! per call. Identical to the SDK side at
//! `tau_plugin_sdk::runners::tool::dispatch_tool`'s wire shape.
//!
//! `name`/`schema`/`capabilities` are cached at construction time:
//! `name` from the plugin manifest, `schema` from the `tool.describe`
//! RPC, and `capabilities` from the `tool.describe_capabilities` RPC,
//! both issued by the host during plugin loading.
//!
//! See spec §7.4. Streaming tool output is out of scope for v0.1.

use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tau_domain::Value;
use tau_plugin_protocol::Frame;
use tau_ports::{SessionContext, ToolError, ToolResult, ToolSpec};
use tokio::sync::oneshot;

use crate::builder::DynTool;

use super::process::{PluginProcess, RpcResult};

/// Wire method names for the tool port.
const TOOL_CALL_METHOD: &str = "tool.call";
const TOOL_DESCRIBE_METHOD: &str = "tool.describe";
/// Wire method name for fetching the tool's required capabilities.
/// Called once during plugin loading; returns Vec<tau_domain::Capability>.
#[allow(dead_code)] // consumed by Task 5 (sdk runner) / Task 6 (plugin_host caller)
const TOOL_DESCRIBE_CAPABILITIES_METHOD: &str = "tool.describe_capabilities";

/// IPC-backed [`DynTool`] adapter.
///
/// `schema` is captured up-front via a `tool.describe` RPC during
/// loading so the kernel's tool-spec broadcast (in
/// `CompletionRequest.tools`) doesn't pay an RPC round-trip per turn.
///
/// Public for the same `__internals` test-export reasons as
/// [`super::ipc_llm::IpcLlmBackend`].
pub struct IpcTool {
    pub(crate) name: String,
    pub(crate) schema: ToolSpec,
    /// Capabilities the plugin requires of the calling agent's package.
    /// Populated during plugin loading via the
    /// `tool.describe_capabilities` wire method. The kernel's capability
    /// filter at `run.rs:272` enforces this against the calling agent's
    /// package grants.
    pub(crate) capabilities: Vec<tau_domain::Capability>,
    pub(crate) process: Arc<PluginProcess>,
}

impl IpcTool {
    /// Construct an `IpcTool` from a plugin name, a pre-fetched
    /// [`ToolSpec`], the plugin's declared [`Capability`] list, and a
    /// shared [`PluginProcess`].
    ///
    /// `capabilities` is populated during plugin loading via the
    /// `tool.describe_capabilities` wire method. The kernel's capability
    /// filter at `run.rs:272` enforces this against the calling agent's
    /// package grants.
    pub fn new(
        name: String,
        schema: ToolSpec,
        capabilities: Vec<tau_domain::Capability>,
        process: Arc<PluginProcess>,
    ) -> Self {
        Self {
            name,
            schema,
            capabilities,
            process,
        }
    }

    /// Issue a `tool.describe` RPC and decode the [`ToolSpec`] response.
    /// Used by [`crate::plugin_host::load_tool`] during loading so the
    /// returned `IpcTool` has its schema cached.
    pub async fn fetch_schema(process: &PluginProcess) -> Result<ToolSpec, ToolError> {
        let id = process.next_msgid.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel::<RpcResult>();
        {
            let mut map = process.in_flight_responses.lock().await;
            map.insert(id, tx);
        }
        // Wire shape: params is a 0-element array, per the SDK side at
        // `tau_plugin_sdk::runners::tool::dispatch_tool`.
        let params_bytes =
            rmp_serde::to_vec::<Vec<()>>(&Vec::new()).map_err(|e| ToolError::Internal {
                message: format!("rmp encode tool.describe params: {e}"),
            })?;
        let frame = Frame::Request {
            id,
            method: TOOL_DESCRIBE_METHOD.to_string(),
            params: params_bytes,
        };
        let frame_bytes = frame.encode().map_err(|e| ToolError::Internal {
            message: format!("frame encode: {e}"),
        })?;
        process
            .send_frame(&frame_bytes)
            .await
            .map_err(|e| ToolError::Internal {
                message: format!("write frame: {e}"),
            })?;
        let result = rx.await.map_err(|_| ToolError::Internal {
            message: "in-flight response sender dropped (plugin crashed?)".to_string(),
        })?;
        match result {
            Ok(bytes) => {
                rmp_serde::from_slice::<ToolSpec>(&bytes).map_err(|e| ToolError::Internal {
                    message: format!("rmp decode ToolSpec: {e}"),
                })
            }
            Err(envelope) => Err(ToolError::Internal {
                message: format!(
                    "plugin error code {} message {}",
                    envelope.code, envelope.message
                ),
            }),
        }
    }

    /// Issue a `tool.describe_capabilities` RPC and decode the
    /// `Vec<Capability>` response. Used by
    /// [`crate::plugin_host::load_tool`] during loading so the returned
    /// `IpcTool` has its declared capabilities cached and the kernel's
    /// capability check at `run.rs:272` enforces them.
    pub async fn fetch_capabilities(
        process: &PluginProcess,
    ) -> Result<Vec<tau_domain::Capability>, ToolError> {
        let id = process.next_msgid.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel::<RpcResult>();
        {
            let mut map = process.in_flight_responses.lock().await;
            map.insert(id, tx);
        }
        // Wire shape: params is a 0-element array, per the SDK side at
        // `tau_plugin_sdk::runners::tool::dispatch_tool`.
        let params_bytes =
            rmp_serde::to_vec::<Vec<()>>(&Vec::new()).map_err(|e| ToolError::Internal {
                message: format!("rmp encode tool.describe_capabilities params: {e}"),
            })?;
        let frame = Frame::Request {
            id,
            method: TOOL_DESCRIBE_CAPABILITIES_METHOD.to_string(),
            params: params_bytes,
        };
        let frame_bytes = frame.encode().map_err(|e| ToolError::Internal {
            message: format!("frame encode: {e}"),
        })?;
        process
            .send_frame(&frame_bytes)
            .await
            .map_err(|e| ToolError::Internal {
                message: format!("write frame: {e}"),
            })?;
        let result = rx.await.map_err(|_| ToolError::Internal {
            message: "in-flight response sender dropped (plugin crashed?)".to_string(),
        })?;
        match result {
            Ok(bytes) => {
                rmp_serde::from_slice::<Vec<tau_domain::Capability>>(&bytes).map_err(|e| {
                    ToolError::Internal {
                        message: format!("rmp decode Vec<Capability>: {e}"),
                    }
                })
            }
            Err(envelope) => Err(ToolError::Internal {
                message: format!(
                    "plugin error code {} message {}",
                    envelope.code, envelope.message
                ),
            }),
        }
    }
}

impl DynTool for IpcTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn schema(&self) -> ToolSpec {
        self.schema.clone()
    }

    fn capabilities(&self) -> &[tau_domain::Capability] {
        &self.capabilities
    }

    fn init<'a>(
        &'a self,
        _ctx: SessionContext,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), ToolError>> + 'a>> {
        // The SDK's tool runner runs init+invoke+teardown inside a
        // single `tool.call` dispatch; the host-side `init` is a no-op
        // (the SessionContext is forwarded as part of the `invoke`
        // RPC).
        Box::pin(async { Ok(()) })
    }

    fn invoke<'a>(
        &'a self,
        ctx: &'a SessionContext,
        _session: &'a mut (),
        args: Value,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ToolResult, ToolError>> + 'a>> {
        let process = self.process.clone();
        // Clone ctx so the async block can own it across the await.
        let ctx_owned = ctx.clone();
        Box::pin(async move {
            let id = process.next_msgid.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = oneshot::channel::<RpcResult>();
            {
                let mut map = process.in_flight_responses.lock().await;
                map.insert(id, tx);
            }
            // Wire shape: params is (SessionContext, Value). Use the ctx
            // passed by the kernel (carries agent_instance_id +
            // granted_capabilities); no longer synthesize a fresh one.
            let params_bytes =
                rmp_serde::to_vec(&(ctx_owned, &args)).map_err(|e| ToolError::Internal {
                    message: format!("rmp encode tool.call params: {e}"),
                })?;
            // ADR-0006 §3.9: emit `tool.args_received` BEFORE forwarding
            // args to the plugin. `args_size_bytes` is the rmp-serde-
            // encoded byte length (the on-wire size). Payload contents
            // are intentionally NOT logged — that's redaction sub-project
            // C's territory.
            tracing::debug!(
                name = tau_observe::vocabulary::EV_TOOL_ARGS_RECEIVED,
                args_size_bytes = params_bytes.len(),
            );
            let frame = Frame::Request {
                id,
                method: TOOL_CALL_METHOD.to_string(),
                params: params_bytes,
            };
            let frame_bytes = frame.encode().map_err(|e| ToolError::Internal {
                message: format!("frame encode: {e}"),
            })?;
            process
                .send_frame(&frame_bytes)
                .await
                .map_err(|e| ToolError::Internal {
                    message: format!("write frame: {e}"),
                })?;
            let result = rx.await.map_err(|_| ToolError::Internal {
                message: "in-flight response sender dropped (plugin crashed?)".to_string(),
            })?;
            match result {
                Ok(bytes) => {
                    // ADR-0006 §3.9: emit `tool.result_received` on the
                    // Ok branch (Err is covered by `tool.invoke_failed`
                    // at the stream.rs call site). `result_size_bytes`
                    // is the rmp-serde-encoded response byte length.
                    tracing::debug!(
                        name = tau_observe::vocabulary::EV_TOOL_RESULT_RECEIVED,
                        result_size_bytes = bytes.len(),
                    );
                    rmp_serde::from_slice::<ToolResult>(&bytes).map_err(|e| ToolError::Internal {
                        message: format!("rmp decode ToolResult: {e}"),
                    })
                }
                Err(envelope) => Err(ToolError::Internal {
                    message: format!(
                        "plugin error code {} message {}",
                        envelope.code, envelope.message
                    ),
                }),
            }
        })
    }

    fn teardown<'a>(
        &'a self,
        _session: (),
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), ToolError>> + 'a>> {
        // Symmetric with `init` — see the comment there.
        Box::pin(async { Ok(()) })
    }
}
