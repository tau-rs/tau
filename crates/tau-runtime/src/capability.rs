// Re-export pure capability logic from the executor-agnostic kernel.
//
// As of β.1.3.5c the tracing-wrapped `check_capabilities_for_tool` also
// lives in core (since `stream.rs` moved there). tau-runtime keeps the
// thin re-export so legacy `crate::capability::*` paths continue to work.

#[allow(unused_imports)]
pub(crate) use tau_runtime_core::capability::{
    capability_kind_str, check_capabilities, check_capabilities_for_tool,
};

#[cfg(test)]
mod tests {
    #[test]
    fn capability_check_span_name_literal_matches_vocabulary_constant() {
        // The `capability.check` span name is asserted to match the
        // vocabulary constant — kept here for legacy import-path
        // discoverability.
        assert_eq!(
            tau_observe::vocabulary::SPAN_CAPABILITY_CHECK,
            "capability.check"
        );
    }
}
