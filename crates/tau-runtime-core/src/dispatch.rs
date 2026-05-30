//! Dispatch resolution helpers — pure, no I/O.
//!
//! All helpers are kernel-internal (`pub(crate)`) — dispatch routing
//! is not part of the public `tau-runtime-core` API surface.
//!
//! The `impl Runtime` resolver methods (resolve_llm_backend,
//! resolve_tool) live in `tau-runtime::dispatch` alongside this
//! module's re-export until the builder (Task 3.4) moves to core.
//!
//! # Dead-code allow
//!
//! [`address_to_tool_name`] is reached only by the dispatcher (Task 10)
//! and tests; we keep the module-level `allow` so the v0.1 surface
//! doesn't sprout one-off annotations.

#![allow(dead_code)]

use tau_domain::Address;

/// Convert a recipient [`Address`] to a tool name. v0.1 only routes
/// to tools (`Address::Tool`); other variants return `None`.
pub fn address_to_tool_name(addr: &Address) -> Option<&str> {
    match addr {
        Address::Tool(name) => Some(name.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tau_domain::AgentInstanceId;

    #[test]
    fn address_to_tool_name_routes_only_tool_addresses() {
        // Tool variant -> Some(name).
        let tool_addr = Address::Tool("foo".into());
        assert_eq!(address_to_tool_name(&tool_addr), Some("foo"));

        // User -> None.
        assert_eq!(address_to_tool_name(&Address::User), None);

        // System -> None.
        assert_eq!(address_to_tool_name(&Address::System), None);

        // Agent -> None.
        let agent_addr = Address::Agent(AgentInstanceId::new());
        assert_eq!(address_to_tool_name(&agent_addr), None);
    }
}
