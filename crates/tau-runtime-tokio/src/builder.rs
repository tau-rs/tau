//! Re-exports of `tau-runtime-core` runtime + builder types.
//!
//! Pre-β.1.3.5b, `Runtime` was a host-shell newtype wrapping
//! `tau_runtime_core::Runtime` so inherent methods like `run_streaming`,
//! `run_with_history`, `spawn_root_agent` could be defined here. Those
//! methods now live as inherent methods on the kernel `Runtime` itself
//! (β.1.3.5c); only the JSONL-persistence wiring of `spawn_root_agent`
//! remained tokio-shell-specific and lives as a free function on
//! [`crate::runtime_ext::spawn_root_agent_with_scope`].

pub use crate::process_gate::DynProcessCapabilityGate;
pub use tau_runtime_core::builder::{
    DynCapabilityGate, DynLlmBackend, DynStorage, DynTool, Runtime, RuntimeBuilder,
};

/// Legacy alias preserved for code that named the builder
/// `TauRuntimeBuilder`. Prefer the re-exported [`RuntimeBuilder`] in
/// new code.
pub type TauRuntimeBuilder = RuntimeBuilder;
