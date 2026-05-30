//! Dispatch resolution helpers.
//!
//! `address_to_tool_name` is re-exported from the executor-agnostic
//! kernel. The `impl Runtime` resolver methods (resolve_llm_backend,
//! resolve_tool) stay here until builder.rs migrates to core (Task 3.4).
//!
//! # Dead-code allow
//!
//! [`address_to_tool_name`] is reached only by the dispatcher (Task 10)
//! and tests; the resolver methods on [`Runtime`] are exercised both by
//! tests and the run loop. We keep the module-level `allow` so the
//! v0.1 surface — small, with a few helpers that are used through
//! transitive call sites — doesn't sprout one-off annotations.

#![allow(dead_code)]

use std::sync::Arc;

#[cfg(test)]
use tau_domain::Address;

#[cfg(test)]
pub(crate) use tau_runtime_core::dispatch::address_to_tool_name;

use crate::builder::{DynLlmBackend, DynTool};
use crate::error::{CoreRuntimeError, RuntimeError};
use crate::Runtime;

impl Runtime {
    /// Resolve an LLM backend by name. Returns
    /// [`RuntimeError::LlmBackendNotRegistered`] if the agent's
    /// requested backend is not in the registry.
    pub(crate) fn resolve_llm_backend(
        &self,
        agent_id: &str,
        backend_name: &str,
    ) -> Result<&Arc<dyn DynLlmBackend>, RuntimeError> {
        self.llm_backends()
            .get(backend_name)
            .ok_or_else(|| RuntimeError::Core(CoreRuntimeError::LlmBackendNotRegistered {
                agent_id: agent_id.to_owned(),
                backend: backend_name.to_owned(),
            }))
    }

    /// Resolve a tool by name. On miss, returns
    /// [`RuntimeError::ToolNotRegistered`] populated with the sorted
    /// list of registered tool names for diagnostics.
    pub(crate) fn resolve_tool(&self, tool_name: &str) -> Result<&Arc<dyn DynTool>, RuntimeError> {
        self.tools().get(tool_name).ok_or_else(|| {
            let mut registered: Vec<String> = self.tools().keys().cloned().collect();
            registered.sort();
            RuntimeError::Core(CoreRuntimeError::ToolNotRegistered {
                tool_name: tool_name.to_owned(),
                registered,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // `Ok` side is `&Arc<dyn DynLlmBackend>` which is not `Debug`,
        // so we can't `{result:?}` the whole `Result` — discriminate
        // first, then debug-format only the `Err`.
        let Err(err) = result else {
            panic!("expected LlmBackendNotRegistered, got Ok")
        };
        let RuntimeError::Core(CoreRuntimeError::LlmBackendNotRegistered {
            agent_id, backend, ..
        }) = err
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
        // `Ok` side is `&Arc<dyn DynTool>` which is not `Debug`, so we
        // can't `{result:?}` the whole `Result` — discriminate first,
        // then debug-format only the `Err`.
        let Err(err) = result else {
            panic!("expected ToolNotRegistered, got Ok")
        };
        let RuntimeError::Core(CoreRuntimeError::ToolNotRegistered {
            tool_name,
            registered,
            ..
        }) = err
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
