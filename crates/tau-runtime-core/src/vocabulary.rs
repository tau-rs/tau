// Some constants are only used when `tool-validation` is enabled (stream.rs).
// Suppress dead-code warnings under the no-default-features build.
#![allow(dead_code)]

//! Fixed `&'static str` names for every span and event in ADR-0006 §3.9.
//!
//! Mirror of `tau_observe::vocabulary` — kept in `tau-runtime-core` so the
//! executor-agnostic kernel does not depend on `tau-observe` (which uses
//! tokio + tracing-subscriber and is std-only). The host shell's
//! `tau_observe::vocabulary` remains the canonical source of truth; this
//! mirror is `pub(crate)` and tested only for value-drift at the
//! integration level (a snapshot test in `tau-runtime` asserts both
//! tables agree).

// --- Spans (§3.9) ---

pub(crate) const SPAN_RUNTIME_AGENT_RUN: &str = "runtime.agent_run";
pub(crate) const SPAN_RUNTIME_TURN: &str = "runtime.turn";
pub(crate) const SPAN_DISPATCH_TOOL: &str = "dispatch.tool";
pub(crate) const SPAN_CAPABILITY_CHECK: &str = "capability.check";
pub(crate) const SPAN_TOOL_SESSION_OPEN: &str = "tool.session_open";
pub(crate) const SPAN_TOOL_INVOKE: &str = "tool.invoke";
pub(crate) const SPAN_TOOL_SESSION_CLOSE: &str = "tool.session_close";

// --- Runtime events ---

pub(crate) const EV_RUNTIME_RUN_STARTED: &str = "runtime.run_started";
pub(crate) const EV_RUNTIME_COMPLETED: &str = "runtime.completed";
pub(crate) const EV_RUNTIME_FAILED: &str = "runtime.failed";
pub(crate) const EV_RUNTIME_LOOP_TERMINATED: &str = "runtime.loop_terminated";
pub(crate) const EV_RUNTIME_MAX_TURNS_REACHED: &str = "runtime.max_turns_reached";
pub(crate) const EV_RUNTIME_TURN_STARTED: &str = "runtime.turn_started";

// --- LLM events ---

pub(crate) const EV_LLM_REQUEST_BUILT: &str = "llm.request_built";
pub(crate) const EV_LLM_RESPONSE_RECEIVED: &str = "llm.response_received";
pub(crate) const EV_LLM_TOKEN_USAGE: &str = "llm.token_usage";
pub(crate) const EV_LLM_STOP_REASON: &str = "llm.stop_reason";
pub(crate) const EV_LLM_TOOL_USE_EMITTED: &str = "llm.tool_use_emitted";

// --- Dispatch events ---

pub(crate) const EV_DISPATCH_TOOL_RESOLVED: &str = "dispatch.tool_resolved";

// --- Capability events ---

pub(crate) const EV_CAPABILITY_REQUIRED_LOADED: &str = "capability.required_loaded";
pub(crate) const EV_CAPABILITY_GRANTED_LOADED: &str = "capability.granted_loaded";
pub(crate) const EV_CAPABILITY_SATISFIES_CHECK: &str = "capability.satisfies_check";
pub(crate) const EV_CAPABILITY_ALLOW: &str = "capability.allow";
pub(crate) const EV_CAPABILITY_DENY: &str = "capability.deny";

// --- Tool events ---

pub(crate) const EV_TOOL_INVOKE_FAILED: &str = "tool.invoke_failed";
pub(crate) const EV_TOOL_SESSION_OPEN_FAILED: &str = "tool.session_open_failed";
pub(crate) const EV_TOOL_SESSION_CLOSE_FAILED: &str = "tool.session_close_failed";

// --- Message events ---

pub(crate) const EV_MESSAGE_ADDED: &str = "message.added";
