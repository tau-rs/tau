//! `DynProcessCapabilityGate` — tokio-shell object-safe wrapper for
//! [`tau_ports::ProcessCapabilityGate`]. Phase β.1.4 folds this into
//! `tau-runtime-tokio` when that crate is created.
//!
//! Core's `DynCapabilityGate` only carries the four universal methods;
//! this trait adds the two process-spawn methods (`wrap_spawn`,
//! `apply_post_spawn`) so the runtime's process gate registry can store
//! a single trait object that exposes both.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tau_ports::{
    CapabilityError, CapabilityHandle, CapabilityPlan, ProcessCapabilityGate,
};

pub use tau_runtime_core::builder::DynCapabilityGate;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Object-safe wrapper of [`ProcessCapabilityGate`].
///
/// Extends [`DynCapabilityGate`] with the two process-spawn methods.
/// The process gate registry in `tau-runtime` (and post-Phase-β.1.4 in
/// `tau-runtime-tokio`) stores `Arc<dyn DynProcessCapabilityGate>`.
pub trait DynProcessCapabilityGate: DynCapabilityGate {
    /// Boxed-future wrapper for [`ProcessCapabilityGate::wrap_spawn`].
    fn wrap_spawn<'a>(
        &'a self,
        plan: &'a CapabilityPlan,
        cmd: &'a mut std::process::Command,
    ) -> BoxFuture<'a, Result<CapabilityHandle, CapabilityError>>;

    /// Boxed-future wrapper for [`ProcessCapabilityGate::apply_post_spawn`].
    fn apply_post_spawn<'a>(
        &'a self,
        plan: &'a CapabilityPlan,
        child_pid: i32,
        handle: &'a mut CapabilityHandle,
    ) -> BoxFuture<'a, Result<(), CapabilityError>>;
}

impl<T: ProcessCapabilityGate + 'static> DynProcessCapabilityGate for T {
    fn wrap_spawn<'a>(
        &'a self,
        plan: &'a CapabilityPlan,
        cmd: &'a mut std::process::Command,
    ) -> BoxFuture<'a, Result<CapabilityHandle, CapabilityError>> {
        Box::pin(ProcessCapabilityGate::wrap_spawn(self, plan, cmd))
    }

    fn apply_post_spawn<'a>(
        &'a self,
        plan: &'a CapabilityPlan,
        child_pid: i32,
        handle: &'a mut CapabilityHandle,
    ) -> BoxFuture<'a, Result<(), CapabilityError>> {
        Box::pin(ProcessCapabilityGate::apply_post_spawn(
            self, plan, child_pid, handle,
        ))
    }
}

/// Helper: wrap any `Arc<dyn DynProcessCapabilityGate>` as
/// `Arc<dyn DynCapabilityGate>`.
///
/// Needed when the process gate is stored in the universal registry
/// that expects `DynCapabilityGate` trait objects.
pub fn as_dyn_capability_gate(
    gate: Arc<dyn DynProcessCapabilityGate>,
) -> Arc<dyn DynCapabilityGate> {
    gate
}
