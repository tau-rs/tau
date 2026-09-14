//! The OpenAI-compatible driver against ADR-0006, over a real socket to a
//! stub: the fixtures map both ways in value, the ceiling is enforced before
//! any HTTP, the output cap is clamped in the bytes actually sent,
//! consumption is what `usage` says at the configured prices, and every
//! failure mode is the error reply the contract names.

#![cfg(feature = "openai")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{Answer, Stub};
use serde_json::{json, Value};
use tau_drivers::model::openai::{ApiKey, OpenAiConfig, OpenAiDriver, OutputCap};
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
const OPENAI_REQUEST: &str = include_str!("fixtures/openai/request.json");
const OPENAI_RESPONSE: &str = include_str!("fixtures/openai/response-tool-call.json");

const KEY: &str = "test-key-not-a-secret";
const MODEL: &str = "Qwen/Qwen3-8B";

fn config(base_url: &str) -> OpenAiConfig {
    let mut c = OpenAiConfig::new(MODEL, Some(ApiKey::new(KEY)), 8_000, 1_024, 5, 25);
    c.base_url = base_url.to_owned();
    c.timeout = Duration::from_secs(2);
    c
}

fn driver(base_url: &str) -> OpenAiDriver {
    OpenAiDriver::new(config(base_url)).unwrap()
}

/// The ADR's example request, seed and all: this column honours it.
fn request() -> ModelRequest {
    serde_json::from_str(REQUEST).unwrap()
}

fn delivery(corr: u64, request: &ModelRequest) -> Delivery {
    Delivery {
        corr: Corr::new(corr),
        from: AgentId::new(1),
        payload: serde_json::to_vec(request).unwrap(),
    }
}

async fn call(driver: &OpenAiDriver, request: &ModelRequest) -> (ModelReply, Consumption) {
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

/// The ADR's reply, as answered by this column's model.
fn expected_reply() -> Value {
    let mut expected: Value = serde_json::from_str(REPLY_TOOL_CALL).unwrap();
    expected
        .as_object_mut()
        .unwrap()
        .insert("model".into(), json!(MODEL));
    expected
}

#[tokio::test]
async fn a_200_maps_to_the_adr_reply_and_bills_what_usage_says() {
    let mut stub = common::start(Answer::Json(200, OPENAI_RESPONSE.into())).await;
    let driver = driver(&stub.base_url);

    let (reply, consumed) = call(&driver, &request()).await;

    assert_eq!(serde_json::to_value(&reply).unwrap(), expected_reply());
    assert_eq!(consumed.get(&DimKey::Tokens), Some(120 + 34));
    assert_eq!(consumed.get(&DimKey::CostMicroUsd), Some(120 * 5 + 34 * 25));
    assert_eq!(consumed.get(&DimKey::Calls), None, "the kernel adds calls");

    let sent = stub.captured.recv().await.unwrap();
    assert!(
        sent.head.starts_with("POST /v1/chat/completions HTTP/1.1"),
        "{}",
        sent.head
    );
    assert_eq!(
        sent.header("authorization"),
        Some("Bearer test-key-not-a-secret")
    );
    assert_eq!(sent.header("content-type"), Some("application/json"));
    let expected_body: Value = serde_json::from_str(OPENAI_REQUEST).unwrap();
    assert_eq!(
        sent.json(),
        expected_body,
        "the bytes on the wire are the recorded body, seed included"
    );
    assert_eq!(driver.in_flight(), 0);
}

#[tokio::test]
async fn without_a_key_no_authorization_header_is_sent() {
    let mut stub = common::start(Answer::Json(200, OPENAI_RESPONSE.into())).await;
    let mut c = config(&stub.base_url);
    c.api_key = None;
    c.input_price_microusd = 0;
    c.output_price_microusd = 0;
    let driver = OpenAiDriver::new(c).unwrap();

    let (reply, consumed) = call(&driver, &request()).await;
    assert_eq!(reply.stop, StopReason::ToolCall);
    assert_eq!(consumed.get(&DimKey::Tokens), Some(154));
    assert_eq!(consumed.get(&DimKey::CostMicroUsd), Some(0));
    let sent = stub.captured.recv().await.unwrap();
    assert_eq!(sent.header("authorization"), None);
}

#[tokio::test]
async fn the_output_cap_is_clamped_in_the_bytes_sent_in_the_configured_field() {
    let mut stub = common::start(Answer::Json(200, OPENAI_RESPONSE.into())).await;
    let driver = driver(&stub.base_url);
    let mut request = request();
    request.max_tokens = 4_096;

    let (reply, _) = call(&driver, &request).await;
    assert_eq!(reply.stop, StopReason::ToolCall);
    let sent = stub.captured.recv().await.unwrap().json();
    assert_eq!(sent.get("max_tokens"), Some(&json!(1_024)));
    assert_eq!(sent.get("max_completion_tokens"), None);

    let mut c = config(&stub.base_url);
    c.output_cap = OutputCap::MaxCompletionTokens;
    let driver = OpenAiDriver::new(c).unwrap();
    let (reply, _) = call(&driver, &request).await;
    assert_eq!(reply.stop, StopReason::ToolCall);
    let sent = stub.captured.recv().await.unwrap().json();
    assert_eq!(sent.get("max_tokens"), None);
    assert_eq!(sent.get("max_completion_tokens"), Some(&json!(1_024)));
}

/// A request whose provider body is exactly 23,025 bytes estimates to 9,210
/// tokens, the number in the ADR's error example.
fn request_estimating_9210() -> ModelRequest {
    let skeleton = json!({
        "model": MODEL,
        "max_tokens": 1024,
        "messages": [{"role": "user", "content": ""}]
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
    let mut stub = common::start(Answer::Json(200, OPENAI_RESPONSE.into())).await;
    let driver = driver(&stub.base_url);

    let (reply, consumed) = call(&driver, &request_estimating_9210()).await;

    let mut expected: Value = serde_json::from_str(REPLY_ERROR).unwrap();
    expected
        .as_object_mut()
        .unwrap()
        .insert("model".into(), json!(MODEL));
    assert_eq!(serde_json::to_value(&reply).unwrap(), expected);
    assert!(consumed.is_empty(), "nothing was billed");
    nothing_sent(&mut stub);
}

#[tokio::test]
async fn exactly_at_the_bound_is_sent() {
    let mut stub = common::start(Answer::Json(200, OPENAI_RESPONSE.into())).await;
    let mut c = config(&stub.base_url);
    c.input_bound = 9_210;
    let driver = OpenAiDriver::new(c).unwrap();

    let (reply, _) = call(&driver, &request_estimating_9210()).await;
    assert_eq!(reply.stop, StopReason::ToolCall);
    assert!(stub.captured.recv().await.is_some());
}

#[tokio::test]
async fn what_the_driver_cannot_honour_is_unsupported_and_nothing_is_sent() {
    let mut stub = common::start(Answer::Json(200, OPENAI_RESPONSE.into())).await;
    let driver = driver(&stub.base_url);

    // A version this driver does not speak.
    let mut future = request();
    future.v = 2;
    let (reply, consumed) = call(&driver, &future).await;
    let err = error_of(&reply);
    assert_eq!(err.kind, ErrorKind::Unsupported);
    assert!(err.message.contains("version 2"), "{}", err.message);
    assert!(consumed.is_empty());
    assert_eq!(reply.model.as_deref(), Some(MODEL));
    assert_eq!(reply.usage, Usage::default());

    // A block in a turn that has no chat-completions shape.
    let mut wrong = request();
    wrong.messages.push(Message {
        role: Role::Assistant,
        content: vec![Content::ToolResult {
            call_id: "call_1".into(),
            content: "hello".into(),
            is_error: false,
            error_kind: None,
        }],
    });
    let (reply, consumed) = call(&driver, &wrong).await;
    let err = error_of(&reply);
    assert_eq!(err.kind, ErrorKind::Unsupported);
    assert!(err.message.contains("tool_result"), "{}", err.message);
    assert!(consumed.is_empty());

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
    // OpenAI's wrapped shape.
    let body = r#"{"error":{"message":"slow down","type":"rate_limit_error","param":null,"code":"rate_limit_exceeded"}}"#;
    let stub = common::start(Answer::Json(429, body.into())).await;
    let driver = driver(&stub.base_url);
    let (reply, consumed) = call(&driver, &request()).await;
    let err = error_of(&reply);
    assert_eq!(err.kind, ErrorKind::Provider);
    assert_eq!(err.message, "HTTP 429 rate_limit_error: slow down");
    assert!(consumed.is_empty());
    assert!(reply.content.is_empty());

    // vLLM's flat shape.
    let body = r#"{"object":"error","message":"The model `nope` does not exist.","type":"NotFoundError","param":null,"code":404}"#;
    let stub = common::start(Answer::Json(404, body.into())).await;
    let driver = self::driver(&stub.base_url);
    let (reply, _) = call(&driver, &request()).await;
    assert_eq!(
        error_of(&reply).message,
        "HTTP 404 NotFoundError: The model `nope` does not exist."
    );

    // Not an error body at all.
    let stub = common::start(Answer::Json(500, "<html>oops</html>".into())).await;
    let driver = self::driver(&stub.base_url);
    let (reply, _) = call(&driver, &request()).await;
    assert_eq!(error_of(&reply).message, "HTTP 500: <html>oops</html>");
}

#[tokio::test]
async fn an_unmappable_200_is_a_provider_error_but_its_usage_is_still_billed() {
    // Arguments that are not JSON: the call happened, the tokens are real.
    let body = json!({
        "model": MODEL,
        "choices": [{"index": 0, "finish_reason": "tool_calls", "message": {
            "role": "assistant", "content": null,
            "tool_calls": [{"id": "call_9", "type": "function",
                "function": {"name": "search", "arguments": "{not json"}}]
        }}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
    });
    let stub = common::start(Answer::Json(200, body.to_string())).await;
    let driver = driver(&stub.base_url);

    let (reply, consumed) = call(&driver, &request()).await;
    let err = error_of(&reply);
    assert_eq!(err.kind, ErrorKind::Provider);
    assert!(err.message.contains("call_9"), "{}", err.message);
    assert_eq!(reply.model.as_deref(), Some(MODEL));
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

    // A finish reason v1 has no name for.
    let body = json!({
        "model": MODEL,
        "choices": [{"index": 0, "finish_reason": "function_call",
            "message": {"role": "assistant", "content": "x"}}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5}
    });
    let stub = common::start(Answer::Json(200, body.to_string())).await;
    let driver = self::driver(&stub.base_url);
    let (reply, consumed) = call(&driver, &request()).await;
    assert!(
        error_of(&reply).message.contains("function_call"),
        "{}",
        error_of(&reply).message
    );
    assert_eq!(consumed.get(&DimKey::Tokens), Some(15));

    // A 200 that is not the response shape at all, but still names usage.
    let body = json!({"usage": {"prompt_tokens": 3, "completion_tokens": 0}, "choices": "nope"});
    let stub = common::start(Answer::Json(200, body.to_string())).await;
    let driver = self::driver(&stub.base_url);
    let (reply, consumed) = call(&driver, &request()).await;
    assert_eq!(error_of(&reply).kind, ErrorKind::Provider);
    assert_eq!(reply.model.as_deref(), Some(MODEL));
    assert_eq!(consumed.get(&DimKey::Tokens), Some(3));

    // A 200 that is not JSON: nothing is known, nothing is billed.
    let stub = common::start(Answer::Json(200, "garbage".into())).await;
    let driver = self::driver(&stub.base_url);
    let (reply, consumed) = call(&driver, &request()).await;
    assert_eq!(error_of(&reply).kind, ErrorKind::Provider);
    assert!(consumed.is_empty());
}

#[tokio::test]
async fn parallel_tool_calls_round_trip_as_consecutive_tool_messages() {
    // The model asks for two things at once.
    let body = json!({
        "model": MODEL,
        "choices": [{"index": 0, "finish_reason": "tool_calls", "message": {
            "role": "assistant", "content": null,
            "tool_calls": [
                {"id": "call_a", "type": "function", "function": {"name": "store", "arguments": "{\"op\":\"read\",\"key\":\"a\"}"}},
                {"id": "call_b", "type": "function", "function": {"name": "store", "arguments": "{\"op\":\"read\",\"key\":\"b\"}"}}
            ]
        }}],
        "usage": {"prompt_tokens": 50, "completion_tokens": 20}
    });
    let mut stub = common::start(Answer::Json(200, body.to_string())).await;
    let driver = driver(&stub.base_url);

    let (reply, _) = call(&driver, &request()).await;
    assert_eq!(reply.stop, StopReason::ToolCall);
    assert_eq!(reply.content.len(), 2);
    let ids: Vec<&str> = reply
        .content
        .iter()
        .map(|c| match c {
            Content::ToolCall { id, .. } => id.as_str(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(ids, ["call_a", "call_b"]);
    stub.captured.recv().await.unwrap();

    // The loop answers both in one user turn (§4); the wire gets two `tool`
    // messages right after the assistant's `tool_calls`.
    let mut next = request();
    next.messages.push(Message {
        role: Role::Assistant,
        content: reply.content,
    });
    next.messages.push(Message {
        role: Role::User,
        content: vec![
            Content::ToolResult {
                call_id: "call_a".into(),
                content: "one".into(),
                is_error: false,
                error_kind: None,
            },
            Content::ToolResult {
                call_id: "call_b".into(),
                content: "missing".into(),
                is_error: true,
                error_kind: None,
            },
        ],
    });
    let (reply, _) = call(&driver, &next).await;
    assert_eq!(reply.stop, StopReason::ToolCall);
    let sent = stub.captured.recv().await.unwrap().json();
    let messages = sent.get("messages").and_then(Value::as_array).unwrap();
    let tail: Vec<&Value> = messages.iter().rev().take(3).collect();
    assert_eq!(
        tail.first().copied(),
        Some(&json!({"role": "tool", "tool_call_id": "call_b", "content": "error: missing"}))
    );
    assert_eq!(
        tail.get(1).copied(),
        Some(&json!({"role": "tool", "tool_call_id": "call_a", "content": "one"}))
    );
    let assistant = tail.get(2).copied().unwrap();
    assert_eq!(assistant.get("role"), Some(&json!("assistant")));
    assert_eq!(assistant.get("content"), Some(&Value::Null));
    assert_eq!(
        assistant
            .get("tool_calls")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(2)
    );
}

#[tokio::test]
async fn a_refused_connection_is_transport() {
    let driver = driver(&common::refused_base_url().await);
    let (reply, consumed) = call(&driver, &request()).await;
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
    let driver = OpenAiDriver::new(c).unwrap();

    let (reply, consumed) = call(&driver, &request()).await;
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
        let mut delivery = delivery(7, &request());
        delivery.corr = corr;
        tokio::spawn(async move { driver.handle(delivery).await })
    };
    // The request is on the wire and the stub is holding it.
    let sent = stub.captured.recv().await.unwrap();
    assert_eq!(sent.json().get("model"), Some(&json!(MODEL)));
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
    let mut stub = common::start(Answer::Json(200, OPENAI_RESPONSE.into())).await;
    let driver = driver(&stub.base_url);
    let request = request();

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
