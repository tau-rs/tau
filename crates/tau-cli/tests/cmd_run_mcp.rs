//! Narrow E2E: McpBackedTool → CassetteTransport → ToolResult.
//!
//! Full setup_mcp_runtime path (with cassette: URL scheme) is deferred to
//! PR-6's conformance fixture #07.
//!
//! This test exercises `McpBackedTool::invoke` against a cassette-driven
//! `McpClient` end-to-end: the cassette intercepts the `tools/call`
//! JSON-RPC request and replays the recorded response, giving us a
//! verified round-trip from the tau tool layer through to `ToolResult`.

use std::sync::Arc;

use tau_domain::{Capability, Value};
use tau_mcp::cassette::CassetteTransport;
use tau_mcp::contract::{ServerContract, ServerInfo};
use tau_mcp_tokio::bridge::McpBackedTool;
use tau_mcp_tokio::host_lifecycle::{McpClient, McpClientOptions};
use tau_ports::tool::{ToolContent, ToolResult};
use tau_ports::Tool;

fn minimal_cassette() -> Vec<u8> {
    // The cassette covers ONE tools/call with id=2, matching the
    // request id McpClient::call_tool emits — `next_id` starts at 2
    // because initialize=0 and tools/list=1 are reserved for the
    // handshake. CassetteTransport bypasses the handshake; the first
    // call_tool from a fresh McpClient::new uses id=2.
    //
    // CassetteTransport's key-normalised matching ignores the "id" field
    // in the request record and matches on (method, payload) — so the
    // id in the "in" line is cosmetic, but the id in the "out" response
    // line is what gets echoed back to the caller.
    let lines = [
        r#"{"version":1}"#,
        r#"{"dir":"in","kind":"request","id":2,"method":"tools/call","payload":{"name":"echo","arguments":{"message":"hi"}}}"#,
        r#"{"dir":"out","kind":"response","id":2,"payload":{"content":[{"type":"text","text":"hi back"}]}}"#,
    ];
    lines.join("\n").into_bytes()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_backed_tool_round_trips_via_cassette() {
    let transport =
        CassetteTransport::from_jsonl_bytes(&minimal_cassette()).expect("parse cassette");
    // Cast Arc<CassetteTransport> to Arc<dyn Transport> for McpClient::new.
    let transport: Arc<dyn tau_mcp::transport::Transport> = transport;

    let contract = ServerContract {
        protocol_version: "2025-03-26".to_string(),
        server_info: ServerInfo {
            name: "mock".to_string(),
            version: "0.0.0".to_string(),
            additional: Default::default(),
        },
        tools: vec![],
    };
    let client = Arc::new(McpClient::new(
        transport,
        contract,
        McpClientOptions::default(),
    ));

    let tool = McpBackedTool::new(
        "weather.echo",
        client.clone(),
        "echo",
        Vec::new(),
        None,
        serde_json::json!({}),
        "echo tool",
    );

    let mut session = ();
    let args: Value =
        serde_json::from_value(serde_json::json!({"message": "hi"})).expect("args to Value");
    let result: ToolResult = tool.invoke(&mut session, args).await.expect("invoke");

    assert!(!result.is_error, "expected is_error=false");
    assert_eq!(result.content.len(), 1, "expected one content block");
    match &result.content[0] {
        ToolContent::Text { text } => {
            assert_eq!(text.as_str(), "hi back");
        }
        other => panic!("expected ToolContent::Text, got {other:?}"),
    }

    // Clamp threading (spec §12): a tool constructed with Some(effective)
    // must surface it via Tool::effective_capabilities; None (the tool
    // above) must report not-narrowed.
    assert!(tau_ports::Tool::effective_capabilities(&*tool).is_none());

    let declared: Capability = serde_json::from_value(serde_json::json!({
        "kind": "net.http",
        "hosts": ["api.weather.com", "evil.example"],
    }))
    .expect("declared cap");
    let effective: Capability = serde_json::from_value(serde_json::json!({
        "kind": "net.http",
        "hosts": ["api.weather.com"],
    }))
    .expect("effective cap");

    let clamped = McpBackedTool::new(
        "weather.echo2",
        client,
        "echo",
        vec![declared],
        Some(vec![effective.clone()]),
        serde_json::json!({}),
        "echo tool",
    );
    assert_eq!(
        tau_ports::Tool::effective_capabilities(&*clamped),
        Some(&[effective][..])
    );
}
