//! Sandboxed subprocess spawn for stdio MCP servers.
//!
//! Wraps a `tokio::process::Command` via
//! `tau_runtime_tokio::process_gate::DynProcessCapabilityGate::wrap_spawn`
//! exactly the same way `plugin_host::process::spawn` does — the
//! `CapabilityPlan` is honored at the OS boundary
//! (landlock/seccomp/sandbox-exec/podman per the four sandbox adapters).
//!
//! After `wrap_spawn` succeeds, the command is spawned under tokio.
//! Stdin / stdout / stderr handles are piped so the caller can wire
//! them into a `JsonLineFramer`.

use std::process::Stdio;
use std::sync::Arc;

use tau_ports::CapabilityPlan;
use tau_runtime_tokio::process_gate::DynProcessCapabilityGate;
use tokio::process::{Child, Command};

use crate::transport_stdio::error::StdioSpawnError;

/// Spawn an MCP server subprocess under the given capability gate.
///
/// The command's stdin/stdout/stderr are piped; the caller wires
/// `child.stdin.take()` and `child.stdout.take()` into a
/// `JsonLineFramer`.
///
/// # Errors
///
/// - [`StdioSpawnError::SandboxRefused`] — the capability gate refused
///   the plan (e.g. the plan demands a sandbox shape the gate adapter
///   doesn't support on this target).
/// - [`StdioSpawnError::TokioSpawn`] — `tokio::process::Command::spawn`
///   failed (binary missing, permission denied, etc.).
pub async fn spawn(
    mut cmd: Command,
    gate: Arc<dyn DynProcessCapabilityGate>,
    plan: &CapabilityPlan,
) -> Result<Child, StdioSpawnError> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let _handle = gate
        .wrap_spawn(plan, cmd.as_std_mut())
        .await
        .map_err(StdioSpawnError::SandboxRefused)?;

    let child = cmd.spawn().map_err(StdioSpawnError::from)?;
    Ok(child)
}
