//! Generic runner for plugins that implement [`tau_ports::Tool`].
//!
//! Drives the full plugin lifecycle:
//!
//! 1. Install the SDK tracing layer.
//! 2. Drive the [`crate::handshake::drive_handshake`] handshake.
//! 3. Loop: read a frame, dispatch to `tool.call` / `tool.describe` /
//!    `meta.describe`, send the response.
//! 4. On `meta.shutdown` notification (or stdin EOF), exit cleanly.
//!
//! v0.1 dispatch policy: `tool.call` opens a session via
//! [`tau_ports::Tool::init`], runs [`tau_ports::Tool::invoke`], and
//! tears it down via [`tau_ports::Tool::teardown`] in one shot. This
//! mirrors the [`tau_ports::StatelessAdapter`] semantics; persistent
//! per-host sessions can be plumbed through later without breaking the
//! wire shape.

use std::collections::BTreeMap;
use std::sync::Arc;

use tau_domain::{PortKind, Value};
use tau_plugin_protocol::{
    error::{RpcErrorEnvelope, INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND},
    handshake::meta,
    Frame, FramedReader, FramedWriter, FramerOptions, MethodSchema,
};
use tau_ports::{SessionContext, Tool};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::configure::Configure;
use crate::error::SdkError;
use crate::handshake::{drive_handshake, PluginMeta};
use crate::tracing_layer;

/// Method name for the tool-call dispatch.
const TOOL_CALL_METHOD: &str = "tool.call";
/// Method name for retrieving the tool's [`tau_ports::ToolSpec`].
const TOOL_DESCRIBE_METHOD: &str = "tool.describe";
/// Wire method name for fetching the tool's required capabilities.
/// Called once during plugin loading; returns Vec<tau_domain::Capability>.
const TOOL_DESCRIBE_CAPABILITIES_METHOD: &str = "tool.describe_capabilities";

/// Run a plugin that implements [`Tool`]. Reads frames from stdin,
/// writes frames to stdout. Returns when:
///
/// * The host sends `meta.shutdown`, OR
/// * Stdin closes (host died), OR
/// * An unrecoverable error occurs.
///
/// The plugin author passes their crate's `env!("CARGO_PKG_NAME")` and
/// `env!("CARGO_PKG_VERSION")` so the handshake response advertises
/// the plugin's own identity (not the SDK's).
///
/// # Example
///
/// ```no_run
/// # use tau_ports::{Tool, SessionContext, ToolResult, ToolSpec, ToolError};
/// # use tau_domain::Value;
/// # struct MyTool;
/// # impl Tool for MyTool {
/// #     type Session = ();
/// #     fn name(&self) -> &str { "my-tool" }
/// #     fn schema(&self) -> ToolSpec { unimplemented!() }
/// #     async fn init(&self, _: SessionContext) -> Result<Self::Session, ToolError> { Ok(()) }
/// #     async fn invoke(&self, _: &mut Self::Session, _: Value) -> Result<ToolResult, ToolError> {
/// #         unimplemented!()
/// #     }
/// #     async fn teardown(&self, _: Self::Session) -> Result<(), ToolError> { Ok(()) }
/// # }
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     tau_plugin_sdk::run_tool(
///         MyTool,
///         env!("CARGO_PKG_NAME"),
///         env!("CARGO_PKG_VERSION"),
///     ).await?;
///     Ok(())
/// }
/// ```
pub async fn run_tool<P>(plugin: P, plugin_name: &str, plugin_version: &str) -> Result<(), SdkError>
where
    P: Tool + Send + Sync + 'static,
{
    tracing_layer::install();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = FramedReader::new(stdin, FramerOptions::default());
    let mut writer = FramedWriter::new(stdout);

    run_tool_with_io(
        &mut reader,
        &mut writer,
        plugin,
        plugin_name,
        plugin_version,
    )
    .await
}

/// Same as [`run_tool`] but accepts an explicit reader and writer.
/// Used by integration tests over `tau_plugin_protocol::test_support::FakeStdioPeer`.
///
/// # Example
///
/// ```no_run
/// # use tau_ports::{Tool, SessionContext, ToolResult, ToolSpec, ToolError};
/// # use tau_domain::Value;
/// # use tau_plugin_protocol::{FramedReader, FramedWriter, FramerOptions};
/// # struct MyTool;
/// # impl Tool for MyTool {
/// #     type Session = ();
/// #     fn name(&self) -> &str { "my-tool" }
/// #     fn schema(&self) -> ToolSpec { unimplemented!() }
/// #     async fn init(&self, _: SessionContext) -> Result<Self::Session, ToolError> { Ok(()) }
/// #     async fn invoke(&self, _: &mut Self::Session, _: Value) -> Result<ToolResult, ToolError> {
/// #         unimplemented!()
/// #     }
/// #     async fn teardown(&self, _: Self::Session) -> Result<(), ToolError> { Ok(()) }
/// # }
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let stdin = tokio::io::stdin();
///     let stdout = tokio::io::stdout();
///     let mut reader = FramedReader::new(stdin, FramerOptions::default());
///     let mut writer = FramedWriter::new(stdout);
///     tau_plugin_sdk::run_tool_with_io(
///         &mut reader,
///         &mut writer,
///         MyTool,
///         env!("CARGO_PKG_NAME"),
///         env!("CARGO_PKG_VERSION"),
///     ).await?;
///     Ok(())
/// }
/// ```
pub async fn run_tool_with_io<R, W, P>(
    reader: &mut FramedReader<R>,
    writer: &mut FramedWriter<W>,
    plugin: P,
    plugin_name: &str,
    plugin_version: &str,
) -> Result<(), SdkError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    P: Tool + Send + Sync + 'static,
{
    let plugin = Arc::new(plugin);

    // 1. Drive handshake.
    let plugin_meta = build_tool_meta(plugin_name, plugin_version);
    let _request = drive_handshake(reader, writer, plugin_meta).await?;

    // 2. Dispatch loop.
    loop {
        let body = match reader.next_frame().await? {
            Some(b) => b,
            None => break,
        };
        let frame = Frame::decode(&body)?;
        match frame {
            Frame::Request { id, method, params } => {
                dispatch_tool(plugin.clone(), writer, id, &method, &params).await?;
            }
            Frame::Notification { method, .. } if method == meta::SHUTDOWN_METHOD => {
                tracing::info!(target: "tau_plugin_sdk", "received meta.shutdown");
                break;
            }
            _ => { /* ignore */ }
        }
    }

    Ok(())
}

/// Variant of [`run_tool`] that constructs the plugin via
/// [`Configure::from_config`] using the JSON config field from the
/// handshake.
///
/// Plugin authors call this when their plugin needs static config from
/// the host. The runner reads the handshake first, deserializes the
/// `config` field as `P::Config`, calls [`Configure::from_config`],
/// then proceeds into the regular dispatch loop.
///
/// ```no_run
/// # use tau_plugin_sdk::{Configure, ConfigError};
/// # use tau_ports::{Tool, SessionContext, ToolResult, ToolSpec, ToolError};
/// # use tau_domain::Value;
/// # use serde::Deserialize;
/// # #[derive(Deserialize)] struct MyConfig { base_url: String }
/// # struct MyTool { _base_url: String }
/// # impl Configure for MyTool {
/// #     type Config = MyConfig;
/// #     fn from_config(c: MyConfig) -> Result<Self, ConfigError> {
/// #         Ok(MyTool { _base_url: c.base_url })
/// #     }
/// # }
/// # impl Tool for MyTool {
/// #     type Session = ();
/// #     fn name(&self) -> &str { "my-tool" }
/// #     fn schema(&self) -> ToolSpec { unimplemented!() }
/// #     async fn init(&self, _: SessionContext) -> Result<Self::Session, ToolError> { Ok(()) }
/// #     async fn invoke(&self, _: &mut Self::Session, _: Value) -> Result<ToolResult, ToolError> {
/// #         unimplemented!()
/// #     }
/// #     async fn teardown(&self, _: Self::Session) -> Result<(), ToolError> { Ok(()) }
/// # }
/// // In plugin main.rs:
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     tau_plugin_sdk::run_tool_with_config::<MyTool>(
///         env!("CARGO_PKG_NAME"),
///         env!("CARGO_PKG_VERSION"),
///     ).await?;
///     Ok(())
/// }
/// ```
pub async fn run_tool_with_config<P>(
    plugin_name: &str,
    plugin_version: &str,
) -> Result<(), SdkError>
where
    P: Tool + Configure + Send + Sync + 'static,
{
    tracing_layer::install();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = FramedReader::new(stdin, FramerOptions::default());
    let mut writer = FramedWriter::new(stdout);

    run_tool_with_config_with_io::<_, _, P>(&mut reader, &mut writer, plugin_name, plugin_version)
        .await
}

/// Same as [`run_tool_with_config`] but accepts an explicit reader and
/// writer. Used by integration tests over
/// `tau_plugin_protocol::test_support::FakeStdioPeer`.
///
/// # Example
///
/// ```no_run
/// # use tau_plugin_sdk::{Configure, ConfigError};
/// # use tau_ports::{Tool, SessionContext, ToolResult, ToolSpec, ToolError};
/// # use tau_domain::Value;
/// # use tau_plugin_protocol::{FramedReader, FramedWriter, FramerOptions};
/// # use serde::Deserialize;
/// # #[derive(Deserialize)] struct MyConfig { base_url: String }
/// # struct MyTool { _base_url: String }
/// # impl Configure for MyTool {
/// #     type Config = MyConfig;
/// #     fn from_config(c: MyConfig) -> Result<Self, ConfigError> {
/// #         Ok(MyTool { _base_url: c.base_url })
/// #     }
/// # }
/// # impl Tool for MyTool {
/// #     type Session = ();
/// #     fn name(&self) -> &str { "my-tool" }
/// #     fn schema(&self) -> ToolSpec { unimplemented!() }
/// #     async fn init(&self, _: SessionContext) -> Result<Self::Session, ToolError> { Ok(()) }
/// #     async fn invoke(&self, _: &mut Self::Session, _: Value) -> Result<ToolResult, ToolError> {
/// #         unimplemented!()
/// #     }
/// #     async fn teardown(&self, _: Self::Session) -> Result<(), ToolError> { Ok(()) }
/// # }
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let stdin = tokio::io::stdin();
///     let stdout = tokio::io::stdout();
///     let mut reader = FramedReader::new(stdin, FramerOptions::default());
///     let mut writer = FramedWriter::new(stdout);
///     tau_plugin_sdk::run_tool_with_config_with_io::<_, _, MyTool>(
///         &mut reader,
///         &mut writer,
///         env!("CARGO_PKG_NAME"),
///         env!("CARGO_PKG_VERSION"),
///     ).await?;
///     Ok(())
/// }
/// ```
pub async fn run_tool_with_config_with_io<R, W, P>(
    reader: &mut FramedReader<R>,
    writer: &mut FramedWriter<W>,
    plugin_name: &str,
    plugin_version: &str,
) -> Result<(), SdkError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    P: Tool + Configure + Send + Sync + 'static,
{
    // 1. Drive handshake; capture HandshakeRequest.config.
    let plugin_meta = build_tool_meta(plugin_name, plugin_version);
    let request = drive_handshake(reader, writer, plugin_meta).await?;

    // 2. Deserialize JSON config and construct the plugin.
    let config: P::Config = serde_json::from_value(request.config)?;
    let plugin = P::from_config(config)?;
    let plugin = Arc::new(plugin);

    // 3. Dispatch loop (same as run_tool_with_io).
    loop {
        let body = match reader.next_frame().await? {
            Some(b) => b,
            None => break,
        };
        let frame = Frame::decode(&body)?;
        match frame {
            Frame::Request { id, method, params } => {
                dispatch_tool(plugin.clone(), writer, id, &method, &params).await?;
            }
            Frame::Notification { method, .. } if method == meta::SHUTDOWN_METHOD => {
                tracing::info!(target: "tau_plugin_sdk", "received meta.shutdown");
                break;
            }
            _ => { /* ignore */ }
        }
    }

    Ok(())
}

async fn dispatch_tool<W, P>(
    plugin: Arc<P>,
    writer: &mut FramedWriter<W>,
    id: u32,
    method: &str,
    params: &[u8],
) -> Result<(), SdkError>
where
    W: AsyncWrite + Unpin,
    P: Tool + Send + Sync + 'static,
{
    match method {
        TOOL_CALL_METHOD => {
            // params is `[SessionContext, Value]` (2-element tuple).
            let parsed: (SessionContext, Value) = match rmp_serde::from_slice(params) {
                Ok(p) => p,
                Err(e) => {
                    send_invalid_params(
                        writer,
                        id,
                        &format!("tool.call params decode failed: {e}"),
                    )
                    .await?;
                    return Ok(());
                }
            };
            let (ctx, args) = parsed;

            // Open session, invoke, teardown. Errors at any stage produce
            // an INTERNAL_ERROR response; semantic tool errors come back
            // inside `ToolResult { is_error: true, .. }`.
            let mut session = match plugin.init(ctx).await {
                Ok(s) => s,
                Err(e) => {
                    send_internal_error(writer, id, format!("tool.init failed: {e}")).await?;
                    return Ok(());
                }
            };

            let invoke_outcome = plugin.invoke(&mut session, args).await;

            // Best-effort teardown; we surface invoke-time errors in
            // preference to teardown-time ones.
            let teardown_outcome = plugin.teardown(session).await;

            match invoke_outcome {
                Ok(result) => {
                    let result_bytes = rmp_serde::to_vec(&result)?;
                    let frame = Frame::Response {
                        id,
                        error: None,
                        result: Some(result_bytes),
                    };
                    writer.write_frame(&frame.encode()?).await?;
                }
                Err(e) => {
                    send_internal_error(writer, id, format!("tool.invoke failed: {e}")).await?;
                }
            }

            if let Err(e) = teardown_outcome {
                tracing::warn!(
                    target: "tau_plugin_sdk",
                    error = %e,
                    "tool.teardown reported an error after dispatch"
                );
            }
        }
        TOOL_DESCRIBE_METHOD => {
            // params is `[]` (0-element). Returns the plugin's ToolSpec.
            let parsed: Vec<()> = match rmp_serde::from_slice(params) {
                Ok(p) => p,
                Err(e) => {
                    send_invalid_params(
                        writer,
                        id,
                        &format!("tool.describe params decode failed: {e}"),
                    )
                    .await?;
                    return Ok(());
                }
            };
            if !parsed.is_empty() {
                send_invalid_params(writer, id, "tool.describe params must be a 0-element array")
                    .await?;
                return Ok(());
            }
            let spec = plugin.schema();
            let result_bytes = rmp_serde::to_vec(&spec)?;
            let frame = Frame::Response {
                id,
                error: None,
                result: Some(result_bytes),
            };
            writer.write_frame(&frame.encode()?).await?;
        }
        TOOL_DESCRIBE_CAPABILITIES_METHOD => {
            // params is `[]` (0-element). Returns Vec<Capability>
            // from plugin.capabilities().
            let parsed: Vec<()> = match rmp_serde::from_slice(params) {
                Ok(p) => p,
                Err(e) => {
                    send_invalid_params(
                        writer,
                        id,
                        &format!("tool.describe_capabilities params decode failed: {e}"),
                    )
                    .await?;
                    return Ok(());
                }
            };
            if !parsed.is_empty() {
                send_invalid_params(
                    writer,
                    id,
                    "tool.describe_capabilities params must be a 0-element array",
                )
                .await?;
                return Ok(());
            }
            let caps: Vec<tau_domain::Capability> = plugin.capabilities().to_vec();
            let result_bytes = rmp_serde::to_vec(&caps)?;
            let frame = Frame::Response {
                id,
                error: None,
                result: Some(result_bytes),
            };
            writer.write_frame(&frame.encode()?).await?;
        }
        meta::DESCRIBE_METHOD => {
            // `meta.describe` params is `[method_name]`. Return an empty
            // schema for Task 9; richer schemas can be wired in later.
            let parsed: Vec<String> = rmp_serde::from_slice(params)?;
            if parsed.len() != 1 {
                send_invalid_params(writer, id, "meta.describe params must be [method_name]")
                    .await?;
                return Ok(());
            }
            let _method_name = &parsed[0];
            let schema = MethodSchema::new(serde_json::json!({}), serde_json::json!({}));
            let result_bytes = rmp_serde::to_vec(&schema)?;
            let frame = Frame::Response {
                id,
                error: None,
                result: Some(result_bytes),
            };
            writer.write_frame(&frame.encode()?).await?;
        }
        _ => {
            let envelope =
                RpcErrorEnvelope::new(METHOD_NOT_FOUND, format!("unknown method: {method}"), None);
            let frame = Frame::Response {
                id,
                error: Some(envelope),
                result: None,
            };
            writer.write_frame(&frame.encode()?).await?;
        }
    }
    Ok(())
}

async fn send_invalid_params<W: AsyncWrite + Unpin>(
    writer: &mut FramedWriter<W>,
    id: u32,
    msg: &str,
) -> Result<(), SdkError> {
    let envelope = RpcErrorEnvelope::new(INVALID_PARAMS, msg.to_string(), None);
    let frame = Frame::Response {
        id,
        error: Some(envelope),
        result: None,
    };
    writer.write_frame(&frame.encode()?).await?;
    Ok(())
}

async fn send_internal_error<W: AsyncWrite + Unpin>(
    writer: &mut FramedWriter<W>,
    id: u32,
    msg: String,
) -> Result<(), SdkError> {
    let envelope = RpcErrorEnvelope::new(INTERNAL_ERROR, msg, None);
    let frame = Frame::Response {
        id,
        error: Some(envelope),
        result: None,
    };
    writer.write_frame(&frame.encode()?).await?;
    Ok(())
}

fn build_tool_meta(plugin_name: &str, plugin_version: &str) -> PluginMeta {
    let mut schemas = BTreeMap::new();
    let empty = MethodSchema::new(serde_json::json!({}), serde_json::json!({}));
    schemas.insert(TOOL_CALL_METHOD.to_string(), empty.clone());
    schemas.insert(TOOL_DESCRIBE_METHOD.to_string(), empty.clone());
    schemas.insert(meta::DESCRIBE_METHOD.to_string(), empty);
    PluginMeta::new(
        plugin_name.to_string(),
        plugin_version.to_string(),
        PortKind::Tool,
        vec![
            TOOL_CALL_METHOD.to_string(),
            TOOL_DESCRIBE_METHOD.to_string(),
            meta::DESCRIBE_METHOD.to_string(),
        ],
        schemas,
    )
}
