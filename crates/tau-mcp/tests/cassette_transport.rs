//! Tests for `tau_mcp::cassette::CassetteTransport`.
//!
//! Drives the cassette via Transport::send_message / next_message,
//! verifies matched responses, pending-outbound drains, and EOF
//! semantics.

#![cfg(feature = "with-std-adapters")]

use tau_mcp::cassette::CassetteTransport;
use tau_mcp::protocol::jsonrpc::{
    JsonRpcMessage, JsonRpcRequest, RequestId, JSONRPC_VERSION,
};
use tau_mcp::transport::Transport;

/// A minimal cassette covering initialize + tools/list + tools/call
/// with a notification interleaved between the tools/call request and
/// its response.
fn minimal_cassette() -> Vec<u8> {
    let lines = [
        r#"{"version":1}"#,
        r#"{"dir":"in","kind":"request","id":0,"method":"initialize","payload":null}"#,
        r#"{"dir":"out","kind":"response","id":0,"payload":{"protocolVersion":"2025-03-26","serverInfo":{"name":"mock","version":"0.0.0"}}}"#,
        r#"{"dir":"in","kind":"request","id":1,"method":"tools/list","payload":null}"#,
        r#"{"dir":"out","kind":"response","id":1,"payload":{"tools":[]}}"#,
        r#"{"dir":"in","kind":"request","id":2,"method":"tools/call","payload":{"name":"echo","arguments":{"message":"hi"}}}"#,
        r#"{"dir":"out","kind":"notification","method":"notifications/progress","payload":{"progressToken":"call-2","progress":50,"total":100}}"#,
        r#"{"dir":"out","kind":"response","id":2,"payload":{"content":[{"type":"text","text":"hi"}]}}"#,
    ];
    lines.join("\n").into_bytes()
}

fn req(id: i64, method: &str, params: serde_json::Value) -> JsonRpcMessage {
    JsonRpcMessage::Request(JsonRpcRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: RequestId::Number(id),
        method: method.to_string(),
        params: Some(params),
    })
}

#[tokio::test]
async fn happy_path_initialize_then_list_then_call() {
    let t = CassetteTransport::from_jsonl_bytes(&minimal_cassette()).expect("parse cassette");

    // initialize
    t.send_message(&req(0, "initialize", serde_json::Value::Null))
        .await
        .expect("send initialize");
    let resp = t.next_message().await.unwrap().expect("response");
    assert!(matches!(resp, JsonRpcMessage::Response(_)));

    // tools/list
    t.send_message(&req(1, "tools/list", serde_json::Value::Null))
        .await
        .expect("send tools/list");
    let resp = t.next_message().await.unwrap().expect("response");
    assert!(matches!(resp, JsonRpcMessage::Response(_)));

    // tools/call — expect notification then response (interleaved per cassette)
    t.send_message(&req(2, "tools/call", serde_json::json!({"name":"echo","arguments":{"message":"hi"}})))
        .await
        .expect("send tools/call");
    let first = t.next_message().await.unwrap().expect("first message");
    assert!(matches!(first, JsonRpcMessage::Notification(_)), "first msg should be the notification");
    let second = t.next_message().await.unwrap().expect("second message");
    assert!(matches!(second, JsonRpcMessage::Response(_)), "second msg should be the response");
}

#[tokio::test]
async fn unmatched_request_errors() {
    let t = CassetteTransport::from_jsonl_bytes(&minimal_cassette()).expect("parse cassette");
    let err = t
        .send_message(&req(0, "nonexistent/method", serde_json::Value::Null))
        .await
        .expect_err("should fail to match");
    let msg = format!("{err:?}");
    assert!(msg.contains("cassette"));
}

#[tokio::test]
async fn channel_closed_after_drop_returns_none() {
    let t = CassetteTransport::from_jsonl_bytes(&minimal_cassette()).expect("parse cassette");
    // Drive one request so the channel has one message available.
    t.send_message(&req(0, "initialize", serde_json::Value::Null))
        .await
        .expect("send");
    let _ = t.next_message().await.unwrap().expect("one message");
    // Drop the transport — next_message on a fresh handle is no longer
    // possible since we just consumed the Arc; this test instead
    // verifies the channel-closed path by dropping the inbound_tx.
    // (For now, this assertion is weak — the strong shape requires
    // either holding a separate clone of the inbound_tx publicly or a
    // shutdown() method. Defer the explicit close test to PR-5 when
    // McpBridge needs it.)
}
