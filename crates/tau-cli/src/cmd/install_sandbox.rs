//! `tau-cli`'s adapter implementing [`tau_pkg::InstallSandbox`] with the real
//! runtime [`SandboxAdapter`] (audit S2). Bridges the sync port to the async
//! `wrap_spawn` and keeps the strict-tier egress proxy alive for the duration
//! of the gated spawn.
//!
//! ## Why a fresh thread + a [`tokio::runtime::Handle`]
//!
//! `wrap_spawn` is async and, on the strict tier, spawns an egress-proxy task
//! that must outlive `cmd.output()`. The bridge drives `wrap_spawn` on a
//! **fresh OS thread** via `Handle::block_on`:
//!
//! - A fresh thread has no ambient runtime, so `Handle::block_on` is legal
//!   (calling `block_on` on the main-runtime *worker* thread would panic with
//!   "Cannot start a runtime from within a runtime").
//! - The proxy task `wrap_spawn` spawns lands on the **main** runtime (via the
//!   captured handle) and lives as long as that runtime — i.e. past the build.
//!   It is aborted when the returned guard's `CapabilityHandle` drops.
//! - The gate owns only a `Handle`, never a `Runtime`, so nothing is dropped in
//!   an async context (which would panic in tokio's blocking shutdown).
//!
//! The same path serves both the synchronous build spawn and the cross-check
//! spawn (which itself runs inside `tau-pkg`'s off-thread bridge): each `wrap`
//! call hops to its own fresh thread.

use std::process::Command;

use tau_pkg::{InstallSandbox, InstallSandboxError, InstallSandboxGuard};
use tau_ports::capability_gate::{CapabilityHandle, CapabilityPlan};
use tau_runtime_tokio::process_gate::resolver::SandboxAdapter;

/// Adapter wrapping a resolved [`SandboxAdapter`] as a [`tau_pkg::InstallSandbox`].
pub struct RuntimeInstallSandbox {
    adapter: SandboxAdapter,
    handle: tokio::runtime::Handle,
    enforced: bool,
}

impl RuntimeInstallSandbox {
    /// Build the adapter. `handle` is a handle to a live (multi-thread) runtime
    /// on which the egress-proxy task will run; `enforced` is the probed
    /// verdict (tier > None), computed by the async caller since probing is
    /// async and must not nest a `block_on` inside the ambient runtime.
    pub fn new(adapter: SandboxAdapter, handle: tokio::runtime::Handle, enforced: bool) -> Self {
        Self {
            adapter,
            handle,
            enforced,
        }
    }
}

impl InstallSandbox for RuntimeInstallSandbox {
    fn is_enforced(&self) -> bool {
        self.enforced
    }

    fn wrap(
        &self,
        plan: &CapabilityPlan,
        cmd: &mut Command,
    ) -> Result<InstallSandboxGuard, InstallSandboxError> {
        let handle = &self.handle;
        let adapter = &self.adapter;
        // Fresh thread: no ambient runtime, so `Handle::block_on` is legal and
        // the proxy task spawned inside `wrap_spawn` lands on the handle's
        // runtime. The scoped thread can borrow `&mut cmd`, `adapter`, `plan`.
        let cap_handle: CapabilityHandle = std::thread::scope(|s| {
            s.spawn(|| handle.block_on(adapter.wrap_spawn(plan, cmd)))
                .join()
                .map_err(|_| InstallSandboxError::WrapFailed("wrap thread panicked".into()))?
                .map_err(|e| InstallSandboxError::WrapFailed(e.to_string()))
        })?;
        Ok(InstallSandboxGuard::new(cap_handle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_runtime_tokio::process_gate::resolver::SandboxAdapter;

    #[test]
    fn reports_constructed_enforcement_flag() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let adapter = SandboxAdapter::Passthrough(Default::default());
        let g = RuntimeInstallSandbox::new(adapter, rt.handle().clone(), false);
        assert!(!g.is_enforced());
    }

    #[test]
    fn passthrough_wrap_is_noop_and_succeeds() {
        // Passthrough wrap_spawn is a no-op; the bridge must drive it to a
        // guard without panicking (exercises the fresh-thread + Handle path
        // from a non-async test). `rt` is dropped at end of test on a
        // non-async thread, which is safe.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let adapter = SandboxAdapter::Passthrough(Default::default());
        let gate = RuntimeInstallSandbox::new(adapter, rt.handle().clone(), false);
        let plan = CapabilityPlan::new(Vec::new(), None, None);
        let mut cmd = Command::new("true");
        let _guard = gate.wrap(&plan, &mut cmd).expect("passthrough wrap ok");
    }
}
