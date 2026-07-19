//! `DynProcessGate` — object-safe (dyn-compatible) form of
//! [`ProcessCapabilityGate`].
//!
//! [`ProcessCapabilityGate`] uses `async fn` in trait (AFIT), which is not
//! dyn-compatible, so a host that must store a process gate behind a trait
//! object — e.g. the MCP stdio transport in `tau-mcp-tokio` — stores
//! `Arc<dyn DynProcessGate>`. The blanket impl below means every
//! `ProcessCapabilityGate` (every sandbox adapter, [`super::passthrough::PassthroughGate`])
//! is automatically a `DynProcessGate`, so adapters need no extra code.
//!
//! This lives in `tau-ports` (behind the `process` feature) rather than a host
//! crate so a transport crate can depend on the *port* without pulling in a
//! whole host runtime and its sandbox/plugin graph. See ADR-0060.

use core::future::Future;
use core::pin::Pin;
use std::process::Command;

use alloc::boxed::Box;

use super::process::ProcessCapabilityGate;
use super::{CapabilityHandle, CapabilityPlan};
use crate::error::CapabilityError;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Object-safe wrapper of [`ProcessCapabilityGate`] with boxed-future methods.
///
/// Implemented automatically for every `ProcessCapabilityGate` via the blanket
/// impl below. Callers store `Arc<dyn DynProcessGate>` and invoke
/// [`DynProcessGate::wrap_spawn`] to gate a subprocess spawn.
pub trait DynProcessGate: Send + Sync {
    /// Boxed-future form of [`ProcessCapabilityGate::wrap_spawn`].
    fn wrap_spawn<'a>(
        &'a self,
        plan: &'a CapabilityPlan,
        cmd: &'a mut Command,
    ) -> BoxFuture<'a, Result<CapabilityHandle, CapabilityError>>;

    /// Boxed-future form of [`ProcessCapabilityGate::apply_post_spawn`].
    fn apply_post_spawn<'a>(
        &'a self,
        plan: &'a CapabilityPlan,
        child_pid: i32,
        handle: &'a mut CapabilityHandle,
    ) -> BoxFuture<'a, Result<(), CapabilityError>>;
}

impl<T: ProcessCapabilityGate + 'static> DynProcessGate for T {
    fn wrap_spawn<'a>(
        &'a self,
        plan: &'a CapabilityPlan,
        cmd: &'a mut Command,
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
