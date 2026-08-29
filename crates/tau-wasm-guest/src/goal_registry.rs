//! In-guest `DeterministicRegistry`: the goal predicates this component's
//! baked IR can actually reach.
//!
//! `schema_valid` / unknown fns are build-time refused (predicate-fit);
//! reaching them here means the gate was bypassed — fail loudly.
//!
//! # Why this module is cfg-gated (#689)
//!
//! `build.rs` scans the baked IR and emits `tau_goal_predicates` /
//! `tau_goal_matches`. This module exists only under the former, and routes
//! through `invoke_alloc_only` rather than `invoke` under `not(the latter)`.
//! Both arms exist to leave `tau_native_tools::goal_predicates::matches_`
//! UNREFERENCED when the IR cannot reach it, so wasm-ld garbage-collects
//! `regex-automata`, `regex_syntax` and regex's Unicode tables — measured at
//! ~770 KiB of a ~2.8 MB component. A Cargo feature could not express this:
//! features resolve before build scripts run, so nothing at feature-
//! resolution time knows which IR is being baked.
//!
//! This narrows WHICH predicates are linked, never what a linked predicate
//! MEANS. A build that can reach `matches` links the identical engine the
//! native `BuiltinDeterministicRegistry` uses, so ADR-0068's cross-target
//! parity is untouched.

extern crate alloc;

use alloc::format;
use serde_json::Value;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::deterministic::DeterministicRegistry;

/// The predicate dispatch this component links.
///
/// `tau_goal_matches` → the full five-predicate table, including the
/// regex-backed `matches`. Otherwise → the four allocation-only predicates,
/// with `matches` declining exactly as an unknown fn does, which surfaces
/// below as the loud "no wasm execution path" error rather than a wrong
/// verdict.
#[cfg(tau_goal_matches)]
use tau_native_tools::goal_predicates::invoke as dispatch;
#[cfg(not(tau_goal_matches))]
use tau_native_tools::goal_predicates::invoke_alloc_only as dispatch;

pub struct GuestGoalRegistry;

impl DeterministicRegistry for GuestGoalRegistry {
    fn invoke(&self, fn_name: &str, args: &Value) -> Result<Value, RuntimeError> {
        match dispatch(fn_name, args) {
            Some(Ok(v)) => Ok(v),
            Some(Err(message)) => Err(RuntimeError::Internal { message }),
            None => Err(RuntimeError::Internal {
                message: format!("tau-wasm-guest: fn {fn_name:?} has no wasm execution path (predicate-fit should have refused this build)"),
            }),
        }
    }
}
