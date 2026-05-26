//! Plugin-side handshake driver: reads the host's `meta.handshake`
//! request, validates it, and sends back the plugin's
//! [`HandshakeResponse`].
//!
//! See `docs/superpowers/specs/2026-04-28-plugin-loading-design.md` §4.4
//! for the wire-level handshake protocol.

use std::collections::BTreeMap;

use tau_domain::PortKind;
use tau_plugin_protocol::{
    error::{RpcErrorEnvelope, INVALID_REQUEST},
    handshake::meta,
    Frame, FramedReader, FramedWriter, HandshakeRequest, HandshakeResponse, MethodSchema,
    PROTOCOL_VERSION,
};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::error::SdkError;

/// Plugin-side metadata describing what the runner advertises to the
/// host in its [`HandshakeResponse`].
///
/// `#[non_exhaustive]`: future revisions may add fields without
/// breaking external callers; per-port runners (see Task 9) construct
/// instances via [`PluginMeta::new`].
///
/// # Example
///
/// ```
/// use std::collections::BTreeMap;
/// use tau_domain::PortKind;
/// use tau_plugin_sdk::handshake::PluginMeta;
///
/// let meta = PluginMeta::new(
///     "my-llm-plugin".to_string(),
///     "0.1.0".to_string(),
///     PortKind::LlmBackend,
///     vec!["llm.complete".to_string(), "llm.stream".to_string()],
///     BTreeMap::new(),
/// );
/// assert_eq!(meta.plugin_name, "my-llm-plugin");
/// assert_eq!(meta.plugin_version, "0.1.0");
/// assert_eq!(meta.port, PortKind::LlmBackend);
/// assert_eq!(meta.methods.len(), 2);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct PluginMeta {
    /// Plugin name (typically `env!("CARGO_PKG_NAME")`).
    pub plugin_name: String,
    /// Plugin version (typically `env!("CARGO_PKG_VERSION")`).
    pub plugin_version: String,
    /// Port this plugin provides.
    pub port: PortKind,
    /// Methods the plugin handles, in addition to the protocol-level
    /// `meta.*` methods.
    pub methods: Vec<String>,
    /// JSON schemas per method (params + result shape).
    pub schemas: BTreeMap<String, MethodSchema>,
}

impl PluginMeta {
    /// Construct a [`PluginMeta`].
    pub fn new(
        plugin_name: String,
        plugin_version: String,
        port: PortKind,
        methods: Vec<String>,
        schemas: BTreeMap<String, MethodSchema>,
    ) -> Self {
        Self {
            plugin_name,
            plugin_version,
            port,
            methods,
            schemas,
        }
    }
}

/// Drive the plugin-side handshake: read the host's `meta.handshake`
/// request, validate it, and send the response. Returns the validated
/// [`HandshakeRequest`] so the caller can extract `trace_context` and
/// `config` for downstream wiring.
///
/// # Errors
///
/// * [`SdkError::HandshakeMissing`] if the transport closes before any
///   frame arrives, the first frame is not a `meta.handshake` request,
///   or the params shape is wrong.
/// * [`SdkError::HandshakePortMismatch`] if the host's requested port
///   does not match `meta.port`. An RPC error envelope is also written
///   back to the host so it can surface a typed error rather than EOF.
/// * [`SdkError::Protocol`] / [`SdkError::PayloadDecodeFailed`] /
///   [`SdkError::PayloadEncodeFailed`] for wire-level failures.
///
/// # Example
///
/// ```no_run
/// # use std::collections::BTreeMap;
/// # use tau_domain::PortKind;
/// # use tau_plugin_protocol::{FramedReader, FramedWriter, FramerOptions};
/// # use tau_plugin_sdk::handshake::{drive_handshake, PluginMeta};
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let stdin = tokio::io::stdin();
///     let stdout = tokio::io::stdout();
///     let mut reader = FramedReader::new(stdin, FramerOptions::default());
///     let mut writer = FramedWriter::new(stdout);
///     let meta = PluginMeta::new(
///         "my-plugin".to_string(),
///         "0.1.0".to_string(),
///         PortKind::LlmBackend,
///         vec!["llm.complete".to_string()],
///         BTreeMap::new(),
///     );
///     let request = drive_handshake(&mut reader, &mut writer, meta).await?;
///     assert_eq!(request.port, PortKind::LlmBackend);
///     Ok(())
/// }
/// ```
pub async fn drive_handshake<R, W>(
    reader: &mut FramedReader<R>,
    writer: &mut FramedWriter<W>,
    meta: PluginMeta,
) -> Result<HandshakeRequest, SdkError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    // 1. Read the next frame (must be the handshake request).
    let body = reader
        .next_frame()
        .await?
        .ok_or(SdkError::HandshakeMissing)?;
    let frame = Frame::decode(&body).map_err(SdkError::Protocol)?;

    let (msgid, params) = match frame {
        Frame::Request { id, method, params } if method == meta::HANDSHAKE_METHOD => (id, params),
        _ => return Err(SdkError::HandshakeMissing),
    };

    // 2. Decode params as [HandshakeRequest] (1-element array).
    let parsed: Vec<HandshakeRequest> = rmp_serde::from_slice(&params)?;
    if parsed.len() != 1 {
        // Send an error response so the host knows what happened.
        let envelope = RpcErrorEnvelope::new(
            INVALID_REQUEST,
            format!(
                "handshake params must be a 1-element array, got {}",
                parsed.len()
            ),
            None,
        );
        let response_frame = Frame::Response {
            id: msgid,
            error: Some(envelope),
            result: None,
        };
        let response_body = response_frame.encode().map_err(SdkError::Protocol)?;
        writer
            .write_frame(&response_body)
            .await
            .map_err(SdkError::Protocol)?;
        return Err(SdkError::HandshakeMissing);
    }
    let request = parsed.into_iter().next().expect("len checked above");

    // 3. Validate port matches.
    if request.port != meta.port {
        let envelope = RpcErrorEnvelope::new(
            INVALID_REQUEST,
            format!(
                "handshake port mismatch: host requested {}, plugin provides {}",
                request.port, meta.port,
            ),
            None,
        );
        let response_frame = Frame::Response {
            id: msgid,
            error: Some(envelope),
            result: None,
        };
        let response_body = response_frame.encode().map_err(SdkError::Protocol)?;
        writer
            .write_frame(&response_body)
            .await
            .map_err(SdkError::Protocol)?;
        return Err(SdkError::HandshakePortMismatch {
            host_requested: request.port,
            plugin_provides: meta.port,
        });
    }

    // 4. Build and send the success response.
    let response = HandshakeResponse::new(
        PROTOCOL_VERSION.to_string(),
        meta.port,
        meta.plugin_name,
        meta.plugin_version,
        meta.methods,
        meta.schemas,
    );
    let result_bytes = rmp_serde::to_vec(&response)?;
    let response_frame = Frame::Response {
        id: msgid,
        error: None,
        result: Some(result_bytes),
    };
    let response_body = response_frame.encode().map_err(SdkError::Protocol)?;
    writer
        .write_frame(&response_body)
        .await
        .map_err(SdkError::Protocol)?;

    Ok(request)
}
