//! `ProcessCapabilityGate` — process-spawn extension of [`CapabilityGate`].
//!
//! Adapters that gate **process** boundaries (OS sandboxes, container
//! sandboxes) implement this in addition to the universal `CapabilityGate`.
//! Adapters that gate non-process boundaries (wasm component import maps;
//! MCP contract wires) implement a different extension trait owned by
//! their respective host crate.

use std::process::Command;

use super::{CapabilityGate, CapabilityHandle, CapabilityPlan};
use crate::error::CapabilityError;

/// Extension trait: adapters that gate process spawn boundaries.
///
/// Implementors must also implement the universal [`CapabilityGate`].
#[allow(async_fn_in_trait)]
pub trait ProcessCapabilityGate: CapabilityGate {
    /// Apply gate enforcement to a `Command` in preparation for spawn.
    /// On Linux native, this registers `pre_exec` hooks. The returned
    /// `CapabilityHandle` holds any ambient resources (cgroup,
    /// namespace fd) and releases them on Drop.
    async fn wrap_spawn(
        &self,
        plan: &CapabilityPlan,
        cmd: &mut Command,
    ) -> Result<CapabilityHandle, CapabilityError>;

    /// Adapter-specific post-spawn setup. Called after `cmd.spawn()`
    /// succeeds and the child PID is known. Default: no-op.
    async fn apply_post_spawn(
        &self,
        plan: &CapabilityPlan,
        child_pid: i32,
        handle: &mut CapabilityHandle,
    ) -> Result<(), CapabilityError> {
        let _ = (plan, child_pid, handle);
        Ok(())
    }
}
