//! Layer 2 — runtime.run (batch) tests.
//!
//! Tests the `runtime.run` dispatcher surface:
//!   - Unknown agent returns -32010 UNKNOWN_AGENT.
//!   - Missing `prompt` param returns -32602 INVALID_PARAMS.
//!   - Missing `agent` param returns -32602 INVALID_PARAMS.
//!   - Missing params object returns -32602 INVALID_PARAMS.
//!
//! Happy-path (agent found + LLM invoked) is deferred to Layer 3 e2e
//! tests because it requires a fully-installed package in the fixture
//! project's scope. All tests here exercise the pre-resolution guard
//! and param-validation paths which fire before any package I/O.

mod common;
use common::Harness;
use std::path::PathBuf;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/handshake-only")
}

/// After a successful handshake, `runtime.run` with an agent_id not
/// present in the project's tau.toml returns -32010 UNKNOWN_AGENT.
#[tokio::test]
async fn run_unknown_agent_returns_32010() {
    let mut h = Harness::new(fixture_dir()).await;
    h.handshake().await;

    h.send_raw(r#"{"jsonrpc":"2.0","id":10,"method":"runtime.run","params":{"agent":"no-such-agent","prompt":"hello"}}"#).await;
    let resp = h.recv().await.expect("no response");
    assert_eq!(resp["id"], 10);
    assert_eq!(
        resp["error"]["code"], -32010,
        "expected UNKNOWN_AGENT, got: {resp}"
    );
    assert!(
        resp["error"]["data"]["agent_id"] == "no-such-agent",
        "expected agent_id in error data, got: {resp}"
    );
}

/// A request missing the required `id` field fails to parse as a Request
/// and returns -32600 INVALID_REQUEST with the spec-correct null id — not a
/// fabricated id 0 that would collide with a legitimate id-0 request.
#[tokio::test]
async fn invalid_request_carries_null_id() {
    let mut h = Harness::new(fixture_dir()).await;
    // No `id` field → not a valid Request.
    h.send_raw(r#"{"jsonrpc":"2.0","method":"meta.ping"}"#)
        .await;
    let resp = h.recv().await.expect("no response");
    assert_eq!(
        resp["error"]["code"], -32600,
        "expected INVALID_REQUEST, got: {resp}"
    );
    assert!(resp["id"].is_null(), "expected null id, got: {resp}");
}

/// `runtime.run` with a missing `prompt` param returns -32602 INVALID_PARAMS.
#[tokio::test]
async fn run_missing_prompt_returns_32602() {
    let mut h = Harness::new(fixture_dir()).await;
    h.handshake().await;

    h.send_raw(
        r#"{"jsonrpc":"2.0","id":11,"method":"runtime.run","params":{"agent":"some-agent"}}"#,
    )
    .await;
    let resp = h.recv().await.expect("no response");
    assert_eq!(resp["id"], 11);
    assert_eq!(
        resp["error"]["code"], -32602,
        "expected INVALID_PARAMS, got: {resp}"
    );
}

/// `runtime.run` with a missing `agent` param returns -32602 INVALID_PARAMS.
#[tokio::test]
async fn run_missing_agent_param_returns_32602() {
    let mut h = Harness::new(fixture_dir()).await;
    h.handshake().await;

    h.send_raw(r#"{"jsonrpc":"2.0","id":12,"method":"runtime.run","params":{"prompt":"hello"}}"#)
        .await;
    let resp = h.recv().await.expect("no response");
    assert_eq!(resp["id"], 12);
    assert_eq!(
        resp["error"]["code"], -32602,
        "expected INVALID_PARAMS, got: {resp}"
    );
}

/// `runtime.run` with a null params value returns -32602 INVALID_PARAMS.
/// (The JSON-RPC `params` field is present but null — not the same as absent.)
#[tokio::test]
async fn run_null_params_returns_32602() {
    let mut h = Harness::new(fixture_dir()).await;
    h.handshake().await;

    h.send_raw(r#"{"jsonrpc":"2.0","id":13,"method":"runtime.run","params":null}"#)
        .await;
    let resp = h.recv().await.expect("no response");
    assert_eq!(resp["id"], 13);
    assert_eq!(
        resp["error"]["code"], -32602,
        "expected INVALID_PARAMS, got: {resp}"
    );
}
