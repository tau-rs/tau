//! The cancel ladder and the wall bound (ADR-0013 §5) through the `claude`
//! driver, against `tau-fake-cli`: the rows that need a rung to time out
//! before the next one — SIGTERM after an ignored interrupt, SIGKILL after
//! an ignored SIGTERM, and the wall bound climbing the same ladder.
//!
//! This binary is the `ci` profile's: each rung is a grace period long by
//! construction, and the `quick` profile's five-second ceiling is for tests
//! that do not wait on anything. The rows that do not wait are in
//! `agent_claude.rs`.

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
use std::time::Duration;

use serde_json::json;
use tau_drivers::agent::claude::ClaudeDriver;
use tau_drivers::agent::wire::{ErrorKind, Limit, Reply, Stop, Usage};
use tau_kernel::abi::{AgentId, Consumption, Corr, DimKey};
use tau_kernel::driver::Driver;
use tau_kernel::kernel::Delivery;

use agent::ClaudeStub;

const PIN: &str = "claude-2.1.272";

/// A driver over the stub with a short grace and wall, so the ladder is
/// seconds long, not the default's tens of seconds.
fn driver(dir: &Path, script: &Path, wall_ms: u64, grace_ms: u64) -> ClaudeDriver {
    let stub = ClaudeStub::new(dir);
    let mut config = stub.config(script, dir);
    config.wall = Duration::from_millis(wall_ms);
    config.abandon_grace = Duration::from_millis(grace_ms);
    ClaudeDriver::new(config).unwrap()
}

/// Starts one run, waits for the fake to say it is ready (its handlers are
/// installed, so the rungs above the interrupt are caught rather than
/// fatal), and abandons it; returns the reply and the bill.
async fn abandoned_run(driver: &ClaudeDriver, ready: &Path) -> (Reply, Consumption) {
    let fut = driver.handle(Delivery {
        corr: Corr::new(1),
        from: AgentId::new(1),
        payload: agent::run_payload("x"),
    });
    let ready = ready.to_path_buf();
    tokio::task::spawn_blocking(move || agent::wait_for(&ready))
        .await
        .unwrap();
    driver.abandon(Corr::new(1));
    let (bytes, consumed) = tokio::time::timeout(Duration::from_secs(30), fut)
        .await
        .expect("the ladder ends");
    (serde_json::from_slice(&bytes).unwrap(), consumed)
}

#[tokio::test]
async fn an_ignored_interrupt_reaches_sigterm_and_bills_the_ceiling() {
    let dir = agent::Temp::new("sigterm");
    // #130 §5c: the CLI prints nothing after SIGTERM and exits 143. The
    // transcript's script has no stdin reaction, so the interrupt is read
    // and dropped, and SIGTERM is what ends it.
    let records = agent::transcript(PIN, "4-sigterm");
    let (mut directives, _) = agent::script_from_transcript(&records);
    let ready = dir.path().join("ready");
    directives.insert(0, json!({ "touch": ready }));
    let script = agent::script(dir.path(), "4-sigterm", &directives);
    let driver = driver(dir.path(), &script, 20_000, 300);
    let (reply, consumed) = abandoned_run(&driver, &ready).await;
    assert_eq!(reply.stop, Stop::Abandoned);
    assert!(reply.envelope.is_none());
    assert_eq!(
        reply.session.as_deref(),
        Some("f572cdeb-9836-4bcf-ac6d-ae6f53ee4b72"),
        "the init still names the session"
    );
    assert_eq!(
        reply.usage,
        Usage::default(),
        "nothing trustworthy to report"
    );
    assert!(
        !reply.transcript.iter().any(|e| e["type"] == "result"),
        "SIGTERM prints nothing"
    );
    assert_eq!(consumed.get(&DimKey::Tokens), Some(400_000), "the ceiling");
    assert_eq!(consumed.get(&DimKey::CostMicroUsd), Some(2_000_000));
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
            json!({ "line": { "type": "system", "subtype": "init", "session_id": "s-1", "apiKeySource": "none" } }),
            json!({ "touch": ready }),
            json!({ "on": { "signal": "SIGTERM" }, "ignore": true }),
        ],
    );
    let driver = driver(dir.path(), &script, 20_000, 300);
    let (reply, consumed) = abandoned_run(&driver, &ready).await;
    let Stop::Error(error) = &reply.stop else {
        panic!("{:?}", reply.stop)
    };
    assert_eq!(error.kind, ErrorKind::Lost, "interrupt, SIGTERM, SIGKILL");
    assert!(error.message.contains("killed"), "{}", error.message);
    assert_eq!(consumed.get(&DimKey::Tokens), Some(400_000), "the ceiling");
    assert_eq!(driver.in_flight(), 0);
}

#[tokio::test]
async fn the_wall_bound_climbs_the_ladder_and_reports_limit_wall() {
    let dir = agent::Temp::new("wall");
    // The CLI sits in a long turn; the wall fires; the interrupt is
    // answered with a result carrying real usage (#130 §5a).
    let script = agent::script(
        dir.path(),
        "long-turn",
        &[
            json!({ "line": { "type": "system", "subtype": "init", "session_id": "s-1", "apiKeySource": "none", "model": "m" } }),
            json!({ "line": { "type": "assistant", "message": { "content": [] } }, "delay_ms": 60_000 }),
            json!({ "on": { "stdin": "\"subtype\":\"interrupt\"" }, "lines": [
                { "type": "control_response", "response": { "subtype": "success", "request_id": "tau-cancel-1" } },
                { "type": "result", "subtype": "error_during_execution", "is_error": true,
                  "terminal_reason": "aborted_tools", "num_turns": 1, "total_cost_usd": 0.05,
                  "usage": { "input_tokens": 10, "cache_read_input_tokens": 90, "output_tokens": 5 } }
            ], "exit": 1 }),
        ],
    );
    // A generous grace: the wall must fire, and the rung that answers it
    // is the interrupt, however slowly the fake started under load.
    let driver = driver(dir.path(), &script, 400, 5_000);
    let fut = driver.handle(Delivery {
        corr: Corr::new(1),
        from: AgentId::new(1),
        payload: agent::run_payload("x"),
    });
    let (bytes, consumed) = tokio::time::timeout(Duration::from_secs(30), fut)
        .await
        .expect("the wall fires");
    let reply: Reply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reply.stop, Stop::Limit(Limit::Wall));
    assert!(reply.envelope.is_none());
    assert_eq!(reply.usage.tokens(), 105, "the CLI reported it: real usage");
    assert_eq!(reply.usage.cost_microusd, Some(50_000));
    assert_eq!(
        consumed.get(&DimKey::Tokens),
        Some(105),
        "reported, not the ceiling"
    );
    assert_eq!(consumed.get(&DimKey::CostMicroUsd), Some(50_000));
}

#[tokio::test]
async fn the_wall_bound_on_a_cli_that_ignores_the_interrupt_bills_the_ceiling() {
    let dir = agent::Temp::new("wall-term");
    let script = agent::script(
        dir.path(),
        "deaf",
        &[
            json!({ "line": { "type": "system", "subtype": "init", "session_id": "s-1" } }),
            json!({ "line": { "type": "never" }, "delay_ms": 60_000 }),
        ],
    );
    let driver = driver(dir.path(), &script, 400, 300);
    let fut = driver.handle(Delivery {
        corr: Corr::new(1),
        from: AgentId::new(1),
        payload: agent::run_payload("x"),
    });
    let (bytes, consumed) = tokio::time::timeout(Duration::from_secs(30), fut)
        .await
        .expect("the wall fires and SIGTERM ends it");
    let reply: Reply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reply.stop, Stop::Limit(Limit::Wall));
    assert_eq!(reply.usage, Usage::default());
    assert_eq!(consumed.get(&DimKey::Tokens), Some(400_000), "the ceiling");
}
