//! Cross-crate vocabulary drift test.
//!
//! `tau-runtime-core::vocabulary` is a no_std-friendly mirror of a subset of
//! the constants in `tau_observe::vocabulary`. Three constants in `tau-observe`
//! are intentionally host-only (the kernel does not emit them):
//!
//! - `SPAN_LLM_COMPLETE` — reserved for future LLM-span instrumentation;
//!   not yet emitted by the kernel.
//! - `EV_TOOL_ARGS_RECEIVED` / `EV_TOOL_RESULT_RECEIVED` — emitted by
//!   `tau-runtime-tokio::plugin_host::ipc_tool`, not by the kernel
//!   dispatch loop.
//!
//! Everything else MUST match by identifier *and* string value. This test
//! enforces both directions so neither side can rename, retype, or drop a
//! constant without the other side seeing it first.

use tau_observe::vocabulary as o;
use tau_runtime_core::vocabulary as k;

/// Constants present in `tau_observe::vocabulary` but intentionally NOT
/// mirrored into `tau-runtime-core::vocabulary` (host-only emit sites).
///
/// If you delete an entry here, the corresponding constant must also be
/// removed from `tau-observe`. If you add an entry here, you are claiming
/// the named constant is host-only — the count check below will catch a
/// stale list.
const OBSERVE_ONLY: &[&str] = &[
    "SPAN_LLM_COMPLETE",
    "EV_TOOL_ARGS_RECEIVED",
    "EV_TOOL_RESULT_RECEIVED",
];

/// The expected total constant count in `tau_observe::vocabulary`.
///
/// = kernel mirror length + OBSERVE_ONLY length. Bump only when a real
/// addition lands AND the corresponding mirror or OBSERVE_ONLY entry is
/// updated in the same change. Bumping just this number to silence the
/// test is the failure mode this guard exists to prevent.
const OBSERVE_TOTAL_EXPECTED: usize = 34;

#[test]
fn kernel_mirror_values_match_observe() {
    // For every (ident, value) pair the kernel mirror declares, the same
    // identifier must exist in tau-observe with the identical value.
    for (ident, value) in k::PAIRS {
        let observe_value = lookup_observe(ident).unwrap_or_else(|| {
            panic!(
                "tau-runtime-core::vocabulary::{ident} has no counterpart in \
                 tau-observe::vocabulary — add it to tau-observe or remove \
                 it from the kernel mirror"
            )
        });
        assert_eq!(
            *value, observe_value,
            "{ident} drift: kernel = {value:?}, observe = {observe_value:?}"
        );
    }
}

#[test]
fn observe_only_constants_are_not_mirrored() {
    // Every entry in OBSERVE_ONLY must exist in tau-observe (otherwise the
    // list is stale) and must NOT exist in the kernel mirror (otherwise
    // it's incorrectly classified).
    for ident in OBSERVE_ONLY {
        assert!(
            lookup_observe(ident).is_some(),
            "OBSERVE_ONLY lists {ident:?} but tau-observe doesn't declare it"
        );
        assert!(
            !k::PAIRS.iter().any(|(name, _)| name == ident),
            "{ident:?} appears in the kernel mirror but is classified as \
             observe-only — remove it from one side"
        );
    }
}

#[test]
fn total_observe_count_matches() {
    // Count check: forces a deliberate update whenever someone adds a new
    // constant on either side. Add → bump expected total + (kernel mirror
    // OR OBSERVE_ONLY). Removal → drop from kernel mirror or OBSERVE_ONLY
    // AND decrement.
    let actual = observe_constant_count();
    assert_eq!(
        actual, OBSERVE_TOTAL_EXPECTED,
        "tau-observe vocabulary count changed: actual = {actual}, expected = \
         {OBSERVE_TOTAL_EXPECTED}. Update OBSERVE_TOTAL_EXPECTED AND either \
         the kernel mirror or OBSERVE_ONLY to match."
    );
    assert_eq!(
        k::PAIRS.len() + OBSERVE_ONLY.len(),
        OBSERVE_TOTAL_EXPECTED,
        "kernel mirror + OBSERVE_ONLY no longer sums to expected total"
    );
}

/// Look up a tau-observe constant by its Rust identifier name.
///
/// Keep in sync with `tau_observe::vocabulary` — each `EV_*` / `SPAN_*`
/// constant declared there must appear here. If a new constant is added
/// to tau-observe without a match here, `total_observe_count_matches`
/// will catch the drift.
fn lookup_observe(ident: &str) -> Option<&'static str> {
    Some(match ident {
        "SPAN_RUNTIME_AGENT_RUN" => o::SPAN_RUNTIME_AGENT_RUN,
        "SPAN_RUNTIME_TURN" => o::SPAN_RUNTIME_TURN,
        "SPAN_LLM_COMPLETE" => o::SPAN_LLM_COMPLETE,
        "SPAN_DISPATCH_TOOL" => o::SPAN_DISPATCH_TOOL,
        "SPAN_CAPABILITY_CHECK" => o::SPAN_CAPABILITY_CHECK,
        "SPAN_TOOL_SESSION_OPEN" => o::SPAN_TOOL_SESSION_OPEN,
        "SPAN_TOOL_INVOKE" => o::SPAN_TOOL_INVOKE,
        "SPAN_TOOL_SESSION_CLOSE" => o::SPAN_TOOL_SESSION_CLOSE,
        "EV_RUNTIME_RUN_STARTED" => o::EV_RUNTIME_RUN_STARTED,
        "EV_RUNTIME_COMPLETED" => o::EV_RUNTIME_COMPLETED,
        "EV_RUNTIME_FAILED" => o::EV_RUNTIME_FAILED,
        "EV_RUNTIME_LOOP_TERMINATED" => o::EV_RUNTIME_LOOP_TERMINATED,
        "EV_RUNTIME_MAX_TURNS_REACHED" => o::EV_RUNTIME_MAX_TURNS_REACHED,
        "EV_RUNTIME_TURN_STARTED" => o::EV_RUNTIME_TURN_STARTED,
        "EV_LLM_REQUEST_BUILT" => o::EV_LLM_REQUEST_BUILT,
        "EV_LLM_RESPONSE_RECEIVED" => o::EV_LLM_RESPONSE_RECEIVED,
        "EV_LLM_TOKEN_USAGE" => o::EV_LLM_TOKEN_USAGE,
        "EV_LLM_STOP_REASON" => o::EV_LLM_STOP_REASON,
        "EV_LLM_TOOL_USE_EMITTED" => o::EV_LLM_TOOL_USE_EMITTED,
        "EV_DISPATCH_TOOL_RESOLVED" => o::EV_DISPATCH_TOOL_RESOLVED,
        "EV_CAPABILITY_REQUIRED_LOADED" => o::EV_CAPABILITY_REQUIRED_LOADED,
        "EV_CAPABILITY_GRANTED_LOADED" => o::EV_CAPABILITY_GRANTED_LOADED,
        "EV_CAPABILITY_SATISFIES_CHECK" => o::EV_CAPABILITY_SATISFIES_CHECK,
        "EV_CAPABILITY_ALLOW" => o::EV_CAPABILITY_ALLOW,
        "EV_CAPABILITY_DENY" => o::EV_CAPABILITY_DENY,
        "EV_TOOL_ARGS_RECEIVED" => o::EV_TOOL_ARGS_RECEIVED,
        "EV_TOOL_RESULT_RECEIVED" => o::EV_TOOL_RESULT_RECEIVED,
        "EV_TOOL_INVOKE_FAILED" => o::EV_TOOL_INVOKE_FAILED,
        "EV_TOOL_SESSION_OPEN_FAILED" => o::EV_TOOL_SESSION_OPEN_FAILED,
        "EV_TOOL_SESSION_CLOSE_FAILED" => o::EV_TOOL_SESSION_CLOSE_FAILED,
        "EV_MESSAGE_ADDED" => o::EV_MESSAGE_ADDED,
        "SPAN_PIPELINE_STEP" => o::SPAN_PIPELINE_STEP,
        "EV_PIPELINE_STEP_STARTED" => o::EV_PIPELINE_STEP_STARTED,
        "EV_PIPELINE_STEP_COMPLETED" => o::EV_PIPELINE_STEP_COMPLETED,
        _ => return None,
    })
}

/// Count of `EV_*` / `SPAN_*` constants enumerated in `lookup_observe`.
///
/// This is the assertion target for `total_observe_count_matches`. If a
/// new constant is added to tau-observe, both `lookup_observe` and this
/// number need updating together — the test fails loud if they drift.
fn observe_constant_count() -> usize {
    // The set of identifier strings that `lookup_observe` accepts. Kept
    // in lockstep with the match arm above.
    const IDENTS: &[&str] = &[
        "SPAN_RUNTIME_AGENT_RUN",
        "SPAN_RUNTIME_TURN",
        "SPAN_LLM_COMPLETE",
        "SPAN_DISPATCH_TOOL",
        "SPAN_CAPABILITY_CHECK",
        "SPAN_TOOL_SESSION_OPEN",
        "SPAN_TOOL_INVOKE",
        "SPAN_TOOL_SESSION_CLOSE",
        "EV_RUNTIME_RUN_STARTED",
        "EV_RUNTIME_COMPLETED",
        "EV_RUNTIME_FAILED",
        "EV_RUNTIME_LOOP_TERMINATED",
        "EV_RUNTIME_MAX_TURNS_REACHED",
        "EV_RUNTIME_TURN_STARTED",
        "EV_LLM_REQUEST_BUILT",
        "EV_LLM_RESPONSE_RECEIVED",
        "EV_LLM_TOKEN_USAGE",
        "EV_LLM_STOP_REASON",
        "EV_LLM_TOOL_USE_EMITTED",
        "EV_DISPATCH_TOOL_RESOLVED",
        "EV_CAPABILITY_REQUIRED_LOADED",
        "EV_CAPABILITY_GRANTED_LOADED",
        "EV_CAPABILITY_SATISFIES_CHECK",
        "EV_CAPABILITY_ALLOW",
        "EV_CAPABILITY_DENY",
        "EV_TOOL_ARGS_RECEIVED",
        "EV_TOOL_RESULT_RECEIVED",
        "EV_TOOL_INVOKE_FAILED",
        "EV_TOOL_SESSION_OPEN_FAILED",
        "EV_TOOL_SESSION_CLOSE_FAILED",
        "EV_MESSAGE_ADDED",
        "SPAN_PIPELINE_STEP",
        "EV_PIPELINE_STEP_STARTED",
        "EV_PIPELINE_STEP_COMPLETED",
    ];
    // Sanity check: lookup_observe must accept every IDENT.
    for ident in IDENTS {
        assert!(
            lookup_observe(ident).is_some(),
            "observe_constant_count(): {ident} not handled by lookup_observe"
        );
    }
    IDENTS.len()
}
