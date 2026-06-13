// Some constants are only used when `tool-validation` is enabled (stream.rs).
// Suppress dead-code warnings under the no-default-features build.
#![allow(dead_code)]

//! Fixed `&'static str` names for every span and event in ADR-0006 §3.9.
//!
//! Mirror of `tau_observe::vocabulary` — kept in `tau-runtime-core` so the
//! executor-agnostic kernel does not depend on `tau-observe` (which uses
//! tokio + tracing-subscriber and is std-only). The host shell's
//! `tau_observe::vocabulary` remains the canonical source of truth.
//!
//! These constants are `#[doc(hidden)] pub` (not the usual `pub(crate)`) so
//! the cross-crate drift test in `tau-runtime-tokio` can cross-reference
//! every name + value. Treat them as crate-private for normal use.

// --- Spans (§3.9) ---

#[doc(hidden)]
pub const SPAN_RUNTIME_AGENT_RUN: &str = "runtime.agent_run";
#[doc(hidden)]
pub const SPAN_RUNTIME_TURN: &str = "runtime.turn";
#[doc(hidden)]
pub const SPAN_DISPATCH_TOOL: &str = "dispatch.tool";
#[doc(hidden)]
pub const SPAN_CAPABILITY_CHECK: &str = "capability.check";
#[doc(hidden)]
pub const SPAN_TOOL_SESSION_OPEN: &str = "tool.session_open";
#[doc(hidden)]
pub const SPAN_TOOL_INVOKE: &str = "tool.invoke";
#[doc(hidden)]
pub const SPAN_TOOL_SESSION_CLOSE: &str = "tool.session_close";

// --- Runtime events ---

#[doc(hidden)]
pub const EV_RUNTIME_RUN_STARTED: &str = "runtime.run_started";
#[doc(hidden)]
pub const EV_RUNTIME_COMPLETED: &str = "runtime.completed";
#[doc(hidden)]
pub const EV_RUNTIME_FAILED: &str = "runtime.failed";
#[doc(hidden)]
pub const EV_RUNTIME_LOOP_TERMINATED: &str = "runtime.loop_terminated";
#[doc(hidden)]
pub const EV_RUNTIME_MAX_TURNS_REACHED: &str = "runtime.max_turns_reached";
#[doc(hidden)]
pub const EV_RUNTIME_TURN_STARTED: &str = "runtime.turn_started";

// --- LLM events ---

#[doc(hidden)]
pub const EV_LLM_REQUEST_BUILT: &str = "llm.request_built";
#[doc(hidden)]
pub const EV_LLM_RESPONSE_RECEIVED: &str = "llm.response_received";
#[doc(hidden)]
pub const EV_LLM_TOKEN_USAGE: &str = "llm.token_usage";
#[doc(hidden)]
pub const EV_LLM_STOP_REASON: &str = "llm.stop_reason";
#[doc(hidden)]
pub const EV_LLM_TOOL_USE_EMITTED: &str = "llm.tool_use_emitted";

// --- Dispatch events ---

#[doc(hidden)]
pub const EV_DISPATCH_TOOL_RESOLVED: &str = "dispatch.tool_resolved";

// --- Capability events ---

#[doc(hidden)]
pub const EV_CAPABILITY_REQUIRED_LOADED: &str = "capability.required_loaded";
#[doc(hidden)]
pub const EV_CAPABILITY_GRANTED_LOADED: &str = "capability.granted_loaded";
#[doc(hidden)]
pub const EV_CAPABILITY_SATISFIES_CHECK: &str = "capability.satisfies_check";
#[doc(hidden)]
pub const EV_CAPABILITY_ALLOW: &str = "capability.allow";
#[doc(hidden)]
pub const EV_CAPABILITY_DENY: &str = "capability.deny";

// --- Tool events ---

#[doc(hidden)]
pub const EV_TOOL_INVOKE_FAILED: &str = "tool.invoke_failed";
#[doc(hidden)]
pub const EV_TOOL_SESSION_OPEN_FAILED: &str = "tool.session_open_failed";
#[doc(hidden)]
pub const EV_TOOL_SESSION_CLOSE_FAILED: &str = "tool.session_close_failed";

// --- Message events ---

#[doc(hidden)]
pub const EV_MESSAGE_ADDED: &str = "message.added";

// --- Pipeline events ---

/// Span wrapping one pipeline step's execution.
#[doc(hidden)]
pub const SPAN_PIPELINE_STEP: &str = "pipeline.step";
/// Emitted when a pipeline step begins.
#[doc(hidden)]
pub const EV_PIPELINE_STEP_STARTED: &str = "pipeline.step_started";
/// Emitted when a pipeline step completes successfully.
#[doc(hidden)]
pub const EV_PIPELINE_STEP_COMPLETED: &str = "pipeline.step_completed";

// --- Check events and built-in goal predicate fn names ---

/// Span wrapping a single check evaluation.
#[doc(hidden)]
pub const SPAN_PIPELINE_CHECK: &str = "pipeline.check";
/// Event: a check produced a verdict.
#[doc(hidden)]
pub const EV_CHECK_EVALUATED: &str = "check.evaluated";
/// Event: a failed check rewound to its gate.
#[doc(hidden)]
pub const EV_CHECK_RETRY: &str = "check.retry";

/// Built-in goal predicate fn name: locus is present (non-absent).
#[doc(hidden)]
pub const FN_BUILTIN_EXISTS: &str = "__tau::goal::exists";
/// Built-in goal predicate fn name: locus is present and non-empty.
#[doc(hidden)]
pub const FN_BUILTIN_NON_EMPTY: &str = "__tau::goal::non_empty";
/// Built-in goal predicate fn name: content equals a given string.
#[doc(hidden)]
pub const FN_BUILTIN_EQUALS: &str = "__tau::goal::equals";
/// Built-in goal predicate fn name: content matches a regex pattern.
#[doc(hidden)]
pub const FN_BUILTIN_MATCHES: &str = "__tau::goal::matches";
/// Built-in goal predicate fn name: content item count meets a minimum.
#[doc(hidden)]
pub const FN_BUILTIN_MIN_COUNT: &str = "__tau::goal::min_count";
/// Built-in goal predicate fn name: content is valid per a JSON schema.
#[doc(hidden)]
pub const FN_BUILTIN_SCHEMA_VALID: &str = "__tau::goal::schema_valid";

/// All `(identifier, value)` pairs in this mirror, in declaration order.
///
/// Consumed by the cross-crate drift test in `tau-runtime-tokio`. The
/// identifier strings are the Rust constant names (used to look up the
/// matching constant in `tau_observe::vocabulary`); the value strings are
/// the runtime span/event names. Doc-hidden because this is an internal
/// integration-test surface, not a stability commitment.
///
/// Note: the `FN_BUILTIN_*` constants are built-in goal predicate registry
/// names, not tracing vocabulary — they are intentionally excluded from
/// `PAIRS` and are not mirrored in `tau_observe::vocabulary`.
#[doc(hidden)]
pub const PAIRS: &[(&str, &str)] = &[
    ("SPAN_RUNTIME_AGENT_RUN", SPAN_RUNTIME_AGENT_RUN),
    ("SPAN_RUNTIME_TURN", SPAN_RUNTIME_TURN),
    ("SPAN_DISPATCH_TOOL", SPAN_DISPATCH_TOOL),
    ("SPAN_CAPABILITY_CHECK", SPAN_CAPABILITY_CHECK),
    ("SPAN_TOOL_SESSION_OPEN", SPAN_TOOL_SESSION_OPEN),
    ("SPAN_TOOL_INVOKE", SPAN_TOOL_INVOKE),
    ("SPAN_TOOL_SESSION_CLOSE", SPAN_TOOL_SESSION_CLOSE),
    ("EV_RUNTIME_RUN_STARTED", EV_RUNTIME_RUN_STARTED),
    ("EV_RUNTIME_COMPLETED", EV_RUNTIME_COMPLETED),
    ("EV_RUNTIME_FAILED", EV_RUNTIME_FAILED),
    ("EV_RUNTIME_LOOP_TERMINATED", EV_RUNTIME_LOOP_TERMINATED),
    ("EV_RUNTIME_MAX_TURNS_REACHED", EV_RUNTIME_MAX_TURNS_REACHED),
    ("EV_RUNTIME_TURN_STARTED", EV_RUNTIME_TURN_STARTED),
    ("EV_LLM_REQUEST_BUILT", EV_LLM_REQUEST_BUILT),
    ("EV_LLM_RESPONSE_RECEIVED", EV_LLM_RESPONSE_RECEIVED),
    ("EV_LLM_TOKEN_USAGE", EV_LLM_TOKEN_USAGE),
    ("EV_LLM_STOP_REASON", EV_LLM_STOP_REASON),
    ("EV_LLM_TOOL_USE_EMITTED", EV_LLM_TOOL_USE_EMITTED),
    ("EV_DISPATCH_TOOL_RESOLVED", EV_DISPATCH_TOOL_RESOLVED),
    (
        "EV_CAPABILITY_REQUIRED_LOADED",
        EV_CAPABILITY_REQUIRED_LOADED,
    ),
    ("EV_CAPABILITY_GRANTED_LOADED", EV_CAPABILITY_GRANTED_LOADED),
    (
        "EV_CAPABILITY_SATISFIES_CHECK",
        EV_CAPABILITY_SATISFIES_CHECK,
    ),
    ("EV_CAPABILITY_ALLOW", EV_CAPABILITY_ALLOW),
    ("EV_CAPABILITY_DENY", EV_CAPABILITY_DENY),
    ("EV_TOOL_INVOKE_FAILED", EV_TOOL_INVOKE_FAILED),
    ("EV_TOOL_SESSION_OPEN_FAILED", EV_TOOL_SESSION_OPEN_FAILED),
    ("EV_TOOL_SESSION_CLOSE_FAILED", EV_TOOL_SESSION_CLOSE_FAILED),
    ("EV_MESSAGE_ADDED", EV_MESSAGE_ADDED),
    ("SPAN_PIPELINE_STEP", SPAN_PIPELINE_STEP),
    ("EV_PIPELINE_STEP_STARTED", EV_PIPELINE_STEP_STARTED),
    ("EV_PIPELINE_STEP_COMPLETED", EV_PIPELINE_STEP_COMPLETED),
    ("SPAN_PIPELINE_CHECK", SPAN_PIPELINE_CHECK),
    ("EV_CHECK_EVALUATED", EV_CHECK_EVALUATED),
    ("EV_CHECK_RETRY", EV_CHECK_RETRY),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_span_and_event_constants_equal_their_literals() {
        assert_eq!(SPAN_PIPELINE_CHECK, "pipeline.check");
        assert_eq!(EV_CHECK_EVALUATED, "check.evaluated");
        assert_eq!(EV_CHECK_RETRY, "check.retry");
    }

    #[test]
    fn builtin_fn_name_constants_equal_their_literals() {
        assert_eq!(FN_BUILTIN_EXISTS, "__tau::goal::exists");
        assert_eq!(FN_BUILTIN_NON_EMPTY, "__tau::goal::non_empty");
        assert_eq!(FN_BUILTIN_EQUALS, "__tau::goal::equals");
        assert_eq!(FN_BUILTIN_MATCHES, "__tau::goal::matches");
        assert_eq!(FN_BUILTIN_MIN_COUNT, "__tau::goal::min_count");
        assert_eq!(FN_BUILTIN_SCHEMA_VALID, "__tau::goal::schema_valid");
    }
}
