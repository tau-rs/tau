//! Executor-agnostic error types for the tau-runtime kernel.
//!
//! All public errors are `#[non_exhaustive]` so additive variants are
//! non-breaking. [`BuildError`] derives
//! `Debug + Clone + PartialEq + Eq + Error`; [`RuntimeError`] derives
//! `Debug + Error` only — some variants carry types that are not
//! `Clone`/`Eq`.
//!
//! The error taxonomy splits into two layers:
//!
//! - [`BuildError`] — failures during `RuntimeBuilder::build()`. The
//!   runtime never gets constructed.
//! - [`RuntimeError`] — kernel-level operational failures during
//!   `Runtime::run`. Composes `tau_ports` plugin errors via `#[from]`.
//!   Agent-level failures (capability denied, max turns reached) are
//!   reported via `Ok(RunOutcome::Failed { status: AgentStatus::Failed
//!   })`, NOT `Err(RuntimeError)`.
//!
//! [`CapabilityDenial`] is a helper type embedded as the `detail`
//! string of `AgentStatus::Failed { kind: PolicyDenied }` when
//! capability enforcement rejects a tool call. It is NOT a variant
//! of `RuntimeError`.
//!
//! Host-shell variants that carry `std::io::Error`,
//! `std::process::ExitStatus`, or crate-internal types (e.g.
//! `SandboxValidationError`) live in the host-shell's `RuntimeError`
//! wrapper rather than here.

use alloc::string::String;
use alloc::vec::Vec;
use thiserror::Error;

/// Tag identifying a plugin kind in error messages and tracing fields.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluginKind {
    /// LLM backend plugin (`kind = "llm-backend"`).
    LlmBackend,
    /// Tool plugin (`kind = "tool"`).
    Tool,
    /// Storage plugin (`kind = "storage"`).
    Storage,
    /// Sandbox plugin (`kind = "sandbox"`); reserved for forward compat.
    Sandbox,
}

impl core::fmt::Display for PluginKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PluginKind::LlmBackend => f.write_str("llm-backend"),
            PluginKind::Tool => f.write_str("tool"),
            PluginKind::Storage => f.write_str("storage"),
            PluginKind::Sandbox => f.write_str("sandbox"),
        }
    }
}

/// Errors from `RuntimeBuilder::build()`.
///
/// # Example
///
/// ```
/// use tau_runtime_core::BuildError;
/// // BuildError::NoLlmBackend can be constructed directly.
/// let err = BuildError::NoLlmBackend;
/// assert!(matches!(err, BuildError::NoLlmBackend));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BuildError {
    /// At least one LLM backend must be registered before `build()`.
    #[error("no LLM backends registered; at least one is required")]
    NoLlmBackend,

    /// Two plugins of the same kind registered with the same `name()`.
    #[error("name collision: two {kind}s registered as {name:?}")]
    NameCollision {
        /// Which plugin kind collided.
        kind: PluginKind,
        /// The colliding name.
        name: String,
    },

    /// A registered tool's declared `input_schema` is not a valid
    /// Draft 7 JSON Schema. Surfaced at `RuntimeBuilder::build()`
    /// via `crate::tool_args::ToolArgsValidator::compile`. Realizes
    /// ADR-0010 (Tier 2 priority 6).
    #[error("tool {tool_name:?}: input_schema is not a valid Draft 7 schema: {detail}")]
    ToolSchemaInvalid {
        /// The tool name whose schema failed to compile.
        tool_name: String,
        /// Diagnostic from the schema compiler.
        detail: String,
    },

    /// Catch-all for invariant violations during build.
    /// See: [escape-hatches.md#builderror-internal](../docs/explanation/escape-hatches.md#builderror-internal).
    #[error("internal: {message}")]
    Internal {
        /// Human-readable message describing the internal failure.
        message: String,
    },
}

/// Capability-denial detail. Embedded as the `detail` string of
/// `AgentStatus::Failed { kind: PolicyDenied, .. }` when capability
/// enforcement rejects a tool call.
///
/// NOT a variant of `RuntimeError` — capability denial is an
/// agent-level failure (`Ok(RunOutcome::Failed)`), not a kernel-level
/// error (`Err(RuntimeError)`). See ADR-0006 for the dichotomy.
///
/// # Example
///
/// ```
/// use tau_runtime_core::error::CapabilityDenial;
///
/// let denial = CapabilityDenial::new(
///     "researcher",
///     "fs-tools@1.0.0",
///     "file_write",
///     "filesystem.write",
///     "/etc/**",
/// );
/// let msg = denial.to_string();
/// assert!(msg.contains("researcher"));
/// assert!(msg.contains("filesystem.write"));
/// assert!(msg.contains("file_write"));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityDenial {
    /// `AgentDefinition::id` formatted via `Display`.
    pub agent_id: String,
    /// `AgentDefinition::package` formatted via `Display`.
    pub package_id: String,
    /// The tool the agent attempted to call.
    pub tool_name: String,
    /// Top-level kind of the missing capability ("filesystem.read",
    /// "network.http", "tool.echo" — convention).
    pub required_kind: String,
    /// Human-readable description of the capability that wasn't satisfied.
    pub required_detail: String,
}

impl CapabilityDenial {
    /// Construct a [`CapabilityDenial`].
    ///
    /// `#[non_exhaustive]` blocks struct-literal construction outside this
    /// crate; use this constructor instead.
    pub fn new(
        agent_id: impl Into<String>,
        package_id: impl Into<String>,
        tool_name: impl Into<String>,
        required_kind: impl Into<String>,
        required_detail: impl Into<String>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            package_id: package_id.into(),
            tool_name: tool_name.into(),
            required_kind: required_kind.into(),
            required_detail: required_detail.into(),
        }
    }
}

impl core::fmt::Display for CapabilityDenial {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "agent {} (package {}) lacks capability `{}` ({}) required to call tool `{}`",
            self.agent_id,
            self.package_id,
            self.required_kind,
            self.required_detail,
            self.tool_name,
        )
    }
}

/// Executor-agnostic kernel errors from `Runtime::run` — kernel-level
/// operational failures.
///
/// Agent-level failures (capability denied, max turns reached) flow
/// through `Ok(RunOutcome::Failed { status: AgentStatus::Failed{..} })`
/// instead. See [`crate::error`] module-level docs for the dichotomy.
///
/// Plugin errors (`LlmError`, `ToolError`, `StorageError`,
/// `CapabilityError`) compose via `#[from]` for ergonomic
/// `?`-propagation throughout the agent loop.
///
/// Host-shell-only variants (`PluginCrashed`, `PluginSpawnFailed`,
/// `SandboxValidationFailed`, `SandboxWrapFailed`) live in the
/// host-shell's wrapper `RuntimeError` rather than here, because they
/// carry `std::io::Error`, `std::process::ExitStatus`, or
/// crate-internal types incompatible with `no_std`.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// Agent's `llm_backend` references a backend that wasn't registered.
    #[error("LLM backend `{backend}` not registered (agent {agent_id} requested it)")]
    LlmBackendNotRegistered {
        /// The agent's `id` formatted via `Display`.
        agent_id: String,
        /// The backend name the agent requested.
        backend: String,
    },

    /// LLM emitted a tool_use targeting a tool not in the registry.
    #[error("tool `{tool_name}` not registered; registered: {registered:?}")]
    ToolNotRegistered {
        /// The tool name the LLM requested.
        tool_name: String,
        /// Names of registered tools (for diagnostics).
        registered: Vec<String>,
    },

    /// Plugin returned successfully but its output violates the contract
    /// (malformed JSON args from LLM, undeserializable response from a
    /// loaded plugin, etc.). Surfaced both by the run loop's
    /// argument-validation pass and by the `plugin_host` IPC adapters
    /// when a plugin's response is structurally invalid.
    #[error("plugin {plugin} contract violation: {detail}")]
    PluginContractViolation {
        /// The plugin name (its `name()` value, or the manifest name
        /// for plugin-host violations).
        plugin: String,
        /// Human-readable detail describing what was malformed.
        detail: String,
    },

    /// Plugin spawned successfully but the `meta.handshake` exchange
    /// failed for one of the reasons enumerated in
    /// [`HandshakeFailureReason`].
    #[error("plugin {plugin} handshake failed: {reason}")]
    PluginHandshakeFailed {
        /// The plugin name (from `LockedPlugin::manifest.name`).
        plugin: String,
        /// Specific reason the handshake failed.
        reason: HandshakeFailureReason,
    },

    /// LLM backend plugin returned an error.
    #[error("llm: {0}")]
    Llm(#[from] tau_ports::LlmError),

    /// Tool plugin returned an error.
    #[error("tool: {0}")]
    Tool(#[from] tau_ports::ToolError),

    /// Storage plugin returned an error.
    #[error("storage: {0}")]
    Storage(#[from] tau_ports::StorageError),

    /// Sandbox adapter returned an error. Reserved for future use;
    /// v0.1 in-tree adapters (`tau-sandbox-native`, etc.) populate this.
    #[error("sandbox: {0}")]
    Sandbox(#[from] tau_ports::CapabilityError),

    /// Manifest validation failed (caller-supplied manifest invalid).
    #[error("manifest validation: {0}")]
    Manifest(#[from] tau_domain::PackageManifestError),

    /// Project capability override expanded the package's grant.
    /// Raised at runtime as a defense-in-depth check (tau-cli also
    /// rejects at parse time via its own ProjectConfigError variant).
    #[error("capability override on {kind:?} expands package grant: {reason}")]
    CapabilityOverrideExpands {
        /// The capability kind that expanded.
        kind: String,
        /// Human-readable reason from `compute_effective`.
        reason: String,
    },

    /// Catch-all for invariant violations / unexpected states.
    /// See: [escape-hatches.md#runtimeerror-internal](../docs/explanation/escape-hatches.md#runtimeerror-internal).
    #[error("internal: {message}")]
    Internal {
        /// Human-readable message describing the internal failure.
        message: String,
    },
}

/// Specific reason a plugin handshake (`meta.handshake` exchange)
/// failed. Carried inside [`RuntimeError::PluginHandshakeFailed`].
///
/// `#[non_exhaustive]`: the Phase-1 protocol surface may grow
/// additional handshake-failure modes as the schema-introspection
/// surface evolves; additive variants must remain non-breaking.
///
/// # Example
///
/// ```
/// use tau_runtime_core::error::HandshakeFailureReason;
///
/// let reason = HandshakeFailureReason::Timeout;
/// assert_eq!(reason.to_string(), "timeout");
///
/// let mismatch = HandshakeFailureReason::ProtocolVersionMismatch {
///     host: "1".into(),
///     plugin: "2".into(),
/// };
/// assert!(mismatch.to_string().contains("mismatch"));
/// assert!(mismatch.to_string().contains('1'));
/// ```
#[non_exhaustive]
#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum HandshakeFailureReason {
    /// Plugin did not respond to `meta.handshake` within the configured
    /// handshake timeout.
    #[error("timeout")]
    Timeout,

    /// Plugin's advertised `protocol_version` differs from the host's.
    #[error("protocol version mismatch: host {host}, plugin {plugin}")]
    ProtocolVersionMismatch {
        /// Host's protocol version.
        host: String,
        /// Plugin's advertised protocol version.
        plugin: String,
    },

    /// Plugin's advertised `provides` port differs from the
    /// `LockedPlugin.manifest.provides` claim. Catches manifest
    /// drift and binary swaps.
    #[error("provides mismatch: manifest says {manifest}, plugin advertises {plugin_advertised}")]
    ProvidesMismatch {
        /// What the install-time manifest declared.
        manifest: tau_domain::PortKind,
        /// What the plugin advertised in its handshake response.
        plugin_advertised: tau_domain::PortKind,
    },

    /// Plugin doesn't advertise a method the host requires for the
    /// declared port (e.g. an LLM backend without `llm.complete`).
    #[error("missing required method: {method}")]
    MissingRequiredMethod {
        /// The wire-method name that was missing
        /// (`"llm.complete"`, `"tool.call"`, etc.).
        method: String,
    },

    /// Plugin's handshake response was structurally malformed (failed
    /// to deserialize, missing required fields, etc.).
    #[error("malformed response: {detail}")]
    Malformed {
        /// Human-readable detail describing the malformation.
        detail: String,
    },
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::string::ToString;
    use alloc::vec;
    use super::*;

    #[test]
    fn plugin_kind_display() {
        assert_eq!(PluginKind::LlmBackend.to_string(), "llm-backend");
        assert_eq!(PluginKind::Tool.to_string(), "tool");
        assert_eq!(PluginKind::Storage.to_string(), "storage");
        assert_eq!(PluginKind::Sandbox.to_string(), "sandbox");
    }

    #[test]
    fn build_error_no_llm_backend_display() {
        let err = BuildError::NoLlmBackend;
        let s = format!("{err}");
        assert!(s.contains("no LLM backends registered"), "got: {s}");
    }

    #[test]
    fn build_error_name_collision_display() {
        let err = BuildError::NameCollision {
            kind: PluginKind::Tool,
            name: "echo".into(),
        };
        let s = format!("{err}");
        assert!(s.contains("name collision"), "got: {s}");
        assert!(s.contains("tool"), "got: {s}");
        assert!(s.contains("echo"), "got: {s}");
    }

    #[test]
    fn capability_denial_display_includes_all_fields() {
        let denial = CapabilityDenial {
            agent_id: "agent-x".into(),
            package_id: "pkg/y@1.0.0".into(),
            tool_name: "file_read".into(),
            required_kind: "filesystem.read".into(),
            required_detail: "/etc/passwd".into(),
        };
        let s = format!("{denial}");
        assert!(s.contains("agent-x"), "got: {s}");
        assert!(s.contains("pkg/y@1.0.0"), "got: {s}");
        assert!(s.contains("filesystem.read"), "got: {s}");
        assert!(s.contains("/etc/passwd"), "got: {s}");
        assert!(s.contains("file_read"), "got: {s}");
    }

    #[test]
    fn runtime_error_llm_backend_not_registered_display() {
        let err = RuntimeError::LlmBackendNotRegistered {
            agent_id: "agent-1".into(),
            backend: "anthropic".into(),
        };
        let s = format!("{err}");
        assert!(s.contains("anthropic"), "got: {s}");
        assert!(s.contains("agent-1"), "got: {s}");
        assert!(s.contains("not registered"), "got: {s}");
    }

    #[test]
    fn runtime_error_tool_not_registered_display() {
        let err = RuntimeError::ToolNotRegistered {
            tool_name: "ghost".into(),
            registered: alloc::vec!["echo".into(), "file_read".into()],
        };
        let s = format!("{err}");
        assert!(s.contains("ghost"), "got: {s}");
        assert!(s.contains("echo"), "got: {s}");
        assert!(s.contains("file_read"), "got: {s}");
    }

    #[test]
    fn runtime_error_plugin_contract_violation_display() {
        let err = RuntimeError::PluginContractViolation {
            plugin: "anthropic".into(),
            detail: "expected JSON object, got array".into(),
        };
        let s = format!("{err}");
        assert!(s.contains("contract violation"), "got: {s}");
        assert!(s.contains("anthropic"), "got: {s}");
        assert!(s.contains("expected JSON object, got array"), "got: {s}");
    }

    #[test]
    fn runtime_error_plugin_handshake_failed_display() {
        let err = RuntimeError::PluginHandshakeFailed {
            plugin: "echo-llm".into(),
            reason: HandshakeFailureReason::Timeout,
        };
        let s = format!("{err}");
        assert!(s.contains("handshake failed"), "got: {s}");
        assert!(s.contains("echo-llm"), "got: {s}");
        assert!(s.contains("timeout"), "got: {s}");
    }

    #[test]
    fn handshake_failure_reason_timeout_display() {
        let r = HandshakeFailureReason::Timeout;
        assert_eq!(format!("{r}"), "timeout");
    }

    #[test]
    fn handshake_failure_reason_protocol_version_mismatch_display() {
        let r = HandshakeFailureReason::ProtocolVersionMismatch {
            host: "1".into(),
            plugin: "2".into(),
        };
        let s = format!("{r}");
        assert!(s.contains("protocol version mismatch"), "got: {s}");
        assert!(s.contains("1"), "got: {s}");
        assert!(s.contains("2"), "got: {s}");
    }

    #[test]
    fn handshake_failure_reason_provides_mismatch_display() {
        let r = HandshakeFailureReason::ProvidesMismatch {
            manifest: tau_domain::PortKind::LlmBackend,
            plugin_advertised: tau_domain::PortKind::Tool,
        };
        let s = format!("{r}");
        assert!(s.contains("provides mismatch"), "got: {s}");
        assert!(s.contains("llm_backend"), "got: {s}");
        assert!(s.contains("tool"), "got: {s}");
    }

    #[test]
    fn handshake_failure_reason_missing_required_method_display() {
        let r = HandshakeFailureReason::MissingRequiredMethod {
            method: "llm.complete".into(),
        };
        let s = format!("{r}");
        assert!(s.contains("missing required method"), "got: {s}");
        assert!(s.contains("llm.complete"), "got: {s}");
    }

    #[test]
    fn handshake_failure_reason_malformed_display() {
        let r = HandshakeFailureReason::Malformed {
            detail: "missing field `protocol_version`".into(),
        };
        let s = format!("{r}");
        assert!(s.contains("malformed response"), "got: {s}");
        assert!(s.contains("protocol_version"), "got: {s}");
    }

    use tau_ports::{LlmError, StorageError, ToolError};

    #[test]
    fn runtime_error_composes_llm_via_from() {
        let llm_err = LlmError::Internal {
            message: "x".into(),
        };
        let runtime_err: RuntimeError = llm_err.into();
        assert!(matches!(
            runtime_err,
            RuntimeError::Llm(LlmError::Internal { .. })
        ));
    }

    #[test]
    fn runtime_error_composes_tool_via_from() {
        let tool_err = ToolError::Internal {
            message: "x".into(),
        };
        let runtime_err: RuntimeError = tool_err.into();
        assert!(matches!(
            runtime_err,
            RuntimeError::Tool(ToolError::Internal { .. })
        ));
    }

    #[test]
    fn runtime_error_composes_storage_via_from() {
        let storage_err = StorageError::Internal {
            message: "x".into(),
        };
        let runtime_err: RuntimeError = storage_err.into();
        assert!(matches!(
            runtime_err,
            RuntimeError::Storage(StorageError::Internal { .. })
        ));
    }

    #[test]
    fn runtime_error_composes_sandbox_via_from() {
        use tau_ports::CapabilityError;
        let sandbox_err = CapabilityError::Internal {
            message: "x".into(),
        };
        let runtime_err: RuntimeError = sandbox_err.into();
        assert!(matches!(
            runtime_err,
            RuntimeError::Sandbox(CapabilityError::Internal { .. })
        ));
    }

    #[test]
    fn runtime_error_internal_display() {
        let err = RuntimeError::Internal {
            message: "unexpected".into(),
        };
        let s = format!("{err}");
        assert!(s.contains("internal"), "got: {s}");
        assert!(s.contains("unexpected"), "got: {s}");
    }

    #[test]
    fn capability_override_expands_displays_with_kind_and_reason() {
        let err = RuntimeError::CapabilityOverrideExpands {
            kind: "fs.read".into(),
            reason: "/etc/** is not a subset".into(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("fs.read"));
        assert!(msg.contains("not a subset"));
    }

    #[test]
    fn build_error_tool_schema_invalid_display() {
        let err = BuildError::ToolSchemaInvalid {
            tool_name: "shell".into(),
            detail: "type 'objectt' is not valid".into(),
        };
        let s = format!("{err}");
        assert!(s.contains("shell"), "got: {s}");
        assert!(s.contains("input_schema"), "got: {s}");
        assert!(s.contains("Draft 7"), "got: {s}");
        assert!(s.contains("'objectt'"), "got: {s}");
    }
}
