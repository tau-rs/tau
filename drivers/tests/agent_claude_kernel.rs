//! The `claude` driver through a real kernel (the ADR-0009 style of
//! `sandbox_kernel.rs`): `send` → reply, and the `Consumption` the driver
//! reported is what the requester's record shows settled — the reservation
//! at the ceiling released, `tokens` and `cost_microusd` spent at what the
//! CLI said, one `call` added by the kernel, no overdraft.
//!
//! The CLI is `tau-fake-cli` replaying #130's transcripts behind the
//! `common/agent.rs::ClaudeStub` wrapper.

#![cfg(all(feature = "agent-claude", unix))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "common/agent.rs"]
mod agent;

use std::path::Path;

use serde_json::Value;
use tau_drivers::agent::claude::ClaudeDriver;
use tau_drivers::agent::envelope::Status;
use tau_drivers::agent::wire::{ErrorKind, Limit, Reply, Stop};
use tau_kernel::abi::{Budget, DimKey, DriverId, Name, Namespace};
use tau_kernel::kernel::{AbortHandle, BoxFuture, Kernel};
use tau_kernel::log::Log;
use tau_kernel::reducer::{Agent, Outcome};
use tau_kernel::syscall::{program, Match};

use agent::ClaudeStub;

const PIN: &str = "claude-2.1.272";

fn tokio_spawner(fut: BoxFuture<()>) -> AbortHandle {
    let task = tokio::spawn(fut);
    Box::new(move || task.abort())
}

fn claude_id() -> DriverId {
    DriverId::new(Name::new("claude").unwrap())
}

/// Runs one request through a real kernel: register, `send`, `recv`, exit
/// with the reply bytes. Returns the reply and the root's record after
/// settlement.
async fn run_one(driver: ClaudeDriver, payload: Vec<u8>) -> (Reply, Agent) {
    let ceiling = driver.ceiling();
    let kernel = Kernel::boot(Log::in_memory(), tokio_spawner);
    let cap = kernel
        .register_driver(claude_id(), driver, ceiling)
        .unwrap();
    let ns = Namespace::from_caps([cap]);
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                let corr = root.send(cap, &payload).unwrap();
                let reply = root.recv(Match::Corr(corr)).await.unwrap();
                let bytes = root.read(reply.payload).unwrap();
                root.exit(&bytes)
            }),
            ns,
            // Enough to reserve the ceiling: 400k tokens and $2.
            Budget::from_dims([
                (DimKey::Tokens, 1_000_000),
                (DimKey::CostMicroUsd, 5_000_000),
                (DimKey::Calls, 2),
            ]),
        )
        .unwrap();
    kernel.drained().await.unwrap();
    kernel.shutdown();
    let outcome = kernel.claim(root).unwrap();
    let Outcome::Exited(blob) = outcome else {
        panic!("the root exited on its own: {outcome:?}");
    };
    let bytes = kernel.read(blob).unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    let reply: Reply = serde_json::from_value(value.clone())
        .unwrap_or_else(|e| panic!("not an agent reply: {e}\n{value:#}"));
    let record = kernel.state().agent(root).unwrap().clone();
    assert!(record.reserved.is_empty(), "settled at the reply");
    (reply, record)
}

fn driver(dir: &Path, run: &str) -> (ClaudeStub, ClaudeDriver) {
    let script = agent::replay_script(dir, PIN, run);
    let stub = ClaudeStub::new(dir);
    let config = stub.config(&script, dir);
    (stub, ClaudeDriver::new(config).unwrap())
}

#[tokio::test]
async fn send_returns_the_reply_and_the_reported_usage_is_what_settles() {
    let dir = agent::Temp::new("kernel-hello");
    let (_stub, driver) = driver(dir.path(), "1-hello");
    let (reply, root) = run_one(
        driver,
        agent::run_payload("write hello.txt containing hello"),
    )
    .await;
    assert_eq!(reply.stop, Stop::Done);
    assert_eq!(reply.envelope.unwrap().status, Status::Ok);
    assert_eq!(
        reply.session.as_deref(),
        Some("affe155e-9ed1-4477-95f0-c17d40bd9a89")
    );

    // ADR-0013 §6: what the CLI reported is what was spent, and the
    // reservation at the ceiling was released.
    assert_eq!(root.spent.get(&DimKey::Tokens), Some(&44_506));
    assert_eq!(root.spent.get(&DimKey::CostMicroUsd), Some(&259_148));
    assert_eq!(
        root.spent.get(&DimKey::Calls),
        Some(&1),
        "the kernel's call"
    );
    assert_eq!(root.budget.get(&DimKey::Tokens), Some(1_000_000 - 44_506));
    assert_eq!(
        root.budget.get(&DimKey::CostMicroUsd),
        Some(5_000_000 - 259_148)
    );
    assert!(root.overdraft.is_empty(), "under the ceiling");
    assert!(
        !root.spent.contains_key(&DimKey::WallMs) || root.spent.get(&DimKey::WallMs) == Some(&0)
    );
}

#[tokio::test]
async fn a_limit_settles_at_real_usage_and_the_session_is_there_to_resume() {
    let dir = agent::Temp::new("kernel-turns");
    let (_stub, driver) = driver(dir.path(), "6-max-turns-1");
    let (reply, root) = run_one(driver, agent::run_payload("write hello.txt")).await;
    assert_eq!(reply.stop, Stop::Limit(Limit::Turns));
    assert!(reply.session.is_some(), "what a `resume` takes");
    assert_eq!(
        root.spent.get(&DimKey::Tokens),
        Some(&(2 + 10_023 + 10_135 + 125))
    );
    assert_eq!(root.spent.get(&DimKey::CostMicroUsd), Some(&109_386));
    assert!(root.overdraft.is_empty());
}

#[tokio::test]
async fn a_logged_out_cli_refuses_through_the_kernel_and_bills_nothing() {
    let dir = agent::Temp::new("kernel-logged-out");
    let script = agent::replay_script(dir.path(), PIN, "1-hello");
    let stub = ClaudeStub::new(dir.path());
    let config = stub.config(&script, dir.path());
    stub.set_logged_in(false);
    let driver = ClaudeDriver::new(config).unwrap();
    let (reply, root) = run_one(driver, agent::run_payload("x")).await;
    let Stop::Error(error) = reply.stop else {
        panic!("{:?}", reply.stop)
    };
    assert_eq!(error.kind, ErrorKind::Unavailable);
    assert_eq!(root.spent.get(&DimKey::Tokens), None, "nothing billed");
    assert_eq!(root.spent.get(&DimKey::CostMicroUsd), None);
    assert_eq!(root.spent.get(&DimKey::Calls), Some(&1), "the send itself");
    assert_eq!(
        root.budget.get(&DimKey::Tokens),
        Some(1_000_000),
        "released whole"
    );
    assert_eq!(
        stub.probes(),
        2,
        "the probe at construction and one re-probe"
    );
}
