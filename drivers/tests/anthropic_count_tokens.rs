//! The Anthropic driver in `InputEstimate::CountTokens` mode, over a real
//! socket to a stub that routes on the request path: the count call goes
//! out first with the subset body `count_tokens` accepts, the exact count is
//! what the bound is checked against, and a count that fails — provider,
//! transport, abandon — ends the flight before `/v1/messages` is touched.

#![cfg(feature = "anthropic")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{Answer, Captured, Stub};
use serde_json::{json, Value};
use tau_drivers::model::anthropic::{AnthropicConfig, AnthropicDriver, ApiKey, InputEstimate};
use tau_kernel::abi::{AgentId, Consumption, Corr, DimKey};
use tau_kernel::bridge::{ErrorKind, ModelError, ModelReply, ModelRequest, StopReason, Usage};
use tau_kernel::driver::Driver;
use tau_kernel::kernel::Delivery;
use tokio::sync::Notify;

const REQUEST: &str = include_str!("../../kernel/tests/fixtures/bridge/request.json");
const ANTHROPIC_REQUEST: &str = include_str!("fixtures/anthropic/request.json");
const ANTHROPIC_RESPONSE: &str = include_str!("fixtures/anthropic/response-tool-use.json");

const COUNT_PATH: &str = "/v1/messages/count_tokens";
const MESSAGES_PATH: &str = "/v1/messages";
const KEY: &str = "test-key-not-a-secret";

fn config(base_url: &str) -> AnthropicConfig {
    let mut c = AnthropicConfig::new("claude-opus-5", ApiKey::new(KEY), 8_000, 1_024, 5, 25);
    c.base_url = base_url.to_owned();
    c.timeout = Duration::from_secs(2);
    c.estimate = InputEstimate::CountTokens;
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

fn count(n: u64) -> Answer {
    Answer::Json(200, json!({"input_tokens": n}).to_string())
}

fn answered() -> Answer {
    Answer::Json(200, ANTHROPIC_RESPONSE.into())
}

/// Every request the stub has seen so far, in order.
fn drain(stub: &mut Stub) -> Vec<Captured> {
    let mut seen = Vec::new();
    while let Ok(c) = stub.captured.try_recv() {
        seen.push(c);
    }
    seen
}

/// The count went out, and nothing else did.
fn only_the_count_was_sent(stub: &mut Stub) {
    let seen = drain(stub);
    let paths: Vec<&str> = seen.iter().map(Captured::path).collect();
    assert_eq!(paths, [COUNT_PATH], "only the count should have gone out");
}

#[tokio::test]
async fn under_the_bound_the_count_goes_first_and_the_call_follows() {
    let mut stub = common::start_routed(vec![
        (COUNT_PATH, count(7_999)),
        (MESSAGES_PATH, answered()),
    ])
    .await;
    let driver = driver(&stub.base_url);

    let (reply, consumed) = call(&driver, &sendable_request()).await;
    assert_eq!(reply.stop, StopReason::ToolCall);
    assert_eq!(
        consumed.get(&DimKey::Tokens),
        Some(120 + 34),
        "the count is not billed; the call's usage is"
    );
    assert_eq!(consumed.get(&DimKey::CostMicroUsd), Some(120 * 5 + 34 * 25));

    let seen = drain(&mut stub);
    let paths: Vec<&str> = seen.iter().map(Captured::path).collect();
    assert_eq!(paths, [COUNT_PATH, MESSAGES_PATH]);

    let count = seen.first().unwrap();
    assert_eq!(count.header("x-api-key"), Some(KEY));
    assert_eq!(count.header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(count.header("content-type"), Some("application/json"));
    // The count body is the main body minus what `count_tokens` does not
    // accept: `max_tokens` and the sampling knobs.
    let mut expected: Value = serde_json::from_str(ANTHROPIC_REQUEST).unwrap();
    let object = expected.as_object_mut().unwrap();
    for key in ["max_tokens", "temperature", "top_p", "stop_sequences"] {
        object.remove(key);
    }
    assert_eq!(count.json(), expected);

    let main: Value = serde_json::from_str(ANTHROPIC_REQUEST).unwrap();
    assert_eq!(
        seen.get(1).unwrap().json(),
        main,
        "the main body is unchanged"
    );
    assert_eq!(driver.in_flight(), 0);
}

#[tokio::test]
async fn exactly_at_the_bound_is_sent() {
    let mut stub = common::start_routed(vec![
        (COUNT_PATH, count(8_000)),
        (MESSAGES_PATH, answered()),
    ])
    .await;
    let driver = driver(&stub.base_url);

    let (reply, _) = call(&driver, &sendable_request()).await;
    assert_eq!(reply.stop, StopReason::ToolCall);
    let paths: Vec<String> = drain(&mut stub)
        .iter()
        .map(|c| c.path().to_owned())
        .collect();
    assert_eq!(paths, [COUNT_PATH, MESSAGES_PATH]);
}

#[tokio::test]
async fn over_the_bound_is_refused_with_the_exact_count_and_the_call_never_happens() {
    let mut stub = common::start_routed(vec![
        (COUNT_PATH, count(8_001)),
        (MESSAGES_PATH, answered()),
    ])
    .await;
    let driver = driver(&stub.base_url);

    let (reply, consumed) = call(&driver, &sendable_request()).await;
    let err = error_of(&reply);
    assert_eq!(err.kind, ErrorKind::OverCeiling);
    assert_eq!(err.message, "input counted at 8001 tokens, bound is 8000");
    assert_eq!(reply.model.as_deref(), Some("claude-opus-5"));
    assert_eq!(reply.usage, Usage::default());
    assert!(consumed.is_empty(), "nothing was billed");
    only_the_count_was_sent(&mut stub);
    assert_eq!(driver.in_flight(), 0);
}

#[tokio::test]
async fn a_count_the_provider_rejects_is_provider_and_the_call_never_happens() {
    let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad tool"}}"#;
    let mut stub = common::start_routed(vec![
        (COUNT_PATH, Answer::Json(400, body.into())),
        (MESSAGES_PATH, answered()),
    ])
    .await;
    let driver = driver(&stub.base_url);

    let (reply, consumed) = call(&driver, &sendable_request()).await;
    let err = error_of(&reply);
    assert_eq!(err.kind, ErrorKind::Provider);
    assert_eq!(err.message, "HTTP 400 invalid_request_error: bad tool");
    assert!(consumed.is_empty());
    only_the_count_was_sent(&mut stub);
}

#[tokio::test]
async fn a_count_that_is_not_a_count_is_provider_and_the_call_never_happens() {
    for body in ["garbage", r#"{"tokens": 12}"#, r#"{"input_tokens": "12"}"#] {
        let mut stub = common::start_routed(vec![
            (COUNT_PATH, Answer::Json(200, body.into())),
            (MESSAGES_PATH, answered()),
        ])
        .await;
        let driver = driver(&stub.base_url);

        let (reply, consumed) = call(&driver, &sendable_request()).await;
        let err = error_of(&reply);
        assert_eq!(err.kind, ErrorKind::Provider, "{body}");
        assert!(
            err.message.starts_with("count_tokens"),
            "{body}: {}",
            err.message
        );
        assert!(consumed.is_empty());
        only_the_count_was_sent(&mut stub);
    }
}

#[tokio::test]
async fn abandon_during_the_count_is_transport_and_the_call_never_happens() {
    let closed = Arc::new(Notify::new());
    let mut stub = common::start_routed(vec![
        (COUNT_PATH, Answer::Hang(Arc::clone(&closed))),
        (MESSAGES_PATH, answered()),
    ])
    .await;
    let driver = driver(&stub.base_url);
    let corr = Corr::new(7);

    let call = {
        let driver = driver.clone();
        let delivery = delivery(7, &sendable_request());
        tokio::spawn(async move { driver.handle(delivery).await })
    };
    // The count is on the wire and the stub is holding it.
    let sent = stub.captured.recv().await.unwrap();
    assert_eq!(sent.path(), COUNT_PATH);
    assert_eq!(driver.in_flight(), 1);

    driver.abandon(corr);

    let (bytes, consumed) = tokio::time::timeout(Duration::from_secs(2), call)
        .await
        .expect("abandon ends the count")
        .unwrap();
    let reply: ModelReply = serde_json::from_slice(&bytes).unwrap();
    let err = error_of(&reply);
    assert_eq!(err.kind, ErrorKind::Transport);
    assert_eq!(err.message, "abandoned by cancel");
    assert!(consumed.is_empty());
    tokio::time::timeout(Duration::from_secs(2), closed.notified())
        .await
        .expect("dropping the count closed the socket");
    assert_eq!(driver.in_flight(), 0);
    assert!(
        stub.captured.try_recv().is_err(),
        "nothing followed the abandoned count"
    );
}

#[tokio::test]
async fn an_abandon_that_arrives_before_the_count_still_takes_effect() {
    let mut stub =
        common::start_routed(vec![(COUNT_PATH, count(1)), (MESSAGES_PATH, answered())]).await;
    let driver = driver(&stub.base_url);
    let request = sendable_request();

    driver.abandon(Corr::new(4));
    let (bytes, consumed) = driver.handle(delivery(4, &request)).await;
    let reply: ModelReply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(error_of(&reply).message, "abandoned by cancel");
    assert!(consumed.is_empty());
    assert!(
        stub.captured.try_recv().is_err(),
        "neither the count nor the call went out"
    );
}

#[tokio::test]
async fn the_count_timeout_is_transport_and_the_call_never_happens() {
    let closed = Arc::new(Notify::new());
    let mut stub = common::start_routed(vec![
        (COUNT_PATH, Answer::Hang(Arc::clone(&closed))),
        (MESSAGES_PATH, answered()),
    ])
    .await;
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
    only_the_count_was_sent(&mut stub);
}

#[tokio::test]
async fn the_default_mode_is_bytes_and_never_counts() {
    assert_eq!(InputEstimate::default(), InputEstimate::Bytes);
    let mut c = config("http://127.0.0.1:9");
    c.estimate = InputEstimate::default();
    assert_eq!(
        AnthropicConfig::new("m", ApiKey::new(KEY), 1, 1, 1, 1).estimate,
        InputEstimate::Bytes
    );

    // A stub with no count route at all: in bytes mode it is never asked.
    let mut stub = common::start_routed(vec![(MESSAGES_PATH, answered())]).await;
    c.base_url = stub.base_url.clone();
    let driver = AnthropicDriver::new(c).unwrap();
    let (reply, _) = call(&driver, &sendable_request()).await;
    assert_eq!(reply.stop, StopReason::ToolCall);
    let paths: Vec<String> = drain(&mut stub)
        .iter()
        .map(|c| c.path().to_owned())
        .collect();
    assert_eq!(paths, [MESSAGES_PATH]);
}

#[tokio::test]
async fn the_stub_answers_404_off_its_routes() {
    // The routing itself: a driver in count mode against a stub that only
    // knows `/v1/messages` gets a provider error from the count, and the
    // error names the endpoint, so a wrong path cannot pass as a wrong body.
    let mut stub = common::start_routed(vec![(MESSAGES_PATH, answered())]).await;
    let driver = driver(&stub.base_url);
    let (reply, _) = call(&driver, &sendable_request()).await;
    let err = error_of(&reply);
    assert_eq!(err.kind, ErrorKind::Provider);
    assert_eq!(
        err.message,
        "HTTP 404 not_found_error: no route for /v1/messages/count_tokens"
    );
    only_the_count_was_sent(&mut stub);
}
