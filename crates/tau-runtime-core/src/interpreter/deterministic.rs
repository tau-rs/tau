//! Execute a `Node::Deterministic` step.
//!
//! v0: the StaticFnRef is resolved at lowering (cache filled by the
//! caller). Here we look it up in a `DeterministicRegistry` (a caller-
//! supplied trait object) and call its pure function. β.7 AOT
//! lowering inlines the call.

use serde_json::Value;
use tau_ir::Deterministic;

use crate::error::RuntimeError;

/// Caller-supplied registry of statically linked deterministic functions.
pub trait DeterministicRegistry: Send + Sync {
    /// Invoke the function named `fn_name` with `args`. Pure; no I/O.
    fn invoke(&self, fn_name: &str, args: &Value) -> Result<Value, RuntimeError>;
}

/// Execute a `Deterministic` step.
pub fn run_step(
    step: &Deterministic,
    registry: &dyn DeterministicRegistry,
    args: &Value,
) -> Result<Value, RuntimeError> {
    registry.invoke(&step.fn_ref.name, args)
}
