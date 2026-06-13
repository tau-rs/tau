//! Fixed `&'static str` names for every span and event in ADR-0006 §3.9.
//!
//! Importing from this module instead of writing string literals keeps
//! the §3.9 vocabulary discoverable by `grep` and prevents drift when
//! sub-projects B/C wire the actual emit sites.

// --- Spans (§3.9) ---

/// Span around an entire agent run (`Runtime::run_with_history`).
pub const SPAN_RUNTIME_AGENT_RUN: &str = "runtime.agent_run";
/// Span around one turn of the agent loop.
pub const SPAN_RUNTIME_TURN: &str = "runtime.turn";
/// Span around an LLM completion call.
pub const SPAN_LLM_COMPLETE: &str = "llm.complete";
/// Span around the tool-dispatch decision and invocation.
pub const SPAN_DISPATCH_TOOL: &str = "dispatch.tool";
/// Span around a capability check before a tool invocation.
pub const SPAN_CAPABILITY_CHECK: &str = "capability.check";
/// Span around a tool plugin's `Open` request path.
pub const SPAN_TOOL_SESSION_OPEN: &str = "tool.session_open";
/// Span around a tool plugin's `Invoke` request path.
pub const SPAN_TOOL_INVOKE: &str = "tool.invoke";
/// Span around a tool plugin's `Close` request path.
pub const SPAN_TOOL_SESSION_CLOSE: &str = "tool.session_close";

// --- Runtime events ---

/// Emitted when a run begins.
pub const EV_RUNTIME_RUN_STARTED: &str = "runtime.run_started";
/// Emitted when a run terminates normally.
pub const EV_RUNTIME_COMPLETED: &str = "runtime.completed";
/// Emitted when a run terminates abnormally (status = Failed).
pub const EV_RUNTIME_FAILED: &str = "runtime.failed";
/// Emitted when the run loop exits without producing tool calls.
pub const EV_RUNTIME_LOOP_TERMINATED: &str = "runtime.loop_terminated";
/// Emitted when the run loop hits `RunOptions::max_turns`.
pub const EV_RUNTIME_MAX_TURNS_REACHED: &str = "runtime.max_turns_reached";
/// Emitted at the start of each turn inside the run loop.
pub const EV_RUNTIME_TURN_STARTED: &str = "runtime.turn_started";

// --- LLM events ---

/// Emitted after the kernel builds a `CompletionRequest` for the backend.
pub const EV_LLM_REQUEST_BUILT: &str = "llm.request_built";
/// Emitted after the backend returns a completion.
pub const EV_LLM_RESPONSE_RECEIVED: &str = "llm.response_received";
/// Emitted with the token-usage fields parsed from the response.
pub const EV_LLM_TOKEN_USAGE: &str = "llm.token_usage";
/// Emitted with the stop-reason field parsed from the response.
pub const EV_LLM_STOP_REASON: &str = "llm.stop_reason";
/// Emitted for each `ToolUse` block the LLM emitted on this turn.
pub const EV_LLM_TOOL_USE_EMITTED: &str = "llm.tool_use_emitted";

// --- Dispatch events ---

/// Emitted after the kernel resolves a `tool_use` to a registered plugin.
pub const EV_DISPATCH_TOOL_RESOLVED: &str = "dispatch.tool_resolved";

// --- Capability events ---

/// Emitted after the kernel loads the `required_capabilities` for a tool.
pub const EV_CAPABILITY_REQUIRED_LOADED: &str = "capability.required_loaded";
/// Emitted after the kernel loads the agent's `granted_capabilities`.
pub const EV_CAPABILITY_GRANTED_LOADED: &str = "capability.granted_loaded";
/// Emitted after the kernel computes the satisfies-check result.
pub const EV_CAPABILITY_SATISFIES_CHECK: &str = "capability.satisfies_check";
/// Emitted on the allow branch of the capability check.
pub const EV_CAPABILITY_ALLOW: &str = "capability.allow";
/// Emitted on the deny branch of the capability check.
pub const EV_CAPABILITY_DENY: &str = "capability.deny";

// --- Tool events ---

/// Emitted when the kernel forwards args to the tool plugin.
pub const EV_TOOL_ARGS_RECEIVED: &str = "tool.args_received";
/// Emitted when the kernel receives an Invoke response from the plugin.
pub const EV_TOOL_RESULT_RECEIVED: &str = "tool.result_received";
/// Emitted when the kernel observes a tool Invoke failure.
pub const EV_TOOL_INVOKE_FAILED: &str = "tool.invoke_failed";
/// Emitted when a tool's `Open` request returns an error.
pub const EV_TOOL_SESSION_OPEN_FAILED: &str = "tool.session_open_failed";
/// Emitted when a tool's `Close` request returns an error.
pub const EV_TOOL_SESSION_CLOSE_FAILED: &str = "tool.session_close_failed";

// --- Message events ---

/// Emitted when a message is appended to the run history.
pub const EV_MESSAGE_ADDED: &str = "message.added";

// --- Pipeline events ---

/// Span wrapping one pipeline step's execution.
pub const SPAN_PIPELINE_STEP: &str = "pipeline.step";
/// Emitted when a pipeline step begins.
pub const EV_PIPELINE_STEP_STARTED: &str = "pipeline.step_started";
/// Emitted when a pipeline step completes successfully.
pub const EV_PIPELINE_STEP_COMPLETED: &str = "pipeline.step_completed";

// --- Check events ---

/// Span wrapping a single check evaluation.
pub const SPAN_PIPELINE_CHECK: &str = "pipeline.check";
/// Emitted when a check produces a verdict.
pub const EV_CHECK_EVALUATED: &str = "check.evaluated";
/// Emitted when a failed check rewound to its gate for retry.
pub const EV_CHECK_RETRY: &str = "check.retry";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_match_adr_0006_section_3_9() {
        assert_eq!(SPAN_RUNTIME_AGENT_RUN, "runtime.agent_run");
        assert_eq!(SPAN_RUNTIME_TURN, "runtime.turn");
        assert_eq!(SPAN_LLM_COMPLETE, "llm.complete");
        assert_eq!(SPAN_DISPATCH_TOOL, "dispatch.tool");
        assert_eq!(SPAN_CAPABILITY_CHECK, "capability.check");
        assert_eq!(SPAN_TOOL_SESSION_OPEN, "tool.session_open");
        assert_eq!(SPAN_TOOL_INVOKE, "tool.invoke");
        assert_eq!(SPAN_TOOL_SESSION_CLOSE, "tool.session_close");
    }

    #[test]
    fn runtime_events_match_adr_0006_section_3_9() {
        assert_eq!(EV_RUNTIME_RUN_STARTED, "runtime.run_started");
        assert_eq!(EV_RUNTIME_COMPLETED, "runtime.completed");
        assert_eq!(EV_RUNTIME_FAILED, "runtime.failed");
        assert_eq!(EV_RUNTIME_LOOP_TERMINATED, "runtime.loop_terminated");
        assert_eq!(EV_RUNTIME_MAX_TURNS_REACHED, "runtime.max_turns_reached");
        assert_eq!(EV_RUNTIME_TURN_STARTED, "runtime.turn_started");
    }

    #[test]
    fn llm_events_match_adr_0006_section_3_9() {
        assert_eq!(EV_LLM_REQUEST_BUILT, "llm.request_built");
        assert_eq!(EV_LLM_RESPONSE_RECEIVED, "llm.response_received");
        assert_eq!(EV_LLM_TOKEN_USAGE, "llm.token_usage");
        assert_eq!(EV_LLM_STOP_REASON, "llm.stop_reason");
        assert_eq!(EV_LLM_TOOL_USE_EMITTED, "llm.tool_use_emitted");
    }

    #[test]
    fn dispatch_events_match_adr_0006_section_3_9() {
        assert_eq!(EV_DISPATCH_TOOL_RESOLVED, "dispatch.tool_resolved");
    }

    #[test]
    fn capability_events_match_adr_0006_section_3_9() {
        assert_eq!(EV_CAPABILITY_REQUIRED_LOADED, "capability.required_loaded");
        assert_eq!(EV_CAPABILITY_GRANTED_LOADED, "capability.granted_loaded");
        assert_eq!(EV_CAPABILITY_SATISFIES_CHECK, "capability.satisfies_check");
        assert_eq!(EV_CAPABILITY_ALLOW, "capability.allow");
        assert_eq!(EV_CAPABILITY_DENY, "capability.deny");
    }

    #[test]
    fn tool_events_match_adr_0006_section_3_9() {
        assert_eq!(EV_TOOL_ARGS_RECEIVED, "tool.args_received");
        assert_eq!(EV_TOOL_RESULT_RECEIVED, "tool.result_received");
        assert_eq!(EV_TOOL_INVOKE_FAILED, "tool.invoke_failed");
        assert_eq!(EV_TOOL_SESSION_OPEN_FAILED, "tool.session_open_failed");
        assert_eq!(EV_TOOL_SESSION_CLOSE_FAILED, "tool.session_close_failed");
    }

    #[test]
    fn message_events_match_adr_0006_section_3_9() {
        assert_eq!(EV_MESSAGE_ADDED, "message.added");
    }

    #[test]
    fn pipeline_events_match_adr_0006_section_3_9() {
        assert_eq!(SPAN_PIPELINE_STEP, "pipeline.step");
        assert_eq!(EV_PIPELINE_STEP_STARTED, "pipeline.step_started");
        assert_eq!(EV_PIPELINE_STEP_COMPLETED, "pipeline.step_completed");
    }

    #[test]
    fn check_events_match_spec() {
        assert_eq!(SPAN_PIPELINE_CHECK, "pipeline.check");
        assert_eq!(EV_CHECK_EVALUATED, "check.evaluated");
        assert_eq!(EV_CHECK_RETRY, "check.retry");
    }
}
