//! Capability override — lifted to `tau_pkg::capability_override` 2026-05-17.
//!
//! This shim re-exports the types so that existing `tau_runtime::capability_override::*`
//! import paths in tau-cli and other consumers continue to compile unchanged.
//!
//! New code SHOULD use `tau_pkg::capability_override::*` directly.

// The glob_subset sub-module is now part of tau_pkg; no local sub-module needed.

pub use tau_pkg::capability_override::{
    compute_effective, CapabilityOverride, EffectiveCapability, OverrideExpandError,
};
