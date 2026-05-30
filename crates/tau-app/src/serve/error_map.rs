//! Map [`tau_runtime::RuntimeError`] variants to JSON-RPC error
//! objects with structured `data` payloads.
//!
//! Per spec §6. Each `RuntimeError` variant maps to one custom code
//! in `super::error_codes`.

use super::error_codes;
use super::protocol::ErrorObject;
use serde_json::json;
use tau_runtime::error::CoreRuntimeError;
use tau_runtime::RuntimeError;

/// Map any `RuntimeError` to an `ErrorObject`.
pub fn from_runtime_error(err: &RuntimeError) -> ErrorObject {
    // Capability denials reach RuntimeError through the
    // Core(Tool(#[from] ToolError)) wrapper. Match that path first.
    if let RuntimeError::Core(CoreRuntimeError::Tool(
        tau_ports::ToolError::CapabilityDenied { capability },
    )) = err
    {
        return ErrorObject {
            code: error_codes::CAPABILITY_DENIED,
            message: format!("Capability denied: {}", capability),
            data: Some(json!({
                "kind": "CapabilityDenial",
                "capability": capability,
                "tool_error_variant": "CapabilityDenied"
            })),
        };
    }
    match err {
        RuntimeError::Core(
            CoreRuntimeError::LlmBackendNotRegistered { .. }
            | CoreRuntimeError::ToolNotRegistered { .. },
        ) => ErrorObject {
            code: error_codes::UNKNOWN_AGENT,
            message: err.to_string(),
            data: Some(json!({"kind": "UnknownAgent"})),
        },
        RuntimeError::Core(CoreRuntimeError::PluginContractViolation { .. })
        | RuntimeError::Core(CoreRuntimeError::PluginHandshakeFailed { .. })
        | RuntimeError::PluginSpawnFailed { .. }
        | RuntimeError::PluginCrashed { .. } => ErrorObject {
            code: error_codes::TOOL_ERROR,
            message: err.to_string(),
            data: Some(json!({"kind": "PluginError"})),
        },
        RuntimeError::SandboxValidationFailed { .. } | RuntimeError::SandboxWrapFailed { .. } => {
            ErrorObject {
                code: error_codes::CAPABILITY_DENIED,
                message: err.to_string(),
                data: Some(json!({"kind": "SandboxError"})),
            }
        }
        RuntimeError::Core(CoreRuntimeError::CapabilityOverrideExpands { .. }) => ErrorObject {
            code: error_codes::RUNTIME_ERROR,
            message: err.to_string(),
            data: Some(json!({"kind": "CapabilityOverrideExpands"})),
        },
        RuntimeError::Core(CoreRuntimeError::Tool(_)) => ErrorObject {
            code: error_codes::TOOL_ERROR,
            message: err.to_string(),
            data: Some(json!({"kind": "ToolError"})),
        },
        RuntimeError::Core(CoreRuntimeError::Llm(_)) => ErrorObject {
            code: error_codes::LLM_ERROR,
            message: err.to_string(),
            data: Some(json!({"kind": "LlmError"})),
        },
        _ => ErrorObject {
            code: error_codes::RUNTIME_ERROR,
            message: err.to_string(),
            data: Some(json!({"kind": "RuntimeError"})),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_tool_maps_to_unknown_agent() {
        let err = RuntimeError::Core(CoreRuntimeError::ToolNotRegistered {
            tool_name: "missing-tool".into(),
            registered: vec!["echo".into()],
        });
        let obj = from_runtime_error(&err);
        assert_eq!(obj.code, error_codes::UNKNOWN_AGENT);
        assert!(obj.message.contains("missing-tool"));
    }

    #[test]
    fn tool_capability_denied_maps_to_capability_denied() {
        let err = RuntimeError::Core(CoreRuntimeError::Tool(
            tau_ports::ToolError::CapabilityDenied {
                capability: "fs.read".into(),
            },
        ));
        let obj = from_runtime_error(&err);
        assert_eq!(obj.code, error_codes::CAPABILITY_DENIED);
        assert!(obj.message.contains("fs.read"));
    }
}
