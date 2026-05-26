//! Layer 3 pre-flight plan validation.
//!
//! [`validate_plan_against_adapter`] cross-checks every capability in a
//! [`tau_ports::SandboxPlan`] against the adapter's declared
//! `supported_shapes`. All validation errors are collected and returned at
//! once so callers see the complete picture in a single pass.

use tau_domain::Capability;
use tau_ports::{Sandbox, SandboxPlan};

/// A single capability-shape rejection produced by
/// [`validate_plan_against_adapter`].
///
/// Carries enough context for callers to format a user-facing message
/// without further synthesis.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct SandboxValidationError {
    /// Identifier the caller passed in (typically a plugin id).
    pub plan_id: String,
    /// The capability that produced the error.
    pub capability: Capability,
    /// Human-readable reason — typically the shape rejected and what
    /// the adapter supports. Concrete enough to render directly to the
    /// user without further synthesis.
    pub reason: String,
}

impl SandboxValidationError {
    /// Construct a [`SandboxValidationError`].
    ///
    /// `#[non_exhaustive]` blocks struct-literal construction outside this
    /// crate; use this constructor instead.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_domain::Capability;
    /// use tau_runtime::sandbox::SandboxValidationError;
    ///
    /// let cap: Capability = serde_json::from_value(serde_json::json!({"kind": "fs.read", "paths": ["/tmp/**"]})).unwrap();
    /// let err = SandboxValidationError::new("my-plugin", cap, "shape not supported");
    /// assert_eq!(err.plan_id, "my-plugin");
    /// assert!(err.to_string().contains("my-plugin"));
    /// assert!(err.to_string().contains("shape not supported"));
    /// ```
    pub fn new(
        plan_id: impl Into<String>,
        capability: Capability,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            plan_id: plan_id.into(),
            capability,
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for SandboxValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "plan {}: {}", self.plan_id, self.reason)
    }
}

impl std::error::Error for SandboxValidationError {}

/// Cross-check every capability in `plan` against `adapter.supported_shapes()`.
///
/// **All** unsupported capabilities are collected before returning — the error
/// `Vec` may contain more than one entry. This is the key value of Layer 3:
/// a single `tau check` run surfaces every problem at once.
///
/// # Arguments
///
/// * `plan_id` — free-form identifier carried into each [`SandboxValidationError`]
///   so callers can format messages like `"plugin foo: capability fs.read shape
///   unsupported by adapter X"`. Typically a plugin id.
/// * `plan` — the [`SandboxPlan`] to validate.
/// * `adapter` — any [`Sandbox`] implementor (e.g. `SandboxAdapter` from the
///   chain, `MockSandbox` from fixtures, or another test double).
///
/// # Returns
///
/// `Ok(())` if every capability shape in `plan` is in `adapter.supported_shapes()`.
/// `Err(errors)` with the complete list of failures otherwise.
///
/// # Example
///
/// ```
/// use tau_domain::Capability;
/// use tau_ports::{SandboxPlan, fixtures::MockSandbox};
/// use tau_runtime::sandbox::validate_plan_against_adapter;
///
/// let adapter = MockSandbox::new("mock");
///
/// // A standard fs.read capability is supported by MockSandbox.
/// let cap: Capability = serde_json::from_value(
///     serde_json::json!({"kind": "fs.read", "paths": ["/tmp/**"]})
/// ).unwrap();
/// let plan = SandboxPlan::new(vec![cap], None, None);
/// assert!(validate_plan_against_adapter("my-plugin", &plan, &adapter).is_ok());
///
/// // An empty plan always passes.
/// let empty = SandboxPlan::new(vec![], None, None);
/// assert!(validate_plan_against_adapter("my-plugin", &empty, &adapter).is_ok());
/// ```
pub fn validate_plan_against_adapter<S: Sandbox>(
    plan_id: &str,
    plan: &SandboxPlan,
    adapter: &S,
) -> Result<(), Vec<SandboxValidationError>> {
    let supported = adapter.supported_shapes();

    let sup_list: String = supported
        .iter()
        .map(|s| format!("{s:?}"))
        .collect::<Vec<_>>()
        .join(", ");

    let mut errors: Vec<SandboxValidationError> = Vec::new();

    for cap in &plan.capabilities {
        let required = cap.required_shape();
        if !supported.contains(&required) {
            let reason =
                format!("adapter does not support shape {required:?} (supported: {sup_list})");
            errors.push(SandboxValidationError::new(plan_id, cap.clone(), reason));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_domain::CapabilityShape;
    use tau_ports::fixtures::MockSandbox;

    fn cap(json: &str) -> Capability {
        serde_json::from_str(json).expect("test capability JSON must be valid")
    }

    fn plan_with(caps: Vec<Capability>) -> SandboxPlan {
        SandboxPlan::new(caps, None, None)
    }

    #[test]
    fn validation_passes_when_all_shapes_supported() {
        // MockSandbox supports the 5 standard shapes (not Custom).
        let adapter = MockSandbox::new("mock");
        let plan = plan_with(vec![cap(r#"{"kind":"fs.read","paths":["/tmp/**"]}"#)]);
        assert!(
            validate_plan_against_adapter("test-plugin", &plan, &adapter).is_ok(),
            "expected Ok for fs.read on MockSandbox"
        );
    }

    #[test]
    fn validation_returns_all_errors_not_just_first() {
        // Two Custom capabilities — MockSandbox does not support CapabilityShape::Custom.
        let adapter = MockSandbox::new("mock");
        let plan = plan_with(vec![
            cap(r#"{"kind":"mcp.tool.use","tool":"x"}"#),
            cap(r#"{"kind":"mcp.tool.other","tool":"y"}"#),
        ]);
        let errors = validate_plan_against_adapter("test-plugin", &plan, &adapter)
            .expect_err("expected validation errors for Custom capabilities");
        assert_eq!(
            errors.len(),
            2,
            "both custom capabilities should produce errors, got {:?}",
            errors.len()
        );
        for e in &errors {
            assert_eq!(e.plan_id, "test-plugin");
        }
    }

    #[test]
    fn validation_includes_plan_id_in_each_error() {
        let adapter = MockSandbox::new("mock");
        let plan = plan_with(vec![cap(r#"{"kind":"mcp.tool.use","tool":"x"}"#)]);
        let errors = validate_plan_against_adapter("test-plugin", &plan, &adapter)
            .expect_err("expected error for Custom capability");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].plan_id, "test-plugin");
    }

    #[test]
    fn validation_empty_plan_is_ok() {
        let adapter = MockSandbox::new("mock");
        let plan = plan_with(vec![]);
        assert!(
            validate_plan_against_adapter("test-plugin", &plan, &adapter).is_ok(),
            "empty plan should always pass"
        );
    }

    #[test]
    fn sandbox_validation_error_display_includes_id_and_reason() {
        let cap_val = cap(r#"{"kind":"mcp.tool.use","tool":"x"}"#);
        let e = SandboxValidationError::new(
            "my-plugin",
            cap_val,
            "adapter does not support shape Custom { name: \"mcp.tool.use\" } (supported: FilesystemRead)",
        );
        let display = format!("{e}");
        assert!(
            display.contains("my-plugin"),
            "display must contain plan_id; got: {display}"
        );
        assert!(
            display.contains("adapter does not support shape"),
            "display must contain the reason text; got: {display}"
        );
        // Verify shape info is in the output too.
        assert!(
            display.contains("Custom"),
            "display must include shape info; got: {display}"
        );
        // Also verify the capability's required_shape is Custom (sanity check on the fixture).
        let shape = e.capability.required_shape();
        assert!(
            matches!(shape, CapabilityShape::Custom { .. }),
            "shape must be Custom"
        );
    }
}
