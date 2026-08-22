//! ratatui rendering + interactive shell for the execution-trace waterfall
//! TUI (M1).
//!
//! [`draw`] is pure frame rendering over a [`tau_trace::TraceModel`]
//! snapshot; it performs no terminal I/O (no raw-mode, no alternate
//! screen, no stdout) and owns no terminal state, which is what makes it
//! drivable with ratatui's `TestBackend` in tests. [`app`] wires the event
//! loop (raw mode, alternate screen, input, tail-follow) around it; a
//! later task wires the CLI subcommand that invokes [`run_tui`].

mod app;
mod render;

pub use app::{run_tui, App, Loop, TraceSource};
pub use render::{draw, Filter, UiState};
