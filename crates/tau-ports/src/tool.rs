//! Tool port — `kind = "tool"` plugin contracts.
//!
//! This module exports the [`Tool`] trait, the supporting data types
//! ([`SessionContext`], [`ToolResult`], [`ToolContent`]) exchanged
//! between tau-runtime and tool-plugin adapters, and the
//! [`StatelessTool`] / [`StatelessAdapter`] pair for the common
//! stateless case.

use alloc::string::String;
#[cfg(test)]
use alloc::string::ToString;
use alloc::vec::Vec;
#[cfg(feature = "process")]
use std::time::SystemTime;

use tau_domain::{AgentInstanceId, Value};
use uuid::Uuid;

use crate::error::ToolError;
use crate::llm::ToolSpec;

/// Per-session context handed to a tool's `init`.
///
/// `SessionContext` is `#[non_exhaustive]`: external callers cannot
/// construct it via struct-literal syntax. Use [`SessionContext::new`]
/// for the basic case and chain
/// [`SessionContext::with_granted_capabilities`] when the agent's
/// grant needs to flow to the plugin.
///
/// # Example
///
/// ```
/// use tau_domain::AgentInstanceId;
/// use tau_ports::tool::SessionContext;
/// use uuid::Uuid;
///
/// let ctx = SessionContext::new(
///     AgentInstanceId::new(),
///     Uuid::now_v7(),
///     None,
/// );
/// # let _ = ctx;
/// ```
#[non_exhaustive]
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SessionContext {
    /// Identity of the agent instance opening this tool session.
    pub agent_instance_id: AgentInstanceId,
    /// Unique identifier for this session, distinct from
    /// `agent_instance_id` because a single agent may open multiple
    /// concurrent sessions against the same tool.
    pub session_id: Uuid,
    /// Optional wall-clock deadline by which the session should
    /// complete. `None` defers to runtime defaults. Only present
    /// when the `process` feature is enabled (no_std hosts have no
    /// `SystemTime`).
    #[cfg(feature = "process")]
    pub deadline: Option<SystemTime>,
    /// Capabilities the calling agent has been granted by its package
    /// manifest. Plugins use this to perform finer-grained scope
    /// checks beyond the kernel's structural capability check at
    /// `tau-runtime::run.rs:272`.
    ///
    /// Populated by tau-runtime at dispatch. Defaults to empty when
    /// constructed via [`SessionContext::new`] — call
    /// [`SessionContext::with_granted_capabilities`] to set.
    #[cfg_attr(feature = "serde", serde(default))]
    pub granted_capabilities: Vec<tau_domain::Capability>,
    /// Per-capability deny carve-outs from the project tau.toml
    /// override. Plugins consult the matching entry (by `kind`)
    /// after their allow check passes — deny wins per spec §9.
    ///
    /// Populated by tau-runtime at dispatch. Defaults to empty when
    /// constructed via [`SessionContext::new`] — call
    /// [`SessionContext::with_deny_entries`] to set.
    #[cfg_attr(feature = "serde", serde(default))]
    pub deny_entries: Vec<DenyEntry>,
}

impl SessionContext {
    /// Construct a [`SessionContext`] with no granted capabilities.
    /// Use [`Self::with_granted_capabilities`] to chain in the
    /// agent's grant when known.
    ///
    /// Provided so external callers — notably tau-runtime, which
    /// mints one per tool dispatch in `Runtime::run` — can build one
    /// without struct-literal syntax (the type is `#[non_exhaustive]`).
    #[cfg(feature = "process")]
    pub fn new(
        agent_instance_id: AgentInstanceId,
        session_id: Uuid,
        deadline: Option<SystemTime>,
    ) -> Self {
        Self {
            agent_instance_id,
            session_id,
            deadline,
            granted_capabilities: Vec::new(),
            deny_entries: Vec::new(),
        }
    }

    /// Construct a [`SessionContext`] with no granted capabilities,
    /// no_std variant (no `deadline` field on this build).
    #[cfg(not(feature = "process"))]
    pub fn new(agent_instance_id: AgentInstanceId, session_id: Uuid) -> Self {
        Self {
            agent_instance_id,
            session_id,
            granted_capabilities: Vec::new(),
            deny_entries: Vec::new(),
        }
    }

    /// Replace the `granted_capabilities` list. Builder-pattern method.
    pub fn with_granted_capabilities(
        mut self,
        granted_capabilities: Vec<tau_domain::Capability>,
    ) -> Self {
        self.granted_capabilities = granted_capabilities;
        self
    }

    /// Replace the `deny_entries` list. Builder-pattern method.
    pub fn with_deny_entries(mut self, deny_entries: Vec<DenyEntry>) -> Self {
        self.deny_entries = deny_entries;
        self
    }
}

/// Per-capability deny carve-out flowing from a project tau.toml
/// `[[agents.<id>.capabilities]]` override into the plugin's
/// [`Tool::init`]. After the plugin's allow check passes, the plugin
/// consults the matching `DenyEntry` (by `kind`) and rejects any path
/// / host / command appearing in `deny`. Deny wins precedence per
/// spec §9.
///
/// `DenyEntry` is `#[non_exhaustive]`: external callers cannot construct
/// it via struct-literal syntax. Use [`DenyEntry::new`].
///
/// # Example
///
/// ```
/// use tau_ports::tool::DenyEntry;
///
/// let entry = DenyEntry::new(
///     "fs.read".to_string(),
///     vec!["${PROJECT}/.env".to_string()],
/// );
/// # let _ = entry;
/// ```
#[non_exhaustive]
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DenyEntry {
    /// Capability kind this entry's deny list applies to. Matches the
    /// wire `kind` field on `Capability` (`fs.read`, `fs.write`,
    /// `fs.exec`, `net.http`, `process.spawn`).
    pub kind: String,
    /// Strings to subtract from the matching allow-list. For path-shaped
    /// capabilities these are globs; for `net.http` host names; for
    /// `process.spawn` command names.
    pub deny: Vec<String>,
}

impl DenyEntry {
    /// Construct a [`DenyEntry`]. `#[non_exhaustive]` blocks struct-literal
    /// construction outside this crate.
    pub fn new(kind: String, deny: Vec<String>) -> Self {
        Self { kind, deny }
    }
}

/// Result of a single [`Tool::invoke`] call.
///
/// Mirrors the MCP tool-result shape: a list of typed content blocks
/// plus an `is_error` flag. The flag distinguishes "the tool ran but
/// reports an error to the LLM" (e.g. file-not-found, HTTP 500) from
/// [`crate::ToolError`], which signals "the tool itself failed to run"
/// (session unhealthy, contract violation, internal bug). The runtime
/// surfaces semantic errors to the agent's LLM via the
/// `MessagePayload::ToolError` envelope; trait-method errors may
/// trigger retry or agent-stop.
///
/// `ToolResult` is `#[non_exhaustive]`: external callers cannot
/// construct it via struct-literal syntax.
///
/// # Example
///
/// ```
/// use tau_ports::tool::{ToolContent, ToolResult};
///
/// // External callers cannot use struct-literal syntax because `ToolResult`
/// // is `#[non_exhaustive]` — use `ToolResult::new`.
/// let ok = ToolResult::new(
///     vec![ToolContent::Text { text: "done".into() }],
///     false,
/// );
/// # let _ = ok;
/// ```
#[non_exhaustive]
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ToolResult {
    /// Multi-block content describing the tool's output.
    pub content: Vec<ToolContent>,
    /// Whether the tool ran but reports a semantic error to the LLM.
    /// Distinct from [`crate::ToolError`], which signals a trait-method
    /// failure.
    pub is_error: bool,
}

impl ToolResult {
    /// Construct a `ToolResult` from outside the `tau-ports` crate. The
    /// struct is `#[non_exhaustive]`, so struct-literal construction is
    /// blocked for external callers — this constructor is the only entry
    /// point they have.
    pub fn new(content: Vec<ToolContent>, is_error: bool) -> Self {
        Self { content, is_error }
    }
}

/// One content block within a [`ToolResult`].
///
/// v0.1 admits [`ToolContent::Text`] and [`ToolContent::Json`] only.
/// The enum is `#[non_exhaustive]` to admit additive variants for
/// image, audio, and resource references without a major bump.
///
/// # Example
///
/// ```
/// use tau_domain::Value;
/// use tau_ports::tool::ToolContent;
///
/// let blocks = vec![
///     ToolContent::Text { text: "hello".into() },
///     ToolContent::Json { data: Value::String("world".into()) },
/// ];
/// # let _ = blocks;
/// ```
#[non_exhaustive]
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ToolContent {
    /// Plain-text content.
    Text {
        /// The text payload.
        text: String,
    },
    /// Structured JSON content, carried as a `tau_domain::Value`.
    Json {
        /// The structured payload.
        data: Value,
    },
    // Future: ImageRef { ... }, AudioRef { ... }, ResourceRef { ... }
}

/// Trait implemented by `kind = "tool"` plugins.
///
/// Stateful by design — see [`StatelessAdapter`] for the common
/// stateless case.
///
/// # Error semantics
///
/// `Err(ToolError)` means *the tool itself failed to run* (session
/// unhealthy, contract violation, internal bug). `Ok(ToolResult { is_error: true, ... })`
/// means *the tool ran but the operation reports an error to the LLM*
/// (file not found, HTTP failure, etc.). The runtime treats these
/// differently: errors may trigger retry/agent-stop; semantic failures
/// are surfaced to the agent's LLM via `MessagePayload::ToolError`.
#[allow(async_fn_in_trait)]
pub trait Tool: Send + Sync {
    /// Per-session state. Use `()` for stateless tools (or use [`StatelessAdapter`]).
    type Session: Send + 'static;

    /// Stable name used for routing. SemVer-stable surface.
    fn name(&self) -> &str;

    /// JSON Schema describing the tool's input. Used both for runtime
    /// validation and for surfacing to the LLM via
    /// `CompletionRequest.tools`.
    fn schema(&self) -> ToolSpec;

    /// Capabilities this tool requires the calling agent's package to declare.
    /// Default: empty (tool is unrestricted; any agent can call it).
    ///
    /// The runtime checks: for every capability in this list, the agent's
    /// package manifest must contain at least one capability that satisfies
    /// it. See `tau_runtime::capability::check_capabilities` for the
    /// satisfies-relation.
    ///
    /// # Example
    ///
    /// ```
    /// // `Capability` is `#[non_exhaustive]`; declared via the manifest path.
    /// use tau_domain::Capability;
    /// use tau_ports::Tool;
    ///
    /// struct MyFileTool;
    /// // impl Tool for MyFileTool {
    /// //     fn capabilities(&self) -> &[Capability] { &[] }
    /// //     // ... other methods ...
    /// // }
    /// # let _ = std::any::type_name::<MyFileTool>();
    /// ```
    fn capabilities(&self) -> &[tau_domain::Capability] {
        &[]
    }

    /// The authority this tool actually runs under, when narrower than
    /// [`Tool::capabilities`] — e.g. an MCP entry whose `net.http` hosts
    /// were meet-clamped against the `[allow.mcp.<entry>].hosts` ceiling
    /// at open time. `None` (the default) means the runtime authority
    /// equals the declared capabilities.
    ///
    /// Observability-only in v0.1: the kernel maps `Some` to a
    /// `CapabilityVerdict::Clamp` on the call's `ToolCall` trace event.
    /// Enforcement is unchanged — the kernel grant gate still checks the
    /// declared capabilities, and the OS boundary enforces the narrowed
    /// plan (execution-trace TUI spec §12).
    fn effective_capabilities(&self) -> Option<&[tau_domain::Capability]> {
        None
    }

    /// Open a session. Called once before any `invoke`.
    async fn init(&self, ctx: SessionContext) -> Result<Self::Session, ToolError>;

    /// Perform a single tool call within an open session.
    async fn invoke(
        &self,
        session: &mut Self::Session,
        args: Value,
    ) -> Result<ToolResult, ToolError>;

    /// Close the session gracefully. If the runtime drops the session
    /// future (cancellation), `teardown` is NOT called — plugin authors
    /// put critical cleanup in `Drop`.
    async fn teardown(&self, session: Self::Session) -> Result<(), ToolError>;
}

/// Simpler trait for stateless tools. Implement this and wrap with
/// [`StatelessAdapter`] to satisfy [`Tool`] with `Session = ()`.
///
/// Most tools (filesystem read, HTTP fetch, calculator, search APIs,
/// MCP) are stateless and should use this trait. Tools that need
/// per-session state (browser drivers, database connections, GPU
/// handles) should implement [`Tool`] directly.
#[allow(async_fn_in_trait)]
pub trait StatelessTool: Send + Sync {
    /// Stable name used for routing. SemVer-stable surface.
    fn name(&self) -> &str;
    /// JSON Schema describing the tool's input. Used both for runtime
    /// validation and for surfacing to the LLM via
    /// `CompletionRequest.tools`.
    fn schema(&self) -> ToolSpec;
    /// Perform a single tool call. Stateless: no session is threaded
    /// through.
    async fn invoke(&self, args: Value) -> Result<ToolResult, ToolError>;
}

/// Newtype that adapts a [`StatelessTool`] to satisfy [`Tool`] with
/// `Session = ()`.
///
/// # Example
///
/// ```text
/// // StatelessAdapter wraps a StatelessTool to give it a Session lifecycle.
/// // Plugin author writes:
/// // let tool = StatelessAdapter(MyTool);
/// // runtime.register_tool(Box::new(tool));
/// ```
pub struct StatelessAdapter<T: StatelessTool>(pub T);

impl<T: StatelessTool> Tool for StatelessAdapter<T> {
    type Session = ();

    fn name(&self) -> &str {
        self.0.name()
    }

    fn schema(&self) -> ToolSpec {
        self.0.schema()
    }

    async fn init(&self, _: SessionContext) -> Result<(), ToolError> {
        Ok(())
    }

    async fn invoke(&self, _: &mut (), args: Value) -> Result<ToolResult, ToolError> {
        self.0.invoke(args).await
    }

    async fn teardown(&self, _: ()) -> Result<(), ToolError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use tau_domain::Value;

    // `SessionContext::new` has two shapes: `process` adds the `deadline`
    // parameter. Mirror that split here so these tests keep compiling — and
    // keep asserting — in the feature-less shape too. Without it the whole
    // `lib test` target fails to build without `process` (#657).
    #[cfg(feature = "process")]
    fn session_ctx() -> SessionContext {
        SessionContext::new(
            tau_domain::AgentInstanceId::new(),
            uuid::Uuid::now_v7(),
            None,
        )
    }

    #[cfg(not(feature = "process"))]
    fn session_ctx() -> SessionContext {
        SessionContext::new(tau_domain::AgentInstanceId::new(), uuid::Uuid::now_v7())
    }

    struct EchoTool;
    impl StatelessTool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn schema(&self) -> ToolSpec {
            ToolSpec {
                name: "echo".into(),
                description: "echo args back".into(),
                input_schema: Value::Object(Default::default()),
            }
        }
        async fn invoke(&self, args: Value) -> Result<ToolResult, ToolError> {
            Ok(ToolResult {
                content: vec![ToolContent::Json { data: args }],
                is_error: false,
            })
        }
    }

    #[tokio::test]
    #[allow(clippy::let_unit_value)] // Session = () for the adapter; we
                                     // bind it explicitly to exercise the full Tool lifecycle.
    async fn stateless_adapter_round_trip() {
        let tool = StatelessAdapter(EchoTool);
        assert_eq!(Tool::name(&tool), "echo");

        let mut session = tool.init(session_ctx()).await.unwrap();

        let result = tool
            .invoke(&mut session, Value::String("hi".into()))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);

        tool.teardown(session).await.unwrap();
    }

    #[test]
    fn session_context_default_deny_entries_is_empty() {
        let ctx = session_ctx();
        assert!(ctx.deny_entries.is_empty());
    }

    #[test]
    fn session_context_with_deny_entries_replaces_field() {
        let entry = DenyEntry::new("fs.read".to_string(), vec!["/etc/secret".to_string()]);
        let ctx = session_ctx().with_deny_entries(vec![entry]);
        assert_eq!(ctx.deny_entries.len(), 1);
        assert_eq!(ctx.deny_entries[0].kind, "fs.read");
        assert_eq!(ctx.deny_entries[0].deny, vec!["/etc/secret".to_string()]);
    }

    #[test]
    fn deny_entry_new_round_trips_kind_and_deny() {
        let entry = DenyEntry::new(
            "process.spawn".to_string(),
            vec!["rm".to_string(), "shutdown".to_string()],
        );
        assert_eq!(entry.kind, "process.spawn");
        assert_eq!(entry.deny, vec!["rm".to_string(), "shutdown".to_string()]);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn session_context_deny_entries_defaults_on_missing_key() {
        // Defends the #[serde(default)] gate: a payload from an older
        // plugin/runtime that doesn't include `deny_entries` must
        // deserialize successfully with the field defaulted to empty.
        let json = serde_json::json!({
            "agent_instance_id": tau_domain::AgentInstanceId::new(),
            "session_id": uuid::Uuid::now_v7(),
            "deadline": null,
            "granted_capabilities": []
        });
        let ctx: SessionContext =
            serde_json::from_value(json).expect("SessionContext deserializes without deny_entries");
        assert!(ctx.deny_entries.is_empty());
    }
}

#[cfg(test)]
mod effective_capabilities_tests {
    use super::Tool;
    use crate::fixtures::{make_tool_spec, MockTool};
    use tau_domain::Value;

    #[test]
    fn default_effective_capabilities_is_none() {
        // The defaulted method means every existing Tool impl reports
        // "not narrowed" without changes.
        let tool = MockTool::new(
            "noop",
            make_tool_spec("noop".into(), "noop".into(), Value::Null),
        );
        assert!(Tool::effective_capabilities(&tool).is_none());
    }
}
