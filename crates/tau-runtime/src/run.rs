//! Agent multi-turn run loop. The kernel surface — see spec §3.7.
//!
//! [`Runtime::run`] receives an initial [`Message`], drives the LLM
//! backend and tool plugins through a turn loop bounded by
//! [`RunOptions::max_turns`], applies capability checks before each
//! tool call, and returns a [`RunOutcome`].
//!
//! # Error vs failure dichotomy (ADR-0006)
//!
//! - Plugin/dispatch failures (LLM error, tool error, missing backend)
//!   bubble up as `Err(RuntimeError)`. The agent terminates abnormally.
//! - Agent-level failures — capability denied, max turns reached —
//!   are reported as `Ok(RunOutcome::Failed { status, .. })` with
//!   `status = AgentStatus::Failed { kind, .. }`. The conversation
//!   history is preserved for inspection.
//!
//! # Tracing
//!
//! Per spec §3.9 the run loop emits a fixed vocabulary of events
//! (`runtime.run_started`, `runtime.turn_started`, `llm.request_built`,
//! …) under named spans (`runtime.agent_run`, `runtime.turn`,
//! `llm.complete`, `dispatch.tool`, `capability.check`,
//! `tool.session_open`, `tool.invoke`, `tool.session_close`).
//! Sensitive-data discipline: arguments and message content never
//! travel above DEBUG; full content is TRACE-only and otherwise
//! truncated to a 256-char preview.

#[cfg(test)]
use std::collections::BTreeMap;

#[cfg(test)]
use tau_domain::AgentInstanceId;
use tau_domain::Capability;
#[cfg(test)]
use tau_domain::{AgentDefinition, Message};
#[cfg(test)]
use tau_domain::{AgentStatus, FailureKind, MessagePayload, Value};
#[cfg(test)]
use tau_ports::{ContentBlock, LlmProviderMessage, ToolUse};

use crate::capability_override::EffectiveCapability;
#[cfg(test)]
use crate::error::CapabilityDenial;
#[cfg(test)]
use crate::options::TokenUsage;
#[cfg(test)]
use crate::outcome::RunOutcome;

// Re-export pure helpers from core so that stream.rs can still use
// `crate::run::*` paths unchanged (Task 3.5 migration shim).
pub(crate) use tau_runtime_core::run::{
    agent_messages_to_provider_messages, build_policy_denied_outcome, content_to_value,
    flatten_content_to_string,
};
// value_to_preview_string is pub in core but not used from tau-runtime directly.
#[allow(unused_imports)]
pub(crate) use tau_runtime_core::run::value_to_preview_string;

// ---------------------------------------------------------------------------
// Internal helpers (named per the plan; some are re-exported from core)
// ---------------------------------------------------------------------------

// agent_messages_to_provider_messages, build_policy_denied_outcome,
// flatten_content_to_string, content_to_value, value_to_preview_string
// are re-exported at top of file from tau_runtime_core::run.

/// Build the post-narrow `Capability` view that flows to plugins via
/// `SessionContext.granted_capabilities`. Capability inner variants are
/// `#[non_exhaustive]` and can't be constructed cross-crate; we serialize
/// the source, splice in the narrowed allow-list / max_bytes, and
/// deserialize back.
///
/// Failure-safe: any serialization failure falls back to `eff.source.clone()`.
/// The kernel's structural cap check still applies — narrowing is best-effort
/// at this layer; panicking on a security-enforcement path would be the
/// wrong failure mode.
pub(crate) fn narrowed_capability_for_session(eff: &EffectiveCapability) -> Capability {
    use serde_json::{json, Value as Jv};

    let source_json = match serde_json::to_value(&eff.source) {
        Ok(v) => v,
        Err(_) => return eff.source.clone(),
    };
    let mut obj = match source_json.as_object() {
        Some(m) => m.clone(),
        None => return eff.source.clone(),
    };
    if let Some(allow) = &eff.allow_override {
        // Replace the kind-appropriate field. For unknown kinds (e.g. Custom),
        // bail and return source unchanged — narrowing is unsupported.
        let field = match obj.get("kind").and_then(Jv::as_str) {
            Some("fs.read") | Some("fs.write") | Some("fs.exec") => "paths",
            Some("net.http") => "hosts",
            Some("process.spawn") => "commands",
            _ => return eff.source.clone(),
        };
        obj.insert(field.to_string(), json!(allow));
    }
    if let Some(mb) = eff.max_bytes_override {
        obj.insert("max_bytes".to_string(), json!(mb));
    }
    serde_json::from_value(Jv::Object(obj)).unwrap_or_else(|_| eff.source.clone())
}

// ---------------------------------------------------------------------------
// Small format/utility helpers (test-only)
// ---------------------------------------------------------------------------

/// Append the assistant's response (text + tool_uses) to the history.
///
/// Now only called from unit tests (the agent loop moved to stream.rs).
#[cfg(test)]
pub(crate) fn append_assistant_response(
    history: &mut Vec<Message>,
    text: &str,
    tool_uses: &[ToolUse],
    agent_id: &AgentInstanceId,
) {
    if !text.is_empty() {
        history.push(Message::new(
            tau_domain::Address::Agent(*agent_id),
            tau_domain::Address::User,
            MessagePayload::Text {
                content: text.to_owned(),
            },
        ));
    } else if tool_uses.is_empty() {
        history.push(Message::new(
            tau_domain::Address::Agent(*agent_id),
            tau_domain::Address::User,
            MessagePayload::Text {
                content: String::new(),
            },
        ));
    }
}

/// Truncate `s` to at most `n` characters (Unicode scalar values).
#[cfg(test)]
fn truncate_to_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use tau_domain::{Address, AgentInstanceId, MessagePayload};

    fn user_text_message(content: &str) -> Message {
        Message::new(
            Address::User,
            Address::Agent(AgentInstanceId::new()),
            MessagePayload::Text {
                content: content.into(),
            },
        )
    }

    // -------------------- build_policy_denied_outcome --------------------

    #[test]
    fn build_policy_denied_outcome_carries_denial_in_status() {
        let denial = CapabilityDenial::new(
            "agent-x",
            "pkg-y",
            "file_read",
            "fs.read",
            "Filesystem(Read { paths: [\"/etc/passwd\"] })",
        );
        let out = build_policy_denied_outcome(denial, vec![], 3, TokenUsage::default());

        let RunOutcome::Failed {
            status,
            total_turns,
            token_usage,
            all_messages,
        } = out.clone()
        else {
            panic!("expected Failed, got {out:?}");
        };
        let AgentStatus::Failed { kind, detail, .. } = status.clone() else {
            panic!("expected AgentStatus::Failed, got {status:?}")
        };
        assert_eq!(kind, FailureKind::PolicyDenied);
        let detail = detail.expect("detail must be set");
        assert!(detail.contains("agent-x"), "got: {detail}");
        assert!(detail.contains("file_read"), "got: {detail}");
        assert!(detail.contains("fs.read"), "got: {detail}");
        assert_eq!(total_turns, 3);
        assert_eq!(token_usage, TokenUsage::default());
        assert!(all_messages.is_empty());
    }

    // -------------------- helper unit tests (smoke) --------------------

    #[test]
    fn agent_messages_to_provider_messages_maps_user_text_to_user_role() {
        let history = vec![user_text_message("hi")];
        let provider = agent_messages_to_provider_messages(&history);
        assert_eq!(provider.len(), 1);
        match &provider[0] {
            LlmProviderMessage::User { content } => {
                assert_eq!(content.len(), 1);
                match &content[0] {
                    ContentBlock::Text(t) => assert_eq!(t, "hi"),
                    other => panic!("expected Text, got {other:?}"),
                }
            }
            other => panic!("expected User, got {other:?}"),
        }
    }

    #[test]
    fn agent_messages_to_provider_messages_skips_lifecycle() {
        let m = Message::new(
            Address::System,
            Address::User,
            MessagePayload::Lifecycle(AgentStatus::Ready),
        );
        let provider = agent_messages_to_provider_messages(&[m]);
        assert!(provider.is_empty());
    }

    #[test]
    fn append_assistant_response_appends_only_text_when_present() {
        let mut history: Vec<Message> = vec![];
        let agent_id = AgentInstanceId::new();
        append_assistant_response(&mut history, "out", &[], &agent_id);
        assert_eq!(history.len(), 1);
        match (&history[0].sender, &history[0].payload) {
            (Address::Agent(id), MessagePayload::Text { content }) => {
                assert_eq!(*id, agent_id);
                assert_eq!(content, "out");
            }
            other => panic!("expected Agent / Text, got {other:?}"),
        }
    }

    #[test]
    fn append_assistant_response_no_text_no_tool_uses_pushes_empty_assistant_message() {
        let mut history: Vec<Message> = vec![];
        let agent_id = AgentInstanceId::new();
        append_assistant_response(&mut history, "", &[], &agent_id);
        assert_eq!(history.len(), 1);
        match (&history[0].sender, &history[0].payload) {
            (Address::Agent(id), MessagePayload::Text { content }) => {
                assert_eq!(*id, agent_id);
                assert!(content.is_empty());
            }
            other => panic!("expected Agent / empty Text, got {other:?}"),
        }
    }

    #[test]
    fn truncate_to_chars_respects_utf8_boundaries() {
        let s = "éééééé"; // 6 chars, 12 bytes
        assert_eq!(truncate_to_chars(s, 3), "ééé");
        assert_eq!(truncate_to_chars(s, 100), "éééééé");
        assert_eq!(truncate_to_chars(s, 0), "");
    }

    #[test]
    fn capability_kind_str_for_filesystem_read() {
        use crate::capability::capability_kind_str;
        #[derive(serde::Deserialize)]
        struct CapWrapper {
            cap: Capability,
        }
        let cap = toml::from_str::<CapWrapper>(
            r#"[cap]
kind = "fs.read"
paths = ["**"]
"#,
        )
        .expect("test fs.read capability TOML must parse")
        .cap;
        assert_eq!(capability_kind_str(&cap), "fs.read");
    }

    #[test]
    fn capability_kind_str_for_custom_variant() {
        use crate::capability::capability_kind_str;
        let mut params = BTreeMap::new();
        params.insert("servers".into(), Value::Null);
        let cap = Capability::Custom {
            name: "mcp.tool.use".into(),
            params,
        };
        assert_eq!(capability_kind_str(&cap), "mcp.tool.use");
    }

    #[test]
    fn capability_kind_str_for_task_list() {
        use crate::capability::capability_kind_str;
        #[derive(serde::Deserialize)]
        struct CapWrapper {
            cap: Capability,
        }
        let cap = toml::from_str::<CapWrapper>(
            r#"[cap]
kind = "task_list"
mode = "write"
"#,
        )
        .expect("test task_list capability TOML must parse")
        .cap;
        assert_eq!(capability_kind_str(&cap), "task_list");
    }

    #[test]
    fn capability_kind_str_for_plan() {
        use crate::capability::capability_kind_str;
        #[derive(serde::Deserialize)]
        struct CapWrapper {
            cap: Capability,
        }
        let cap = toml::from_str::<CapWrapper>(
            r#"[cap]
kind = "plan"
mode = "read"
"#,
        )
        .expect("test plan capability TOML must parse")
        .cap;
        assert_eq!(capability_kind_str(&cap), "plan");
    }

    // -------------------- invoke_tool --------------------

    #[tokio::test]
    async fn invoke_tool_dispatches_to_registered_tool_and_returns_result() {
        use crate::runtime_ext::RuntimeShellExt;
        use std::str::FromStr;
        use tau_domain::{AgentId, PackageId, PackageName, Version};
        use tau_ports::fixtures::{make_tool_result, make_tool_spec, MockLlmBackend, MockTool};
        use tau_ports::ToolContent;

        let spec = make_tool_spec(
            "echo".to_string(),
            "echo tool".to_string(),
            Value::Object(Default::default()),
        );
        let canned_result = make_tool_result(
            vec![ToolContent::Text {
                text: "pong".to_string(),
            }],
            false,
        );
        let tool = MockTool::new("echo", spec).with_result(canned_result.clone());

        let runtime = crate::builder::Runtime::builder()
            .with_llm_backend(MockLlmBackend::new("gpt-4"))
            .with_tool(tool)
            .build()
            .expect("build runtime");

        let pkg = PackageId::new(
            PackageName::from_str("test-pkg").unwrap(),
            Version::parse("0.1.0").unwrap(),
        );
        let agent_def = AgentDefinition::new(
            AgentId::from_str("test-agent").unwrap(),
            "test".to_string(),
            pkg,
            PackageName::from_str("gpt-4").unwrap(),
        );

        let toml_str = r#"
            name = "test-pkg"
            version = "0.1.0"
            description = "test package"
            authors = []
            source = "https://example.com/test.git"
            kind = "tool"
            dependencies = []
            capabilities = []
        "#;
        let unchecked: tau_domain::UncheckedManifest = toml::from_str(toml_str).unwrap();
        let manifest = unchecked.validate().unwrap();

        let result =
            RuntimeShellExt::invoke_tool(&runtime, &agent_def, &manifest, "echo", Value::Null)
                .await
                .expect("invoke_tool must succeed");

        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            tau_ports::ToolContent::Text { text } => assert_eq!(text, "pong"),
            other => panic!("expected Text content, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_tool_returns_err_for_unknown_tool() {
        use crate::error::{CoreRuntimeError, RuntimeError};
        use crate::runtime_ext::RuntimeShellExt;
        use std::str::FromStr;
        use tau_domain::{AgentId, PackageId, PackageName, Version};
        use tau_ports::fixtures::MockLlmBackend;

        let runtime = crate::builder::Runtime::builder()
            .with_llm_backend(MockLlmBackend::new("gpt-4"))
            .build()
            .expect("build runtime");

        let pkg = PackageId::new(
            PackageName::from_str("test-pkg").unwrap(),
            Version::parse("0.1.0").unwrap(),
        );
        let agent_def = AgentDefinition::new(
            AgentId::from_str("test-agent").unwrap(),
            "test".to_string(),
            pkg,
            PackageName::from_str("gpt-4").unwrap(),
        );
        let toml_str = r#"
            name = "test-pkg"
            version = "0.1.0"
            description = "test package"
            authors = []
            source = "https://example.com/test.git"
            kind = "tool"
            dependencies = []
            capabilities = []
        "#;
        let unchecked: tau_domain::UncheckedManifest = toml::from_str(toml_str).unwrap();
        let manifest = unchecked.validate().unwrap();

        let err = RuntimeShellExt::invoke_tool(
            &runtime,
            &agent_def,
            &manifest,
            "no-such-tool",
            Value::Null,
        )
        .await
        .expect_err("should return ToolNotRegistered");

        assert!(
            matches!(
                err,
                RuntimeError::Core(CoreRuntimeError::ToolNotRegistered { .. })
            ),
            "expected ToolNotRegistered, got {err:?}"
        );
    }
}
