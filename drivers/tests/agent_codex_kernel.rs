//! The `codex` driver through a real kernel (the ADR-0009 style of
//! `sandbox_kernel.rs`): `send` → reply, and the `Consumption` the driver
//! reported is what the requester's record shows settled — the reservation
//! at the ceiling released, `tokens` spent at what the CLI said, no cost
//! (codex states none and no price is configured), one `call` added by the
//! kernel, no overdraft.
//!
//! The CLI is `tau-fake-cli` replaying the #128 transcripts behind the
//! `common/agent.rs::CodexStub` wrapper.

#![cfg(all(feature = "agent-codex", unix))]
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
use tau_drivers::agent::codex::CodexDriver;
use tau_drivers::agent::envelope::Status;
use tau_drivers::agent::wire::{ErrorKind, Reply, Stop};
use tau_kernel::abi::{Budget, DimKey, DriverId, Name, Namespace};
use tau_kernel::kernel::{AbortHandle, BoxFuture, Kernel};
use tau_kernel::log::Log;
use tau_kernel::reducer::{Agent, Outcome};
use tau_kernel::syscall::{program, Match};

use agent::CodexStub;

const PIN: &str = "codex-0.154.0";

fn tokio_spawner(fut: BoxFuture<()>) -> AbortHandle {
    let task = tokio::spawn(fut);
    Box::new(move || task.abort())
}

fn codex_id() -> DriverId {
    DriverId::new(Name::new("codex").unwrap())
}

/// Runs one request through a real kernel: register, `send`, `recv`, exit
/// with the reply bytes. Returns the reply and the root's record after
/// settlement.
async fn run_one(driver: CodexDriver, payload: Vec<u8>) -> (Reply, Agent) {
    let ceiling = driver.ceiling();
    let kernel = Kernel::boot(Log::in_memory(), tokio_spawner);
    let cap = kernel.register_driver(codex_id(), driver, ceiling).unwrap();
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

fn driver(dir: &Path, run: &str) -> (CodexStub, CodexDriver) {
    let script = agent::replay_script(dir, PIN, run);
    let stub = CodexStub::new(dir);
    let config = stub.config(&script, dir);
    (stub, CodexDriver::new(config).unwrap())
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
        Some("01a0dd5b-4680-7482-a0c0-c96ba75b1f7f")
    );
    assert_eq!(reply.mode.as_deref(), Some("Logged in using ChatGPT"));

    // ADR-0013 §6: what the CLI reported is what was spent, and the
    // reservation at the ceiling was released.
    assert_eq!(root.spent.get(&DimKey::Tokens), Some(&(71_179 + 490)));
    assert_eq!(
        root.spent.get(&DimKey::CostMicroUsd),
        None,
        "codex states no cost and no price is configured"
    );
    assert_eq!(
        root.spent.get(&DimKey::Calls),
        Some(&1),
        "the kernel's call"
    );
    assert_eq!(
        root.budget.get(&DimKey::Tokens),
        Some(1_000_000 - 71_179 - 490)
    );
    assert_eq!(
        root.budget.get(&DimKey::CostMicroUsd),
        Some(5_000_000),
        "the $2 reservation came back whole"
    );
    assert!(root.overdraft.is_empty(), "under the ceiling");
}

#[tokio::test]
async fn a_resume_settles_its_own_turn_on_the_same_thread() {
    let dir = agent::Temp::new("kernel-resume");
    let (_stub, driver) = driver(dir.path(), "4-resume");
    let (reply, root) = run_one(
        driver,
        agent::resume_payload(
            "01a0dd5b-4680-7482-a0c0-c96ba75b1f7f",
            "Also write bye.txt containing bye",
        ),
    )
    .await;
    assert_eq!(reply.stop, Stop::Done);
    assert_eq!(reply.envelope.unwrap().artifacts[0].path, "bye.txt");
    assert_eq!(
        reply.session.as_deref(),
        Some("01a0dd5b-4680-7482-a0c0-c96ba75b1f7f")
    );
    assert_eq!(root.spent.get(&DimKey::Tokens), Some(&(119_001 + 764)));
    assert!(root.overdraft.is_empty());
}

#[tokio::test]
async fn a_logged_out_cli_refuses_through_the_kernel_and_bills_nothing() {
    let dir = agent::Temp::new("kernel-logged-out");
    let script = agent::replay_script(dir.path(), PIN, "1-hello");
    let stub = CodexStub::new(dir.path());
    let config = stub.config(&script, dir.path());
    stub.set_logged_in(false);
    let driver = CodexDriver::new(config).unwrap();
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
