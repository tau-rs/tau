//! ratatui rendering for the execution-trace waterfall TUI (M1).
//!
//! [`draw`] is pure frame rendering over a [`tau_trace::TraceModel`]
//! snapshot; it performs no terminal I/O (no raw-mode, no alternate
//! screen, no stdout) and owns no terminal state, which is what makes it
//! drivable with ratatui's `TestBackend` in tests. A later task wires the
//! event loop (raw mode, alternate screen, input) around it and the CLI
//! subcommand that invokes it.

mod render;

pub use render::{draw, Filter, UiState};
