//! Run-time options for the tau-runtime host shell.
//!
//! As of β.1.3.5b, `RunOptions` is fully defined in `tau-runtime-core`
//! and re-exported here. The host shell adds no extra fields — callers
//! that used to set `project_override: Vec<CapabilityOverride>` now
//! build a [`crate::capability_resolver_impl::TauPkgCapabilityResolver`]
//! and stuff it into `capability_resolver` instead.
//!
//! `TokenUsage` is also re-exported from core.

pub use tau_runtime_core::options::{RunOptions, TokenUsage};
