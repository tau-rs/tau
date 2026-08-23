//! In-guest `DeterministicRegistry`: the five no_std goal predicates.
//! `schema_valid` / unknown fns are build-time refused (predicate-fit);
//! reaching them here means the gate was bypassed — fail loudly.

extern crate alloc;

use alloc::format;
use serde_json::Value;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::deterministic::DeterministicRegistry;

pub struct GuestGoalRegistry;

impl DeterministicRegistry for GuestGoalRegistry {
    fn invoke(&self, fn_name: &str, args: &Value) -> Result<Value, RuntimeError> {
        match tau_native_tools::goal_predicates::invoke(fn_name, args) {
            Some(Ok(v)) => Ok(v),
            Some(Err(message)) => Err(RuntimeError::Internal { message }),
            None => Err(RuntimeError::Internal {
                message: format!("tau-wasm-guest: fn {fn_name:?} has no wasm execution path (predicate-fit should have refused this build)"),
            }),
        }
    }
}
