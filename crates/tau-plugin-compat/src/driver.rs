//! Port-aware test driver for sub-project D's Layer 4 plugin compat
//! tests.
//!
//! Wraps the public `tau_runtime::plugin_host::load_{tool,llm_backend,storage}`
//! functions. Tests use this to spawn a real plugin under the resolved
//! sandbox adapter and invoke the high-level `DynTool` / `DynLlmBackend` /
//! `DynStorage` traits directly — no manual `Frame::Request` construction.
//!
//! # Why this exists
//!
//! Sub-project B's `tau plugin run --script` driver hardcoded the
//! handshake port to `LlmBackend`, breaking tool-port plugin tests
//! (commit a449c10 marked them `#[ignore]`'d). This module is the
//! port-aware replacement: the port is determined by which `spawn_*`
//! function the test calls (`spawn_tool_under_sandbox` for tool plugins,
//! etc.).
//!
//! Internally this calls into `tau_runtime::plugin_host::load_*`
//! which themselves call `PluginProcess::spawn_and_handshake` (private).
//! Tests don't need raw `Frame::Request` access.

use std::sync::Arc;

use tau_pkg::LockedPlugin;
use tau_ports::CapabilityPlan;
use tau_runtime::builder::{DynLlmBackend, DynStorage, DynTool};
use tau_runtime::plugin_host::{self, PluginHostOptions};
use tau_runtime::sandbox::SandboxAdapter;

/// Errors raised by driver helpers.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum DriveError {
    /// `tau_runtime::plugin_host::load_*` returned a `RuntimeError`.
    #[error("plugin load failed: {0}")]
    LoadFailed(String),
    /// The plugin's port doesn't match what the caller expected.
    #[error("port mismatch: caller expected {expected:?}, plugin provides {actual:?}")]
    PortMismatch {
        /// Port kind the caller asked the driver to load.
        expected: String,
        /// Port kind the plugin's manifest actually declared.
        actual: String,
    },
    /// A tool invocation returned an error.
    #[error("tool invocation failed: {0}")]
    ToolFailed(String),
    /// An LLM completion returned an error.
    #[error("llm completion failed: {0}")]
    LlmFailed(String),
    /// A storage call returned an error.
    #[error("storage call failed: {0}")]
    StorageFailed(String),
}

/// Construct test trace context. Each test call gets a fresh context;
/// use a synthetic run/agent/span ID stable enough for log correlation
/// but unique enough that parallel test runs don't conflate.
pub fn test_trace_context() -> tau_plugin_protocol::handshake::TraceContext {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Atomic counter guarantees uniqueness across parallel test runs even
    // when nanosecond clock resolution can't distinguish two consecutive
    // calls (observed on macOS CI where two identical nanos timestamps
    // surfaced from back-to-back invocations).
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    tau_plugin_protocol::handshake::TraceContext::new(
        format!("test-driver-run-{nanos}-{seq}"),
        format!("test-agent-{nanos}-{seq}"),
        format!("test-span-{nanos}-{seq}"),
    )
}

/// Construct `PluginHostOptions` for a test, with the supplied sandbox
/// adapter and standard test timeouts.
///
/// `PluginHostOptions` is `#[non_exhaustive]`, so struct-literal
/// initialisation is not allowed from outside the defining crate. We
/// call `default()` and mutate the one field we care about.
pub fn test_plugin_host_options(adapter: Option<Arc<SandboxAdapter>>) -> PluginHostOptions {
    let mut opts = PluginHostOptions::default();
    opts.sandbox_adapter = adapter;
    opts
}

/// Spawn a tool plugin under the given sandbox adapter and return the
/// `DynTool` handle for direct method invocation.
pub async fn spawn_tool_under_sandbox(
    plugin: &LockedPlugin,
    config: serde_json::Value,
    adapter: Option<Arc<SandboxAdapter>>,
    sandbox_plan: Option<&CapabilityPlan>,
) -> Result<Arc<dyn DynTool>, DriveError> {
    let trace = test_trace_context();
    let options = test_plugin_host_options(adapter);
    plugin_host::load_tool(plugin, config, trace, options, sandbox_plan)
        .await
        .map_err(|e| DriveError::LoadFailed(format!("{e:?}")))
}

/// Spawn an llm-backend plugin under the given sandbox adapter and
/// return the `DynLlmBackend` handle.
pub async fn spawn_llm_under_sandbox(
    plugin: &LockedPlugin,
    config: serde_json::Value,
    adapter: Option<Arc<SandboxAdapter>>,
    sandbox_plan: Option<&CapabilityPlan>,
) -> Result<Arc<dyn DynLlmBackend>, DriveError> {
    let trace = test_trace_context();
    let options = test_plugin_host_options(adapter);
    plugin_host::load_llm_backend(plugin, config, trace, options, sandbox_plan)
        .await
        .map_err(|e| DriveError::LoadFailed(format!("{e:?}")))
}

/// Spawn a storage plugin under the given sandbox adapter and return
/// the `DynStorage` handle.
pub async fn spawn_storage_under_sandbox(
    plugin: &LockedPlugin,
    config: serde_json::Value,
    adapter: Option<Arc<SandboxAdapter>>,
    sandbox_plan: Option<&CapabilityPlan>,
) -> Result<Arc<dyn DynStorage>, DriveError> {
    let trace = test_trace_context();
    let options = test_plugin_host_options(adapter);
    plugin_host::load_storage(plugin, config, trace, options, sandbox_plan)
        .await
        .map_err(|e| DriveError::LoadFailed(format!("{e:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_error_display_includes_detail() {
        let err = DriveError::LoadFailed("plugin handshake timed out".to_string());
        let msg = format!("{err}");
        assert!(msg.contains("plugin handshake timed out"));
    }

    #[test]
    fn drive_error_is_non_exhaustive_via_match() {
        let err = DriveError::LoadFailed("e".to_string());
        match err {
            DriveError::LoadFailed(_)
            | DriveError::PortMismatch { .. }
            | DriveError::ToolFailed(_)
            | DriveError::LlmFailed(_)
            | DriveError::StorageFailed(_) => {}
        }
    }

    #[test]
    fn test_trace_context_unique_across_calls() {
        let a = test_trace_context();
        let b = test_trace_context();
        // Fields are pub per tau-plugin-protocol handshake design.
        assert!(
            a.run_id != b.run_id,
            "expected unique run_ids; got a={:?}, b={:?}",
            a.run_id,
            b.run_id
        );
    }

    #[test]
    fn test_plugin_host_options_carries_no_adapter_by_default() {
        let opts = test_plugin_host_options(None);
        assert!(opts.sandbox_adapter.is_none());
    }

    #[test]
    fn port_mismatch_error_displays_clearly() {
        let err = DriveError::PortMismatch {
            expected: "Tool".to_string(),
            actual: "LlmBackend".to_string(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("Tool"));
        assert!(msg.contains("LlmBackend"));
    }
}
