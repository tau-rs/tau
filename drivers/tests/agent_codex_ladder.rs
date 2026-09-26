//! The cancel ladder and the wall bound (ADR-0013 §5) through the `codex`
//! driver, against `tau-fake-cli`: the rows that need a rung to time out
//! before the next one — SIGTERM after an ignored SIGINT, SIGKILL after an
//! ignored SIGTERM, the wall bound climbing the same ladder, and the 401
//! loop of #130 §3 ended by the ladder and reported as `unavailable`.
//!
//! This binary is the `ci` profile's: each rung is a grace period long by
//! construction, and the `quick` profile's five-second ceiling is for tests
//! that do not wait on anything. The SIGINT row, which the fake answers at
//! once, is in `agent_codex.rs`.
//!
//! A `codex` child gets no stdin, so a fake whose timeline is spent exits
//! at once; every script here keeps a minute-long line pending, the way a
//! CLI mid-turn is still working when the signal lands.

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
use std::time::Duration;

use serde_json::json;
use tau_drivers::agent::codex::CodexDriver;
use tau_drivers::agent::wire::{ErrorKind, Limit, Reply, Stop, Usage};
use tau_drivers::agent::Verdict;
use tau_kernel::abi::{AgentId, Consumption, Corr, DimKey};
use tau_kernel::driver::Driver;
use tau_kernel::kernel::Delivery;

use agent::CodexStub;

const PIN: &str = "codex-0.154.0";

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
async fn an_ignored_sigint_reaches_sigterm_and_bills_the_ceiling() {
    let dir = agent::Temp::new("sigterm");
    // `3-sigterm`: nothing printed after SIGTERM, exit 143. Here SIGINT is
    // swallowed first, so SIGTERM is the rung that ends it.
    let records = agent::transcript(PIN, "3-sigterm");
    let (mut directives, _) = agent::script_from_transcript(&records);
    let ready = dir.path().join("ready");
    directives.insert(2, json!({ "touch": ready }));
    directives.insert(3, agent::still_working());
    directives.push(json!({ "on": { "signal": "SIGINT" }, "ignore": true }));
    let script = agent::script(dir.path(), "3-sigterm", &directives);
    let driver = driver(dir.path(), &script, 20_000, 300);
    let (reply, consumed) = abandoned_run(&driver, &ready).await;
    assert_eq!(reply.stop, Stop::Abandoned);
    assert!(reply.envelope.is_none());
    assert_eq!(
        reply.session.as_deref(),
        Some("01a0dd5b-467f-7ec3-a9a0-30e797fcd3f6"),
        "thread.started still names the thread"
    );
    assert_eq!(
        reply.usage,
        Usage::default(),
        "nothing trustworthy to report"
    );
    assert_eq!(reply.transcript.len(), 2, "SIGTERM prints nothing");
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
            json!({ "line": { "type": "thread.started", "thread_id": "t-stubborn" } }),
            json!({ "line": { "type": "turn.started" } }),
            json!({ "touch": ready }),
            agent::still_working(),
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
    assert_eq!(reply.session.as_deref(), Some("t-stubborn"));
    assert_eq!(consumed.get(&DimKey::Tokens), Some(400_000), "the ceiling");
    assert_eq!(consumed.get(&DimKey::CostMicroUsd), Some(2_000_000));
    assert_eq!(driver.in_flight(), 0);
}

#[tokio::test]
async fn the_wall_bound_climbs_the_ladder_and_reports_the_wall_limit() {
    let dir = agent::Temp::new("wall");
    let script = agent::script(
        dir.path(),
        "slow",
        &[
            json!({ "line": { "type": "thread.started", "thread_id": "t-slow" } }),
            json!({ "line": { "type": "turn.started" } }),
            agent::still_working(),
            // `2-sigint`: SIGINT prints nothing more, exit 1.
            json!({ "on": { "signal": "SIGINT" }, "exit": 1 }),
        ],
    );
    let driver = driver(dir.path(), &script, 400, 300);
    let (bytes, consumed) = tokio::time::timeout(
        Duration::from_secs(30),
        driver.handle(Delivery {
            corr: Corr::new(2),
            from: AgentId::new(1),
            payload: agent::run_payload("x"),
        }),
    )
    .await
    .unwrap();
    let reply: Reply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reply.stop, Stop::Limit(Limit::Wall));
    assert!(reply.envelope.is_none());
    assert_eq!(reply.session.as_deref(), Some("t-slow"));
    assert_eq!(consumed.get(&DimKey::Tokens), Some(400_000), "the ceiling");
}

/// #130 §3: an unsigned `codex exec` starts a thread and retries 401s
/// until killed. The probe is the gate; when a run gets past it anyway,
/// the driver ends the loop — the wall bound, or an abandon, the same
/// ladder — and the reply says `unavailable` rather than what the ladder
/// says, flips the verdict, and bills nothing: nothing was served.
#[tokio::test]
async fn the_401_loop_ended_by_the_ladder_is_unavailable_and_flips_the_verdict() {
    let dir = agent::Temp::new("401-loop");
    let ready = dir.path().join("ready");
    let retry = |n: u32| {
        json!({ "line": { "type": "error", "message": format!(
            "stream error: exceeded retry limit, last status: 401 Unauthorized, request id: x-{n}; retrying {n}/5 in 188ms…"
        ) } })
    };
    let script = agent::script(
        dir.path(),
        "401-loop",
        &[
            json!({ "line": { "type": "thread.started", "thread_id": "t-401" } }),
            json!({ "line": { "type": "turn.started" } }),
            retry(1),
            retry(2),
            json!({ "touch": ready }),
            agent::still_working(),
            // `2-sigint`: SIGINT prints nothing more, exit 1.
            json!({ "on": { "signal": "SIGINT" }, "exit": 1 }),
        ],
    );
    let driver = driver(dir.path(), &script, 20_000, 300);
    assert!(driver.verdict().is_ready(), "the probe said so");
    let (reply, consumed) = abandoned_run(&driver, &ready).await;
    let Stop::Error(error) = &reply.stop else {
        panic!("{:?}\n{:#?}", reply.stop, reply.transcript)
    };
    assert_eq!(error.kind, ErrorKind::Unavailable);
    assert!(
        error.message.contains("401 Unauthorized"),
        "{}",
        error.message
    );
    assert_eq!(reply.transcript.len(), 4, "the loop, as far as it got");
    assert_eq!(consumed, Consumption::none(), "nothing was served");
    assert!(
        matches!(driver.verdict(), Verdict::Unavailable { .. }),
        "the next send re-probes instead of running"
    );
}
