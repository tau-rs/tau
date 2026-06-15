//! Execution profiles. A profile produces a normalized [`ConformanceEvent`]
//! stream from a [`Scenario`]. See ADR-0046.
//!
//! Two profiles exist:
//!
//! - [`DevProfile`] (this task) drives the interpreted runtime
//!   (`run_ir_streaming`) with a [`Captor`](tau_observe::capture::Captor)
//!   tracing layer installed, interleaving tracing + `RunEvent` at each
//!   generator yield.
//! - [`WasmProfile`] (Task 10) is a stub until β.7.5 ships `tau build wasm`.
//!
//! Both produce a `Vec<ConformanceEvent>`; the gate (Task 11/12) diffs the
//! two streams with [`crate::differ`].

use crate::event::ConformanceEvent;
use crate::scenario::Scenario;

pub mod dev;
pub mod wasm;

pub use dev::DevProfile;
pub use wasm::WasmProfile;

/// Opaque profile failure (wiring error, lowering refusal, transport
/// failure). The string is human-facing diagnostic only.
#[derive(Debug)]
pub struct ProfileError(pub String);

/// A profile drives one scenario to a normalized conformance-event stream.
///
/// `?Send` because [`DevProfile`] holds a `tracing` `DefaultGuard` across
/// awaits on a single-threaded executor (the guard is `!Send`).
#[async_trait::async_trait(?Send)]
pub trait Profile {
    /// Stable profile identifier (`"dev"`, `"wasm"`), used in diff reports.
    fn name(&self) -> &str;
    /// Drive `scenario` to its normalized conformance-event stream.
    async fn run(&self, scenario: &Scenario) -> Result<Vec<ConformanceEvent>, ProfileError>;
}
