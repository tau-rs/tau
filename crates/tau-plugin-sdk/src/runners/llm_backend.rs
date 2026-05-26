//! Generic runner for plugins that implement [`tau_ports::LlmBackend`].
//!
//! Drives the full plugin lifecycle:
//!
//! 1. Install the SDK tracing layer.
//! 2. Drive the [`crate::handshake::drive_handshake`] handshake.
//! 3. Loop: read a frame, dispatch to `complete` / `stream` / `meta.describe`,
//!    send the response.
//! 4. On `meta.shutdown` notification (or stdin EOF), exit cleanly.
//!
//! See `docs/superpowers/specs/2026-04-28-plugin-loading-design.md` §5.2.

use std::collections::BTreeMap;
use std::sync::Arc;

use tau_domain::PortKind;
use tau_plugin_protocol::{
    error::{RpcErrorEnvelope, INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND},
    handshake::meta,
    Frame, FramedReader, FramedWriter, FramerOptions, MethodSchema,
};
use tau_ports::LlmBackend;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::configure::Configure;
use crate::error::SdkError;
use crate::handshake::{drive_handshake, PluginMeta};
use crate::streaming::stream_completion;
use crate::tracing_layer;

/// Method name for the streaming completion call.
const LLM_STREAM_METHOD: &str = "llm.stream";
/// Method name for the batch completion call.
const LLM_COMPLETE_METHOD: &str = "llm.complete";

/// Run a plugin that implements [`LlmBackend`]. Reads frames from
/// stdin, writes frames to stdout. Returns when:
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
/// # use tau_ports::{LlmBackend, CompletionRequest, CompletionResponse, CompletionStream, LlmError};
/// # struct MyPlugin;
/// # impl LlmBackend for MyPlugin {
/// #     fn name(&self) -> &str { "my-plugin" }
/// #     async fn complete(&self, _: CompletionRequest) -> Result<CompletionResponse, LlmError> {
/// #         unimplemented!()
/// #     }
/// #     async fn stream(&self, _: CompletionRequest) -> Result<CompletionStream, LlmError> {
/// #         unimplemented!()
/// #     }
/// # }
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     tau_plugin_sdk::run_llm_backend(
///         MyPlugin,
///         env!("CARGO_PKG_NAME"),
///         env!("CARGO_PKG_VERSION"),
///     ).await?;
///     Ok(())
/// }
/// ```
pub async fn run_llm_backend<P>(
    plugin: P,
    plugin_name: &str,
    plugin_version: &str,
) -> Result<(), SdkError>
where
    P: LlmBackend + Send + Sync + 'static,
{
    tracing_layer::install();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = FramedReader::new(stdin, FramerOptions::default());
    let mut writer = FramedWriter::new(stdout);

    run_llm_backend_with_io(
        &mut reader,
        &mut writer,
        plugin,
        plugin_name,
        plugin_version,
    )
    .await
}

/// Same as [`run_llm_backend`] but accepts an explicit reader and writer.
/// Used by integration tests over `tau_plugin_protocol::test_support::FakeStdioPeer`.
///
/// # Example
///
/// ```no_run
/// # use tau_ports::{LlmBackend, CompletionRequest, CompletionResponse, CompletionStream, LlmError};
/// # use tau_plugin_protocol::{FramedReader, FramedWriter, FramerOptions};
/// # struct MyPlugin;
/// # impl LlmBackend for MyPlugin {
/// #     fn name(&self) -> &str { "my-plugin" }
/// #     async fn complete(&self, _: CompletionRequest) -> Result<CompletionResponse, LlmError> {
/// #         unimplemented!()
/// #     }
/// #     async fn stream(&self, _: CompletionRequest) -> Result<CompletionStream, LlmError> {
/// #         unimplemented!()
/// #     }
/// # }
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let stdin = tokio::io::stdin();
///     let stdout = tokio::io::stdout();
///     let mut reader = FramedReader::new(stdin, FramerOptions::default());
///     let mut writer = FramedWriter::new(stdout);
///     tau_plugin_sdk::run_llm_backend_with_io(
///         &mut reader,
///         &mut writer,
///         MyPlugin,
///         env!("CARGO_PKG_NAME"),
///         env!("CARGO_PKG_VERSION"),
///     ).await?;
///     Ok(())
/// }
/// ```
pub async fn run_llm_backend_with_io<R, W, P>(
    reader: &mut FramedReader<R>,
    writer: &mut FramedWriter<W>,
    plugin: P,
    plugin_name: &str,
    plugin_version: &str,
) -> Result<(), SdkError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    P: LlmBackend + Send + Sync + 'static,
{
    let plugin = Arc::new(plugin);

    // 1. Drive handshake.
    let plugin_meta = build_llm_backend_meta(plugin_name, plugin_version);
    let _request = drive_handshake(reader, writer, plugin_meta).await?;

    // 2. Dispatch loop.
    loop {
        let body = match reader.next_frame().await? {
            Some(b) => b,
            None => break, // host closed stdin
        };
        let frame = Frame::decode(&body)?;
        match frame {
            Frame::Request { id, method, params } => {
                dispatch_llm(plugin.clone(), writer, id, &method, &params).await?;
            }
            Frame::Notification { method, .. } if method == meta::SHUTDOWN_METHOD => {
                tracing::info!(target: "tau_plugin_sdk", "received meta.shutdown");
                break;
            }
            _ => { /* ignore other notifications/responses */ }
        }
    }

    Ok(())
}

/// Variant of [`run_llm_backend`] that constructs the plugin via
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
/// # use tau_ports::{LlmBackend, CompletionRequest, CompletionResponse, CompletionStream, LlmError};
/// # use serde::Deserialize;
/// # #[derive(Deserialize)] struct MyConfig { api_key: String }
/// # struct MyPlugin { _api_key: String }
/// # impl Configure for MyPlugin {
/// #     type Config = MyConfig;
/// #     fn from_config(c: MyConfig) -> Result<Self, ConfigError> {
/// #         Ok(MyPlugin { _api_key: c.api_key })
/// #     }
/// # }
/// # impl LlmBackend for MyPlugin {
/// #     fn name(&self) -> &str { "my-plugin" }
/// #     async fn complete(&self, _: CompletionRequest) -> Result<CompletionResponse, LlmError> {
/// #         unimplemented!()
/// #     }
/// #     async fn stream(&self, _: CompletionRequest) -> Result<CompletionStream, LlmError> {
/// #         unimplemented!()
/// #     }
/// # }
/// // In plugin main.rs:
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     tau_plugin_sdk::run_llm_backend_with_config::<MyPlugin>(
///         env!("CARGO_PKG_NAME"),
///         env!("CARGO_PKG_VERSION"),
///     ).await?;
///     Ok(())
/// }
/// ```
pub async fn run_llm_backend_with_config<P>(
    plugin_name: &str,
    plugin_version: &str,
) -> Result<(), SdkError>
where
    P: LlmBackend + Configure + Send + Sync + 'static,
{
    tracing_layer::install();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = FramedReader::new(stdin, FramerOptions::default());
    let mut writer = FramedWriter::new(stdout);

    run_llm_backend_with_config_with_io::<_, _, P>(
        &mut reader,
        &mut writer,
        plugin_name,
        plugin_version,
    )
    .await
}

/// Same as [`run_llm_backend_with_config`] but accepts an explicit
/// reader and writer. Used by integration tests over
/// `tau_plugin_protocol::test_support::FakeStdioPeer`.
///
/// # Example
///
/// ```no_run
/// # use tau_plugin_sdk::{Configure, ConfigError};
/// # use tau_ports::{LlmBackend, CompletionRequest, CompletionResponse, CompletionStream, LlmError};
/// # use tau_plugin_protocol::{FramedReader, FramedWriter, FramerOptions};
/// # use serde::Deserialize;
/// # #[derive(Deserialize)] struct MyConfig { api_key: String }
/// # struct MyPlugin { _api_key: String }
/// # impl Configure for MyPlugin {
/// #     type Config = MyConfig;
/// #     fn from_config(c: MyConfig) -> Result<Self, ConfigError> {
/// #         Ok(MyPlugin { _api_key: c.api_key })
/// #     }
/// # }
/// # impl LlmBackend for MyPlugin {
/// #     fn name(&self) -> &str { "my-plugin" }
/// #     async fn complete(&self, _: CompletionRequest) -> Result<CompletionResponse, LlmError> {
/// #         unimplemented!()
/// #     }
/// #     async fn stream(&self, _: CompletionRequest) -> Result<CompletionStream, LlmError> {
/// #         unimplemented!()
/// #     }
/// # }
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let stdin = tokio::io::stdin();
///     let stdout = tokio::io::stdout();
///     let mut reader = FramedReader::new(stdin, FramerOptions::default());
///     let mut writer = FramedWriter::new(stdout);
///     tau_plugin_sdk::run_llm_backend_with_config_with_io::<_, _, MyPlugin>(
///         &mut reader,
///         &mut writer,
///         env!("CARGO_PKG_NAME"),
///         env!("CARGO_PKG_VERSION"),
///     ).await?;
///     Ok(())
/// }
/// ```
pub async fn run_llm_backend_with_config_with_io<R, W, P>(
    reader: &mut FramedReader<R>,
    writer: &mut FramedWriter<W>,
    plugin_name: &str,
    plugin_version: &str,
) -> Result<(), SdkError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    P: LlmBackend + Configure + Send + Sync + 'static,
{
    // 1. Drive handshake; capture HandshakeRequest.config.
    let plugin_meta = build_llm_backend_meta(plugin_name, plugin_version);
    let request = drive_handshake(reader, writer, plugin_meta).await?;

    // 2. Deserialize the JSON config as P::Config and construct the
    //    plugin. ConfigError -> SdkError::Configure via #[from].
    let config: P::Config = serde_json::from_value(request.config)?;
    let plugin = P::from_config(config)?;
    let plugin = Arc::new(plugin);

    // 3. Dispatch loop (same as run_llm_backend_with_io).
    loop {
        let body = match reader.next_frame().await? {
            Some(b) => b,
            None => break,
        };
        let frame = Frame::decode(&body)?;
        match frame {
            Frame::Request { id, method, params } => {
                dispatch_llm(plugin.clone(), writer, id, &method, &params).await?;
            }
            Frame::Notification { method, .. } if method == meta::SHUTDOWN_METHOD => {
                tracing::info!(target: "tau_plugin_sdk", "received meta.shutdown");
                break;
            }
            _ => { /* ignore other notifications/responses */ }
        }
    }

    Ok(())
}

async fn dispatch_llm<W, P>(
    plugin: Arc<P>,
    writer: &mut FramedWriter<W>,
    id: u32,
    method: &str,
    params: &[u8],
) -> Result<(), SdkError>
where
    W: AsyncWrite + Unpin,
    P: LlmBackend + Send + Sync + 'static,
{
    match method {
        LLM_COMPLETE_METHOD => {
            // params is `[CompletionRequest]` (1-element array).
            let parsed: Vec<tau_ports::CompletionRequest> = rmp_serde::from_slice(params)?;
            if parsed.len() != 1 {
                send_invalid_params(writer, id, "llm.complete params must be a 1-element array")
                    .await?;
                return Ok(());
            }
            let req = parsed.into_iter().next().expect("len checked above");
            match plugin.complete(req).await {
                Ok(resp) => {
                    let result_bytes = rmp_serde::to_vec(&resp)?;
                    let frame = Frame::Response {
                        id,
                        error: None,
                        result: Some(result_bytes),
                    };
                    writer.write_frame(&frame.encode()?).await?;
                }
                Err(llm_err) => {
                    send_internal_error(writer, id, format!("complete failed: {llm_err}")).await?;
                }
            }
        }
        LLM_STREAM_METHOD => {
            // params is `[CompletionRequest]` (1-element array).
            let parsed: Vec<tau_ports::CompletionRequest> = rmp_serde::from_slice(params)?;
            if parsed.len() != 1 {
                send_invalid_params(writer, id, "llm.stream params must be a 1-element array")
                    .await?;
                return Ok(());
            }
            let req = parsed.into_iter().next().expect("len checked above");
            match plugin.stream(req).await {
                Ok(stream) => {
                    stream_completion(writer, id, stream).await?;
                }
                Err(llm_err) => {
                    send_internal_error(writer, id, format!("stream failed: {llm_err}")).await?;
                }
            }
        }
        meta::DESCRIBE_METHOD => {
            // `meta.describe` params is `[method_name]`. For Task 9 we return
            // an empty schema; richer schemas can be wired in later.
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

fn build_llm_backend_meta(plugin_name: &str, plugin_version: &str) -> PluginMeta {
    let mut schemas = BTreeMap::new();
    let empty = MethodSchema::new(serde_json::json!({}), serde_json::json!({}));
    schemas.insert(LLM_COMPLETE_METHOD.to_string(), empty.clone());
    schemas.insert(LLM_STREAM_METHOD.to_string(), empty.clone());
    schemas.insert(meta::DESCRIBE_METHOD.to_string(), empty);
    PluginMeta::new(
        plugin_name.to_string(),
        plugin_version.to_string(),
        PortKind::LlmBackend,
        vec![
            LLM_COMPLETE_METHOD.to_string(),
            LLM_STREAM_METHOD.to_string(),
            meta::DESCRIBE_METHOD.to_string(),
        ],
        schemas,
    )
}
