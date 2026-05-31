//! Re-exports of `tau-runtime-core` runtime + builder types.
//!
//! Pre-β.1.3.5b, `Runtime` was a host-shell newtype wrapping
//! `tau_runtime_core::Runtime` so inherent methods like `run_streaming`,
//! `run_with_history`, `spawn_root_agent` could be defined here. As of
//! β.1.3.5b those methods live on a [`crate::runtime_ext::RuntimeShellExt`]
//! extension trait so the kernel `Runtime` type can be the canonical type
//! shared with `no_std` host shells.

pub use crate::process_gate::DynProcessCapabilityGate;
pub use tau_runtime_core::builder::{
    DynCapabilityGate, DynLlmBackend, DynStorage, DynTool, Runtime, RuntimeBuilder,
};

/// Legacy alias preserved for code that named the builder
/// `TauRuntimeBuilder`. Prefer the re-exported [`RuntimeBuilder`] in
/// new code.
pub type TauRuntimeBuilder = RuntimeBuilder;
