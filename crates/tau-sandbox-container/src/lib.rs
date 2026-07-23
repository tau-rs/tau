//! Container sandbox adapter for tau.
//!
//! Shells out to `docker` or `podman` to wrap plugin processes in a container
//! with capability-derived isolation. Works on any host with one of the
//! runtimes installed (Linux / macOS / Windows with Docker Desktop).
//!
//! Compared to [`tau_sandbox_native`](https://docs.rs/tau-sandbox-native):
//! - **Pros**: cross-platform; stronger isolation (full container); no kernel
//!   feature requirements (works on macOS/Windows hosts).
//! - **Cons**: requires docker/podman on PATH; container-start latency
//!   (~50-200 ms cold); image management (pull / authentication).

#![deny(missing_docs)]

mod probe;
mod runner;

use std::process::Command;
use std::sync::Arc;

use tokio::sync::OnceCell;

use tau_ports::{
    CapabilityError, CapabilityGate, CapabilityHandle, CapabilityPlan, CapabilityProbe,
    CapabilityShapeSet, ProcessCapabilityGate,
};

/// Container runtime selection passed to [`ContainerSandbox::new`].
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerRuntime {
    /// Use the `docker` binary on PATH.
    Docker,
    /// Use the `podman` binary on PATH.
    Podman,
    /// Probe `podman` first, then `docker`; first found wins.
    ///
    /// Podman is preferred because it is daemonless, rootless-by-default,
    /// and Apache-2.0-licensed.
    Auto,
}

/// Container sandbox adapter.
///
/// Construct via [`ContainerSandbox::new`]. The probe result and resolved
/// runtime are cached together in a [`tokio::sync::OnceCell`] so the runtime
/// binary is shelled out to at most once per adapter instance — even for
/// [`ContainerRuntime::Auto`], which would otherwise re-probe on every spawn.
pub struct ContainerSandbox {
    name: String,
    runtime: ContainerRuntime,
    /// Cached `(probe result, resolved runtime)`. Populated lazily on the
    /// first call to [`CapabilityGate::probe`] or [`ProcessCapabilityGate::wrap_spawn`].
    probe_cache: Arc<OnceCell<(CapabilityProbe, probe::ResolvedRuntime)>>,
}

impl ContainerSandbox {
    /// Construct a container adapter using the given runtime selection.
    ///
    /// The probe is deferred until the first call to [`CapabilityGate::probe`].
    pub fn new(name: impl Into<String>, runtime: ContainerRuntime) -> Self {
        Self {
            name: name.into(),
            runtime,
            probe_cache: Arc::new(OnceCell::new()),
        }
    }
}

impl CapabilityGate for ContainerSandbox {
    fn name(&self) -> &str {
        &self.name
    }

    async fn probe(&self) -> CapabilityProbe {
        self.probe_cache
            .get_or_init(|| async { probe::run_probe(self.runtime).await })
            .await
            .0
            .clone()
    }

    fn supported_shapes(&self) -> CapabilityShapeSet {
        let mut set = CapabilityShapeSet::new();
        set.insert(tau_domain::CapabilityShape::FilesystemRead);
        set.insert(tau_domain::CapabilityShape::FilesystemWrite);
        set.insert(tau_domain::CapabilityShape::ProcessExec);
        set.insert(tau_domain::CapabilityShape::NetworkHttp);
        set
    }

    fn validate_plan(&self, plan: &CapabilityPlan) -> Result<(), CapabilityError> {
        let supported = self.supported_shapes();
        for cap in &plan.capabilities {
            let shape = cap.required_shape();
            if !supported.contains(&shape) {
                return Err(CapabilityError::ShapeUnsupported { shape });
            }
        }
        Ok(())
    }
}

impl ProcessCapabilityGate for ContainerSandbox {
    async fn wrap_spawn(
        &self,
        plan: &CapabilityPlan,
        cmd: &mut Command,
    ) -> Result<CapabilityHandle, CapabilityError> {
        self.validate_plan(plan)?;

        let (probe_result, runtime_kind) = self
            .probe_cache
            .get_or_init(|| async { probe::run_probe(self.runtime).await })
            .await;
        match probe_result {
            CapabilityProbe::Available { .. } => runner::wrap_command(plan, cmd, *runtime_kind),
            CapabilityProbe::Unavailable { reason } => Err(CapabilityError::Unavailable {
                reason: reason.clone(),
            }),
            // Non-exhaustive catch-all: treat any unknown future variant as an
            // internal error rather than silently proceeding.
            other => Err(CapabilityError::WrapFailed {
                message: format!("unexpected probe result: {other:?}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_round_trip() {
        let s = ContainerSandbox::new("ctr", ContainerRuntime::Docker);
        assert_eq!(s.name(), "ctr");
    }

    #[test]
    fn supported_shapes_includes_all() {
        let s = ContainerSandbox::new("ctr", ContainerRuntime::Auto);
        let supported = s.supported_shapes();
        assert!(supported.contains(&tau_domain::CapabilityShape::FilesystemRead));
        assert!(supported.contains(&tau_domain::CapabilityShape::FilesystemWrite));
        assert!(supported.contains(&tau_domain::CapabilityShape::ProcessExec));
        assert!(supported.contains(&tau_domain::CapabilityShape::NetworkHttp));
    }

    #[test]
    fn validate_plan_rejects_custom() {
        let s = ContainerSandbox::new("ctr", ContainerRuntime::Auto);
        let plan_json = serde_json::json!({
            "capabilities": [{ "kind": "custom.weird" }],
            "context": null,
            "limits": null,
        });
        let plan: CapabilityPlan = serde_json::from_value(plan_json).expect("decode");
        let err = s.validate_plan(&plan).expect_err("must reject custom");
        assert!(
            matches!(err, CapabilityError::ShapeUnsupported { .. }),
            "expected ShapeUnsupported, got {err:?}"
        );
    }

    #[test]
    fn validate_plan_accepts_known_shapes() {
        let s = ContainerSandbox::new("ctr", ContainerRuntime::Docker);
        let plan_json = serde_json::json!({
            "capabilities": [
                { "kind": "fs.read",  "paths": ["/etc"] },
                { "kind": "fs.write", "paths": ["/tmp"] },
                { "kind": "net.http", "hosts": ["example.com"], "methods": ["GET"] }
            ],
            "context": null,
            "limits": null,
        });
        let plan: CapabilityPlan = serde_json::from_value(plan_json).expect("decode");
        s.validate_plan(&plan)
            .expect("known shapes must be accepted");
    }

    #[test]
    fn runtime_variants_are_copy() {
        // ContainerRuntime derives Copy; verify it can be used by value.
        let r = ContainerRuntime::Docker;
        let _r2 = r;
        let _r3 = r; // would fail to compile if not Copy
    }
}
