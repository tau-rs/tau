//! Sandbox mismatch / configuration error tests.
//!
//! Cross-platform: uses MockCapabilityGate to deterministically test the
//! validation paths.

use tau_domain::Capability;
use tau_ports::fixtures::MockCapabilityGate;
use tau_runtime_tokio::process_gate::{build_plan, validate_plan_against_adapter};

fn plan_with_custom_capability() -> tau_ports::CapabilityPlan {
    let plan_json = serde_json::json!({
        "capabilities": [{ "kind": "weird.thing" }],
        "context": null,
        "limits": null,
    });
    serde_json::from_value(plan_json).expect("decode")
}

#[tokio::test]
async fn plugin_with_custom_capability_rejected_via_validate_plan_against_adapter() {
    let mock = MockCapabilityGate::new("mock");
    let plan = plan_with_custom_capability();
    let result = validate_plan_against_adapter("plug-x", &plan, &mock);
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert_eq!(errors.len(), 1, "exactly one error for one bad cap");
    assert!(
        errors[0].reason.contains("Custom"),
        "reason should mention Custom shape"
    );
}

#[tokio::test]
async fn plugin_with_supported_capability_accepted() {
    let mock = MockCapabilityGate::new("mock");
    let plan_json = serde_json::json!({
        "capabilities": [{
            "kind": "fs.read",
            "paths": ["/tmp"]
        }],
        "context": null,
        "limits": null,
    });
    let plan: tau_ports::CapabilityPlan = serde_json::from_value(plan_json).expect("decode");
    let result = validate_plan_against_adapter("plug-y", &plan, &mock);
    assert!(result.is_ok(), "fs.read should be accepted by mock");
}

#[tokio::test]
async fn build_plan_passes_through_to_validate_plan_against_adapter() {
    // Combined integration: build_plan + validate_plan_against_adapter
    // is the canonical Task 9/10 call sequence.
    let cap_json = serde_json::json!([{
        "kind": "fs.read",
        "paths": ["/tmp"]
    }]);
    let caps: Vec<Capability> = serde_json::from_value(cap_json).expect("decode caps");

    let plan = build_plan(&caps, &[], None, None).expect("build_plan");
    let mock = MockCapabilityGate::new("mock");
    assert!(validate_plan_against_adapter("plug-z", &plan, &mock).is_ok());
}
