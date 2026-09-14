//! The model bridge contract, as bytes: the JSON examples in ADR-0006 are the
//! fixtures under `fixtures/bridge/`, and each must round-trip through the
//! shared types without changing in value.
//!
//! If a fixture and a type disagree, one of them is wrong and this is where
//! it shows. The examples in the ADR are copies of these files.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use tau_kernel::bridge::{
    Content, ErrorKind, ModelReply, ModelRequest, Role, StopReason, ToolErrorKind, VERSION,
};

const REQUEST: &str = include_str!("fixtures/bridge/request.json");
const REPLY_TOOL_CALL: &str = include_str!("fixtures/bridge/reply-tool-call.json");
const REPLY_ERROR: &str = include_str!("fixtures/bridge/reply-error.json");
const TOOL_RESULTS: &str = include_str!("fixtures/bridge/tool-results.json");

/// Parses `json` as `T`, serializes it back, and checks the value is the same.
/// Returns the typed value for further assertions.
fn round_trip<T>(json: &str) -> T
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let expected: Value = serde_json::from_str(json).expect("fixture is valid JSON");
    let typed: T = serde_json::from_str(json).expect("fixture parses as the bridge type");
    let back = serde_json::to_value(&typed).expect("bridge type serializes");
    assert_eq!(back, expected, "serialized form differs from the fixture");
    let again: T = serde_json::from_value(back).expect("serialized form parses again");
    assert_eq!(again, typed, "second parse differs from the first");
    typed
}

#[test]
fn request_round_trips() {
    let request: ModelRequest = round_trip(REQUEST);
    assert_eq!(request.v, VERSION);
    assert_eq!(request.messages.len(), 3);
    let roles: Vec<Role> = request.messages.iter().map(|m| m.role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant, Role::User]);
    let tool_names: Vec<&str> = request.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(tool_names, ["store"]);
    assert_eq!(request.max_tokens, 1024);
    let sampling = request.sampling.expect("fixture sets sampling");
    assert_eq!(sampling.seed, Some(42));
    assert_eq!(sampling.temperature, Some(0.0));
    assert_eq!(sampling.top_p, None);
}

#[test]
fn request_tool_call_and_result_blocks_parse() {
    let request: ModelRequest = round_trip(REQUEST);
    let block = |turn: usize, at: usize| {
        request
            .messages
            .get(turn)
            .and_then(|m| m.content.get(at))
            .expect("fixture has the block")
    };
    match block(1, 1) {
        Content::ToolCall { id, name, input } => {
            assert_eq!(id, "call_1");
            assert_eq!(name, "store");
            assert_eq!(input["op"], "read");
        }
        other => panic!("expected a tool call, got {other:?}"),
    }
    match block(2, 0) {
        Content::ToolResult {
            call_id,
            content,
            is_error,
            error_kind,
        } => {
            assert_eq!(call_id, "call_1");
            assert_eq!(content, "hello");
            assert!(!is_error);
            assert_eq!(*error_kind, None);
        }
        other => panic!("expected a tool result, got {other:?}"),
    }
}

#[test]
fn reply_with_tool_call_round_trips() {
    let reply: ModelReply = round_trip(REPLY_TOOL_CALL);
    assert_eq!(reply.v, VERSION);
    assert_eq!(reply.model.as_deref(), Some("claude-opus-5"));
    assert_eq!(reply.stop, StopReason::ToolCall);
    assert_eq!(reply.usage.input_tokens, 120);
    assert_eq!(reply.usage.output_tokens, 34);
    assert!(matches!(
        reply.content.get(1),
        Some(Content::ToolCall { name, .. }) if name == "search"
    ));
}

#[test]
fn reply_with_error_round_trips() {
    let reply: ModelReply = round_trip(REPLY_ERROR);
    match reply.stop {
        StopReason::Error(err) => {
            assert_eq!(err.kind, ErrorKind::OverCeiling);
            assert!(err.message.contains("9210"));
        }
        other => panic!("expected an error stop, got {other:?}"),
    }
    assert!(reply.content.is_empty());
}

#[test]
fn every_failure_kind_is_a_tool_result() {
    let results: Vec<Content> = round_trip(TOOL_RESULTS);
    let kinds: Vec<ToolErrorKind> = results
        .iter()
        .map(|block| match block {
            Content::ToolResult {
                is_error: true,
                error_kind: Some(kind),
                ..
            } => *kind,
            other => panic!("expected an error tool result, got {other:?}"),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            ToolErrorKind::UnknownTool,
            ToolErrorKind::BadArgs,
            ToolErrorKind::Denied,
            ToolErrorKind::Failed,
        ]
    );
}

#[test]
fn optional_fields_default_when_absent() {
    // The smallest request a v1 driver must accept: no system, no tools, no
    // sampling. Absent means "provider default", and the fields must not
    // reappear on the way out.
    let minimal = r#"{"v":1,"messages":[],"max_tokens":16}"#;
    let request: ModelRequest = round_trip(minimal);
    assert_eq!(request.system, None);
    assert!(request.tools.is_empty());
    assert_eq!(request.sampling, None);

    // A tool result without `is_error` is a success.
    let ok = r#"{"type":"tool_result","call_id":"c","content":"fine"}"#;
    let block: Content = serde_json::from_str(ok).unwrap();
    assert!(matches!(
        block,
        Content::ToolResult {
            is_error: false,
            error_kind: None,
            ..
        }
    ));
}

#[test]
fn stop_reasons_have_the_documented_wire_forms() {
    let plain = [
        (StopReason::EndTurn, "\"end_turn\""),
        (StopReason::ToolCall, "\"tool_call\""),
        (StopReason::MaxTokens, "\"max_tokens\""),
        (StopReason::StopSequence, "\"stop_sequence\""),
        (StopReason::Refusal, "\"refusal\""),
    ];
    for (reason, wire) in plain {
        assert_eq!(serde_json::to_string(&reason).unwrap(), wire);
    }
    for (kind, wire) in [
        (ErrorKind::OverCeiling, "over_ceiling"),
        (ErrorKind::Unsupported, "unsupported"),
        (ErrorKind::Provider, "provider"),
        (ErrorKind::Transport, "transport"),
    ] {
        assert_eq!(serde_json::to_value(kind).unwrap(), wire);
    }
}

#[test]
fn a_tool_definition_name_is_validated_but_a_call_name_is_not() {
    // The definition comes from the harness (a `DriverId`), so it is a `Name`
    // and an invalid one is a parse error.
    let bad_def = r#"{"name":"Not A Name","description":"","input_schema":{}}"#;
    assert!(serde_json::from_str::<tau_kernel::bridge::ToolDef>(bad_def).is_err());

    // The call comes from the model, which can write anything; the loop must
    // be able to represent it in order to answer `unknown_tool`.
    let bad_call = r#"{"type":"tool_call","id":"c","name":"Serch!","input":{}}"#;
    let block: Content = serde_json::from_str(bad_call).unwrap();
    assert!(matches!(block, Content::ToolCall { name, .. } if name == "Serch!"));
}
