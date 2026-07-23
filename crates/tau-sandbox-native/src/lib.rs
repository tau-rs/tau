//! Linux native sandbox adapter for tau.
//!
//! Implements [`tau_ports::CapabilityGate`] using:
//! - **landlock** (kernel 5.13+) for filesystem path isolation,
//! - **seccompiler** for syscall filtering (Strict tier — Task 4),
//! - **nix unshare** for user/network namespaces (Strict tier — Task 5).
//!
//! On non-Linux hosts the adapter exists but `probe()` returns
//! `CapabilityProbe::Unavailable` and all other methods return
//! `CapabilityError::Unavailable`.

// Opt out of the workspace `unsafe_code = "warn"` lint: the Linux exec path
// legitimately calls libc/seccomp/landlock via `unsafe`. See `exec`/`light`.
#![allow(unsafe_code)]
#![deny(missing_docs)]

mod shape;

#[cfg(target_os = "linux")]
mod exec;
#[cfg(target_os = "linux")]
mod light;
#[cfg(target_os = "linux")]
mod net;
#[cfg(target_os = "linux")]
mod probe;
#[cfg(target_os = "linux")]
mod strict;

#[cfg(not(target_os = "linux"))]
mod stub;

use std::process::Command;

use tau_ports::{
    CapabilityError, CapabilityGate, CapabilityHandle, CapabilityPlan, CapabilityProbe,
    CapabilityShapeSet, CapabilityTier, ProcessCapabilityGate,
};

/// Linux native sandbox adapter. Probe-driven: at construction time the
/// adapter is inert; calling [`Sandbox::probe`] discovers what the host
/// kernel can offer and the runtime caches the result.
pub struct NativeSandbox {
    name: String,
    // Used in #[cfg(target_os = "linux")] branches; suppress dead_code on other platforms.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    requested_tier: CapabilityTier,
}

impl NativeSandbox {
    /// Construct an adapter that will deliver up to the given tier. The
    /// effective tier is `min(requested_tier, probe_tier)`.
    pub fn new(name: impl Into<String>, requested_tier: CapabilityTier) -> Self {
        Self {
            name: name.into(),
            requested_tier,
        }
    }
}

impl CapabilityGate for NativeSandbox {
    fn name(&self) -> &str {
        &self.name
    }

    async fn probe(&self) -> CapabilityProbe {
        #[cfg(target_os = "linux")]
        {
            probe::probe(self.requested_tier).await
        }
        #[cfg(not(target_os = "linux"))]
        {
            stub::unavailable_probe()
        }
    }

    fn supported_shapes(&self) -> CapabilityShapeSet {
        #[cfg(target_os = "linux")]
        {
            shape::shapes_for_tier(self.requested_tier)
        }
        #[cfg(not(target_os = "linux"))]
        {
            CapabilityShapeSet::new()
        }
    }

    fn validate_plan(&self, plan: &CapabilityPlan) -> Result<(), CapabilityError> {
        let supported = self.supported_shapes();
        if supported.is_empty() {
            return Err(CapabilityError::Unavailable {
                reason: "tau-sandbox-native requires Linux".into(),
            });
        }
        for cap in &plan.capabilities {
            let shape = cap.required_shape();
            if !supported.contains(&shape) {
                return Err(CapabilityError::ShapeUnsupported { shape });
            }
        }

        Ok(())
    }
}

impl ProcessCapabilityGate for NativeSandbox {
    async fn apply_post_spawn(
        &self,
        plan: &CapabilityPlan,
        child_pid: i32,
        handle: &mut CapabilityHandle,
    ) -> Result<(), CapabilityError> {
        let _ = (plan, child_pid, handle);
        Ok(())
    }

    async fn wrap_spawn(
        &self,
        plan: &CapabilityPlan,
        cmd: &mut Command,
    ) -> Result<CapabilityHandle, CapabilityError> {
        self.validate_plan(plan)?;
        #[cfg(target_os = "linux")]
        {
            match self.requested_tier {
                CapabilityTier::Light => light::apply_landlock(plan, cmd),
                CapabilityTier::Strict => strict::apply_strict(plan, cmd),
                CapabilityTier::None => Ok(CapabilityHandle::noop()),
                other => Err(CapabilityError::Unsupported {
                    what: format!("tier {other:?} not implemented"),
                }),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (plan, cmd);
            Err(CapabilityError::Unavailable {
                reason: "tau-sandbox-native requires Linux".into(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_domain::fixtures as domain_fixtures;
    #[cfg(target_os = "linux")]
    use tau_domain::CapabilityShape;
    use tau_ports::fixtures as ports_fixtures;

    #[test]
    fn name_and_tier_round_trip() {
        let s = NativeSandbox::new("native-light", CapabilityTier::Light);
        assert_eq!(s.name(), "native-light");
    }

    #[test]
    fn supported_shapes_light_includes_fs() {
        let s = NativeSandbox::new("n", CapabilityTier::Light);
        let supported = s.supported_shapes();
        #[cfg(target_os = "linux")]
        {
            assert!(supported.contains(&CapabilityShape::FilesystemRead));
            assert!(supported.contains(&CapabilityShape::FilesystemWrite));
        }
        #[cfg(not(target_os = "linux"))]
        {
            assert!(supported.is_empty());
        }
    }

    #[test]
    fn validate_plan_rejects_unsupported_shape_at_light_tier() {
        let s = NativeSandbox::new("n", CapabilityTier::Light);
        let plan =
            ports_fixtures::plan_from_capabilities(vec![domain_fixtures::cap_custom("weird")]);
        let err = s.validate_plan(&plan).expect_err("must reject");
        #[cfg(target_os = "linux")]
        assert!(matches!(err, CapabilityError::ShapeUnsupported { .. }));
        #[cfg(not(target_os = "linux"))]
        assert!(matches!(err, CapabilityError::Unavailable { .. }));
    }

    #[tokio::test]
    async fn probe_on_non_linux_is_unavailable() {
        #[cfg(not(target_os = "linux"))]
        {
            let s = NativeSandbox::new("n", CapabilityTier::Light);
            let p = s.probe().await;
            assert!(matches!(p, CapabilityProbe::Unavailable { .. }));
        }
    }

    #[test]
    fn validate_plan_unavailable_on_non_linux() {
        #[cfg(not(target_os = "linux"))]
        {
            let s = NativeSandbox::new("n", CapabilityTier::Light);
            let plan =
                ports_fixtures::plan_from_capabilities(vec![domain_fixtures::cap_fs_read(&[
                    "/tmp",
                ])]);
            assert!(matches!(
                s.validate_plan(&plan),
                Err(CapabilityError::Unavailable { .. })
            ));
        }
    }

    #[test]
    fn shapes_strict_tier_includes_exec_and_net() {
        let s = NativeSandbox::new("n", CapabilityTier::Strict);
        let supported = s.supported_shapes();
        #[cfg(target_os = "linux")]
        {
            assert!(supported.contains(&CapabilityShape::ProcessExec));
            assert!(supported.contains(&CapabilityShape::NetworkHttp));
        }
        #[cfg(not(target_os = "linux"))]
        {
            assert!(supported.is_empty());
        }
    }
}
