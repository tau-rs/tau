//! Layer 3 pre-flight plan construction.
//!
//! [`build_plan`] assembles a [`tau_ports::CapabilityPlan`] from per-plugin
//! manifest capabilities, project-level overrides, and execution-context
//! hints. The returned plan is ready to be cross-checked against an
//! adapter's `supported_shapes` via
//! [`crate::sandbox::validate_plan_against_adapter`].

use tau_domain::Capability;
use tau_ports::{ResourceLimits, CapabilityPlan, WorkingContext};

use crate::capability_override::{CapabilityOverride, OverrideExpandError};

/// Assemble a [`CapabilityPlan`] from manifest capabilities + project overrides.
///
/// Steps:
/// 1. Calls [`crate::capability_override::compute_effective`] to intersect
///    `package_caps` with `project_override`, propagating any
///    [`OverrideExpandError`] to the caller.
/// 2. Maps each [`crate::capability_override::EffectiveCapability`] to its `source` [`Capability`] (the
///    package-declared shape — override narrowing is enforcement-side, not
///    shape-relevant).
/// 3. Constructs and returns a [`CapabilityPlan`] with the resulting capability
///    list, `working_context`, and `limits` threaded through unchanged.
///
/// # Example
///
/// ```
/// use tau_domain::Capability;
/// use tau_runtime::sandbox::build_plan;
///
/// let cap: Capability = serde_json::from_str(r#"{"kind":"fs.read","paths":["/data/**"]}"#)
///     .expect("valid capability JSON");
/// let plan = build_plan(&[cap], &[], None, None).expect("build_plan succeeds with no override");
/// assert_eq!(plan.capabilities.len(), 1);
/// assert!(plan.context.is_none());
/// assert!(plan.limits.is_none());
/// ```
pub fn build_plan(
    package_caps: &[Capability],
    project_override: &[CapabilityOverride],
    working_context: Option<WorkingContext>,
    limits: Option<ResourceLimits>,
) -> Result<CapabilityPlan, OverrideExpandError> {
    let effective = crate::capability_override::compute_effective(package_caps, project_override)?;

    let capabilities: Vec<Capability> = effective.into_iter().map(|ec| ec.source).collect();

    Ok(CapabilityPlan::new(capabilities, working_context, limits))
}

#[cfg(test)]
mod tests {
    use super::*;

    use tau_ports::fixtures as ports_fixtures;

    fn cap(json: &str) -> Capability {
        serde_json::from_str(json).expect("test capability JSON must be valid")
    }

    #[test]
    fn build_plan_includes_capability_shapes() {
        let package_caps = vec![
            cap(r#"{"kind":"fs.read","paths":["/proj/**"]}"#),
            cap(r#"{"kind":"net.http","hosts":["api.example.com"],"methods":["GET"]}"#),
        ];
        let plan = build_plan(&package_caps, &[], None, None).unwrap();
        assert_eq!(plan.capabilities.len(), 2);
        use tau_domain::CapabilityShape;
        assert_eq!(
            plan.capabilities[0].required_shape(),
            CapabilityShape::FilesystemRead
        );
        assert_eq!(
            plan.capabilities[1].required_shape(),
            CapabilityShape::NetworkHttp
        );
    }

    #[test]
    fn build_plan_propagates_override_error() {
        // Override that targets a kind not present in package_caps → OverrideExpandError.
        let package_caps = vec![cap(r#"{"kind":"fs.read","paths":["/proj/**"]}"#)];
        let bad_override = vec![CapabilityOverride::new(
            "fs.write".into(),
            Some(vec!["/proj/**".into()]),
            vec![],
            None,
        )];
        let result = build_plan(&package_caps, &bad_override, None, None);
        assert!(result.is_err(), "expected OverrideExpandError, got Ok");
    }

    #[test]
    fn build_plan_passes_through_context_and_limits() {
        use std::path::PathBuf;
        let ctx = ports_fixtures::working_context("/workspace", Default::default());
        let lim = ports_fixtures::resource_limits(Some(268_435_456), Some(10));
        let plan = build_plan(&[], &[], Some(ctx), Some(lim)).unwrap();
        assert!(plan.context.is_some());
        assert_eq!(
            plan.context.as_ref().unwrap().working_dir,
            Some(PathBuf::from("/workspace"))
        );
        assert!(plan.limits.is_some());
        assert_eq!(plan.limits.unwrap().memory_bytes, Some(268_435_456));
    }

    #[test]
    fn build_plan_with_empty_capabilities_yields_empty_plan() {
        let plan = build_plan(&[], &[], None, None).unwrap();
        assert!(plan.capabilities.is_empty());
        assert!(plan.context.is_none());
        assert!(plan.limits.is_none());
    }
}
