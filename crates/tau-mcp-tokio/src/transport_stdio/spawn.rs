//! Sandboxed subprocess spawn for stdio MCP servers.
//!
//! Wraps a `tokio::process::Command` via
//! `tau_ports::DynProcessGate::wrap_spawn` exactly the same way
//! `plugin_host::process::spawn` does — the
//! `CapabilityPlan` is honored at the OS boundary
//! (landlock/seccomp/sandbox-exec/podman per the four sandbox adapters).
//!
//! After `wrap_spawn` succeeds, the command is spawned under tokio.
//! Stdin / stdout / stderr handles are piped so the caller can wire
//! them into a `JsonLineFramer`.

use std::process::Stdio;
use std::sync::Arc;

use tau_ports::CapabilityPlan;
use tau_ports::DynProcessGate;
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
    gate: Arc<dyn DynProcessGate>,
    plan: &CapabilityPlan,
) -> Result<Child, StdioSpawnError> {
    let _handle = gate
        .wrap_spawn(plan, cmd.as_std_mut())
        .await
        .map_err(StdioSpawnError::SandboxRefused)?;

    // Apply piped stdio + kill_on_drop AFTER `wrap_spawn`. The command-rebuilding
    // sandbox adapters (darwin sandbox-exec, windows launcher, container docker
    // run) do `*cmd = Command::new(...)` and re-emit only program/args/envs/cwd,
    // so anything set before `wrap_spawn` is discarded — and `std::process::Command`
    // exposes no stdio getters, so an adapter cannot re-apply it. Setting it here
    // is safe for every adapter (native/mock/passthrough don't rebuild).
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let child = cmd.spawn().map_err(StdioSpawnError::from)?;
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::process::Command as StdCommand;

    use tau_ports::{
        CapabilityError, CapabilityGate, CapabilityHandle, CapabilityProbe, CapabilityShapeSet,
        CapabilityTier, ProcessCapabilityGate,
    };

    /// Test double that reproduces the `*cmd = Command::new(...)` command
    /// rebuild performed by the real command-rebuilding sandbox adapters
    /// (`tau-sandbox-darwin` / `-windows` / `-container`). Those adapters
    /// re-emit only program + args + envs + cwd on the fresh `Command`, so
    /// any stdio / `kill_on_drop` config the caller set *before* `wrap_spawn`
    /// is discarded. `std::process::Command` exposes no stdio getters, so an
    /// adapter physically cannot re-apply it — the caller must set stdio
    /// *after* `wrap_spawn`. This gate is the platform-agnostic stand-in for
    /// that behavior (a real adapter needs sandbox-exec / AppContainer /
    /// docker and only rebuilds on its own OS).
    struct RebuildingGate;

    impl CapabilityGate for RebuildingGate {
        fn name(&self) -> &str {
            "rebuilding-test-gate"
        }

        async fn probe(&self) -> CapabilityProbe {
            CapabilityProbe::Available {
                tier: CapabilityTier::None,
                details: "test: rebuilds command like sandbox-exec/launcher".into(),
            }
        }

        fn supported_shapes(&self) -> CapabilityShapeSet {
            CapabilityShapeSet::new()
        }

        fn validate_plan(&self, _plan: &CapabilityPlan) -> Result<(), CapabilityError> {
            Ok(())
        }
    }

    impl ProcessCapabilityGate for RebuildingGate {
        async fn wrap_spawn(
            &self,
            _plan: &CapabilityPlan,
            cmd: &mut StdCommand,
        ) -> Result<CapabilityHandle, CapabilityError> {
            // Re-emit program + args + envs on a fresh Command, exactly as the
            // real rebuilding adapters do — deliberately NOT stdio/kill_on_drop.
            let program = cmd.get_program().to_os_string();
            let args: Vec<std::ffi::OsString> = cmd.get_args().map(|a| a.to_os_string()).collect();
            let envs: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)> = cmd
                .get_envs()
                .map(|(k, v)| (k.to_os_string(), v.map(|v| v.to_os_string())))
                .collect();
            *cmd = StdCommand::new(program);
            cmd.args(args);
            for (k, v) in envs {
                match v {
                    Some(val) => {
                        cmd.env(k, val);
                    }
                    None => {
                        cmd.env_remove(k);
                    }
                }
            }
            Ok(CapabilityHandle::noop())
        }
    }

    /// A long-lived, cross-platform child that blocks on stdin so the piped
    /// handles stay open long enough to inspect.
    #[cfg(unix)]
    const SPAWN_PROGRAM: &str = "/bin/cat";
    #[cfg(windows)]
    const SPAWN_PROGRAM: &str = "cmd";

    /// Regression guard for the launcher/sandbox-exec stdio-loss bug: when the
    /// gate rebuilds the command (as the darwin/windows/container adapters do),
    /// `spawn()` must still hand back piped stdin/stdout/stderr because it
    /// applies the stdio config *after* `wrap_spawn`. Before the fix, stdio was
    /// set before `wrap_spawn` and the rebuild discarded it, leaving the child's
    /// stdin/stdout/stderr as `None` (and panicking the plugin-host caller).
    #[tokio::test]
    async fn spawn_pipes_stdio_even_when_gate_rebuilds_command() {
        let gate: Arc<dyn DynProcessGate> = Arc::new(RebuildingGate);
        let plan = CapabilityPlan::new(Vec::new(), None, None);
        let cmd = Command::new(SPAWN_PROGRAM);

        let mut child = spawn(cmd, gate, &plan).await.expect("spawn should succeed");

        assert!(
            child.stdin.is_some(),
            "stdin must be piped after a command-rebuilding wrap_spawn"
        );
        assert!(
            child.stdout.is_some(),
            "stdout must be piped after a command-rebuilding wrap_spawn"
        );
        assert!(
            child.stderr.is_some(),
            "stderr must be piped after a command-rebuilding wrap_spawn"
        );

        // Don't rely on kill_on_drop timing in the test harness; reap eagerly.
        let _ = child.kill().await;
    }
}
