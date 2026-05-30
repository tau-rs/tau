//! Dispatch resolution helpers — pure, no I/O.
//!
//! `address_to_tool_name` converts an [`Address`] to a tool name.
//! The `impl Runtime` resolver methods ([`Runtime::resolve_llm_backend`],
//! [`Runtime::resolve_tool`]) live here so host shells and core tests can
//! both use them without going through tau-runtime.
//!
//! # Dead-code allow
//!
//! [`address_to_tool_name`] is reached only by the dispatcher (Task 10)
//! and tests; we keep the module-level `allow` so the v0.1 surface
//! doesn't sprout one-off annotations.

#![allow(dead_code)]

use alloc::borrow::ToOwned;
use alloc::sync::Arc;
use alloc::vec::Vec;

use tau_domain::Address;

use crate::builder::{DynLlmBackend, DynTool, Runtime};
use crate::error::RuntimeError;

/// Convert a recipient [`Address`] to a tool name. v0.1 only routes
/// to tools (`Address::Tool`); other variants return `None`.
pub fn address_to_tool_name(addr: &Address) -> Option<&str> {
    match addr {
        Address::Tool(name) => Some(name.as_str()),
        _ => None,
    }
}

impl Runtime {
    /// Resolve an LLM backend by name. Returns
    /// [`RuntimeError::Core(CoreRuntimeError::LlmBackendNotRegistered)`] if the agent's
    /// requested backend is not in the registry.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Runtime is #[non_exhaustive]; construct via builder in tests.
    /// let backend = runtime.resolve_llm_backend("agent-x", "gpt-4")?;
    /// ```
    pub fn resolve_llm_backend(
        &self,
        agent_id: &str,
        backend_name: &str,
    ) -> Result<&Arc<dyn DynLlmBackend>, RuntimeError> {
        self.llm_backends()
            .get(backend_name)
            .ok_or_else(|| RuntimeError::LlmBackendNotRegistered {
                agent_id: agent_id.to_owned(),
                backend: backend_name.to_owned(),
            })
    }

    /// Resolve a tool by name. On miss, returns
    /// [`RuntimeError::ToolNotRegistered`] populated with the sorted
    /// list of registered tool names for diagnostics.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Runtime is #[non_exhaustive]; construct via builder in tests.
    /// let tool = runtime.resolve_tool("echo")?;
    /// ```
    pub fn resolve_tool(&self, tool_name: &str) -> Result<&Arc<dyn DynTool>, RuntimeError> {
        self.tools().get(tool_name).ok_or_else(|| {
            let mut registered: Vec<alloc::string::String> =
                self.tools().keys().cloned().collect();
            registered.sort();
            RuntimeError::ToolNotRegistered {
                tool_name: tool_name.to_owned(),
                registered,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use alloc::string::ToString;
    use alloc::vec;
    use tau_domain::{AgentInstanceId, Value};
    use tau_ports::fixtures::{make_tool_spec, MockLlmBackend, MockTool};

    fn empty_tool_spec(name: &str) -> tau_ports::ToolSpec {
        make_tool_spec(
            name.to_string(),
            "mock tool".to_string(),
            Value::Object(Default::default()),
        )
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
        let RuntimeError::LlmBackendNotRegistered {
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
        let RuntimeError::ToolNotRegistered {
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
}
