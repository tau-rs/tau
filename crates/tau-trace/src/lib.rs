#![forbid(unsafe_code)]
//! Renderable trace model over `tau_ports::TraceEvent`. No runtime or TUI deps.
mod ingest;
mod model;
pub use ingest::{parse_line, IngestError};
pub use model::{Span, SpanKind, SpanStatus, TraceModel};
