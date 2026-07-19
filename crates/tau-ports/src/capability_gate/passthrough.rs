//! `PassthroughGate` — process capability gate with no isolation.
//!
//! The explicit opt-out path: selected only when a project's `required_tier`
//! is `None` or a host disables sandboxing. It probes as `Available { tier:
//! None }`, supports every capability shape, validates every plan, and wraps a
//! spawn as a no-op. Lives in `tau-ports` (behind the `process` feature) so a
//! transport crate can use the default gate without depending on a host
//! runtime's sandbox graph. See ADR-0060.

use std::process::Command;

use alloc::string::String;

use tau_domain::{CapabilityShape, CapabilityShapeSet};

use super::process::ProcessCapabilityGate;
use super::{
    CapabilityError, CapabilityGate, CapabilityHandle, CapabilityPlan, CapabilityProbe,
    CapabilityTier,
};

/// Passthrough capability gate (no isolation).
///
/// Implements [`CapabilityGate`] + [`ProcessCapabilityGate`]:
/// - `probe()` returns `Available { tier: None, details: "passthrough (no isolation)" }`.
/// - `supported_shapes()` returns the union of all known shapes (so any
///   Layer-3 shape check passes).
/// - `validate_plan(_)` always returns `Ok(())`.
/// - `wrap_spawn(_, _)` is a no-op; returns `CapabilityHandle::noop()`.
///
/// # Example
///
/// ```
/// use tau_ports::capability_gate::passthrough::PassthroughGate;
/// use tau_ports::CapabilityGate;
///
/// let p = PassthroughGate::new();
/// assert_eq!(p.name(), "passthrough");
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct PassthroughGate;

impl PassthroughGate {
    /// Construct a fresh passthrough gate.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_ports::capability_gate::passthrough::PassthroughGate;
    /// use tau_ports::{CapabilityGate, CapabilityPlan};
    ///
    /// let p = PassthroughGate::new();
    /// let plan = CapabilityPlan::new(vec![], None, None);
    /// assert!(p.validate_plan(&plan).is_ok());
    /// ```
    pub fn new() -> Self {
        Self
    }
}

impl CapabilityGate for PassthroughGate {
    fn name(&self) -> &str {
        "passthrough"
    }

    async fn probe(&self) -> CapabilityProbe {
        CapabilityProbe::Available {
            tier: CapabilityTier::None,
            details: String::from("passthrough (no isolation)"),
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

    fn validate_plan(&self, _plan: &CapabilityPlan) -> Result<(), CapabilityError> {
        Ok(())
    }
}

impl ProcessCapabilityGate for PassthroughGate {
    async fn wrap_spawn(
        &self,
        _plan: &CapabilityPlan,
        _cmd: &mut Command,
    ) -> Result<CapabilityHandle, CapabilityError> {
        Ok(CapabilityHandle::noop())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[tokio::test]
    async fn probe_reports_available_with_tier_none() {
        let p = PassthroughGate::new();
        let probe = p.probe().await;
        match probe {
            CapabilityProbe::Available { tier, details } => {
                assert_eq!(tier, CapabilityTier::None);
                assert!(details.contains("passthrough"));
            }
            other => panic!("expected Available, got {other:?}"),
        }
    }

    #[test]
    fn supported_shapes_includes_all_known() {
        let p = PassthroughGate::new();
        let shapes = p.supported_shapes();
        assert!(shapes.contains(&CapabilityShape::FilesystemRead));
        assert!(shapes.contains(&CapabilityShape::FilesystemWrite));
        assert!(shapes.contains(&CapabilityShape::ProcessExec));
        assert!(shapes.contains(&CapabilityShape::NetworkHttp));
        assert!(shapes.contains(&CapabilityShape::AgentSpawn));
    }

    #[test]
    fn validate_plan_always_ok() {
        let p = PassthroughGate::new();
        let plan = CapabilityPlan::new(vec![], None, None);
        assert!(p.validate_plan(&plan).is_ok());
    }

    #[tokio::test]
    async fn wrap_spawn_returns_noop_handle() {
        let p = PassthroughGate::new();
        let plan = CapabilityPlan::new(vec![], None, None);
        let mut cmd = Command::new("/bin/true");
        let _h = p.wrap_spawn(&plan, &mut cmd).await.expect("wrap_spawn");
        // Drop is what matters; the cleanup closure is None so Drop is a no-op.
    }
}
