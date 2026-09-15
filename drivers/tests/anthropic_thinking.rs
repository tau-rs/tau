//! ADR-0007 over a real socket: a reply's thinking blocks are sealed into
//! the bridge, and the second call of a two-turn tool exchange sends them
//! back to the provider unchanged in value — first with the driver alone,
//! then with the real `libtau` tool loop driving it through the kernel.

#![cfg(feature = "anthropic")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use libtau::{prompt, tool_loop, Toolbox};
use serde_json::{json, Value};
use tau_drivers::model::anthropic::{AnthropicConfig, AnthropicDriver, ApiKey, ThinkingMode};
use tau_kernel::abi::{AgentId, Budget, Consumption, Corr, DimKey, DriverId, Name, Namespace};
use tau_kernel::bridge::{Content, ModelReply, ModelRequest, Role, StopReason};
use tau_kernel::driver::{Driver, ToolSchema};
use tau_kernel::kernel::{AbortHandle, BoxFuture, Delivery, Kernel};
use tau_kernel::log::Log;
use tau_kernel::syscall::program;

const REQUEST_THINKING: &str =
    include_str!("../../kernel/tests/fixtures/bridge/request-thinking.json");
const REPLY_THINKING: &str = include_str!("../../kernel/tests/fixtures/bridge/reply-thinking.json");
const ANTHROPIC_REQUEST_THINKING: &str = include_str!("fixtures/anthropic/request-thinking.json");
const ANTHROPIC_RESPONSE_THINKING: &str = include_str!("fixtures/anthropic/response-thinking.json");

/// The provider's answer to the second call: no tool use, the turn is over.
fn end_turn_body() -> String {
    json!({
        "id": "msg_02",
        "type": "message",
        "role": "assistant",
        "model": "claude-opus-5",
        "content": [
            {"type": "thinking", "thinking": "", "signature": "second-turn-sig"},
            {"type": "text", "text": "It is `hello`."}
        ],
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": {"input_tokens": 300, "output_tokens": 40}
    })
    .to_string()
}

fn driver(base_url: &str) -> AnthropicDriver {
    let mut c = AnthropicConfig::new(
        "claude-opus-5",
        ApiKey::new("test-key"),
        8_000,
        1_024,
        5,
        25,
    );
    c.base_url = base_url.to_owned();
    c.timeout = Duration::from_secs(2);
    AnthropicDriver::new(c).unwrap()
}

/// The sealed blocks of a provider body's assistant turn, verbatim.
fn sealed_blocks_of(body: &Value) -> Vec<Value> {
    sealed_in(body.pointer("/messages/1/content").unwrap())
}

fn response_blocks() -> Vec<Value> {
    let response: Value = serde_json::from_str(ANTHROPIC_RESPONSE_THINKING).unwrap();
    sealed_in(response.get("content").unwrap())
}

fn sealed_in(content: &Value) -> Vec<Value> {
    content
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| {
            matches!(
                b.get("type").and_then(Value::as_str),
                Some("thinking" | "redacted_thinking")
            )
        })
        .cloned()
        .collect()
}

#[tokio::test]
async fn the_driver_alone_replays_what_it_sealed() {
    let mut stub = common::start(common::script([
        (200, ANTHROPIC_RESPONSE_THINKING.to_owned()),
        (200, end_turn_body()),
    ]))
    .await;
    let driver = driver(&stub.base_url);

    // Turn one: the request fixture's first user turn only.
    let mut request: ModelRequest = serde_json::from_str(REQUEST_THINKING).unwrap();
    request.messages.truncate(1);
    let (bytes, _) = driver
        .handle(Delivery {
            corr: Corr::new(1),
            from: AgentId::new(1),
            payload: serde_json::to_vec(&request).unwrap(),
        })
        .await;
    let reply: ModelReply = serde_json::from_slice(&bytes).unwrap();
    let expected: Value = serde_json::from_str(REPLY_THINKING).unwrap();
    assert_eq!(serde_json::to_value(&reply).unwrap(), expected);
    let _first = stub.captured.recv().await.unwrap();

    // Turn two: the reply's content appended intact, then the tool result,
    // which is exactly the request fixture.
    request.messages.push(tau_kernel::bridge::Message {
        role: Role::Assistant,
        content: reply.content,
    });
    request.messages.push(tau_kernel::bridge::Message {
        role: Role::User,
        content: vec![Content::ToolResult {
            call_id: "call_1".into(),
            content: "hello".into(),
            is_error: false,
            error_kind: None,
        }],
    });
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        serde_json::from_str::<Value>(REQUEST_THINKING).unwrap()
    );
    let (bytes, _) = driver
        .handle(Delivery {
            corr: Corr::new(2),
            from: AgentId::new(1),
            payload: serde_json::to_vec(&request).unwrap(),
        })
        .await;
    let reply: ModelReply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reply.stop, StopReason::EndTurn);

    let second = stub.captured.recv().await.unwrap().json();
    let expected: Value = serde_json::from_str(ANTHROPIC_REQUEST_THINKING).unwrap();
    assert_eq!(second, expected, "the second body is the recorded one");
    assert_eq!(
        sealed_blocks_of(&second),
        response_blocks(),
        "what the provider sent is what it got back"
    );
}

// --- the real loop, through the kernel -----------------------------------

fn tokio_spawner(fut: BoxFuture<()>) -> AbortHandle {
    let task = tokio::spawn(fut);
    Box::new(move || task.abort())
}

/// The store of ADR-0006 §2: answers `read` with `hello`.
#[derive(Clone, Default)]
struct StoreDriver {
    seen: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Driver for StoreDriver {
    fn handle(&self, request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        self.seen.lock().unwrap().push(request.payload);
        Box::pin(async move {
            (
                b"hello".to_vec(),
                Consumption::from_dims([(DimKey::Tokens, 1)]),
            )
        })
    }

    fn describe(&self) -> Option<ToolSchema> {
        let request: Value = serde_json::from_str(REQUEST_THINKING).unwrap();
        let tool = request.pointer("/tools/0").unwrap();
        Some(ToolSchema {
            description: tool
                .get("description")
                .and_then(Value::as_str)
                .unwrap()
                .to_owned(),
            input_schema: serde_json::to_vec(tool.get("input_schema").unwrap()).unwrap(),
        })
    }
}

#[tokio::test]
async fn the_tool_loop_over_the_driver_replays_thinking_on_its_second_call() {
    let mut stub = common::start(common::script([
        (200, ANTHROPIC_RESPONSE_THINKING.to_owned()),
        (200, end_turn_body()),
    ]))
    .await;
    let driver = driver(&stub.base_url);
    let ceiling = driver.ceiling();
    let store = StoreDriver::default();

    let kernel = Kernel::boot(Log::in_memory(), tokio_spawner);
    let model_cap = kernel
        .register_driver(DriverId::new(Name::new("opus").unwrap()), driver, ceiling)
        .unwrap();
    let store_cap = kernel
        .register_driver(
            DriverId::new(Name::new("store").unwrap()),
            store.clone(),
            Budget::from_dims([(DimKey::Tokens, 16)]),
        )
        .unwrap();
    let ns = Namespace::from_caps([model_cap, store_cap]);

    let outcome: Arc<Mutex<Option<(ModelRequest, ModelReply)>>> = Arc::default();
    let sink = Arc::clone(&outcome);
    kernel
        .spawn_root(
            program(move |h| async move {
                let toolbox = Toolbox::project(&h, &[store_cap]).expect("projection");
                let mut request = prompt("What is stored under key a?", 1024);
                request.system =
                    Some("You are a research assistant with a key-value store.".into());
                let reply = tool_loop(&h, model_cap, &toolbox, &mut request)
                    .await
                    .expect("the loop completes");
                sink.lock().unwrap().replace((request, reply));
                h.exit(b"")
            }),
            ns,
            Budget::from_dims([
                (DimKey::Tokens, 100_000),
                (DimKey::CostMicroUsd, 1_000_000),
                (DimKey::Calls, 8),
            ]),
        )
        .unwrap();
    kernel.drained().await.unwrap();
    kernel.shutdown();

    let (transcript, last) = outcome.lock().unwrap().take().expect("the loop ran");
    assert_eq!(last.stop, StopReason::EndTurn);
    assert_eq!(
        store.seen.lock().unwrap().as_slice(),
        [br#"{"key":"a","op":"read"}"#.to_vec()],
        "the tool was called with the tool_call input bytes"
    );

    // What the loop sent the driver on its second call is the ADR fixture...
    let expected: Value = serde_json::from_str(REQUEST_THINKING).unwrap();
    let mut sent = serde_json::to_value(&transcript).unwrap();
    // ...up to the final assistant turn the loop appended after `end_turn`.
    sent.get_mut("messages")
        .and_then(Value::as_array_mut)
        .unwrap()
        .truncate(3);
    assert_eq!(sent, expected);

    // ...and what the driver put on the wire is the recorded provider body,
    // sealed blocks unchanged in value.
    let _first = stub.captured.recv().await.unwrap();
    let second = stub.captured.recv().await.unwrap().json();
    let expected: Value = serde_json::from_str(ANTHROPIC_REQUEST_THINKING).unwrap();
    assert_eq!(second, expected);
    assert_eq!(sealed_blocks_of(&second), response_blocks());
}

#[tokio::test]
async fn thinking_off_reaches_the_wire_and_a_rejection_is_a_provider_error() {
    let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"thinking.type: disabled is not supported for this model"}}"#;
    let mut stub = common::start(common::script([(400, body.to_owned())])).await;
    let mut c = AnthropicConfig::new(
        "claude-fable-5-1",
        ApiKey::new("test-key"),
        8_000,
        1_024,
        10,
        50,
    );
    c.base_url = stub.base_url.clone();
    c.timeout = Duration::from_secs(2);
    c.thinking = ThinkingMode::Disabled;
    let driver = AnthropicDriver::new(c).unwrap();

    let (bytes, consumed) = driver
        .handle(Delivery {
            corr: Corr::new(1),
            from: AgentId::new(1),
            payload: serde_json::to_vec(&prompt("hi", 8)).unwrap(),
        })
        .await;
    let reply: ModelReply = serde_json::from_slice(&bytes).unwrap();
    let StopReason::Error(err) = reply.stop else {
        panic!("expected an error reply");
    };
    assert_eq!(err.kind, tau_kernel::bridge::ErrorKind::Provider);
    assert!(err.message.contains("thinking.type"), "{}", err.message);
    assert!(consumed.is_empty(), "a 400 bills nothing");

    let sent = stub.captured.recv().await.unwrap().json();
    assert_eq!(sent.get("thinking"), Some(&json!({"type": "disabled"})));
}
