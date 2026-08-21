#![forbid(unsafe_code)]
//! Renderable trace model over `tau_ports::TraceEvent`. No runtime or TUI deps.
mod model;
pub use model::{Span, SpanKind, SpanStatus, TraceModel};
