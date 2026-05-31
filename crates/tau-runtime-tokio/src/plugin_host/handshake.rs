//! Host-side handshake driver.
//!
//! Drives the host end of the `meta.handshake` exchange:
//!
//! 1. Build a [`HandshakeRequest`] from the host's protocol version,
//!    expected port, trace context, and per-plugin config.
//! 2. Encode it as the `params[0]` of a `meta.handshake` request frame
//!    (msgid 1) and write it.
//! 3. Await the response with a timeout.
//! 4. Validate `protocol_version`, `provides`, and `required_methods`.
//!
//! Each failure mode maps to a specific
//! [`HandshakeFailureReason`] so the calling code surfaces typed
//! errors instead of stringly-typed wire failures.
//!
//! See `docs/superpowers/specs/2026-04-28-plugin-loading-design.md`
//! §7.3 (the host side of the handshake) and §4.4 (the wire-level
//! protocol the plugin SDK implements).

use std::time::Duration;

use tau_domain::PortKind;
use tau_plugin_protocol::{
    handshake::{meta, HandshakeRequest, HandshakeResponse, TraceContext, PROTOCOL_VERSION},
    Frame, FramedReader, FramedWriter,
};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::error::{CoreRuntimeError, HandshakeFailureReason, RuntimeError};

/// Msgid used for the handshake request. Subsequent calls (Task 15+)
/// allocate from `PluginProcess::next_msgid` starting at 2.
const HANDSHAKE_MSGID: u32 = 1;

/// Drive the host end of the `meta.handshake` exchange.
///
/// Returns the validated [`HandshakeResponse`] on success; otherwise
/// returns a [`RuntimeError::PluginHandshakeFailed`] carrying the
/// typed [`HandshakeFailureReason`].
///
/// # Errors
///
/// * [`HandshakeFailureReason::Timeout`] if the plugin doesn't reply
///   within `handshake_timeout`.
/// * [`HandshakeFailureReason::ProtocolVersionMismatch`] if the plugin
///   advertises a different `protocol_version`.
/// * [`HandshakeFailureReason::ProvidesMismatch`] if the plugin
///   advertises a different port than the host expected.
/// * [`HandshakeFailureReason::MissingRequiredMethod`] if the plugin's
///   advertised `methods` doesn't include one of `required_methods`.
/// * [`HandshakeFailureReason::Malformed`] for any structural failure
///   (frame decode, msgid mismatch, missing result field, plugin
///   returned an error envelope, etc.).
#[allow(clippy::too_many_arguments)]
#[doc(hidden)]
pub async fn drive_handshake<R, W>(
    reader: &mut FramedReader<R>,
    writer: &mut FramedWriter<W>,
    plugin_name: &str,
    expected_port: PortKind,
    required_methods: &[&str],
    config: serde_json::Value,
    trace_context: TraceContext,
    handshake_timeout: Duration,
) -> Result<HandshakeResponse, RuntimeError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    // 1. Build the handshake request.
    let request = HandshakeRequest::new(
        PROTOCOL_VERSION.to_string(),
        expected_port,
        trace_context,
        config,
    );

    // The wire shape for `meta.handshake` params is a 1-element array
    // (`Vec<HandshakeRequest>`) per the SDK's matching decoder; see
    // `tau_plugin_sdk::handshake::drive_handshake`.
    let params_bytes = rmp_serde::to_vec(&vec![&request])
        .map_err(|e| handshake_malformed(plugin_name, format!("encode handshake params: {e}")))?;
    let request_frame = Frame::Request {
        id: HANDSHAKE_MSGID,
        method: meta::HANDSHAKE_METHOD.to_string(),
        params: params_bytes,
    };
    let request_body = request_frame
        .encode()
        .map_err(|e| handshake_malformed(plugin_name, format!("encode handshake frame: {e}")))?;
    writer
        .write_frame(&request_body)
        .await
        .map_err(|e| handshake_malformed(plugin_name, format!("write handshake frame: {e}")))?;

    // 2. Await the response with a timeout. The timeout *only* covers
    //    the next frame read — once the response arrives, decode is
    //    fast and not subject to the same SLO.
    let read_outcome = tokio::time::timeout(handshake_timeout, reader.next_frame()).await;
    let response_body = match read_outcome {
        Err(_elapsed) => {
            return Err(RuntimeError::Core(
                CoreRuntimeError::PluginHandshakeFailed {
                    plugin: plugin_name.to_string(),
                    reason: HandshakeFailureReason::Timeout,
                },
            ));
        }
        Ok(Err(e)) => {
            return Err(handshake_malformed(
                plugin_name,
                format!("read handshake response: {e}"),
            ));
        }
        Ok(Ok(None)) => {
            return Err(handshake_malformed(
                plugin_name,
                "EOF before handshake response".to_string(),
            ));
        }
        Ok(Ok(Some(body))) => body,
    };

    // 3. Decode the response frame.
    let response_frame = Frame::decode(&response_body)
        .map_err(|e| handshake_malformed(plugin_name, format!("decode handshake response: {e}")))?;

    let (id, error, result) = match response_frame {
        Frame::Response { id, error, result } => (id, error, result),
        other => {
            return Err(handshake_malformed(
                plugin_name,
                format!("expected Response frame, got {other:?}"),
            ));
        }
    };
    if id != HANDSHAKE_MSGID {
        return Err(handshake_malformed(
            plugin_name,
            format!("expected msgid {HANDSHAKE_MSGID}, got {id}"),
        ));
    }
    if let Some(envelope) = error {
        return Err(handshake_malformed(
            plugin_name,
            format!(
                "plugin returned error envelope: code={} message={}",
                envelope.code, envelope.message
            ),
        ));
    }
    let result_bytes = result.ok_or_else(|| {
        handshake_malformed(plugin_name, "handshake response missing result".to_string())
    })?;

    let response: HandshakeResponse = rmp_serde::from_slice(&result_bytes).map_err(|e| {
        handshake_malformed(plugin_name, format!("decode handshake response body: {e}"))
    })?;

    // 4. Validate protocol_version.
    if response.protocol_version != PROTOCOL_VERSION {
        return Err(RuntimeError::Core(
            CoreRuntimeError::PluginHandshakeFailed {
                plugin: plugin_name.to_string(),
                reason: HandshakeFailureReason::ProtocolVersionMismatch {
                    host: PROTOCOL_VERSION.to_string(),
                    plugin: response.protocol_version,
                },
            },
        ));
    }

    // 5. Validate provides matches expected_port.
    if response.provides != expected_port {
        return Err(RuntimeError::Core(
            CoreRuntimeError::PluginHandshakeFailed {
                plugin: plugin_name.to_string(),
                reason: HandshakeFailureReason::ProvidesMismatch {
                    manifest: expected_port,
                    plugin_advertised: response.provides,
                },
            },
        ));
    }

    // 6. Validate required methods.
    for required in required_methods {
        if !response.methods.iter().any(|m| m == required) {
            return Err(RuntimeError::Core(
                CoreRuntimeError::PluginHandshakeFailed {
                    plugin: plugin_name.to_string(),
                    reason: HandshakeFailureReason::MissingRequiredMethod {
                        method: (*required).to_string(),
                    },
                },
            ));
        }
    }

    tracing::info!(
        target: "tau_runtime_tokio::plugin_host",
        plugin = plugin_name,
        methods = ?response.methods,
        "plugin.handshake.completed"
    );

    Ok(response)
}

/// Helper: build a `PluginHandshakeFailed { reason: Malformed }`.
fn handshake_malformed(plugin: &str, detail: String) -> RuntimeError {
    RuntimeError::Core(CoreRuntimeError::PluginHandshakeFailed {
        plugin: plugin.to_string(),
        reason: HandshakeFailureReason::Malformed { detail },
    })
}
