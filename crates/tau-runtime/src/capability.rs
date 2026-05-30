// Re-export pure capability logic from the executor-agnostic kernel.
// Only `check_capabilities` and `capability_kind_str` are used in
// non-test production code; the per-namespace `*_satisfies` helpers
// are consumed by tests only (via `use super::*`).
pub(crate) use tau_runtime_core::capability::{capability_kind_str, check_capabilities};

#[cfg(test)]
pub(crate) use tau_runtime_core::capability::{
    agent_satisfies, custom_params_satisfy, fs_satisfies, net_satisfies, plan_satisfies,
    process_satisfies, skill_satisfies, task_list_satisfies,
};

// Re-export capability_satisfies for tests and builder use.
#[cfg(test)]
pub(crate) use tau_runtime_core::capability::capability_satisfies;

use tau_domain::Capability;

/// Tool-dispatch wrapper around [`check_capabilities`] that owns the
/// ADR-0006 §3.9 `capability.check` span and the five capability
/// lifecycle events.
///
/// This wrapper exists because the pure satisfies-relation has no notion
/// of *which tool* is being dispatched, but the §3.9 vocabulary requires
/// `tool_name`-scoped diagnostics. The wrapper is used by the dispatch
/// sites in `stream.rs` and `run::invoke_tool`. The `builder.rs` startup
/// path that filters the LLM tool-spec list deliberately keeps using the
/// pure helper — it runs once per registered tool at build time and
/// emitting per-tool allow events there would pollute traces with N
/// events on every spawn.
///
/// `#[instrument]` inherits its parent from the *current* tracing span.
/// In the streaming pump the `dispatch.tool` span is never `.enter()`'d
/// (it straddles awaits), so call sites must wrap with
/// `dispatch_span.in_scope(|| check_capabilities_for_tool(...))` to nest
/// `capability.check` correctly. `run::invoke_tool` is itself wrapped
/// with `#[instrument]` and entered for its own body, so calling this
/// wrapper from there nests naturally without extra plumbing.
// `#[instrument(name = ...)]` requires a string literal, so the
// SPAN_CAPABILITY_CHECK constant cannot be used in the macro argument
// position. We pin the literal here and assert it matches the constant
// in a unit test below to prevent drift.
#[tracing::instrument(
    name = "capability.check",
    skip_all,
    fields(tool_name = %tool_name),
)]
pub(crate) fn check_capabilities_for_tool<'a>(
    tool_name: &str,
    granted: &[Capability],
    required: &'a [Capability],
) -> Option<&'a Capability> {
    use tau_observe::vocabulary as v;
    tracing::debug!(
        name = v::EV_CAPABILITY_REQUIRED_LOADED,
        required_count = required.len(),
    );
    tracing::debug!(
        name = v::EV_CAPABILITY_GRANTED_LOADED,
        granted_count = granted.len(),
    );
    let missing = check_capabilities(granted, required);
    tracing::debug!(
        name = v::EV_CAPABILITY_SATISFIES_CHECK,
        satisfied = missing.is_none(),
    );
    match missing {
        None => {
            tracing::info!(
                name = v::EV_CAPABILITY_ALLOW,
                tool_name = %tool_name,
            );
            None
        }
        Some(cap) => {
            let kind = capability_kind_str(cap);
            tracing::warn!(
                name = v::EV_CAPABILITY_DENY,
                tool_name = %tool_name,
                missing_kind = %kind,
            );
            Some(cap)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_check_span_name_literal_matches_vocabulary_constant() {
        // `#[instrument(name = ...)]` on `check_capabilities_for_tool`
        // requires a string literal, so the SPAN_CAPABILITY_CHECK
        // constant cannot be referenced directly. This guard prevents
        // the literal from drifting out of sync with the vocabulary.
        assert_eq!(
            tau_observe::vocabulary::SPAN_CAPABILITY_CHECK,
            "capability.check"
        );
    }
}
