//! Passthrough sandbox adapter — no isolation; explicit opt-out path.
//!
//! Selected only when the project's `required_tier` is `None` OR the
//! `--no-sandbox` CLI flag is set. The default chain (Native + Container)
//! does NOT include passthrough; selection is always explicit.

use std::process::Command;

use tau_domain::{CapabilityShape, CapabilityShapeSet};
use tau_ports::{Sandbox, SandboxError, SandboxHandle, SandboxPlan, SandboxProbe, SandboxTier};

/// Passthrough sandbox adapter (no isolation).
///
/// Implements [`tau_ports::Sandbox`]:
/// - `probe()` returns `Available { tier: None, details: "passthrough (no isolation)" }`.
/// - `supported_shapes()` returns the union of all known shapes (so any
///   Layer-3 shape check passes).
/// - `validate_plan(_)` always returns `Ok(())`.
/// - `wrap_spawn(_, _)` is a no-op; returns `SandboxHandle::noop()`.
///
/// # Example
///
/// ```
/// use tau_runtime::sandbox::passthrough::PassthroughSandbox;
/// use tau_ports::Sandbox;
///
/// let p = PassthroughSandbox::new();
/// assert_eq!(p.name(), "passthrough");
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct PassthroughSandbox;

impl PassthroughSandbox {
    /// Construct a fresh passthrough adapter.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime::sandbox::passthrough::PassthroughSandbox;
    /// use tau_ports::{Sandbox, SandboxPlan};
    ///
    /// let p = PassthroughSandbox::new();
    /// let plan = SandboxPlan::new(vec![], None, None);
    /// assert!(p.validate_plan(&plan).is_ok());
    /// ```
    pub fn new() -> Self {
        Self
    }
}

impl Sandbox for PassthroughSandbox {
    fn name(&self) -> &str {
        "passthrough"
    }

    async fn probe(&self) -> SandboxProbe {
        SandboxProbe::Available {
            tier: SandboxTier::None,
            details: "passthrough (no isolation)".to_owned(),
        }
    }

    fn supported_shapes(&self) -> CapabilityShapeSet {
        let mut set = CapabilityShapeSet::new();
        set.insert(CapabilityShape::FilesystemRead);
        set.insert(CapabilityShape::FilesystemWrite);
        set.insert(CapabilityShape::ProcessExec);
        set.insert(CapabilityShape::NetworkHttp);
        set.insert(CapabilityShape::AgentSpawn);
        set
    }

    fn validate_plan(&self, _plan: &SandboxPlan) -> Result<(), SandboxError> {
        Ok(())
    }

    async fn wrap_spawn(
        &self,
        _plan: &SandboxPlan,
        _cmd: &mut Command,
    ) -> Result<SandboxHandle, SandboxError> {
        Ok(SandboxHandle::noop())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn probe_reports_available_with_tier_none() {
        let p = PassthroughSandbox::new();
        let probe = p.probe().await;
        match probe {
            SandboxProbe::Available { tier, details } => {
                assert_eq!(tier, SandboxTier::None);
                assert!(details.contains("passthrough"));
            }
            other => panic!("expected Available, got {other:?}"),
        }
    }

    #[test]
    fn supported_shapes_includes_all_known() {
        let p = PassthroughSandbox::new();
        let shapes = p.supported_shapes();
        assert!(shapes.contains(&CapabilityShape::FilesystemRead));
        assert!(shapes.contains(&CapabilityShape::FilesystemWrite));
        assert!(shapes.contains(&CapabilityShape::ProcessExec));
        assert!(shapes.contains(&CapabilityShape::NetworkHttp));
        assert!(shapes.contains(&CapabilityShape::AgentSpawn));
    }

    #[test]
    fn validate_plan_always_ok() {
        let p = PassthroughSandbox::new();
        let plan = SandboxPlan::new(vec![], None, None);
        assert!(p.validate_plan(&plan).is_ok());
    }

    #[tokio::test]
    async fn wrap_spawn_returns_noop_handle() {
        let p = PassthroughSandbox::new();
        let plan = SandboxPlan::new(vec![], None, None);
        let mut cmd = Command::new("/bin/true");
        let _h = p.wrap_spawn(&plan, &mut cmd).await.expect("wrap_spawn");
        // No assertion on the handle itself — Drop is what matters; the
        // cleanup closure is None so Drop is a no-op.
    }
}
