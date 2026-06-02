//! Transport-agnostic message-level cassette format (spec §11).
//!
//! Lives in `tau-mcp` (not `tau-mcp-tokio`) so wasm + embassy shells
//! can replay cassettes in tests without a tokio dependency. The
//! `transport` submodule (which provides `CassetteTransport`) requires
//! std + futures and is therefore gated on `with-std-adapters`.

pub mod message;
pub mod recorder;
pub mod replayer;

#[cfg(feature = "with-std-adapters")]
pub mod transport;

#[cfg(feature = "with-std-adapters")]
pub use transport::CassetteTransport;

pub use message::{CassetteMessage, Direction, MessageKind, CASSETTE_VERSION};
pub use recorder::Recorder;
pub use replayer::{ReplayError, Replayer};
