//! Dial a cassette as if it were a live MCP server.
//!
//! Reads a JSONL cassette from disk, constructs a `CassetteTransport`,
//! and wraps it so `host_lifecycle::open()` can drive the handshake
//! uniformly with stdio + HTTP paths.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tau_mcp::cassette::CassetteTransport;
use tracing::{info, instrument};

use crate::host_lifecycle::error::LifecycleError;

/// Options for cassette dialing (currently no fields; reserved for future use).
#[derive(Debug, Default, Clone)]
pub struct CassetteDialOptions {}

/// Open a cassette file as a `CassetteTransport`.
///
/// Reads the JSONL cassette from `path`, parses it, and returns an
/// `Arc<CassetteTransport>` ready to be used with `drive_handshake`.
#[instrument(name = "cassette_dial", skip(_options), fields(path = %path.display()))]
pub fn dial(
    path: &Path,
    _options: CassetteDialOptions,
) -> Result<Arc<CassetteTransport>, LifecycleError> {
    info!("dialing cassette");
    let bytes = std::fs::read(path).map_err(|e| LifecycleError::Io {
        path: PathBuf::from(path),
        source: e,
    })?;
    let transport = CassetteTransport::from_jsonl_bytes(&bytes)
        .map_err(|e| LifecycleError::CassetteParse { source: e })?;
    Ok(transport)
}
