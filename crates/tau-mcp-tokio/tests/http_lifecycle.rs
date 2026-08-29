//! End-to-end Streamable HTTP MCP tests against wiremock-rs.

use std::time::Duration;

use serde_json::json;
use tau_mcp_tokio::host_lifecycle::handshake::HandshakeOptions;
use tau_mcp_tokio::{open, LifecycleError, McpClientOptions};
use tau_ports::CapabilityPlan;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn passthrough_gate() -> std::sync::Arc<dyn tau_ports::DynProcessGate> {
    std::sync::Arc::new(tau_ports::PassthroughGate::new())
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

/// Build a wiremock response that emits the initialize + tools/list +
/// (optionally) tools/call responses as ONE SSE event each per HTTP
/// response (one HTTP request → one response). The MCP host posts
/// each request individually, so each Mock matches one POST.
fn sse_event(msg: serde_json::Value) -> String {
    format!("data: {}\n\n", serde_json::to_string(&msg).unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_happy_path_via_wiremock() {
    let server = MockServer::start().await;
    let url = server.uri();

    // initialize response
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .insert_header("Mcp-Session-Id", "session-xyz")
                .set_body_string(sse_event(json!({
                    "jsonrpc": "2.0",
                    "id": 0,
                    "result": {
                        "protocolVersion": "2025-03-26",
                        "serverInfo": {"name": "mock", "version": "0.0.0"}
                    }
                }))),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;

    // tools/list response (must echo Mcp-Session-Id)
    Mock::given(method("POST"))
        .and(path("/"))
        .and(header("Mcp-Session-Id", "session-xyz"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(sse_event(json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {"tools": []}
                }))),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;

    let client = open(
        &url,
        &empty_plan(),
        passthrough_gate(),
        McpClientOptions {
            handshake: HandshakeOptions {
                handshake_timeout: Duration::from_secs(5),
                ..HandshakeOptions::default()
            },
            ..McpClientOptions::default()
        },
    )
    .await
    .expect("open succeeds");
    assert_eq!(client.contract().server_info.name, "mock");
    assert_eq!(client.contract().tools.len(), 0);
}

/// Session-aware wiremock responder: dispatches on the JSON-RPC `id` of the
/// POSTed request so every POST gets *its own* response body.
///
/// The naive fixture (one `Mock` with one canned body for every POST) cannot
/// serve a two-event `initialize` response, because the `tools/list` POST then
/// replays the same body and `recv_response_for` sees the stale `id=0` while
/// waiting for `id=1` — it skips it, finds nothing else, and the handshake
/// times out. Keying on the request id is the "richer session-aware fixture"
/// the old `#[ignore]` was waiting on.
struct ByRequestId;

impl wiremock::Respond for ByRequestId {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let body: serde_json::Value =
            serde_json::from_slice(&request.body).expect("POST body is a JSON-RPC message");
        let base = ResponseTemplate::new(200).insert_header("Content-Type", "text/event-stream");
        match body.get("id").and_then(serde_json::Value::as_i64) {
            // initialize — TWO events: a notification THEN the response, so the
            // handshake driver's skip-loop is what makes this pass.
            Some(0) => base
                .insert_header("Mcp-Session-Id", "session-xyz")
                .set_body_string(format!(
                    "{}{}",
                    sse_event(json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/progress",
                        "params": {"progress": 50}
                    })),
                    sse_event(json!({
                        "jsonrpc": "2.0",
                        "id": 0,
                        "result": {
                            "protocolVersion": "2025-03-26",
                            "serverInfo": {"name": "mock", "version": "0.0.0"}
                        }
                    })),
                )),
            // tools/list
            Some(1) => base.set_body_string(sse_event(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {"tools": []}
            }))),
            other => panic!("unexpected JSON-RPC id in POST: {other:?}"),
        }
    }
}

/// A leading notification inside the `initialize` SSE body must not derail the
/// handshake: `recv_response_for` skips non-matching messages until it sees the
/// response whose id it is waiting for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_event_sse_response() {
    let server = MockServer::start().await;
    let url = server.uri();

    Mock::given(method("POST"))
        .respond_with(ByRequestId)
        .mount(&server)
        .await;

    let client = open(
        &url,
        &empty_plan(),
        passthrough_gate(),
        McpClientOptions {
            handshake: HandshakeOptions {
                handshake_timeout: Duration::from_secs(5),
                ..HandshakeOptions::default()
            },
            ..McpClientOptions::default()
        },
    )
    .await
    .expect("open succeeds despite leading notification");
    assert_eq!(client.contract().server_info.name, "mock");
    assert_eq!(client.contract().tools.len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_2xx_response_surfaces_as_handshake_error() {
    let server = MockServer::start().await;
    let url = server.uri();

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503).set_body_string("server overloaded"))
        .mount(&server)
        .await;

    let err = require_err(
        open(
            &url,
            &empty_plan(),
            passthrough_gate(),
            McpClientOptions {
                handshake: HandshakeOptions {
                    handshake_timeout: Duration::from_secs(2),
                    ..HandshakeOptions::default()
                },
                ..McpClientOptions::default()
            },
        )
        .await,
        "non_2xx_response",
    );
    match err {
        LifecycleError::Handshake(_) => {} // ok
        other => panic!("expected Handshake error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn url_parse_failure_propagates_for_http() {
    let err = require_err(
        open(
            "http://",
            &empty_plan(),
            passthrough_gate(),
            McpClientOptions::default(),
        )
        .await,
        "url_parse_failure",
    );
    assert!(matches!(err, LifecycleError::UrlParse(_)));
}
