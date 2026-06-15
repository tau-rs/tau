//! β.6 cross-profile conformance gate. See
//! `docs/superpowers/specs/2026-06-14-beta-6-conformance-gate-design.md`
//! and ADR-0046.
//!
//! Modules are added incrementally per implementation task (event,
//! differ, normalize, sequenced_llm, dispatcher, scenario, profile).

pub mod differ;
pub mod event;
pub mod normalize;

mod sequenced_llm;

pub use event::{ConformanceEvent, CONFORMANCE_EVENT_VERSION};
