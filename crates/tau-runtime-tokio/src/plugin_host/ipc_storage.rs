//! Host-side [`crate::builder::DynStorage`] implementation backed by a
//! spawned plugin subprocess.
//!
//! Each `Storage` method maps onto one `storage.*` RPC. The wire
//! shapes follow the spec §7.4 conventions: params is a tuple of the
//! method's positional arguments, the result is the method's return
//! type, both rmp-serde-encoded.
//!
//! v0.1 host integration is loadable but not exercised end-to-end (see
//! spec §1.1) — the kernel doesn't route through `Storage` from the
//! run loop. This adapter exists so the four-port symmetry is in
//! place; mock-peer integration tests cover the dispatch plumbing.

use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tau_plugin_protocol::Frame;
use tau_ports::{Key, Namespace, StorageError};
use tokio::sync::oneshot;

use crate::builder::DynStorage;

use super::process::{PluginProcess, RpcResult};

/// Wire method names for the storage port.
const STORAGE_GET_METHOD: &str = "storage.get";
const STORAGE_PUT_METHOD: &str = "storage.put";
const STORAGE_DELETE_METHOD: &str = "storage.delete";
const STORAGE_LIST_METHOD: &str = "storage.list";

/// IPC-backed [`DynStorage`] adapter.
///
/// Public for the same `__internals` test-export reasons as
/// [`super::ipc_llm::IpcLlmBackend`].
pub struct IpcStorage {
    pub(crate) name: String,
    pub(crate) process: Arc<PluginProcess>,
}

impl IpcStorage {
    /// Construct an `IpcStorage` from a plugin name and a shared
    /// [`PluginProcess`].
    pub fn new(name: String, process: Arc<PluginProcess>) -> Self {
        Self { name, process }
    }
}

/// Issue a single `storage.*` RPC and return the raw response bytes
/// (or a typed [`StorageError`] wrapping the wire error envelope /
/// transport failure).
async fn dispatch_storage(
    process: &PluginProcess,
    method: &str,
    params_bytes: Vec<u8>,
) -> Result<Vec<u8>, StorageError> {
    let id = process.next_msgid.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel::<RpcResult>();
    {
        let mut map = process.in_flight_responses.lock().await;
        map.insert(id, tx);
    }
    let frame = Frame::Request {
        id,
        method: method.to_string(),
        params: params_bytes,
    };
    let frame_bytes = frame.encode().map_err(|e| StorageError::Internal {
        message: format!("frame encode: {e}"),
    })?;
    process
        .send_frame(&frame_bytes)
        .await
        .map_err(|e| StorageError::Internal {
            message: format!("write frame: {e}"),
        })?;
    let result = rx.await.map_err(|_| StorageError::Internal {
        message: "in-flight response sender dropped (plugin crashed?)".to_string(),
    })?;
    match result {
        Ok(bytes) => Ok(bytes),
        Err(envelope) => Err(StorageError::Internal {
            message: format!(
                "plugin error code {} message {}",
                envelope.code, envelope.message
            ),
        }),
    }
}

impl DynStorage for IpcStorage {
    fn name(&self) -> &str {
        &self.name
    }

    fn get<'a>(
        &'a self,
        namespace: &'a Namespace,
        key: &'a Key,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Option<Vec<u8>>, StorageError>> + 'a>>
    {
        let process = self.process.clone();
        let namespace = namespace.clone();
        let key = key.clone();
        Box::pin(async move {
            let params_bytes =
                rmp_serde::to_vec(&(&namespace, &key)).map_err(|e| StorageError::Internal {
                    message: format!("rmp encode storage.get params: {e}"),
                })?;
            let bytes = dispatch_storage(&process, STORAGE_GET_METHOD, params_bytes).await?;
            rmp_serde::from_slice::<Option<Vec<u8>>>(&bytes).map_err(|e| StorageError::Internal {
                message: format!("rmp decode storage.get response: {e}"),
            })
        })
    }

    fn put<'a>(
        &'a self,
        namespace: &'a Namespace,
        key: &'a Key,
        value: &'a [u8],
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), StorageError>> + 'a>> {
        let process = self.process.clone();
        let namespace = namespace.clone();
        let key = key.clone();
        let value = value.to_vec();
        Box::pin(async move {
            let params_bytes = rmp_serde::to_vec(&(&namespace, &key, &value)).map_err(|e| {
                StorageError::Internal {
                    message: format!("rmp encode storage.put params: {e}"),
                }
            })?;
            let bytes = dispatch_storage(&process, STORAGE_PUT_METHOD, params_bytes).await?;
            // `Option::None` is the spec's `null` result for unit-returning
            // methods; tolerate either an explicit unit encoding or a
            // zero-byte payload (read_loop maps `Frame::Response { result:
            // None, error: None }` to a zero-byte vec).
            if bytes.is_empty() {
                return Ok(());
            }
            rmp_serde::from_slice::<()>(&bytes).map_err(|e| StorageError::Internal {
                message: format!("rmp decode storage.put response: {e}"),
            })
        })
    }

    fn delete<'a>(
        &'a self,
        namespace: &'a Namespace,
        key: &'a Key,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<bool, StorageError>> + 'a>> {
        let process = self.process.clone();
        let namespace = namespace.clone();
        let key = key.clone();
        Box::pin(async move {
            let params_bytes =
                rmp_serde::to_vec(&(&namespace, &key)).map_err(|e| StorageError::Internal {
                    message: format!("rmp encode storage.delete params: {e}"),
                })?;
            let bytes = dispatch_storage(&process, STORAGE_DELETE_METHOD, params_bytes).await?;
            rmp_serde::from_slice::<bool>(&bytes).map_err(|e| StorageError::Internal {
                message: format!("rmp decode storage.delete response: {e}"),
            })
        })
    }

    fn list<'a>(
        &'a self,
        namespace: &'a Namespace,
        prefix: &'a str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Vec<Key>, StorageError>> + 'a>> {
        let process = self.process.clone();
        let namespace = namespace.clone();
        let prefix = prefix.to_string();
        Box::pin(async move {
            let params_bytes =
                rmp_serde::to_vec(&(&namespace, &prefix)).map_err(|e| StorageError::Internal {
                    message: format!("rmp encode storage.list params: {e}"),
                })?;
            let bytes = dispatch_storage(&process, STORAGE_LIST_METHOD, params_bytes).await?;
            rmp_serde::from_slice::<Vec<Key>>(&bytes).map_err(|e| StorageError::Internal {
                message: format!("rmp decode storage.list response: {e}"),
            })
        })
    }
}
