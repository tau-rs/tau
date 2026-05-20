//! Errors emitted by the plugin SDK runners.

use tau_plugin_protocol::ProtocolError;
use thiserror::Error;

/// Errors that can be raised by SDK runner functions.
///
/// `#[non_exhaustive]`: additive variants are non-breaking.
///
/// # Example
///
/// ```
/// use tau_plugin_sdk::SdkError;
/// let err = SdkError::HandshakeMissing;
/// assert!(format!("{err}").contains("handshake"));
/// ```
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum SdkError {
    /// Wire-protocol failure (framing, codec, encode/decode).
    #[error(transparent)]
    Protocol(#[from] ProtocolError),

    /// MessagePack-RPC payload could not be decoded as the expected
    /// type (typically `params` of an inbound request).
    #[error("payload decode failed: {0}")]
    PayloadDecodeFailed(#[from] rmp_serde::decode::Error),

    /// MessagePack-RPC payload could not be encoded.
    #[error("payload encode failed: {0}")]
    PayloadEncodeFailed(#[from] rmp_serde::encode::Error),

    /// JSON config payload from the handshake could not be deserialized
    /// as the plugin's `Configure::Config` type.
    #[error("config decode failed: {0}")]
    ConfigDecodeFailed(#[from] serde_json::Error),

    /// Plugin's [`crate::configure::Configure::from_config`] returned
    /// an error during runner startup.
    #[error("config initialization failed: {0}")]
    Configure(#[from] crate::configure::ConfigError),

    /// IO error reading/writing stdio (typically EOF on stdin meaning
    /// host exited).
    #[error("stdio io error: {0}")]
    Io(#[from] std::io::Error),

    /// Plugin received a frame before the handshake completed.
    #[error("expected handshake request as the first frame, got something else")]
    HandshakeMissing,

    /// The host's handshake request didn't match the plugin's port
    /// declaration. The plugin SDK rejects this and exits.
    #[error("handshake port mismatch: host requested {host_requested}, plugin provides {plugin_provides}")]
    HandshakePortMismatch {
        /// Port the host requested in `meta.handshake`.
        host_requested: tau_domain::PortKind,
        /// Port the plugin advertises.
        plugin_provides: tau_domain::PortKind,
    },
}
