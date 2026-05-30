//! Dispatch resolution helpers.
//!
//! `address_to_tool_name` is re-exported from the executor-agnostic
//! kernel. The `impl Runtime` resolver methods (resolve_llm_backend,
//! resolve_tool) now live in `tau_runtime_core::dispatch` — tau-runtime
//! reaches them via `Deref` on the newtype wrapper (Task 3.5).
//!
//! # Dead-code allow
//!
//! [`address_to_tool_name`] is reached only by the dispatcher (Task 10)
//! and tests; the resolver methods are exercised both by tests and the
//! run loop. We keep the module-level `allow` so the v0.1 surface
//! doesn't sprout one-off annotations.

#![allow(dead_code)]

#[cfg(test)]
use tau_domain::Address;

#[cfg(test)]
pub(crate) use tau_runtime_core::dispatch::address_to_tool_name;

// resolve_llm_backend and resolve_tool are now inherent methods on
// tau_runtime_core::Runtime, accessible via the newtype's Deref impl.

#[cfg(test)]
mod tests {
    use super::*;

    use tau_domain::{AgentInstanceId, Value};
    use tau_ports::fixtures::{make_tool_spec, MockLlmBackend, MockTool};
    use crate::builder::Runtime;
    use crate::error::CoreRuntimeError;

    fn empty_tool_spec(name: &str) -> tau_ports::ToolSpec {
        make_tool_spec(
            name.to_string(),
            "mock tool".to_string(),
            Value::Object(Default::default()),
        )
    }

    #[test]
    fn resolve_llm_backend_present_returns_arc() {
        let runtime = Runtime::builder()
            .with_llm_backend(MockLlmBackend::new("gpt-4"))
            .build()
            .expect("build runtime");

        let backend = runtime
            .resolve_llm_backend("agent-x", "gpt-4")
            .expect("backend present");
        assert_eq!(backend.name(), "gpt-4");
    }

    #[test]
    fn resolve_llm_backend_absent_returns_error() {
        let runtime = Runtime::builder()
            .with_llm_backend(MockLlmBackend::new("gpt-4"))
            .build()
            .expect("build runtime");

        let result = runtime.resolve_llm_backend("agent-x", "missing-backend");
        let Err(err) = result else {
            panic!("expected LlmBackendNotRegistered, got Ok")
        };
        // resolve_llm_backend returns CoreRuntimeError (tau_runtime_core::error::RuntimeError).
        let CoreRuntimeError::LlmBackendNotRegistered {
            agent_id, backend, ..
        } = err
        else {
            panic!("expected LlmBackendNotRegistered: {err:?}")
        };
        assert_eq!(agent_id, "agent-x");
        assert_eq!(backend, "missing-backend");
    }

    #[test]
    fn resolve_tool_present_returns_arc() {
        let runtime = Runtime::builder()
            .with_llm_backend(MockLlmBackend::new("gpt-4"))
            .with_tool(MockTool::new("echo", empty_tool_spec("echo")))
            .build()
            .expect("build runtime");

        let tool = runtime.resolve_tool("echo").expect("tool present");
        assert_eq!(tool.name(), "echo");
    }

    #[test]
    fn resolve_tool_absent_returns_error_with_registered_list() {
        let runtime = Runtime::builder()
            .with_llm_backend(MockLlmBackend::new("gpt-4"))
            .with_tool(MockTool::new("echo", empty_tool_spec("echo")))
            .with_tool(MockTool::new("reverse", empty_tool_spec("reverse")))
            .build()
            .expect("build runtime");

        let result = runtime.resolve_tool("missing");
        let Err(err) = result else {
            panic!("expected ToolNotRegistered, got Ok")
        };
        // resolve_tool returns CoreRuntimeError (tau_runtime_core::error::RuntimeError).
        let CoreRuntimeError::ToolNotRegistered {
            tool_name,
            registered,
            ..
        } = err
        else {
            panic!("expected ToolNotRegistered: {err:?}")
        };
        assert_eq!(tool_name, "missing");
        assert_eq!(
            registered,
            vec!["echo".to_string(), "reverse".to_string()],
            "registered list should contain both tools, sorted"
        );
    }

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
