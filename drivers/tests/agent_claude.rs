//! The `claude` adapter (ADR-0013 §7, the `claude` column) against
//! `tau-fake-cli` replaying the #130 transcripts: the fixed argv, the event
//! mapping over all seven runs, and one test per row of the `stop` table
//! and the `error.kind` table that does not wait on a grace period.
//!
//! The rows that do — SIGTERM, SIGKILL, the wall bound — are
//! `agent_claude_ladder.rs`, the `ci` profile's. The driver through a real
//! kernel is `agent_claude_kernel.rs`.
//!
//! The binary the driver is constructed over is a `/bin/sh` stub
//! (`common/agent.rs::ClaudeStub`): `tau-fake-cli` replays its script
//! whatever its argv, so it cannot answer the version and login probes on
//! its own. The stub answers those the way #130 recorded them, records the
//! argv of every run, and `exec`s the fake for the run itself.

#![cfg(all(feature = "agent-claude", unix))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::type_complexity
)]

#[path = "common/agent.rs"]
mod agent;

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use tau_drivers::agent::claude::{self, ClaudeDriver, CAPS, CONTRACT, INTERRUPT};
use tau_drivers::agent::envelope::Status;
use tau_drivers::agent::process::{Ending, Interrupt, Run};
use tau_drivers::agent::wire::{ErrorKind, Limit, Reply, Stop, Usage};
use tau_drivers::agent::{accept, wire, Accepted, ConfigError, Verdict};
use tau_kernel::abi::{AgentId, Consumption, Corr, DimKey};
use tau_kernel::driver::Driver;
use tau_kernel::kernel::Delivery;

use agent::ClaudeStub;

const PIN: &str = "claude-2.1.272";
const SESSION_HELLO: &str = "affe155e-9ed1-4477-95f0-c17d40bd9a89";

/// The stub and a driver over it, replaying `script`, rooted at `dir`.
fn driver(dir: &Path, script: &Path) -> (ClaudeStub, ClaudeDriver) {
    let stub = ClaudeStub::new(dir);
    let config = stub.config(script, dir);
    let driver = ClaudeDriver::new(config).unwrap();
    (stub, driver)
}

/// One `send` through the driver, outside a kernel.
async fn send(driver: &ClaudeDriver, corr: u64, payload: Vec<u8>) -> (Reply, Consumption) {
    let (bytes, consumed) = driver
        .handle(Delivery {
            corr: Corr::new(corr),
            from: AgentId::new(1),
            payload,
        })
        .await;
    let reply: Reply = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "not an agent reply: {e}\n{}",
            String::from_utf8_lossy(&bytes)
        )
    });
    (reply, consumed)
}

fn error_of(reply: &Reply) -> (ErrorKind, String) {
    match &reply.stop {
        Stop::Error(error) => (error.kind, error.message.clone()),
        other => panic!("expected an error, got {other:?}"),
    }
}

/// A committed transcript as the `Run` the supervisor would have produced,
/// had the CLI run to its end: every stdout line, the `result` marked.
fn run_from(name: &str) -> Run {
    let stdout = agent::transcript_stdout(&agent::transcript(PIN, name));
    Run {
        lines: stdout
            .iter()
            .map(|line| line.to_string().into_bytes())
            .collect(),
        dropped: 0,
        truncated: false,
        stderr: String::new(),
        ending: Ending::Completed,
        terminal_at: stdout.iter().position(|line| line["type"] == "result"),
        status: None,
    }
}

fn accepted(config: &tau_drivers::agent::AgentConfig, payload: Value) -> Accepted {
    accept(&serde_json::from_value(payload).unwrap(), config, CAPS).unwrap()
}

// --- the Invocation half ------------------------------------------------------

#[test]
fn the_fixed_argv_is_the_adrs_then_the_config_then_the_request() {
    let dir = agent::Temp::new("argv");
    let stub = ClaudeStub::new(dir.path());
    let script = agent::script(dir.path(), "none", &[json!({ "exit": 0 })]);
    let config = stub.config(&script, dir.path());

    let run = accepted(&config, json!({ "op": "run", "task": "write hello.txt" }));
    let invocation = claude::invocation(&config, &run);
    assert_eq!(invocation.program, stub.binary);
    assert_eq!(invocation.cwd, dir.path());
    assert_eq!(
        invocation.env, config.env,
        "exactly the configured environment"
    );
    let args = invocation.args.clone();
    assert_eq!(
        &args[..9],
        [
            "-p",
            "--safe-mode",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-prompts",
            "none",
        ],
        "the fixed argv, ADR-0013 §7"
    );
    let after = |flag: &str| {
        let at = args
            .iter()
            .position(|a| a == flag)
            .unwrap_or_else(|| panic!("{flag}"));
        args[at + 1].clone()
    };
    assert_eq!(after("--permission-mode"), "acceptEdits");
    assert_eq!(after("--append-system-prompt"), CONTRACT);
    assert!(
        !args.iter().any(|a| a == "--system-prompt"),
        "never --system-prompt: it strips the CLI's own scaffolding"
    );
    assert_eq!(
        after("--allowedTools"),
        "Read,Edit,Bash",
        "one argument: `<tools...>` is variadic"
    );
    assert_eq!(after("--max-budget-usd"), "2", "$2, from the registration");
    assert_eq!(after("--max-turns"), "40");
    assert!(!args.iter().any(|a| a == "--resume"));
    assert!(
        !args.iter().any(|a| a == "--json-schema"),
        "off: see the amendment"
    );
    assert!(!args.iter().any(|a| a == "--model" || a == "--effort"));

    // The task is the first stdin message, and the interrupt is in-band.
    let first: Value = serde_json::from_str(invocation.first_stdin.as_deref().unwrap()).unwrap();
    assert_eq!(
        first,
        json!({ "type": "user", "message": { "role": "user", "content": "write hello.txt" } })
    );
    assert!(invocation.first_stdin.unwrap().ends_with('\n'));
    assert_eq!(
        invocation.interrupt,
        Interrupt::InBand(INTERRUPT.to_owned())
    );
    assert_eq!(INTERRUPT, agent::INTERRUPT, "the one #130 §5a acknowledged");

    // The request's narrowing, and the config's model and effort.
    let mut config = config;
    config.model = Some("claude-opus-5".to_owned());
    config.effort = Some("high".to_owned());
    let narrowed = accepted(
        &config,
        json!({
            "op": "run", "task": "x", "tools": ["Read"],
            "budget": { "cost_microusd": 500_000, "turns": 3 }
        }),
    );
    let args = claude::invocation(&config, &narrowed).args;
    let after = |flag: &str| {
        let at = args.iter().position(|a| a == flag).unwrap();
        args[at + 1].clone()
    };
    assert_eq!(after("--allowedTools"), "Read");
    assert_eq!(after("--max-budget-usd"), "0.5", "exact decimal, no float");
    assert_eq!(after("--max-turns"), "3");
    assert_eq!(after("--model"), "claude-opus-5");
    assert_eq!(after("--effort"), "high");

    // `resume`: the amendment as the first stdin message, `--resume <id>`.
    let resume = accepted(
        &config,
        json!({ "op": "resume", "session": SESSION_HELLO, "task": "now world.txt" }),
    );
    let invocation = claude::invocation(&config, &resume);
    let args = invocation.args;
    let at = args.iter().position(|a| a == "--resume").unwrap();
    assert_eq!(args[at + 1], SESSION_HELLO);
    assert!(invocation.first_stdin.unwrap().contains("now world.txt"));
}

#[test]
fn a_budget_of_one_microdollar_is_written_exactly() {
    let dir = agent::Temp::new("micro");
    let stub = ClaudeStub::new(dir.path());
    let script = agent::script(dir.path(), "none", &[json!({ "exit": 0 })]);
    let mut config = stub.config(&script, dir.path());
    config.task_cost_microusd = Some(1);
    let run = accepted(&config, json!({ "op": "run", "task": "x" }));
    let args = claude::invocation(&config, &run).args;
    let at = args.iter().position(|a| a == "--max-budget-usd").unwrap();
    assert_eq!(args[at + 1], "0.000001");
}

#[test]
fn the_terminal_line_is_a_result_event_and_nothing_that_merely_mentions_one() {
    let dir = agent::Temp::new("terminal");
    let stub = ClaudeStub::new(dir.path());
    let script = agent::script(dir.path(), "none", &[json!({ "exit": 0 })]);
    let config = stub.config(&script, dir.path());
    let run = accepted(&config, json!({ "op": "run", "task": "x" }));
    let terminal = claude::invocation(&config, &run).terminal;
    assert!(terminal(br#"{"type":"result","subtype":"success"}"#));
    assert!(
        terminal(br#"{"subtype":"success","type":"result"}"#),
        "any key order"
    );
    assert!(
        !terminal(
            br#"{"type":"assistant","message":{"content":[{"type":"text","text":"a {\"type\":\"result\"} line"}]}}"#
        ),
        "an assistant message that quotes one"
    );
    assert!(!terminal(b"not json \"type\":\"result\""));
    assert!(!terminal(br#"{"type":"system","subtype":"init"}"#));
}

// --- the Outcome half, over the seven #130 transcripts -----------------------

#[test]
fn the_hello_run_reads_session_model_mode_usage_and_the_envelope() {
    let outcome = claude::outcome(&run_from("1-hello"));
    assert_eq!(
        outcome.session.as_deref(),
        Some(SESSION_HELLO),
        "the first init's"
    );
    assert_eq!(
        outcome.model.as_deref(),
        Some("claude-opus-5[1m]"),
        "verbatim"
    );
    assert_eq!(
        outcome.mode.as_deref(),
        Some("none"),
        "init.apiKeySource on a claude.ai login, verbatim"
    );
    assert_eq!(
        outcome.usage,
        Usage {
            input_tokens: 4 + 23_851 + 20_270,
            output_tokens: 381,
            cost_microusd: Some(259_148),
            turns: Some(2),
        },
        "every input class summed; the cost exact"
    );
    assert_eq!(outcome.stop, None, "success: the envelope parser decides");
    let envelope = tau_drivers::agent::envelope::parse(&outcome.final_message.unwrap()).unwrap();
    assert_eq!(envelope.status, Status::Ok);
    assert_eq!(envelope.artifacts.len(), 1);
}

#[test]
fn every_transcript_maps_as_the_adr_table_says() {
    // (run, stop, input tokens, cost in microdollars, turns, has a final message)
    let table: &[(&str, Option<Stop>, u64, Option<u64>, Option<u64>, bool)] = &[
        ("1-hello", None, 44_125, Some(259_148), Some(2), true),
        (
            "2-stdin-cancel",
            Some(Stop::Error(tau_drivers::agent::refusal(
                ErrorKind::Provider,
                "[ede_diagnostic] result_type=user last_content_type=n/a stop_reason=tool_use",
            ))),
            2 + 10_084 + 10_135,
            Some(112_628),
            Some(3),
            false,
        ),
        (
            "3-sigint",
            Some(Stop::Error(tau_drivers::agent::refusal(
                ErrorKind::Provider,
                "[ede_diagnostic] result_type=user last_content_type=n/a stop_reason=null",
            ))),
            0,
            Some(1_010),
            Some(3),
            false,
        ),
        ("4-sigterm", None, 0, None, None, false),
        (
            "5-resume",
            None,
            4 + 1_926 + 48_489,
            Some(53_325),
            Some(2),
            true,
        ),
        (
            "6-max-turns-1",
            Some(Stop::Limit(Limit::Turns)),
            2 + 10_023 + 10_135,
            Some(109_386),
            Some(2),
            false,
        ),
        (
            "7-safe-mode",
            None,
            4 + 5_036 + 25_072,
            Some(70_894),
            Some(2),
            true,
        ),
    ];
    for (run, stop, input, cost, turns, final_message) in table {
        let outcome = claude::outcome(&run_from(run));
        assert_eq!(&outcome.stop, stop, "{run}");
        assert_eq!(outcome.usage.input_tokens, *input, "{run}");
        assert_eq!(outcome.usage.cost_microusd, *cost, "{run}: rounded up");
        assert_eq!(outcome.usage.turns, *turns, "{run}");
        assert_eq!(outcome.final_message.is_some(), *final_message, "{run}");
        assert!(
            outcome.session.is_some(),
            "{run}: every run printed an init"
        );
        assert_eq!(outcome.mode.as_deref(), Some("none"), "{run}");
    }
    // Interrupted mid-turn, the session id is still the init's, and the
    // SIGTERM run — no result at all — still names its session.
    assert_eq!(
        claude::outcome(&run_from("4-sigterm")).session.as_deref(),
        Some("f572cdeb-9836-4bcf-ac6d-ae6f53ee4b72")
    );
}

// --- the stop table, through the driver ---------------------------------------

#[tokio::test]
async fn a_hello_run_is_done_with_its_envelope_and_the_reported_bill() {
    let dir = agent::Temp::new("hello");
    let script = agent::replay_script(dir.path(), PIN, "1-hello");
    let (stub, driver) = driver(dir.path(), &script);
    assert_eq!(driver.version(), "2.1.272 (Claude Code)");
    assert!(driver.verdict().is_ready());

    let (reply, consumed) = send(
        &driver,
        1,
        agent::run_payload("write hello.txt containing hello"),
    )
    .await;
    assert_eq!(reply.stop, Stop::Done, "{:?}", reply.stop);
    assert_eq!(reply.v, wire::VERSION);
    assert_eq!(reply.cli.name, "claude");
    assert_eq!(reply.cli.version, "2.1.272 (Claude Code)");
    let envelope = reply.envelope.unwrap();
    assert_eq!(envelope.status, Status::Ok);
    assert!(
        envelope.summary.contains("hello.txt"),
        "{}",
        envelope.summary
    );
    assert_eq!(reply.session.as_deref(), Some(SESSION_HELLO));
    assert_eq!(reply.model.as_deref(), Some("claude-opus-5[1m]"));
    assert_eq!(reply.mode.as_deref(), Some("none"));
    assert_eq!(reply.usage.tokens(), 44_506);
    assert_eq!(reply.usage.cost_microusd, Some(259_148));
    assert_eq!(reply.transcript.len(), 11, "every event, hook to idle");
    assert!(!reply.truncated.transcript);
    assert_eq!(
        consumed.get(&DimKey::Tokens),
        Some(44_506),
        "what the CLI reported"
    );
    assert_eq!(
        consumed.get(&DimKey::CostMicroUsd),
        Some(259_148),
        "the CLI's own figure, whatever the login"
    );
    assert_eq!(
        consumed.get(&DimKey::Calls),
        None,
        "the kernel counts calls"
    );
    assert_eq!(driver.in_flight(), 0);

    // What the CLI was actually run with: the argv the pure half built.
    let config = driver.config();
    let expected = claude::invocation(
        config,
        &accepted(
            config,
            json!({ "op": "run", "task": "write hello.txt containing hello" }),
        ),
    );
    assert_eq!(
        stub.argv(),
        expected.args,
        "the driver spawned the fixed argv"
    );
}

#[tokio::test]
async fn resume_continues_the_session_the_reply_named() {
    let dir = agent::Temp::new("resume");
    let script = agent::replay_script(dir.path(), PIN, "5-resume");
    let (stub, driver) = driver(dir.path(), &script);
    let payload = serde_json::to_vec(&json!({
        "op": "resume", "session": SESSION_HELLO, "task": "now also write world.txt containing world"
    }))
    .unwrap();
    let (reply, consumed) = send(&driver, 1, payload).await;
    assert_eq!(reply.stop, Stop::Done);
    assert_eq!(reply.session.as_deref(), Some(SESSION_HELLO), "the same id");
    assert!(reply.envelope.unwrap().summary.contains("world.txt"));
    assert_eq!(
        consumed.get(&DimKey::Tokens),
        Some(4 + 1_926 + 48_489 + 392)
    );
    let argv = stub.argv();
    let at = argv.iter().position(|a| a == "--resume").unwrap();
    assert_eq!(argv[at + 1], SESSION_HELLO);
}

#[tokio::test]
async fn max_turns_exhaustion_is_limit_turns_with_no_envelope_and_real_usage() {
    let dir = agent::Temp::new("turns");
    let script = agent::replay_script(dir.path(), PIN, "6-max-turns-1");
    let (_stub, driver) = driver(dir.path(), &script);
    let (reply, consumed) = send(&driver, 1, agent::run_payload("write hello.txt")).await;
    assert_eq!(reply.stop, Stop::Limit(Limit::Turns), "#130 run 6");
    assert!(
        reply.envelope.is_none(),
        "the model got no turn to write one"
    );
    assert_eq!(
        reply.session.as_deref(),
        Some("0857dc38-4b77-4ded-8142-537939377ec9")
    );
    assert_eq!(reply.usage.turns, Some(2));
    assert_eq!(
        consumed.get(&DimKey::Tokens),
        Some(2 + 10_023 + 10_135 + 125),
        "real"
    );
    assert_eq!(consumed.get(&DimKey::CostMicroUsd), Some(109_386));
    let last = reply
        .transcript
        .iter()
        .rev()
        .find(|e| e["type"] == "result")
        .unwrap();
    assert_eq!(last["subtype"], "error_max_turns");
}

#[tokio::test]
async fn a_budget_exhaustion_is_limit_cost() {
    let dir = agent::Temp::new("cost");
    let script = agent::synthetic_script(
        dir.path(),
        "budget",
        json!({
            "type": "result", "subtype": "error_max_budget_usd", "is_error": true,
            "terminal_reason": "max_budget_usd", "num_turns": 4, "total_cost_usd": 0.5,
            "usage": { "input_tokens": 10, "output_tokens": 5 },
            "errors": ["Reached maximum budget ($0.50)"]
        }),
        1,
    );
    let (_stub, driver) = driver(dir.path(), &script);
    let (reply, consumed) = send(&driver, 1, agent::run_payload("x")).await;
    assert_eq!(reply.stop, Stop::Limit(Limit::Cost));
    assert!(reply.envelope.is_none());
    assert_eq!(consumed.get(&DimKey::CostMicroUsd), Some(500_000));
    assert_eq!(consumed.get(&DimKey::Tokens), Some(15));
}

#[tokio::test]
async fn a_final_message_that_is_not_an_envelope_is_error_envelope_with_the_transcript() {
    let dir = agent::Temp::new("envelope");
    let script = agent::synthetic_script(
        dir.path(),
        "prose",
        json!({
            "type": "result", "subtype": "success", "is_error": false,
            "terminal_reason": "completed", "num_turns": 1, "total_cost_usd": 0.01,
            "usage": { "input_tokens": 100, "cache_read_input_tokens": 50, "output_tokens": 20 },
            "result": "I could not do it, sorry."
        }),
        0,
    );
    let (_stub, driver) = driver(dir.path(), &script);
    let (reply, consumed) = send(&driver, 1, agent::run_payload("x")).await;
    let (kind, message) = error_of(&reply);
    assert_eq!(kind, ErrorKind::Envelope);
    assert!(message.contains("no balanced JSON object"), "{message}");
    assert!(
        reply.envelope.is_none(),
        "never a synthesized {{status: failed}}"
    );
    assert_eq!(reply.transcript.len(), 2, "what the CLI actually said");
    assert_eq!(
        consumed.get(&DimKey::Tokens),
        Some(170),
        "somebody paid for it"
    );
    assert_eq!(consumed.get(&DimKey::CostMicroUsd), Some(10_000));
}

#[tokio::test]
async fn a_run_that_ends_without_its_result_is_lost_at_the_ceiling() {
    let dir = agent::Temp::new("lost");
    let records = agent::transcript(PIN, "1-hello");
    let (mut directives, _) = agent::script_from_transcript(&records);
    directives.insert(0, json!({ "withhold": "\"type\":\"result\"" }));
    let script = agent::script(dir.path(), "lost", &directives);
    let (_stub, driver) = driver(dir.path(), &script);
    let (reply, consumed) = send(&driver, 1, agent::run_payload("x")).await;
    let (kind, message) = error_of(&reply);
    assert_eq!(kind, ErrorKind::Lost);
    assert!(message.contains("without its terminal event"), "{message}");
    assert_eq!(reply.usage, Usage::default(), "nothing trustworthy");
    assert_eq!(
        reply.session.as_deref(),
        Some(SESSION_HELLO),
        "the init still names it"
    );
    assert_eq!(consumed.get(&DimKey::Tokens), Some(400_000), "the ceiling");
    assert_eq!(consumed.get(&DimKey::CostMicroUsd), Some(2_000_000));
}

// --- the error.kind table -----------------------------------------------------

#[tokio::test]
async fn an_unsupported_request_spawns_nothing_and_bills_nothing() {
    let dir = agent::Temp::new("unsupported");
    let script = agent::replay_script(dir.path(), PIN, "1-hello");
    let (stub, driver) = driver(dir.path(), &script);
    for (payload, needle) in [
        (json!({ "v": 2, "op": "run", "task": "x" }), "version 2"),
        (
            json!({ "op": "run", "task": "x", "tools": ["Task"] }),
            "`Task`",
        ),
        (
            json!({ "op": "run", "task": "x", "budget": { "turns": 41 } }),
            "41",
        ),
        (
            json!({ "op": "run", "task": "x", "workspace": "../.." }),
            "relative",
        ),
        (json!({ "op": "steer", "task": "x" }), "steer"),
    ] {
        let (reply, consumed) = send(&driver, 1, serde_json::to_vec(&payload).unwrap()).await;
        let (kind, message) = error_of(&reply);
        assert_eq!(kind, ErrorKind::Unsupported, "{payload}");
        assert!(message.contains(needle), "{payload}: {message}");
        assert!(reply.transcript.is_empty());
        assert_eq!(consumed, Consumption::none(), "{payload}");
    }
    assert!(stub.argv().is_empty(), "nothing was ever run");
    assert_eq!(stub.probes(), 1, "only the probe at construction");
}

#[tokio::test]
async fn a_workspace_that_is_not_a_directory_is_a_host_error() {
    let dir = agent::Temp::new("host");
    let script = agent::replay_script(dir.path(), PIN, "1-hello");
    let (stub, driver) = driver(dir.path(), &script);
    let payload = json!({ "op": "run", "task": "x", "workspace": "missing" });
    let (reply, consumed) = send(&driver, 1, serde_json::to_vec(&payload).unwrap()).await;
    let (kind, message) = error_of(&reply);
    assert_eq!(kind, ErrorKind::Host);
    assert!(message.contains("missing"), "{message}");
    assert_eq!(consumed, Consumption::none());
    assert!(stub.argv().is_empty());
}

#[test]
fn a_binary_that_cannot_run_or_does_not_match_the_pin_is_refused_at_registration() {
    let dir = agent::Temp::new("config");
    let stub = ClaudeStub::new(dir.path());
    let script = agent::script(dir.path(), "none", &[json!({ "exit": 0 })]);

    let mut missing = stub.config(&script, dir.path());
    missing.binary = dir.path().join("no-such-claude");
    let err = ClaudeDriver::new(missing).unwrap_err();
    assert!(matches!(err, ConfigError::Binary { .. }), "{err}");

    let mut pinned = stub.config(&script, dir.path());
    pinned.expect_version = Some("2.2.0".to_owned());
    let err = ClaudeDriver::new(pinned).unwrap_err();
    assert!(matches!(err, ConfigError::Version { .. }), "{err}");
    assert!(err.to_string().contains("2.1.272"), "{err}");

    let mut matching = stub.config(&script, dir.path());
    matching.expect_version = Some("2.1.272".to_owned());
    ClaudeDriver::new(matching).unwrap();
}

#[tokio::test]
async fn logged_out_is_a_clean_refusal_that_reprobes_once_per_send() {
    let dir = agent::Temp::new("logged-out");
    let script = agent::replay_script(dir.path(), PIN, "1-hello");
    let stub = ClaudeStub::new(dir.path());
    let config = stub.config(&script, dir.path());
    stub.set_logged_in(false);

    // Construction does not fail over a login: it is a runtime state.
    let driver = ClaudeDriver::new(config).unwrap();
    assert_eq!(stub.probes(), 1);
    let Verdict::Unavailable { message } = driver.verdict() else {
        panic!("logged out")
    };
    assert!(
        message.starts_with("claude auth status: exit 1: "),
        "{message}"
    );
    assert!(message.contains("Not logged in"), "{message}");

    // Refused, nothing spawned, nothing billed — and re-probed exactly once.
    let (reply, consumed) = send(&driver, 1, agent::run_payload("x")).await;
    let (kind, message) = error_of(&reply);
    assert_eq!(kind, ErrorKind::Unavailable);
    assert!(message.contains("Not logged in"), "{message}");
    assert!(reply.transcript.is_empty() && reply.envelope.is_none());
    assert_eq!(reply.mode, None, "the probe's document never crosses");
    assert_eq!(consumed, Consumption::none());
    assert!(stub.argv().is_empty(), "the CLI was not run");
    assert_eq!(stub.probes(), 2, "one re-probe, not a loop");

    let (reply, _) = send(&driver, 2, agent::run_payload("x")).await;
    assert_eq!(error_of(&reply).0, ErrorKind::Unavailable);
    assert_eq!(stub.probes(), 3);

    // The human logs in: the next send probes, finds it, and runs.
    stub.set_logged_in(true);
    let (reply, _) = send(&driver, 3, agent::run_payload("x")).await;
    assert_eq!(reply.stop, Stop::Done);
    assert_eq!(stub.probes(), 4);
    let (reply, _) = send(&driver, 4, agent::run_payload("x")).await;
    assert_eq!(reply.stop, Stop::Done);
    assert_eq!(stub.probes(), 4, "a ready verdict costs no subprocess");
}

#[tokio::test]
async fn a_result_that_names_an_auth_failure_is_unavailable_and_flips_the_verdict() {
    let dir = agent::Temp::new("auth-failure");
    let script = agent::synthetic_script(
        dir.path(),
        "auth",
        json!({
            "type": "result", "subtype": "error_during_execution", "is_error": true,
            "num_turns": 1, "total_cost_usd": 0.0, "api_error_status": 401,
            "usage": { "input_tokens": 0, "output_tokens": 0 },
            "errors": ["authentication_error: OAuth token has expired. Please run /login."]
        }),
        1,
    );
    let (stub, driver) = driver(dir.path(), &script);
    let (reply, consumed) = send(&driver, 1, agent::run_payload("x")).await;
    let (kind, message) = error_of(&reply);
    assert_eq!(kind, ErrorKind::Unavailable);
    assert!(message.contains("OAuth token has expired"), "{message}");
    assert_eq!(
        consumed.get(&DimKey::Tokens),
        Some(0),
        "what the CLI reported before it stopped"
    );
    assert!(
        !driver.verdict().is_ready(),
        "the run's own events flipped it"
    );

    // The next send re-probes once; the stub is still logged in, so it runs.
    let (_, _) = send(&driver, 2, agent::run_payload("x")).await;
    assert_eq!(stub.probes(), 2);
}

#[tokio::test]
async fn a_rate_limit_is_throttled_by_status_or_by_name() {
    let dir = agent::Temp::new("throttled");
    for (name, result) in [
        (
            "status",
            json!({
                "type": "result", "subtype": "error_during_execution", "is_error": true,
                "num_turns": 1, "total_cost_usd": 0.001, "api_error_status": 429,
                "usage": { "input_tokens": 10, "output_tokens": 0 },
                "errors": ["429 {\"type\":\"error\"}"]
            }),
        ),
        (
            "name",
            json!({
                "type": "result", "subtype": "error_during_execution", "is_error": true,
                "num_turns": 1, "total_cost_usd": 0.001,
                "usage": { "input_tokens": 10, "output_tokens": 0 },
                "errors": ["Rate limit reached for the five_hour window; resets at 12:00"]
            }),
        ),
    ] {
        let script = agent::synthetic_script(dir.path(), name, result, 1);
        let (_stub, driver) = driver(dir.path(), &script);
        let (reply, consumed) = send(&driver, 1, agent::run_payload("x")).await;
        let (kind, _) = error_of(&reply);
        assert_eq!(kind, ErrorKind::Throttled, "{name}");
        assert_eq!(
            consumed.get(&DimKey::Tokens),
            Some(10),
            "{name}: what it reported"
        );
        assert!(
            driver.verdict().is_ready(),
            "{name}: a rate limit is not a logout"
        );
    }
}

#[tokio::test]
async fn any_other_error_result_is_provider_with_the_clis_text() {
    let dir = agent::Temp::new("provider");
    let script = agent::synthetic_script(
        dir.path(),
        "provider",
        json!({
            "type": "result", "subtype": "error_during_execution", "is_error": true,
            "num_turns": 2, "total_cost_usd": 0.02, "api_error_status": 500,
            "usage": { "input_tokens": 200, "output_tokens": 30 },
            "errors": ["Internal server error"]
        }),
        1,
    );
    let (_stub, driver) = driver(dir.path(), &script);
    let (reply, consumed) = send(&driver, 1, agent::run_payload("x")).await;
    let (kind, message) = error_of(&reply);
    assert_eq!(kind, ErrorKind::Provider);
    assert_eq!(message, "Internal server error");
    assert_eq!(consumed.get(&DimKey::Tokens), Some(230));
    assert_eq!(consumed.get(&DimKey::CostMicroUsd), Some(20_000));
}

// --- the ladder rows that do not wait -----------------------------------------

#[tokio::test]
async fn an_abandon_before_the_run_starts_answers_abandoned_and_spawns_nothing() {
    let dir = agent::Temp::new("early");
    let script = agent::replay_script(dir.path(), PIN, "1-hello");
    let (stub, driver) = driver(dir.path(), &script);
    driver.abandon(Corr::new(7));
    let (reply, consumed) = send(&driver, 7, agent::run_payload("x")).await;
    assert_eq!(reply.stop, Stop::Abandoned);
    assert_eq!(reply.usage, Usage::default(), "zeros");
    assert!(reply.transcript.is_empty());
    assert_eq!(consumed, Consumption::none(), "nothing billed");
    assert!(stub.argv().is_empty(), "nothing spawned");
    assert_eq!(driver.in_flight(), 0);
}

#[tokio::test]
async fn the_interrupt_is_answered_and_the_run_is_abandoned_at_the_reported_usage() {
    let dir = agent::Temp::new("interrupt");
    let script = agent::replay_script(dir.path(), PIN, "2-stdin-cancel");
    let (_stub, driver) = driver(dir.path(), &script);
    let fut = driver.handle(Delivery {
        corr: Corr::new(1),
        from: AgentId::new(1),
        payload: agent::run_payload("Create files a.txt … f.txt"),
    });
    // Registered at `handle`, so the abandon reaches the run whether or not
    // the CLI has printed anything yet; the ladder starts at once.
    driver.abandon(Corr::new(1));
    let (bytes, consumed) = tokio::time::timeout(Duration::from_secs(4), fut)
        .await
        .expect("the interrupt is answered at once");
    let reply: Reply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reply.stop, Stop::Abandoned, "#130 §5a");
    assert!(
        reply.envelope.is_none(),
        "no interrupt gives the model a turn"
    );
    assert_eq!(
        reply.session.as_deref(),
        Some("a16000ab-5032-4812-b652-7bc8717bcaf0")
    );
    assert_eq!(
        reply.usage,
        Usage {
            input_tokens: 2 + 10_084 + 10_135,
            output_tokens: 228,
            cost_microusd: Some(112_628),
            turns: Some(3),
        },
        "the CLI's own figures stand on the first rung"
    );
    assert_eq!(
        consumed.get(&DimKey::Tokens),
        Some(20_449),
        "reported, not the ceiling"
    );
    let kinds: Vec<&str> = reply
        .transcript
        .iter()
        .filter_map(|e| e["type"].as_str())
        .collect();
    assert!(kinds.contains(&"control_response"), "{kinds:?}");
    let result = reply
        .transcript
        .iter()
        .find(|e| e["type"] == "result")
        .unwrap();
    assert_eq!(result["terminal_reason"], "aborted_tools");
    assert_eq!(driver.in_flight(), 0);
}

#[tokio::test]
async fn an_interrupt_the_cli_exits_on_without_a_result_is_abandoned_at_the_ceiling() {
    let dir = agent::Temp::new("silent-exit");
    // Answers the interrupt by leaving, saying nothing: the row between
    // "reported" and "SIGTERM was needed".
    let script = agent::script(
        dir.path(),
        "silent",
        &[
            json!({ "line": { "type": "system", "subtype": "init", "session_id": "s-1", "apiKeySource": "none" } }),
            json!({ "on": { "stdin": "\"subtype\":\"interrupt\"" }, "exit": 1 }),
        ],
    );
    let (_stub, driver) = driver(dir.path(), &script);
    let fut = driver.handle(Delivery {
        corr: Corr::new(1),
        from: AgentId::new(1),
        payload: agent::run_payload("x"),
    });
    driver.abandon(Corr::new(1));
    let (bytes, consumed) = tokio::time::timeout(Duration::from_secs(4), fut)
        .await
        .expect("the fake exits on the interrupt");
    let reply: Reply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reply.stop, Stop::Abandoned);
    assert_eq!(reply.session.as_deref(), Some("s-1"));
    assert_eq!(reply.usage, Usage::default(), "nothing trustworthy");
    assert_eq!(consumed.get(&DimKey::Tokens), Some(400_000), "the ceiling");
}

#[tokio::test]
async fn dropping_the_driver_mid_run_stops_the_cli() {
    let dir = agent::Temp::new("drop");
    let script = agent::replay_script(dir.path(), PIN, "2-stdin-cancel");
    let (_stub, driver) = driver(dir.path(), &script);
    let fut = driver.handle(Delivery {
        corr: Corr::new(1),
        from: AgentId::new(1),
        payload: agent::run_payload("x"),
    });
    let run = tokio::spawn(fut);
    drop(driver);
    let (bytes, _) = tokio::time::timeout(Duration::from_secs(4), run)
        .await
        .expect("the run ended once the driver was dropped")
        .unwrap();
    let reply: Reply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        reply.stop,
        Stop::Abandoned,
        "a harness shutting down leaves no CLI behind"
    );
}

// --- describe -----------------------------------------------------------------

#[test]
fn describe_is_the_adr_projection() {
    let dir = agent::Temp::new("describe");
    let script = agent::script(dir.path(), "none", &[json!({ "exit": 0 })]);
    let (_stub, driver) = driver(dir.path(), &script);
    let schema = driver.describe().unwrap();
    let expected: Value =
        serde_json::from_str(include_str!("fixtures/agent/describe.json")).unwrap();
    assert_eq!(schema.description, expected["description"]);
    let input: Value = serde_json::from_slice(&schema.input_schema).unwrap();
    assert_eq!(input, expected["input_schema"]);
    assert_eq!(driver.ceiling().get(&DimKey::Tokens), Some(400_000));
    assert_eq!(driver.ceiling().get(&DimKey::CostMicroUsd), Some(2_000_000));
}
