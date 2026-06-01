//! Transport-agnostic message-level cassette format.
//!
//! Captures MCP-message traffic at the handler-dispatch boundary (above
//! the transport layer), so the same cassette replays under any transport
//! (stdio, HTTP, future ws) and any host shell (tokio, wasm, embassy).
//!
//! Spec: design doc §11.

pub mod message;
pub mod recorder;
pub mod replayer;

pub use message::{CassetteMessage, Direction, MessageKind, CASSETTE_VERSION};
pub use recorder::Recorder;
pub use replayer::{ReplayError, Replayer};
