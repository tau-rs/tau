//! The ADR-0006 fixtures through this crate's serializer: what `infer`
//! sends is the request fixture, what it decodes is the reply fixtures, and
//! the `tool_result` blocks the loop renders match the tool-results fixture.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use libtau::{decode_reply, encode_request, InferError};
use serde_json::Value;
use tau_kernel::bridge::{Content, ErrorKind, ModelRequest, StopReason, ToolErrorKind};

const REQUEST: &str = include_str!("../../kernel/tests/fixtures/bridge/request.json");
const REPLY_TOOL_CALL: &str =
    include_str!("../../kernel/tests/fixtures/bridge/reply-tool-call.json");
const REPLY_ERROR: &str = include_str!("../../kernel/tests/fixtures/bridge/reply-error.json");
const TOOL_RESULTS: &str = include_str!("../../kernel/tests/fixtures/bridge/tool-results.json");

#[test]
fn the_request_fixture_survives_encode() {
    let request: ModelRequest = serde_json::from_str(REQUEST).unwrap();
    let bytes = encode_request(&request).unwrap();
    let back: Value = serde_json::from_slice(&bytes).unwrap();
    let expected: Value = serde_json::from_str(REQUEST).unwrap();
    assert_eq!(back, expected);
}

#[test]
fn the_reply_fixtures_decode() {
    let reply = decode_reply(REPLY_TOOL_CALL.as_bytes()).unwrap();
    assert_eq!(reply.stop, StopReason::ToolCall);
    assert!(matches!(
        reply.content.get(1),
        Some(Content::ToolCall { name, .. }) if name == "search"
    ));

    let reply = decode_reply(REPLY_ERROR.as_bytes()).unwrap();
    assert!(matches!(
        reply.stop,
        StopReason::Error(ref e) if e.kind == ErrorKind::OverCeiling
    ));
    assert!(reply.content.is_empty());
}

#[test]
fn a_reply_from_another_bridge_version_is_refused() {
    let mut v: Value = serde_json::from_str(REPLY_TOOL_CALL).unwrap();
    v.as_object_mut()
        .unwrap()
        .insert("v".into(), Value::from(2));
    match decode_reply(v.to_string().as_bytes()) {
        Err(InferError::Version { found: 2 }) => {}
        other => panic!("expected a version error, got {other:?}"),
    }
    assert!(matches!(
        decode_reply(b"not json"),
        Err(InferError::Decode(_))
    ));
}

#[test]
fn the_tool_results_fixture_is_what_the_loop_renders() {
    // The loop's wording for `unknown_tool` and `denied` is the fixture's,
    // so a transcript reads the same as the ADR. `bad_args` carries the
    // validator's message and `failed` the reason, so only their shape is
    // pinned here; their producers are tested in `tool_loop.rs`.
    let blocks: Vec<Content> = serde_json::from_str(TOOL_RESULTS).unwrap();
    let kinds: Vec<Option<ToolErrorKind>> = blocks
        .iter()
        .map(|b| match b {
            Content::ToolResult {
                is_error: true,
                error_kind,
                ..
            } => *error_kind,
            other => panic!("fixture block is not an error result: {other:?}"),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            Some(ToolErrorKind::UnknownTool),
            Some(ToolErrorKind::BadArgs),
            Some(ToolErrorKind::Denied),
            Some(ToolErrorKind::Failed),
        ]
    );
    let content = |i: usize| match blocks.get(i) {
        Some(Content::ToolResult { content, .. }) => content.as_str(),
        _ => unreachable!(),
    };
    assert_eq!(content(0), "unknown tool `serch`; available: search, store");
    assert_eq!(content(2), "denied: this agent does not hold `search`");
    assert!(content(1).starts_with("bad args for `search`: "));
}

/// Found by the Tier 2 fuzz target on `decode_reply` in its first fifteen
/// minutes: serde_json's default float parser is not correctly rounded, so
/// a tool-call `input` decoded, re-encoded, and decoded again could hold a
/// different f64 than the model wrote. The workspace enables
/// `float_roundtrip`; this pins that it stays enabled.
#[test]
fn a_tool_call_input_with_a_float_at_the_edge_survives_a_round_trip() {
    let bytes = br#"{"v":1,"content":[{"type":"tool_call","id":"call_1","name":"compute",
        "input":{"x":1.2999999999999999e+73,"y":-0,"z":9007199254740993}}],
        "stop":"tool_call","usage":{"input_tokens":1,"output_tokens":1}}"#;
    let first = decode_reply(bytes).unwrap();
    let again = decode_reply(&serde_json::to_vec(&first).unwrap()).unwrap();
    assert_eq!(again, first);
}
