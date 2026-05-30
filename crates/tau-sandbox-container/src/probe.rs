//! Container runtime detection.
//!
//! Shells out to `docker --version` or `podman --version` with a 2-second
//! timeout. Failures (binary not on PATH, daemon hung, non-zero exit) map to
//! [`CapabilityProbe::Unavailable`].

use std::process::Stdio;
use std::time::Duration;

use tau_ports::{CapabilityProbe, CapabilityTier};
use tokio::process::Command;
use tokio::time::timeout;

use crate::ContainerRuntime;

/// What runtime was actually selected after probing? Used by `wrap_spawn`
/// to know which binary to invoke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolvedRuntime {
    /// The `docker` binary.
    Docker,
    /// The `podman` binary.
    Podman,
}

impl ResolvedRuntime {
    /// The binary name for this runtime.
    pub(crate) fn binary(self) -> &'static str {
        match self {
            ResolvedRuntime::Docker => "docker",
            ResolvedRuntime::Podman => "podman",
        }
    }
}

/// Run the probe for the given runtime selection. Returns both the probe result
/// and the resolved runtime so both can be cached together in a single
/// [`tokio::sync::OnceCell`], eliminating any per-spawn re-probe.
///
/// For [`ContainerRuntime::Auto`] podman is tried first; docker is the
/// fallback if podman is not found or is unresponsive. Podman is preferred
/// because it is daemonless, rootless-by-default, and Apache-2.0-licensed
/// (no commercial-use restriction the way Docker Desktop has).
///
/// **Env-var override.** When the caller passes [`ContainerRuntime::Auto`],
/// the `TAU_CONTAINER_RUNTIME` env var (if set to `docker` or `podman`)
/// pins the choice without probing. Explicit `Docker`/`Podman` selections
/// from the caller are honored as-is and ignore the env var.
///
/// CI sets `TAU_CONTAINER_RUNTIME=docker` so plugin-compat tests use the
/// same runtime as `cargo xtask build-plugin-images` did when constructing
/// the per-plugin images, matching image storage.
///
/// **Note:** When the probe returns `Unavailable`, the `ResolvedRuntime`
/// placeholder (`Podman`) must not be used — callers MUST check the
/// `CapabilityProbe` branch first.
pub(crate) async fn run_probe(runtime: ContainerRuntime) -> (CapabilityProbe, ResolvedRuntime) {
    let effective = match runtime {
        ContainerRuntime::Auto => match std::env::var("TAU_CONTAINER_RUNTIME").ok().as_deref() {
            Some("docker") => ContainerRuntime::Docker,
            Some("podman") => ContainerRuntime::Podman,
            _ => ContainerRuntime::Auto,
        },
        other => other,
    };
    match effective {
        ContainerRuntime::Docker => (probe_one("docker").await, ResolvedRuntime::Docker),
        ContainerRuntime::Podman => (probe_one("podman").await, ResolvedRuntime::Podman),
        ContainerRuntime::Auto => match probe_one("podman").await {
            ok @ CapabilityProbe::Available { .. } => (ok, ResolvedRuntime::Podman),
            _ => {
                let docker_probe = probe_one("docker").await;
                (docker_probe, ResolvedRuntime::Docker)
            }
        },
    }
}

/// Probe a single binary by running `<binary> --version` with a 2-second
/// timeout. Returns `Available` on success, `Unavailable` otherwise.
async fn probe_one(binary: &'static str) -> CapabilityProbe {
    let probe_fut = async {
        let output = Command::new(binary)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;
        match output {
            Ok(out) if out.status.success() => {
                Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
            }
            _ => None,
        }
    };
    match timeout(Duration::from_secs(2), probe_fut).await {
        Ok(Some(version)) => CapabilityProbe::Available {
            tier: CapabilityTier::Strict,
            details: format!("{binary}: {version}"),
        },
        _ => CapabilityProbe::Unavailable {
            reason: format!("{binary} not on PATH or unresponsive"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn probe_unknown_binary_fails_fast() {
        // A binary that definitely does not exist should return Unavailable
        // within the 2-second timeout.
        let probe = probe_one("definitely-not-a-real-binary-7zX9").await;
        assert!(
            matches!(probe, CapabilityProbe::Unavailable { .. }),
            "expected Unavailable, got {probe:?}"
        );
    }

    #[tokio::test]
    async fn auto_runtime_falls_back_gracefully() {
        // When docker is absent and podman is absent we still get Unavailable,
        // not a panic. run_probe now returns (SandboxProbe, ResolvedRuntime).
        let (probe, _runtime) = run_probe(ContainerRuntime::Auto).await;
        // Result depends on the host; we only assert it is one of the two
        // valid enum variants (Available or Unavailable), never a panic.
        assert!(
            matches!(
                probe,
                CapabilityProbe::Available { .. } | CapabilityProbe::Unavailable { .. }
            ),
            "unexpected probe result: {probe:?}"
        );
    }
}
