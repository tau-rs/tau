//! End-to-end stdio MCP tests against the in-tree mock-mcp-server.
//!
//! Scenario driving: CLI-arg approach — each test passes a positional
//! argument to the fixture binary via the stdio URL (`stdio:<path> <scenario>`).
//! The URL parser whitespace-splits the command, so `argv[1]` becomes the
//! scenario name inside the fixture. This is race-safe under nextest (no
//! shared env mutation).

mod common;

use std::time::Duration;

use tau_mcp_tokio::host_lifecycle::handshake::HandshakeOptions;
use tau_mcp_tokio::{open, LifecycleError, McpClientOptions};
use tau_ports::CapabilityPlan;

use crate::common::{mock_server_path, passthrough_gate};

/// URL for the default happy-path scenario.
fn stdio_url() -> String {
    format!("stdio:{}", mock_server_path().to_string_lossy())
}

/// URL that passes a scenario name as argv[1] to the fixture binary.
fn stdio_url_with_scenario(scenario: &str) -> String {
    format!(
        "stdio:{} {}",
        mock_server_path().to_string_lossy(),
        scenario
    )
}

fn empty_plan() -> CapabilityPlan {
    CapabilityPlan::new(vec![], None, None)
}

/// Extract a `LifecycleError` from a Result without requiring `T: Debug`.
fn require_err(result: Result<impl Sized, LifecycleError>, ctx: &str) -> LifecycleError {
    match result {
        Ok(_) => panic!("{ctx}: expected Err but got Ok"),
        Err(e) => e,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_happy_path() {
    let client = open(
        &stdio_url(),
        &empty_plan(),
        passthrough_gate(),
        McpClientOptions::default(),
    )
    .await
    .expect("open succeeds");
    assert_eq!(client.contract().server_info.name, "mock");
    assert_eq!(client.contract().tools.len(), 1);
    assert_eq!(client.contract().tools[0].name, "echo");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tools_call_echo_round_trips() {
    let client = open(
        &stdio_url(),
        &empty_plan(),
        passthrough_gate(),
        McpClientOptions::default(),
    )
    .await
    .expect("open succeeds");
    let resp = client
        .call_tool("echo", serde_json::json!({"message": "hi"}))
        .await
        .expect("call succeeds");
    assert_eq!(resp.content.len(), 1);
    match &resp.content[0] {
        tau_mcp::protocol::tools::ContentBlock::Text { text } => assert_eq!(text, "hi"),
        other => panic!("expected Text block, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_timeout_fires_under_slow_scenario() {
    let options = McpClientOptions {
        handshake: HandshakeOptions {
            handshake_timeout: Duration::from_millis(500),
            ..HandshakeOptions::default()
        },
        ..McpClientOptions::default()
    };
    let result = open(
        &stdio_url_with_scenario("handshake_slow"),
        &empty_plan(),
        passthrough_gate(),
        options,
    )
    .await;
    let err = require_err(result, "handshake_timeout_fires_under_slow_scenario");
    match err {
        LifecycleError::Handshake(tau_mcp_tokio::host_lifecycle::HandshakeError::Timeout {
            ..
        }) => {}
        other => panic!("expected Timeout, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_refuses_initialize_surfaces_server_error() {
    let result = open(
        &stdio_url_with_scenario("refuse_initialize"),
        &empty_plan(),
        passthrough_gate(),
        McpClientOptions::default(),
    )
    .await;
    let err = require_err(result, "server_refuses_initialize_surfaces_server_error");
    match err {
        LifecycleError::Handshake(tau_mcp_tokio::host_lifecycle::HandshakeError::ServerError {
            code,
            ..
        }) => {
            assert_eq!(code, -32603);
        }
        other => panic!("expected ServerError, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_crash_mid_call_surfaces_transport_error() {
    let client = open(
        &stdio_url_with_scenario("crash_on_call"),
        &empty_plan(),
        passthrough_gate(),
        McpClientOptions::default(),
    )
    .await
    .expect("handshake succeeds");
    let result = client
        .call_tool("echo", serde_json::json!({"message": "x"}))
        .await;
    assert!(result.is_err(), "call should fail when child crashed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn url_parse_failure_propagates() {
    let result = open(
        "ws://invalid",
        &empty_plan(),
        passthrough_gate(),
        McpClientOptions::default(),
    )
    .await;
    let err = require_err(result, "url_parse_failure_propagates");
    match err {
        LifecycleError::UrlParse(_) => {}
        other => panic!("expected UrlParse, got {other:?}"),
    }
}
