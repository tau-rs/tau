//! Agent-loop methods on [`Runtime`] and pure helper functions.
//!
//! This module contains the methods that can run in any async executor
//! (no tokio specifics), plus the pure free-function helpers used by
//! the streaming pump (`stream.rs` in the host shell).
//!
//! # What lives here vs. the host shell
//!
//! - [`Runtime::invoke_tool`] — single-tool direct dispatch; no LLM loop.
//! - [`build_policy_denied_outcome`], [`agent_messages_to_provider_messages`],
//!   [`flatten_content_to_string`], [`content_to_value`],
//!   [`narrowed_capability_for_session`] — pure helpers used by the
//!   streaming pump.
//!
//! - `run`, `run_with_history`, `run_default`, `spawn_root_agent` —
//!   **stay in the host shell** until `stream.rs` and the orchestration
//!   submodules migrate to core (Tasks 3.6/3.7).
//!
//! # Error routing
//!
//! Methods return `crate::error::RuntimeError` (the core error). Host-shell
//! callers whose return type is the shell-level `RuntimeError` get automatic
//! `?` conversion via `#[from] CoreRuntimeError`.

extern crate alloc;

use alloc::borrow::ToOwned;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use tau_domain::{
    AgentInstanceId, AgentStatus, Capability, FailureKind, Message, MessagePayload,
    PackageManifest, Value,
};
use tau_ports::{ContentBlock, LlmProviderMessage, ToolContent, ToolUse};
use tracing::{debug, instrument};

use crate::builder::Runtime;
use crate::capability::capability_kind_str;
use crate::error::{CapabilityDenial, RuntimeError};
use crate::options::TokenUsage;
use crate::outcome::RunOutcome;

impl Runtime {
    /// Invoke a single tool by name without engaging the LLM loop.
    ///
    /// Bypasses the multi-turn agent driver — useful for callers that
    /// want to compose tools directly (e.g., `tau-workflow`'s
    /// `tool.call` step kind). The tool's capability requirements are
    /// still checked against the `agent_def`'s package grant set, so
    /// the caller must pass the workflow's default-agent definition.
    ///
    /// Follows the same sequence as the run loop's tool-dispatch arm:
    /// `resolve_tool → capability check → init → invoke → teardown`.
    ///
    /// The `clock` and `random` parameters supply entropy for minting the
    /// tool session ID. Pass `None` to fall back to deterministic
    /// test-fixture defaults (acceptable in tests; production callers
    /// must supply real implementations via their shell's `drive` entry
    /// point).
    ///
    /// # Errors
    ///
    /// - [`RuntimeError::ToolNotRegistered`] — the tool name is unknown.
    /// - [`RuntimeError::Internal`] — the agent's package does not grant
    ///   a capability required by the tool.
    /// - [`RuntimeError::Tool`] — the tool's `init`, `invoke`, or
    ///   `teardown` returned a [`tau_ports::ToolError`].
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Runtime is #[non_exhaustive]; construct via builder.
    /// let result = runtime
    ///     .invoke_tool(&agent_def, &manifest, "echo", Value::Null, None, None)
    ///     .await?;
    /// ```
    #[instrument(
        name = "dispatch.tool",
        skip_all,
        fields(tool_name = %tool_name),
    )]
    pub async fn invoke_tool(
        &self,
        agent_def: &tau_domain::AgentDefinition,
        package_manifest: &PackageManifest,
        tool_name: &str,
        args: tau_domain::Value,
        // _clock: reserved for Task 3.6 — will be used for trace-event
        // timestamps once orchestration RunState moves to core.
        _clock: Option<Arc<dyn tau_ports::Clock>>,
        random: Option<Arc<dyn tau_ports::RandomSource>>,
    ) -> Result<tau_ports::ToolResult, RuntimeError> {
        use tau_ports::SessionContext;

        let tool = self.resolve_tool(tool_name)?.clone();
        debug!(
            name = "dispatch.tool_resolved",
            tool_name = %tool_name,
            plugin_id = %tool.name(),
        );

        // Capability check: mirror the run loop's structural check.
        let granted: Vec<Capability> = package_manifest.capabilities().to_vec();
        let required: &[Capability] = tool.capabilities();
        if let Some(missing) = crate::capability::check_capabilities(&granted, required) {
            let denial = CapabilityDenial::new(
                agent_def.id.to_string(),
                agent_def.package.name.to_string(),
                tool_name.to_owned(),
                capability_kind_str(missing),
                alloc::format!("{missing:?}"),
            );
            return Err(RuntimeError::Internal {
                message: alloc::format!("capability denied: {denial}"),
            });
        }

        // Mint a session UUID from the random source. When the caller
        // passes `None` (backwards-compat / test path), fall back to
        // the test-fixture DeterministicRandom if available, otherwise
        // produce a nil UUID. Production callers must supply a real
        // RandomSource via their shell's `drive` entry point.
        let session_uuid = match random {
            Some(ref r) => crate::ids::uuid_v4(r),
            None => {
                #[cfg(any(test, feature = "test-fixtures"))]
                {
                    let r: Arc<dyn tau_ports::RandomSource> =
                        Arc::new(tau_ports::DeterministicRandom::seeded(0));
                    crate::ids::uuid_v4(&r)
                }
                #[cfg(not(any(test, feature = "test-fixtures")))]
                {
                    uuid::Uuid::nil()
                }
            }
        };

        // Build a minimal SessionContext (no deadline, no deny entries).
        let ctx = SessionContext::new(AgentInstanceId::new(), session_uuid, None)
            .with_granted_capabilities(granted);

        tool.init(ctx.clone()).await.map_err(RuntimeError::from)?;
        let result = tool.invoke(&ctx, &mut (), args).await;
        // teardown best-effort: don't mask invoke's error if both fail.
        let _ = tool.teardown(()).await;
        result.map_err(RuntimeError::from)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers (pure free functions used by the streaming pump)
// ---------------------------------------------------------------------------

/// Build the `RunOutcome::Failed { kind: PolicyDenied, .. }` returned
/// when [`check_capabilities`] rejects a tool invocation. Centralizes
/// the construction so the run loop's denial branch reads cleanly.
pub fn build_policy_denied_outcome(
    denial: CapabilityDenial,
    all_messages: Vec<Message>,
    total_turns: u32,
    token_usage: TokenUsage,
) -> RunOutcome {
    RunOutcome::Failed {
        status: AgentStatus::failed(FailureKind::PolicyDenied, Some(alloc::format!("{denial}"))),
        all_messages,
        total_turns,
        token_usage,
    }
}

/// Project the agent's [`Message`] history onto the LLM-call shape.
///
/// Per `tau_ports::llm` module-level docs, `tau_domain::Message`
/// (universal envelope) and [`LlmProviderMessage`] (provider call
/// shape) are intentionally distinct. This function is the single
/// projection point in the kernel.
pub fn agent_messages_to_provider_messages(history: &[Message]) -> Vec<LlmProviderMessage> {
    let mut out = Vec::with_capacity(history.len());
    for m in history {
        match (&m.sender, &m.payload) {
            (tau_domain::Address::User, MessagePayload::Text { content }) => {
                out.push(LlmProviderMessage::user(vec![ContentBlock::Text(
                    content.clone(),
                )]));
            }
            (tau_domain::Address::Agent(_), MessagePayload::Text { content }) => {
                out.push(LlmProviderMessage::assistant(vec![ContentBlock::Text(
                    content.clone(),
                )]));
            }
            (tau_domain::Address::Agent(_), MessagePayload::ToolCall { args }) => {
                let tool_name = match &m.recipient {
                    tau_domain::Address::Tool(name) => name.clone(),
                    _ => String::new(),
                };
                out.push(LlmProviderMessage::assistant(vec![ContentBlock::ToolUse(
                    ToolUse::new(alloc::format!("toolu_{}", m.id), tool_name, args.clone()),
                )]));
            }
            (tau_domain::Address::Tool(_), MessagePayload::ToolResult { body }) => {
                out.push(LlmProviderMessage::tool_result(
                    alloc::format!("toolu_{}", m.id),
                    vec![ContentBlock::Text(value_to_preview_string(body))],
                    false,
                ));
            }
            (
                tau_domain::Address::Tool(_),
                MessagePayload::ToolError {
                    kind: _,
                    message,
                    details: _,
                },
            ) => {
                out.push(LlmProviderMessage::tool_result(
                    alloc::format!("toolu_{}", m.id),
                    vec![ContentBlock::Text(message.clone())],
                    true,
                ));
            }
            _ => {}
        }
    }
    out
}

/// Flatten a tool's content blocks into a single human-readable string.
pub fn flatten_content_to_string(blocks: &[ToolContent]) -> String {
    let mut out = String::new();
    for block in blocks {
        match block {
            ToolContent::Text { text } => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
            ToolContent::Json { data } => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&value_to_preview_string(data));
            }
            _ => {}
        }
    }
    out
}

/// Build a [`Value`] from a tool's content blocks.
pub fn content_to_value(blocks: &[ToolContent]) -> Value {
    if blocks.len() == 1 {
        if let ToolContent::Json { data } = &blocks[0] {
            return data.clone();
        }
    }
    let arr: Vec<Value> = blocks
        .iter()
        .map(|b| match b {
            ToolContent::Text { text } => Value::String(text.clone()),
            ToolContent::Json { data } => data.clone(),
            _ => Value::Null,
        })
        .collect();
    let mut obj = BTreeMap::new();
    obj.insert("content".to_string(), Value::Array(arr));
    Value::Object(obj)
}

/// Compact preview string for a [`Value`].
pub fn value_to_preview_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Bytes(b) => alloc::format!("<{} bytes>", b.len()),
        Value::Array(_) | Value::Object(_) => alloc::format!("{v:?}"),
        _ => alloc::format!("{v:?}"),
    }
}

// narrowed_capability_for_session uses tau-pkg::EffectiveCapability (a
// tau-pkg type), so it stays in tau-runtime until capability_override
// migrates to core in Task 3.7.
