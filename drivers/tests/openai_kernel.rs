//! The OpenAI-compatible driver through the kernel: registered with its own
//! ceiling, a `send` reserves that ceiling, and the reply settles it against
//! exactly what the driver reported.

#![cfg(feature = "openai")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use common::Answer;
use serde_json::{json, Value};
use tau_drivers::model::openai::{ApiKey, OpenAiConfig, OpenAiDriver};
use tau_kernel::abi::{Budget, DimKey, DriverId, Name, Namespace};
use tau_kernel::kernel::{AbortHandle, BoxFuture, Kernel};
use tau_kernel::log::Log;
use tau_kernel::reducer::{Outcome, Status};
use tau_kernel::syscall::{program, Match};

const REQUEST: &str = include_str!("../../kernel/tests/fixtures/bridge/request.json");
const REPLY_TOOL_CALL: &str =
    include_str!("../../kernel/tests/fixtures/bridge/reply-tool-call.json");
const OPENAI_RESPONSE: &str = include_str!("fixtures/openai/response-tool-call.json");

const MODEL: &str = "Qwen/Qwen3-8B";

fn tokio_spawner(fut: BoxFuture<()>) -> AbortHandle {
    let task = tokio::spawn(fut);
    Box::new(move || task.abort())
}

#[tokio::test]
async fn a_send_reserves_the_ceiling_and_settles_to_the_reported_consumption() {
    let stub = common::start(Answer::Json(200, OPENAI_RESPONSE.into())).await;
    let mut c = OpenAiConfig::new(MODEL, Some(ApiKey::new("test-key")), 8_000, 1_024, 5, 25);
    c.base_url = stub.base_url.clone();
    c.timeout = Duration::from_secs(2);
    let driver = OpenAiDriver::new(c).unwrap();
    let ceiling = driver.ceiling();
    assert_eq!(ceiling.get(&DimKey::Tokens), Some(9_024));
    assert_eq!(ceiling.get(&DimKey::CostMicroUsd), Some(65_600));

    let kernel = Kernel::boot(Log::in_memory(), tokio_spawner);
    let id = DriverId::new(Name::new("qwen").unwrap());
    let cap = kernel.register_driver(id, driver, ceiling).unwrap();
    let ns = Namespace::from_caps([cap]);
    let payload = REQUEST.as_bytes().to_vec();
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
    let mut expected: Value = serde_json::from_str(REPLY_TOOL_CALL).unwrap();
    expected
        .as_object_mut()
        .unwrap()
        .insert("model".into(), json!(MODEL));
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
