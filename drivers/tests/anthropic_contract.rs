//! The Anthropic driver against ADR-0006, over a real socket to a stub:
//! the fixtures map both ways in value, the ceiling is enforced before any
//! HTTP, `max_tokens` is clamped in the bytes actually sent, consumption is
//! what `usage` says at the configured prices, and every failure mode is the
//! error reply the contract names.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{Answer, Stub};
use serde_json::{json, Value};
use tau_drivers::model::anthropic::{AnthropicConfig, AnthropicDriver, ApiKey};
use tau_kernel::abi::{AgentId, Consumption, Corr, DimKey};
use tau_kernel::bridge::{
    Content, ErrorKind, Message, ModelError, ModelReply, ModelRequest, Role, StopReason, Usage,
    VERSION,
};
use tau_kernel::driver::Driver;
use tau_kernel::kernel::Delivery;
use tokio::sync::Notify;

const REQUEST: &str = include_str!("../../kernel/tests/fixtures/bridge/request.json");
const REPLY_TOOL_CALL: &str =
    include_str!("../../kernel/tests/fixtures/bridge/reply-tool-call.json");
const REPLY_ERROR: &str = include_str!("../../kernel/tests/fixtures/bridge/reply-error.json");
const ANTHROPIC_REQUEST: &str = include_str!("fixtures/anthropic/request.json");
const ANTHROPIC_RESPONSE: &str = include_str!("fixtures/anthropic/response-tool-use.json");

const KEY: &str = "test-key-not-a-secret";

fn config(base_url: &str) -> AnthropicConfig {
    let mut c = AnthropicConfig::new("claude-opus-5", ApiKey::new(KEY), 8_000, 1_024, 5, 25);
    c.base_url = base_url.to_owned();
    c.timeout = Duration::from_secs(2);
    c
}

fn driver(base_url: &str) -> AnthropicDriver {
    AnthropicDriver::new(config(base_url)).unwrap()
}

/// The ADR's example request, minus the seed Anthropic cannot honour.
fn sendable_request() -> ModelRequest {
    let mut request: ModelRequest = serde_json::from_str(REQUEST).unwrap();
    request.sampling.as_mut().unwrap().seed = None;
    request
}

fn delivery(corr: u64, request: &ModelRequest) -> Delivery {
    Delivery {
        corr: Corr::new(corr),
        from: AgentId::new(1),
        payload: serde_json::to_vec(request).unwrap(),
    }
}

async fn call(driver: &AnthropicDriver, request: &ModelRequest) -> (ModelReply, Consumption) {
    let (bytes, consumed) = driver.handle(delivery(1, request)).await;
    (serde_json::from_slice(&bytes).unwrap(), consumed)
}

fn error_of(reply: &ModelReply) -> &ModelError {
    match &reply.stop {
        StopReason::Error(e) => e,
        other => panic!("expected an error reply, got {other:?}"),
    }
}

fn nothing_sent(stub: &mut Stub) {
    assert!(
        stub.captured.try_recv().is_err(),
        "the stub should not have seen a request"
    );
}

#[tokio::test]
async fn a_200_maps_to_the_adr_reply_and_bills_what_usage_says() {
    let mut stub = common::start(Answer::Json(200, ANTHROPIC_RESPONSE.into())).await;
    let driver = driver(&stub.base_url);

    let (reply, consumed) = call(&driver, &sendable_request()).await;

    let expected: Value = serde_json::from_str(REPLY_TOOL_CALL).unwrap();
    assert_eq!(serde_json::to_value(&reply).unwrap(), expected);
    assert_eq!(consumed.get(&DimKey::Tokens), Some(120 + 34));
    assert_eq!(consumed.get(&DimKey::CostMicroUsd), Some(120 * 5 + 34 * 25));
    assert_eq!(consumed.get(&DimKey::Calls), None, "the kernel adds calls");

    let sent = stub.captured.recv().await.unwrap();
    assert!(
        sent.head.starts_with("POST /v1/messages HTTP/1.1"),
        "{}",
        sent.head
    );
    assert_eq!(sent.header("x-api-key"), Some(KEY));
    assert_eq!(sent.header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(sent.header("content-type"), Some("application/json"));
    let expected_body: Value = serde_json::from_str(ANTHROPIC_REQUEST).unwrap();
    assert_eq!(
        sent.json(),
        expected_body,
        "the bytes on the wire are the recorded body"
    );
    assert_eq!(driver.in_flight(), 0);
}

#[tokio::test]
async fn max_tokens_is_clamped_in_the_bytes_sent() {
    let mut stub = common::start(Answer::Json(200, ANTHROPIC_RESPONSE.into())).await;
    let driver = driver(&stub.base_url);
    let mut request = sendable_request();
    request.max_tokens = 4_096;

    let (reply, _) = call(&driver, &request).await;
    assert_eq!(reply.stop, StopReason::ToolCall);
    let sent = stub.captured.recv().await.unwrap();
    assert_eq!(sent.json().get("max_tokens"), Some(&json!(1_024)));
}

/// A request whose provider body is exactly 23,025 bytes estimates to 9,210
/// tokens, the number in the ADR's error example.
fn request_estimating_9210() -> ModelRequest {
    let skeleton = json!({
        "model": "claude-opus-5",
        "max_tokens": 1024,
        "messages": [{"role": "user", "content": [{"type": "text", "text": ""}]}]
    });
    let overhead = serde_json::to_vec(&skeleton).unwrap().len();
    ModelRequest {
        v: VERSION,
        system: None,
        messages: vec![Message {
            role: Role::User,
            content: vec![Content::Text {
                text: "x".repeat(23_025 - overhead),
            }],
        }],
        tools: vec![],
        max_tokens: 1_024,
        sampling: None,
    }
}

#[tokio::test]
async fn over_the_bound_is_refused_before_any_http_and_matches_the_adr_fixture() {
    let mut stub = common::start(Answer::Json(200, ANTHROPIC_RESPONSE.into())).await;
    let driver = driver(&stub.base_url);

    let (reply, consumed) = call(&driver, &request_estimating_9210()).await;

    let expected: Value = serde_json::from_str(REPLY_ERROR).unwrap();
    assert_eq!(serde_json::to_value(&reply).unwrap(), expected);
    assert!(consumed.is_empty(), "nothing was billed");
    nothing_sent(&mut stub);
}

#[tokio::test]
async fn exactly_at_the_bound_is_sent() {
    let mut stub = common::start(Answer::Json(200, ANTHROPIC_RESPONSE.into())).await;
    let mut c = config(&stub.base_url);
    c.input_bound = 9_210;
    let driver = AnthropicDriver::new(c).unwrap();

    let (reply, _) = call(&driver, &request_estimating_9210()).await;
    assert_eq!(reply.stop, StopReason::ToolCall);
    assert!(stub.captured.recv().await.is_some());
}

#[tokio::test]
async fn what_the_driver_cannot_honour_is_unsupported_and_nothing_is_sent() {
    let mut stub = common::start(Answer::Json(200, ANTHROPIC_RESPONSE.into())).await;
    let driver = driver(&stub.base_url);

    // The ADR's example carries `seed: 42`.
    let seeded: ModelRequest = serde_json::from_str(REQUEST).unwrap();
    let (reply, consumed) = call(&driver, &seeded).await;
    let err = error_of(&reply);
    assert_eq!(err.kind, ErrorKind::Unsupported);
    assert!(err.message.contains("seed"), "{}", err.message);
    assert!(consumed.is_empty());
    assert_eq!(reply.model.as_deref(), Some("claude-opus-5"));
    assert_eq!(reply.usage, Usage::default());

    // A version this driver does not speak.
    let mut future = sendable_request();
    future.v = 2;
    let (reply, _) = call(&driver, &future).await;
    let err = error_of(&reply);
    assert_eq!(err.kind, ErrorKind::Unsupported);
    assert!(err.message.contains("version 2"), "{}", err.message);

    // Not a request at all.
    let (bytes, consumed) = driver
        .handle(Delivery {
            corr: Corr::new(9),
            from: AgentId::new(1),
            payload: b"not json".to_vec(),
        })
        .await;
    let reply: ModelReply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(error_of(&reply).kind, ErrorKind::Unsupported);
    assert!(consumed.is_empty());

    nothing_sent(&mut stub);
}

#[tokio::test]
async fn a_provider_rejection_carries_the_status_and_bills_nothing() {
    let body = r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#;
    let stub = common::start(Answer::Json(429, body.into())).await;
    let driver = driver(&stub.base_url);

    let (reply, consumed) = call(&driver, &sendable_request()).await;
    let err = error_of(&reply);
    assert_eq!(err.kind, ErrorKind::Provider);
    assert_eq!(err.message, "HTTP 429 rate_limit_error: slow down");
    assert!(consumed.is_empty());
    assert!(reply.content.is_empty());

    let stub = common::start(Answer::Json(500, "<html>oops</html>".into())).await;
    let driver = self::driver(&stub.base_url);
    let (reply, _) = call(&driver, &sendable_request()).await;
    assert_eq!(error_of(&reply).message, "HTTP 500: <html>oops</html>");
}

#[tokio::test]
async fn an_unmappable_200_is_a_provider_error_but_its_usage_is_still_billed() {
    let body = json!({
        "model": "claude-opus-5",
        "content": [],
        "stop_reason": "pause_turn",
        "usage": {"input_tokens": 10, "output_tokens": 5}
    });
    let stub = common::start(Answer::Json(200, body.to_string())).await;
    let driver = driver(&stub.base_url);

    let (reply, consumed) = call(&driver, &sendable_request()).await;
    let err = error_of(&reply);
    assert_eq!(err.kind, ErrorKind::Provider);
    assert!(err.message.contains("pause_turn"), "{}", err.message);
    assert_eq!(
        reply.usage,
        Usage {
            input_tokens: 10,
            output_tokens: 5
        },
        "the usage is real even when the body cannot be mapped"
    );
    assert_eq!(consumed.get(&DimKey::Tokens), Some(15));
    assert_eq!(consumed.get(&DimKey::CostMicroUsd), Some(10 * 5 + 5 * 25));

    // A 200 that is not the response shape at all, but still names usage.
    let body = json!({"usage": {"input_tokens": 3, "output_tokens": 0}, "content": "nope"});
    let stub = common::start(Answer::Json(200, body.to_string())).await;
    let driver = self::driver(&stub.base_url);
    let (reply, consumed) = call(&driver, &sendable_request()).await;
    assert_eq!(error_of(&reply).kind, ErrorKind::Provider);
    assert_eq!(reply.model.as_deref(), Some("claude-opus-5"));
    assert_eq!(consumed.get(&DimKey::Tokens), Some(3));

    // A 200 that is not JSON: nothing is known, nothing is billed.
    let stub = common::start(Answer::Json(200, "garbage".into())).await;
    let driver = self::driver(&stub.base_url);
    let (reply, consumed) = call(&driver, &sendable_request()).await;
    assert_eq!(error_of(&reply).kind, ErrorKind::Provider);
    assert!(consumed.is_empty());
}

#[tokio::test]
async fn a_refused_connection_is_transport() {
    let driver = driver(&common::refused_base_url().await);
    let (reply, consumed) = call(&driver, &sendable_request()).await;
    let err = error_of(&reply);
    assert_eq!(err.kind, ErrorKind::Transport);
    assert!(err.message.starts_with("request failed"), "{}", err.message);
    assert!(consumed.is_empty());
    assert_eq!(driver.in_flight(), 0);
}

#[tokio::test]
async fn the_timeout_is_transport() {
    let closed = Arc::new(Notify::new());
    let stub = common::start(Answer::Hang(Arc::clone(&closed))).await;
    let mut c = config(&stub.base_url);
    c.timeout = Duration::from_millis(200);
    let driver = AnthropicDriver::new(c).unwrap();

    let (reply, consumed) = call(&driver, &sendable_request()).await;
    let err = error_of(&reply);
    assert_eq!(err.kind, ErrorKind::Transport);
    assert!(
        err.message.starts_with("no answer within"),
        "{}",
        err.message
    );
    assert!(consumed.is_empty());
    tokio::time::timeout(Duration::from_secs(2), closed.notified())
        .await
        .expect("the stub saw the connection close");
}

#[tokio::test]
async fn abandon_cuts_the_call_and_the_reply_is_transport() {
    let closed = Arc::new(Notify::new());
    let mut stub = common::start(Answer::Hang(Arc::clone(&closed))).await;
    let driver = driver(&stub.base_url);
    let corr = Corr::new(7);

    let call = {
        let driver = driver.clone();
        let mut delivery = delivery(7, &sendable_request());
        delivery.corr = corr;
        tokio::spawn(async move { driver.handle(delivery).await })
    };
    // The request is on the wire and the stub is holding it.
    let sent = stub.captured.recv().await.unwrap();
    assert_eq!(sent.json().get("model"), Some(&json!("claude-opus-5")));
    assert_eq!(driver.in_flight(), 1);

    driver.abandon(corr);

    let (bytes, consumed) = tokio::time::timeout(Duration::from_secs(2), call)
        .await
        .expect("abandon ends the call")
        .unwrap();
    let reply: ModelReply = serde_json::from_slice(&bytes).unwrap();
    let err = error_of(&reply);
    assert_eq!(err.kind, ErrorKind::Transport);
    assert_eq!(err.message, "abandoned by cancel");
    assert!(consumed.is_empty());
    tokio::time::timeout(Duration::from_secs(2), closed.notified())
        .await
        .expect("dropping the request closed the socket");
    assert_eq!(driver.in_flight(), 0);

    // An abandon for a corr the driver never saw, or has finished, is a no-op.
    driver.abandon(corr);
    driver.abandon(Corr::new(99));
}

#[tokio::test]
async fn an_abandon_that_arrives_before_the_call_starts_still_takes_effect() {
    // A live stub, so a call that did go out would come back `tool_call`;
    // the reply says `abandoned` only if the abandon was honoured.
    let mut stub = common::start(Answer::Json(200, ANTHROPIC_RESPONSE.into())).await;
    let driver = driver(&stub.base_url);
    let request = sendable_request();

    // Between `handle` and the first poll.
    let fut = driver.handle(delivery(3, &request));
    assert_eq!(driver.in_flight(), 1, "registered synchronously");
    driver.abandon(Corr::new(3));
    let (bytes, consumed) = fut.await;
    let reply: ModelReply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(error_of(&reply).message, "abandoned by cancel");
    assert!(consumed.is_empty());
    assert_eq!(driver.in_flight(), 0);

    // Before `handle` at all: the delivery was still queued when the
    // sender was cancelled.
    driver.abandon(Corr::new(4));
    let (bytes, _) = driver.handle(delivery(4, &request)).await;
    let reply: ModelReply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(error_of(&reply).message, "abandoned by cancel");

    // A future dropped without being polled leaves nothing behind.
    let fut = driver.handle(delivery(5, &request));
    assert_eq!(driver.in_flight(), 1);
    drop(fut);
    assert_eq!(driver.in_flight(), 0);

    nothing_sent(&mut stub);
}
