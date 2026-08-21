//! EPIC 7.1: CompletionResponse must be constructible by external crates
//! (no `test-fixtures`), so `LlmBackend` plugins can build their return value.
use tau_ports::{CompletionResponse, StopReason, ToolUse};

#[test]
fn completion_response_new_builds_a_tool_use_response() {
    let tu = ToolUse::new(
        "call-1".into(),
        "echo".into(),
        serde_json::from_value(serde_json::json!({"text": "hi"})).unwrap(),
    );
    let resp = CompletionResponse::new(String::new(), vec![tu], StopReason::ToolUse, None);
    assert_eq!(resp.text, "");
    assert_eq!(resp.tool_uses.len(), 1);
    assert_eq!(resp.stop_reason, StopReason::ToolUse);
    assert!(resp.usage.is_none());
}

#[test]
fn completion_response_new_builds_a_text_response() {
    let resp = CompletionResponse::new("done".into(), Vec::new(), StopReason::EndTurn, None);
    assert_eq!(resp.text, "done");
    assert!(resp.tool_uses.is_empty());
    assert_eq!(resp.stop_reason, StopReason::EndTurn);
}
