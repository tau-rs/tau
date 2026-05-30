//! Map [`tau_domain::CapabilityShape`] onto the set this adapter supports
//! at a given tier.

use tau_domain::{CapabilityShape, CapabilityShapeSet};
use tau_ports::CapabilityTier;

/// Capability shapes this adapter can enforce at the given tier.
// Called only on Linux; suppress dead_code lint on other platforms.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn shapes_for_tier(tier: CapabilityTier) -> CapabilityShapeSet {
    let mut set = CapabilityShapeSet::new();
    match tier {
        CapabilityTier::None => {}
        CapabilityTier::Light => {
            // Light tier: filesystem isolation only.
            set.insert(CapabilityShape::FilesystemRead);
            set.insert(CapabilityShape::FilesystemWrite);
        }
        CapabilityTier::Strict => {
            // Strict tier (Tasks 4-5): adds exec gating + network egress.
            set.insert(CapabilityShape::FilesystemRead);
            set.insert(CapabilityShape::FilesystemWrite);
            set.insert(CapabilityShape::ProcessExec);
            set.insert(CapabilityShape::NetworkHttp);
        }
        other => {
            tracing::warn!(?other, "unknown SandboxTier — returning empty shape set");
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_tier_yields_empty_set() {
        let set = shapes_for_tier(CapabilityTier::None);
        assert!(set.is_empty());
    }

    #[test]
    fn light_tier_includes_filesystem_only() {
        let set = shapes_for_tier(CapabilityTier::Light);
        assert!(set.contains(&CapabilityShape::FilesystemRead));
        assert!(set.contains(&CapabilityShape::FilesystemWrite));
        assert!(!set.contains(&CapabilityShape::ProcessExec));
        assert!(!set.contains(&CapabilityShape::NetworkHttp));
        assert!(!set.contains(&CapabilityShape::AgentSpawn));
    }

    #[test]
    fn strict_tier_extends_light_with_exec_and_network() {
        let set = shapes_for_tier(CapabilityTier::Strict);
        // Strict is a strict superset of Light.
        let light = shapes_for_tier(CapabilityTier::Light);
        assert!(light.is_subset_of(&set));
        // Plus exec + network.
        assert!(set.contains(&CapabilityShape::ProcessExec));
        assert!(set.contains(&CapabilityShape::NetworkHttp));
        // But still no Agent or Custom — those are not enforced by this
        // adapter at any tier.
        assert!(!set.contains(&CapabilityShape::AgentSpawn));
    }

    #[test]
    fn light_is_proper_subset_of_strict() {
        let light = shapes_for_tier(CapabilityTier::Light);
        let strict = shapes_for_tier(CapabilityTier::Strict);
        assert!(light.is_subset_of(&strict));
        assert!(!strict.is_subset_of(&light));
    }
}
