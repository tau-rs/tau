//! Tau serve mode: JSON-RPC 2.0 over NDJSON-framed stdio.
//!
//! Public entry point: [`run`]. Builds a `Runtime` from
//! [`ServeOptions::project_path`], spawns the reader/dispatcher/writer
//! tasks, and blocks until shutdown.

mod cancel;
mod dispatch;
mod dispatch_run;
mod error_codes;
mod error_map;
mod framing;
mod handshake;
mod idle;
mod lifecycle;
mod methods;
mod options;
mod project;
mod protocol;
mod tracing_init;
mod wire;

pub use options::ServeOptions;

// Test-only re-exports. `#[doc(hidden)]` keeps them out of the public
// API surface (docs + semver) while `pub` allows integration tests in
// `tests/` to reach them.
#[doc(hidden)]
pub use cancel::CancelRegistry;
#[doc(hidden)]
pub use dispatch::Dispatcher;
#[doc(hidden)]
pub use framing::Inbound;
#[doc(hidden)]
pub use handshake::HandshakeState;
#[doc(hidden)]
pub use project::{Project, ResolveError};
#[doc(hidden)]
pub use protocol::{Outbound, RequestId};

use anyhow::Result;

/// Run the serve loop until shutdown.
///
/// Builds the runtime, starts the I/O tasks, and blocks. Returns
/// `Ok(())` on graceful shutdown; returns `Err` on startup failure.
pub async fn run(opts: ServeOptions) -> Result<()> {
    lifecycle::run(opts).await
}
