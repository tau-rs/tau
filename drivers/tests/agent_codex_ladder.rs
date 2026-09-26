//! The cancel ladder and the wall bound (ADR-0013 §5) through the `codex`
//! driver, against `tau-fake-cli`: the rows that need a rung to time out
//! before the next one — SIGTERM after an ignored SIGINT, SIGKILL after an
//! ignored SIGTERM, and the wall bound climbing the same ladder.
//!
//! Every row here bills the ceiling, because that is what `codex` 0.157.1
//! does: `SIGINT` ends the run with nothing printed (#128 run 2), so no
//! rung of the ladder ever reports usage. The row where the first rung
//! *is* answered with a terminal event is `agent_ladder.rs`'s, against a
//! shell that traps the signal; it is not a row this CLI can produce.
//!
//! This binary is the `ci` profile's: each rung is a grace period long by
//! construction, and the `quick` profile's five-second ceiling is for tests
//! that do not wait on anything. The rows that do not wait are in
//! `agent_codex.rs`.

#![cfg(all(feature = "agent-codex", unix))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "common/agent.rs"]
mod agent;
#[path = "common/codex.rs"]
mod codex;

use std::path::Path;
use std::time::Duration;

use serde_json::json;
use tau_drivers::agent::codex::CodexDriver;
use tau_drivers::agent::wire::{ErrorKind, Limit, Reply, Stop, Usage};
use tau_kernel::abi::{AgentId, Consumption, Corr, DimKey};
use tau_kernel::driver::Driver;
use tau_kernel::kernel::Delivery;

use codex::{CodexStub, PIN};

/// A driver over the stub with a short grace and wall, so the ladder is
/// seconds long, not the default's tens of seconds.
fn driver(dir: &Path, script: &Path, wall_ms: u64, grace_ms: u64) -> CodexDriver {
    let stub = CodexStub::new(dir);
    let mut config = stub.config(script, dir);
    config.wall = Duration::from_millis(wall_ms);
    config.abandon_grace = Duration::from_millis(grace_ms);
    CodexDriver::new(config).unwrap()
}

/// Starts one run, waits for the fake to say it is ready (its handlers are
/// installed, so the rungs above the interrupt are caught rather than
/// fatal), and abandons it; returns the reply and the bill.
async fn abandoned_run(driver: &CodexDriver, ready: &Path) -> (Reply, Consumption) {
    let fut = driver.handle(Delivery {
        corr: Corr::new(1),
        from: AgentId::new(1),
        payload: codex::run_payload("x"),
    });
    let ready = ready.to_path_buf();
    tokio::task::spawn_blocking(move || codex::wait_for(&ready))
        .await
        .unwrap();
    driver.abandon(Corr::new(1));
    let (bytes, consumed) = tokio::time::timeout(Duration::from_secs(30), fut)
        .await
        .expect("the ladder ends");
    (serde_json::from_slice(&bytes).unwrap(), consumed)
}

#[tokio::test]
async fn an_ignored_sigint_reaches_sigterm_and_bills_the_ceiling() {
    let dir = agent::Temp::new("sigterm");
    // Run 3: the CLI prints nothing after SIGTERM and exits 0. Scripted
    // here with SIGINT swallowed, so that SIGTERM is the rung that ends it.
    let records = agent::transcript(PIN, "3-sigterm");
    let (mut directives, _) = agent::script_from_transcript(&records);
    let ready = dir.path().join("ready");
    directives.insert(0, json!({ "touch": ready }));
    directives.insert(1, json!({ "on": { "signal": "SIGINT" }, "ignore": true }));
    let script = agent::script(dir.path(), "3-sigterm", &directives);
    let driver = driver(dir.path(), &script, 20_000, 300);
    let (reply, consumed) = abandoned_run(&driver, &ready).await;
    assert_eq!(reply.stop, Stop::Abandoned);
    assert!(reply.envelope.is_none());
    assert_eq!(
        reply.session.as_deref(),
        Some("01a0dd5b-8539-7612-8c4d-6ff32230a011"),
        "thread.started still names the thread"
    );
    assert_eq!(
        reply.usage,
        Usage::default(),
        "nothing trustworthy to report"
    );
    assert!(
        !reply
            .transcript
            .iter()
            .any(|e| e["type"] == "turn.completed"),
        "SIGTERM prints nothing"
    );
    assert_eq!(consumed.get(&DimKey::Tokens), Some(400_000), "the ceiling");
    assert_eq!(driver.in_flight(), 0);
}

#[tokio::test]
async fn a_cli_that_ignores_sigterm_too_is_killed_and_the_run_is_lost() {
    let dir = agent::Temp::new("sigkill");
    let ready = dir.path().join("ready");
    let script = agent::script(
        dir.path(),
        "stubborn",
        &[
            json!({ "line": { "type": "thread.started", "thread_id": "t-1" } }),
            json!({ "touch": ready }),
            json!({ "on": { "signal": "SIGINT" }, "ignore": true }),
            json!({ "on": { "signal": "SIGTERM" }, "ignore": true }),
        ],
    );
    let driver = driver(dir.path(), &script, 20_000, 300);
    let (reply, consumed) = abandoned_run(&driver, &ready).await;
    let Stop::Error(error) = &reply.stop else {
        panic!("{:?}", reply.stop)
    };
    assert_eq!(error.kind, ErrorKind::Lost, "SIGINT, SIGTERM, SIGKILL");
    assert!(error.message.contains("killed"), "{}", error.message);
    assert_eq!(consumed.get(&DimKey::Tokens), Some(400_000), "the ceiling");
    assert_eq!(driver.in_flight(), 0);
}

#[tokio::test]
async fn the_wall_bound_on_a_cli_that_says_nothing_bills_the_ceiling() {
    let dir = agent::Temp::new("wall-silent");
    // Run 2's shape: SIGINT ends it, nothing more said. The same reply
    // whether the fake had its handler installed when the wall fired or
    // not — a `SIGINT` before the handler is fatal and silent too.
    let script = agent::script(
        dir.path(),
        "silent",
        &[
            json!({ "line": { "type": "thread.started", "thread_id": "t-1" } }),
            json!({ "line": { "type": "turn.started" }, "delay_ms": 60_000 }),
            json!({ "on": { "signal": "SIGINT" }, "exit": 1 }),
        ],
    );
    let driver = driver(dir.path(), &script, 400, 300);
    let fut = driver.handle(Delivery {
        corr: Corr::new(1),
        from: AgentId::new(1),
        payload: codex::run_payload("x"),
    });
    let (bytes, consumed) = tokio::time::timeout(Duration::from_secs(30), fut)
        .await
        .expect("the wall fires and SIGINT ends it");
    let reply: Reply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reply.stop, Stop::Limit(Limit::Wall));
    assert_eq!(reply.usage, Usage::default());
    assert_eq!(consumed.get(&DimKey::Tokens), Some(400_000), "the ceiling");
}
