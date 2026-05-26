//! Generic runner for plugins that implement [`tau_ports::Storage`].
//!
//! v0.1 stub: drives the handshake correctly so a host can load the
//! plugin, but returns [`METHOD_NOT_FOUND`] for `storage.*` methods
//! since no toy plugin exercises them end-to-end. The full dispatch
//! lands in a subsequent sub-project alongside the host-side
//! `IpcStorage` adapter.
//!
//! See `docs/superpowers/specs/2026-04-28-plugin-loading-design.md` §5.2.

use std::collections::BTreeMap;
use std::sync::Arc;

use tau_domain::PortKind;
use tau_plugin_protocol::{
    error::{RpcErrorEnvelope, METHOD_NOT_FOUND},
    handshake::meta,
    Frame, FramedReader, FramedWriter, FramerOptions, MethodSchema,
};
use tau_ports::Storage;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::configure::Configure;
use crate::error::SdkError;
use crate::handshake::{drive_handshake, PluginMeta};
use crate::tracing_layer;

/// Run a plugin that implements [`Storage`]. Reads frames from stdin,
/// writes frames to stdout. Returns when the host sends
/// `meta.shutdown` or stdin closes.
///
/// v0.1 stub: handshake works; `storage.*` methods return
/// `METHOD_NOT_FOUND` until the host wiring lands.
///
/// # Example
///
/// ```no_run
/// # use tau_ports::{Storage, StorageError, Namespace, Key};
/// # struct MyStorage;
/// # impl Storage for MyStorage {
/// #     fn name(&self) -> &str { "my-storage" }
/// #     async fn get(&self, _: &Namespace, _: &Key) -> Result<Option<Vec<u8>>, StorageError> { Ok(None) }
/// #     async fn put(&self, _: &Namespace, _: &Key, _: &[u8]) -> Result<(), StorageError> { Ok(()) }
/// #     async fn delete(&self, _: &Namespace, _: &Key) -> Result<bool, StorageError> { Ok(false) }
/// #     async fn list(&self, _: &Namespace, _: &str) -> Result<Vec<Key>, StorageError> { Ok(vec![]) }
/// # }
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     tau_plugin_sdk::run_storage(
///         MyStorage,
///         env!("CARGO_PKG_NAME"),
///         env!("CARGO_PKG_VERSION"),
///     ).await?;
///     Ok(())
/// }
/// ```
pub async fn run_storage<P>(
    plugin: P,
    plugin_name: &str,
    plugin_version: &str,
) -> Result<(), SdkError>
where
    P: Storage + Send + Sync + 'static,
{
    tracing_layer::install();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = FramedReader::new(stdin, FramerOptions::default());
    let mut writer = FramedWriter::new(stdout);

    run_storage_with_io(
        &mut reader,
        &mut writer,
        plugin,
        plugin_name,
        plugin_version,
    )
    .await
}

/// Same as [`run_storage`] but accepts an explicit reader and writer.
///
/// # Example
///
/// ```no_run
/// # use tau_ports::{Storage, StorageError, Namespace, Key};
/// # use tau_plugin_protocol::{FramedReader, FramedWriter, FramerOptions};
/// # struct MyStorage;
/// # impl Storage for MyStorage {
/// #     fn name(&self) -> &str { "my-storage" }
/// #     async fn get(&self, _: &Namespace, _: &Key) -> Result<Option<Vec<u8>>, StorageError> { Ok(None) }
/// #     async fn put(&self, _: &Namespace, _: &Key, _: &[u8]) -> Result<(), StorageError> { Ok(()) }
/// #     async fn delete(&self, _: &Namespace, _: &Key) -> Result<bool, StorageError> { Ok(false) }
/// #     async fn list(&self, _: &Namespace, _: &str) -> Result<Vec<Key>, StorageError> { Ok(vec![]) }
/// # }
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let stdin = tokio::io::stdin();
///     let stdout = tokio::io::stdout();
///     let mut reader = FramedReader::new(stdin, FramerOptions::default());
///     let mut writer = FramedWriter::new(stdout);
///     tau_plugin_sdk::run_storage_with_io(
///         &mut reader,
///         &mut writer,
///         MyStorage,
///         env!("CARGO_PKG_NAME"),
///         env!("CARGO_PKG_VERSION"),
///     ).await?;
///     Ok(())
/// }
/// ```
pub async fn run_storage_with_io<R, W, P>(
    reader: &mut FramedReader<R>,
    writer: &mut FramedWriter<W>,
    plugin: P,
    plugin_name: &str,
    plugin_version: &str,
) -> Result<(), SdkError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    P: Storage + Send + Sync + 'static,
{
    let _plugin = Arc::new(plugin);

    let plugin_meta = build_storage_meta(plugin_name, plugin_version);
    let _request = drive_handshake(reader, writer, plugin_meta).await?;

    loop {
        let body = match reader.next_frame().await? {
            Some(b) => b,
            None => break,
        };
        let frame = Frame::decode(&body)?;
        match frame {
            Frame::Request { id, method, .. } => {
                let envelope = RpcErrorEnvelope::new(
                    METHOD_NOT_FOUND,
                    format!("storage runner does not yet dispatch: {method}"),
                    None,
                );
                let response = Frame::Response {
                    id,
                    error: Some(envelope),
                    result: None,
                };
                writer.write_frame(&response.encode()?).await?;
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

/// Variant of [`run_storage`] that constructs the plugin via
/// [`Configure::from_config`] using the JSON config field from the
/// handshake. v0.1 stub: same dispatch shape as [`run_storage`] —
/// handshake-and-loop with `METHOD_NOT_FOUND` for `storage.*`.
///
/// # Example
///
/// ```no_run
/// # use tau_plugin_sdk::{Configure, ConfigError};
/// # use tau_ports::{Storage, StorageError, Namespace, Key};
/// # use serde::Deserialize;
/// # #[derive(Deserialize)] struct MyConfig { path: String }
/// # struct MyStorage { _path: String }
/// # impl Configure for MyStorage {
/// #     type Config = MyConfig;
/// #     fn from_config(c: MyConfig) -> Result<Self, ConfigError> {
/// #         Ok(MyStorage { _path: c.path })
/// #     }
/// # }
/// # impl Storage for MyStorage {
/// #     fn name(&self) -> &str { "my-storage" }
/// #     async fn get(&self, _: &Namespace, _: &Key) -> Result<Option<Vec<u8>>, StorageError> { Ok(None) }
/// #     async fn put(&self, _: &Namespace, _: &Key, _: &[u8]) -> Result<(), StorageError> { Ok(()) }
/// #     async fn delete(&self, _: &Namespace, _: &Key) -> Result<bool, StorageError> { Ok(false) }
/// #     async fn list(&self, _: &Namespace, _: &str) -> Result<Vec<Key>, StorageError> { Ok(vec![]) }
/// # }
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     tau_plugin_sdk::run_storage_with_config::<MyStorage>(
///         env!("CARGO_PKG_NAME"),
///         env!("CARGO_PKG_VERSION"),
///     ).await?;
///     Ok(())
/// }
/// ```
pub async fn run_storage_with_config<P>(
    plugin_name: &str,
    plugin_version: &str,
) -> Result<(), SdkError>
where
    P: Storage + Configure + Send + Sync + 'static,
{
    tracing_layer::install();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = FramedReader::new(stdin, FramerOptions::default());
    let mut writer = FramedWriter::new(stdout);

    run_storage_with_config_with_io::<_, _, P>(
        &mut reader,
        &mut writer,
        plugin_name,
        plugin_version,
    )
    .await
}

/// Same as [`run_storage_with_config`] but accepts an explicit reader
/// and writer.
///
/// # Example
///
/// ```no_run
/// # use tau_plugin_sdk::{Configure, ConfigError};
/// # use tau_ports::{Storage, StorageError, Namespace, Key};
/// # use tau_plugin_protocol::{FramedReader, FramedWriter, FramerOptions};
/// # use serde::Deserialize;
/// # #[derive(Deserialize)] struct MyConfig { path: String }
/// # struct MyStorage { _path: String }
/// # impl Configure for MyStorage {
/// #     type Config = MyConfig;
/// #     fn from_config(c: MyConfig) -> Result<Self, ConfigError> {
/// #         Ok(MyStorage { _path: c.path })
/// #     }
/// # }
/// # impl Storage for MyStorage {
/// #     fn name(&self) -> &str { "my-storage" }
/// #     async fn get(&self, _: &Namespace, _: &Key) -> Result<Option<Vec<u8>>, StorageError> { Ok(None) }
/// #     async fn put(&self, _: &Namespace, _: &Key, _: &[u8]) -> Result<(), StorageError> { Ok(()) }
/// #     async fn delete(&self, _: &Namespace, _: &Key) -> Result<bool, StorageError> { Ok(false) }
/// #     async fn list(&self, _: &Namespace, _: &str) -> Result<Vec<Key>, StorageError> { Ok(vec![]) }
/// # }
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let stdin = tokio::io::stdin();
///     let stdout = tokio::io::stdout();
///     let mut reader = FramedReader::new(stdin, FramerOptions::default());
///     let mut writer = FramedWriter::new(stdout);
///     tau_plugin_sdk::run_storage_with_config_with_io::<_, _, MyStorage>(
///         &mut reader,
///         &mut writer,
///         env!("CARGO_PKG_NAME"),
///         env!("CARGO_PKG_VERSION"),
///     ).await?;
///     Ok(())
/// }
/// ```
pub async fn run_storage_with_config_with_io<R, W, P>(
    reader: &mut FramedReader<R>,
    writer: &mut FramedWriter<W>,
    plugin_name: &str,
    plugin_version: &str,
) -> Result<(), SdkError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    P: Storage + Configure + Send + Sync + 'static,
{
    let plugin_meta = build_storage_meta(plugin_name, plugin_version);
    let request = drive_handshake(reader, writer, plugin_meta).await?;

    let config: P::Config = serde_json::from_value(request.config)?;
    let _plugin = Arc::new(P::from_config(config)?);

    loop {
        let body = match reader.next_frame().await? {
            Some(b) => b,
            None => break,
        };
        let frame = Frame::decode(&body)?;
        match frame {
            Frame::Request { id, method, .. } => {
                let envelope = RpcErrorEnvelope::new(
                    METHOD_NOT_FOUND,
                    format!("storage runner does not yet dispatch: {method}"),
                    None,
                );
                let response = Frame::Response {
                    id,
                    error: Some(envelope),
                    result: None,
                };
                writer.write_frame(&response.encode()?).await?;
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

fn build_storage_meta(plugin_name: &str, plugin_version: &str) -> PluginMeta {
    let mut schemas = BTreeMap::new();
    schemas.insert(
        meta::DESCRIBE_METHOD.to_string(),
        MethodSchema::new(serde_json::json!({}), serde_json::json!({})),
    );
    PluginMeta::new(
        plugin_name.to_string(),
        plugin_version.to_string(),
        PortKind::Storage,
        vec![meta::DESCRIBE_METHOD.to_string()],
        schemas,
    )
}
