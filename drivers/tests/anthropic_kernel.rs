//! The Anthropic driver through the kernel: registered with its own
//! ceiling, a `send` reserves that ceiling, the reply settles it against
//! exactly what the driver reported, and a `cancel` reaches `abandon` while
//! the HTTP call is in flight.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::Answer;
use serde_json::Value;
use tau_drivers::model::anthropic::{AnthropicConfig, AnthropicDriver, ApiKey};
use tau_kernel::abi::{Budget, DimKey, DriverId, Name, Namespace};
use tau_kernel::kernel::{AbortHandle, BoxFuture, Kernel};
use tau_kernel::log::Log;
use tau_kernel::reducer::{Outcome, Status};
use tau_kernel::syscall::{program, CancelMode, Match, WaitFor};
use tokio::sync::Notify;

const REQUEST: &str = include_str!("../../kernel/tests/fixtures/bridge/request.json");
const REPLY_TOOL_CALL: &str =
    include_str!("../../kernel/tests/fixtures/bridge/reply-tool-call.json");
const ANTHROPIC_RESPONSE: &str = include_str!("fixtures/anthropic/response-tool-use.json");

fn tokio_spawner(fut: BoxFuture<()>) -> AbortHandle {
    let task = tokio::spawn(fut);
    Box::new(move || task.abort())
}

fn model_id() -> DriverId {
    DriverId::new(Name::new("opus").unwrap())
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

fn sendable_request() -> Vec<u8> {
    let mut request: Value = serde_json::from_str(REQUEST).unwrap();
    request
        .get_mut("sampling")
        .and_then(Value::as_object_mut)
        .unwrap()
        .remove("seed");
    serde_json::to_vec(&request).unwrap()
}

#[tokio::test]
async fn a_send_reserves_the_ceiling_and_settles_to_the_reported_consumption() {
    let stub = common::start(Answer::Json(200, ANTHROPIC_RESPONSE.into())).await;
    let driver = driver(&stub.base_url);
    let ceiling = driver.ceiling();
    assert_eq!(ceiling.get(&DimKey::Tokens), Some(9_024));
    assert_eq!(ceiling.get(&DimKey::CostMicroUsd), Some(65_600));

    let kernel = Kernel::boot(Log::in_memory(), tokio_spawner);
    let cap = kernel.register_driver(model_id(), driver, ceiling).unwrap();
    let ns = Namespace::from_caps([cap]);
    let payload = sendable_request();
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                let corr = root.send(cap, &payload).unwrap();
                let reply = root.recv(Match::Corr(corr)).await.unwrap();
                let bytes = root.read(reply.payload).unwrap();
                root.exit(&bytes)
            }),
            ns,
            Budget::from_dims([
                (DimKey::Tokens, 10_000),
                (DimKey::CostMicroUsd, 100_000),
                (DimKey::Calls, 2),
            ]),
        )
        .unwrap();
    kernel.drained().await.unwrap();
    kernel.shutdown();

    let Outcome::Exited(blob) = kernel.claim(root).unwrap() else {
        panic!("the root exited on its own");
    };
    let got: Value = serde_json::from_slice(&kernel.read(blob).unwrap()).unwrap();
    let expected: Value = serde_json::from_str(REPLY_TOOL_CALL).unwrap();
    assert_eq!(got, expected, "the agent read the ADR's reply");

    let state = kernel.state();
    let rec = state.agent(root).unwrap();
    assert_eq!(rec.status, Status::Exited);
    assert!(rec.reserved.is_empty(), "settled at the reply");
    assert!(rec.overdraft.is_empty(), "within the ceiling");
    assert_eq!(rec.spent.get(&DimKey::Tokens), Some(&154));
    assert_eq!(
        rec.spent.get(&DimKey::CostMicroUsd),
        Some(&(120 * 5 + 34 * 25))
    );
    assert_eq!(rec.spent.get(&DimKey::Calls), Some(&1));
    assert_eq!(rec.budget.get(&DimKey::Tokens), Some(10_000 - 154));
    assert_eq!(rec.budget.get(&DimKey::CostMicroUsd), Some(100_000 - 1_450));
    assert_eq!(rec.budget.get(&DimKey::Calls), Some(1));
}

#[tokio::test]
async fn a_cancel_reaches_abandon_and_the_socket_closes() {
    let closed = Arc::new(Notify::new());
    let mut stub = common::start(Answer::Hang(Arc::clone(&closed))).await;
    let driver = driver(&stub.base_url);
    let probe = driver.clone();
    let ceiling = driver.ceiling();

    let kernel = Kernel::boot(Log::in_memory(), tokio_spawner);
    let cap = kernel.register_driver(model_id(), driver, ceiling).unwrap();
    let ns = Namespace::from_caps([cap]);
    let payload = sendable_request();
    let cancel_now = Arc::new(Notify::new());
    let cancel_signal = Arc::clone(&cancel_now);
    let child_ns = ns.clone();
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                let child = root
                    .spawn(
                        program(move |child| async move {
                            let corr = child.send(cap, &payload).unwrap();
                            // Held here until the abort: the reply for an
                            // abandoned call is dead letter, and this agent
                            // is gone before it could arrive.
                            let _ = child.recv(Match::Corr(corr)).await;
                            child.exit(b"unreachable")
                        }),
                        child_ns,
                        Budget::from_dims([
                            (DimKey::Tokens, 10_000),
                            (DimKey::CostMicroUsd, 100_000),
                            (DimKey::Calls, 1),
                        ]),
                    )
                    .unwrap();
                cancel_signal.notified().await;
                root.cancel(child, CancelMode::immediate()).unwrap();
                let done = root.wait(WaitFor::Child(child)).await.unwrap();
                assert_eq!(done.outcome, Outcome::Aborted);
                root.exit(b"")
            }),
            ns,
            Budget::from_dims([
                (DimKey::Tokens, 20_000),
                (DimKey::CostMicroUsd, 200_000),
                (DimKey::Calls, 2),
                (DimKey::Depth, 1),
            ]),
        )
        .unwrap();

    // The request is on the wire; the stub is holding the connection.
    let sent = stub.captured.recv().await.unwrap();
    assert_eq!(sent.header("x-api-key"), Some("test-key"));
    assert_eq!(probe.in_flight(), 1);

    cancel_now.notify_one();

    tokio::time::timeout(Duration::from_secs(2), closed.notified())
        .await
        .expect("abandon dropped the request and the stub saw the socket close");
    kernel.drained().await.unwrap();
    kernel.shutdown();
    assert_eq!(probe.in_flight(), 0);

    let state = kernel.state();
    let child = state
        .agents()
        .find_map(|(id, a)| (a.parent == Some(root)).then_some(id))
        .unwrap();
    let rec = state.agent(child).unwrap();
    assert_eq!(rec.status, Status::Aborted);
    assert!(
        !rec.spent.contains_key(&DimKey::Tokens),
        "nothing was billed"
    );
}
