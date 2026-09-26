//! The `codex` adapter (ADR-0013 §7, the `codex` column) against
//! `tau-fake-cli` replaying the #128 transcripts: the fixed argv, the event
//! mapping over every recorded run, and one test per row of the `stop`
//! table and the `error.kind` table that does not wait on a grace period.
//!
//! The rows that do — SIGTERM after an ignored SIGINT, SIGKILL, the wall
//! bound — are `agent_codex_ladder.rs`, the `ci` profile's. The driver
//! through a real kernel is `agent_codex_kernel.rs`.
//!
//! The binary the driver is constructed over is a `/bin/sh` stub
//! (`common/codex.rs::CodexStub`): `tau-fake-cli` replays its script
//! whatever its argv, so it cannot answer the version and login probes on
//! its own. The stub answers those the way #128 recorded them, records the
//! argv of every run, and `exec`s the fake for the run itself.

#![cfg(all(feature = "agent-codex", unix))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::type_complexity
)]

#[path = "common/agent.rs"]
mod agent;
#[path = "common/codex.rs"]
mod codex;

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use tau_drivers::agent::codex::{self as adapter, CodexDriver, CAPS, CONTRACT};
use tau_drivers::agent::envelope::{self, Status};
use tau_drivers::agent::process::{Ending, Interrupt, Run};
use tau_drivers::agent::wire::{ErrorKind, Reply, Stop, Usage};
use tau_drivers::agent::{accept, wire, Accepted, AgentConfig, ConfigError, Verdict};
use tau_kernel::abi::{AgentId, Consumption, Corr, DimKey};
use tau_kernel::driver::Driver;
use tau_kernel::kernel::Delivery;

use codex::{CodexStub, MODE, PIN, THREAD_HELLO, VERSION};

/// The stub and a driver over it, replaying `script`, rooted at `dir`.
fn driver(dir: &Path, script: &Path) -> (CodexStub, CodexDriver) {
    let stub = CodexStub::new(dir);
    let config = stub.config(script, dir);
    let driver = CodexDriver::new(config).unwrap();
    (stub, driver)
}

/// One `send` through the driver, outside a kernel.
async fn send(driver: &CodexDriver, corr: u64, payload: Vec<u8>) -> (Reply, Consumption) {
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
/// had the CLI run to its end: every stdout line, the `turn.*` end marked.
fn run_from(pin: &str, name: &str) -> Run {
    let stdout = agent::transcript_stdout(&agent::transcript(pin, name));
    Run {
        lines: stdout
            .iter()
            .map(|line| line.to_string().into_bytes())
            .collect(),
        dropped: 0,
        truncated: false,
        stderr: String::new(),
        ending: Ending::Completed,
        terminal_at: stdout
            .iter()
            .position(|line| line["type"] == "turn.completed" || line["type"] == "turn.failed"),
        status: None,
    }
}

fn accepted(config: &AgentConfig, payload: Value) -> Accepted {
    accept(&serde_json::from_value(payload).unwrap(), config, CAPS).unwrap()
}

fn after(args: &[String], flag: &str) -> String {
    let at = args
        .iter()
        .position(|a| a == flag)
        .unwrap_or_else(|| panic!("{flag} in {args:?}"));
    args[at + 1].clone()
}

// --- the Invocation half ------------------------------------------------------

#[test]
fn the_fixed_argv_is_the_adrs_then_the_config_then_the_op() {
    let dir = agent::Temp::new("argv");
    let stub = CodexStub::new(dir.path());
    let script = agent::script(dir.path(), "none", &[json!({ "exit": 0 })]);
    let config = stub.config(&script, dir.path());
    let schema = dir.path().join("schema.json");

    let run = accepted(&config, json!({ "op": "run", "task": "write hello.txt" }));
    let invocation = adapter::invocation(&config, &run, &schema);
    assert_eq!(invocation.program, stub.binary);
    assert_eq!(invocation.cwd, dir.path());
    assert_eq!(
        invocation.env, config.env,
        "exactly the configured environment"
    );
    let args = invocation.args.clone();
    assert_eq!(
        &args[..7],
        [
            "-a",
            "never",
            "exec",
            "--json",
            "--skip-git-repo-check",
            "--ignore-user-config",
            "--output-schema",
        ],
        "the fixed argv, ADR-0013 §7: `-a never` before the subcommand"
    );
    assert_eq!(
        after(&args, "--output-schema"),
        schema.display().to_string()
    );
    assert_eq!(after(&args, "--sandbox"), "workspace-write");
    assert_eq!(after(&args, "--cd"), dir.path().display().to_string());
    assert!(!args.iter().any(|a| a == "-m" || a == "-c"));
    assert_eq!(
        args.last().unwrap(),
        &format!("{CONTRACT}\n\nTask: write hello.txt"),
        "the contract prepended to the task, as the positional prompt (run 1)"
    );
    assert!(
        !args.iter().any(|a| a == "--ephemeral"),
        "never --ephemeral: a resume needs the thread on disk"
    );
    assert_eq!(
        invocation.first_stdin, None,
        "the task is the prompt, not stdin"
    );
    assert_eq!(
        invocation.interrupt,
        Interrupt::Signal,
        "exec has no in-band channel"
    );

    // The config's model and effort go after `exec`: a top-level `-m` is
    // silently ignored (#128 probed it).
    let mut config = config;
    config.model = Some("gpt-6-luna".to_owned());
    config.effort = Some("high".to_owned());
    let args = adapter::invocation(&config, &run, &schema).args;
    let exec_at = args.iter().position(|a| a == "exec").unwrap();
    let model_at = args.iter().position(|a| a == "-m").unwrap();
    assert!(model_at > exec_at, "{args:?}");
    assert_eq!(after(&args, "-m"), "gpt-6-luna");
    assert_eq!(after(&args, "-c"), "model_reasoning_effort=\"high\"");

    // `resume`: the id and the amendment as positionals, the cage as a
    // `-c` override because `exec resume` has no `--sandbox`, no `--cd`
    // because a resumed thread works in the child's cwd (runs 4, 5).
    let resume = accepted(
        &config,
        json!({ "op": "resume", "session": THREAD_HELLO, "task": "Also write bye.txt containing bye" }),
    );
    let invocation = adapter::invocation(&config, &resume, &schema);
    let args = invocation.args;
    assert_eq!(&args[..5], ["-a", "never", "exec", "resume", THREAD_HELLO]);
    assert!(!args.iter().any(|a| a == "--sandbox" || a == "--cd"));
    let overrides: Vec<&String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| *a == "-c" && *i > 0)
        .map(|(i, _)| &args[i + 1])
        .collect();
    assert_eq!(
        overrides,
        [
            "sandbox_mode=\"workspace-write\"",
            "model_reasoning_effort=\"high\""
        ]
    );
    assert_eq!(
        args.last().unwrap(),
        "Also write bye.txt containing bye",
        "the amendment alone: the thread has the contract"
    );
    assert_eq!(invocation.cwd, dir.path(), "the workspace is the cwd");
    assert!(!args.iter().any(|a| a.contains(CONTRACT)));
}

#[test]
fn the_terminal_line_is_a_turn_end_and_nothing_that_merely_mentions_one() {
    let dir = agent::Temp::new("terminal");
    let stub = CodexStub::new(dir.path());
    let script = agent::script(dir.path(), "none", &[json!({ "exit": 0 })]);
    let config = stub.config(&script, dir.path());
    let run = accepted(&config, json!({ "op": "run", "task": "x" }));
    let terminal = adapter::invocation(&config, &run, Path::new("s.json")).terminal;
    assert!(terminal(br#"{"type":"turn.completed","usage":{}}"#));
    assert!(terminal(
        br#"{"type":"turn.failed","error":{"message":"x"}}"#
    ));
    assert!(
        terminal(br#"{"usage":{},"type":"turn.completed"}"#),
        "any key order"
    );
    assert!(
        !terminal(br#"{"type":"turn.started"}"#),
        "the turn starting is not the turn ending"
    );
    assert!(
        !terminal(
            br#"{"type":"item.completed","item":{"type":"agent_message","text":"a {\"type\":\"turn.completed\"} line"}}"#
        ),
        "an agent message that quotes one"
    );
    assert!(!terminal(b"not json \"type\":\"turn.completed\""));
}

// --- the Outcome half, over the committed transcripts -------------------------

#[test]
fn the_hello_run_reads_thread_usage_and_the_envelope() {
    let outcome = adapter::outcome(&run_from(PIN, "1-hello"));
    assert_eq!(
        outcome.session.as_deref(),
        Some(THREAD_HELLO),
        "thread.started's thread_id"
    );
    assert_eq!(outcome.model, None, "no event names the model at 0.157.1");
    assert_eq!(outcome.mode, None, "the driver fills it from the probe");
    assert_eq!(
        outcome.usage,
        Usage {
            input_tokens: 27_831,
            output_tokens: 122,
            cost_microusd: None,
            turns: None,
        },
        "as stated: cached_input_tokens is a subset, not summed"
    );
    assert_eq!(
        outcome.stop, None,
        "turn.completed: the envelope parser decides"
    );
    let envelope = envelope::parse(&outcome.final_message.unwrap()).unwrap();
    assert_eq!(envelope.status, Status::Ok);
    assert_eq!(envelope.artifacts[0].path, "hello.txt");
}

#[test]
fn every_transcript_maps_as_the_adr_table_says() {
    // (pin, run, stop kind, input tokens, output tokens, final message parses as, session)
    let table: &[(
        &str,
        &str,
        Option<ErrorKind>,
        u64,
        u64,
        Option<Status>,
        Option<&str>,
    )] = &[
        (
            PIN,
            "1-hello",
            None,
            27_831,
            122,
            Some(Status::Ok),
            Some(THREAD_HELLO),
        ),
        (
            PIN,
            "2-sigint",
            None,
            0,
            0,
            None,
            Some("01a0dd5b-8645-7911-bca0-456c91b4e599"),
        ),
        (
            PIN,
            "3-sigterm",
            None,
            0,
            0,
            None,
            Some("01a0dd5b-8539-7612-8c4d-6ff32230a011"),
        ),
        (
            PIN,
            "4-resume",
            None,
            42_140,
            222,
            Some(Status::Failed),
            Some(THREAD_HELLO),
        ),
        (
            PIN,
            "5-resume-workspace-write",
            None,
            72_300,
            333,
            Some(Status::Ok),
            Some(THREAD_HELLO),
        ),
        (
            PIN,
            "6-schema-rejected",
            Some(ErrorKind::Provider),
            0,
            0,
            None,
            Some("01a0dd5a-597e-73d2-9d16-8ab2b265950a"),
        ),
        (
            PIN,
            "7-hello-ignore-user-config",
            None,
            42_297,
            171,
            Some(Status::Ok),
            Some("01a0dd62-f233-7530-a98a-07290cff0995"),
        ),
        (
            "codex-0.46.0",
            "1-hello",
            Some(ErrorKind::Provider),
            0,
            0,
            None,
            Some("01a0dd55-5f8e-7d42-b364-e9875ebfa89e"),
        ),
    ];
    for (pin, run, kind, input, output, status, session) in table {
        let outcome = adapter::outcome(&run_from(pin, run));
        let got = match &outcome.stop {
            None => None,
            Some(Stop::Error(error)) => Some(error.kind),
            other => panic!("{pin}/{run}: {other:?}"),
        };
        assert_eq!(got, *kind, "{pin}/{run}");
        assert_eq!(outcome.usage.input_tokens, *input, "{pin}/{run}");
        assert_eq!(outcome.usage.output_tokens, *output, "{pin}/{run}");
        assert_eq!(
            outcome.usage.cost_microusd, None,
            "{pin}/{run}: codex states no cost"
        );
        assert_eq!(outcome.usage.turns, None, "{pin}/{run}: nor a turn count");
        let parsed = outcome
            .final_message
            .as_deref()
            .map(|text| envelope::parse(text).unwrap().status);
        assert_eq!(parsed, *status, "{pin}/{run}");
        assert_eq!(outcome.session.as_deref(), *session, "{pin}/{run}");
        assert_eq!(outcome.model, None, "{pin}/{run}");
    }
    // The two failures say what the backend said, verbatim.
    let schema = adapter::outcome(&run_from(PIN, "6-schema-rejected"));
    let Some(Stop::Error(error)) = schema.stop else {
        panic!()
    };
    assert!(
        error.message.contains("invalid_json_schema"),
        "{}",
        error.message
    );
    let model = adapter::outcome(&run_from("codex-0.46.0", "1-hello"));
    let Some(Stop::Error(error)) = model.stop else {
        panic!()
    };
    assert!(
        error
            .message
            .contains("not supported when using Codex with a ChatGPT account"),
        "{}",
        error.message
    );
    assert_eq!(
        adapter::auth_failure(&run_from("codex-0.46.0", "1-hello")),
        None,
        "a refused model is not a logout"
    );
}

#[test]
fn the_401_retry_loop_of_130_names_an_auth_failure() {
    // #130 §3's unsigned run: `error` events retrying on 401, no terminal
    // event. The transcript is the gist's `codex-unsigned.out`, first line.
    let lines = [
        json!({ "type": "thread.started", "thread_id": "01a0ab3f-a6ce-7af0-9152-d904c3c67a6f" }),
        json!({ "type": "turn.started" }),
        json!({ "type": "error", "message": "stream error: exceeded retry limit, last status: 401 Unauthorized, request id: a3c190c4c8367829-CDG; retrying 1/5 in 188ms…" }),
    ];
    let run = Run {
        lines: lines.iter().map(|l| l.to_string().into_bytes()).collect(),
        dropped: 0,
        truncated: false,
        stderr: String::new(),
        ending: Ending::WallLimit(tau_drivers::agent::process::Rung::Term),
        terminal_at: None,
        status: None,
    };
    let said = adapter::auth_failure(&run).unwrap();
    assert!(said.contains("401 Unauthorized"), "{said}");
    assert_eq!(
        adapter::outcome(&run).stop,
        None,
        "no terminal event: settle says lost or wall"
    );
}

// --- the stop table, through the driver ---------------------------------------

#[tokio::test]
async fn a_hello_run_is_done_with_its_envelope_and_the_reported_bill() {
    let dir = agent::Temp::new("hello");
    let script = codex::replay_script(dir.path(), PIN, "1-hello");
    let (stub, driver) = driver(dir.path(), &script);
    assert_eq!(driver.version(), VERSION);
    assert!(driver.verdict().is_ready());
    assert_eq!(
        driver.verdict().mode(),
        Some(MODE),
        "the stderr line, verbatim (run 0)"
    );

    let (reply, consumed) = send(
        &driver,
        1,
        codex::run_payload("write hello.txt containing hello"),
    )
    .await;
    assert_eq!(reply.stop, Stop::Done, "{:?}", reply.stop);
    assert_eq!(reply.v, wire::VERSION);
    assert_eq!(reply.cli.name, "codex");
    assert_eq!(reply.cli.version, VERSION);
    let envelope = reply.envelope.unwrap();
    assert_eq!(envelope.status, Status::Ok);
    assert!(
        envelope.summary.contains("hello.txt"),
        "{}",
        envelope.summary
    );
    assert_eq!(reply.session.as_deref(), Some(THREAD_HELLO));
    assert_eq!(reply.model, None);
    assert_eq!(reply.mode.as_deref(), Some(MODE));
    assert_eq!(reply.usage.tokens(), 27_831 + 122);
    assert_eq!(reply.usage.cost_microusd, None);
    assert_eq!(reply.transcript.len(), 7, "every event, thread to turn");
    assert!(!reply.truncated.transcript);
    assert_eq!(
        consumed.get(&DimKey::Tokens),
        Some(27_953),
        "what the CLI reported"
    );
    assert_eq!(
        consumed.get(&DimKey::CostMicroUsd),
        None,
        "no cost stated, no prices configured"
    );
    assert_eq!(
        consumed.get(&DimKey::Calls),
        None,
        "the kernel counts calls"
    );
    assert_eq!(driver.in_flight(), 0);

    // What the CLI was actually run with: the argv the pure half built,
    // over a schema file that existed for the run and is gone after it.
    let argv = stub.argv();
    let schema_path = after(&argv, "--output-schema");
    assert!(
        schema_path.ends_with(".schema.json") && schema_path.contains("tau-agent-codex-"),
        "{schema_path}"
    );
    assert!(
        !Path::new(&schema_path).exists(),
        "removed when the run ended"
    );
    assert_eq!(
        stub.schema_seen().unwrap(),
        envelope::schema(),
        "the strict envelope schema, as the CLI read it"
    );
    let config = driver.config();
    let expected = adapter::invocation(
        config,
        &accepted(
            config,
            json!({ "op": "run", "task": "write hello.txt containing hello" }),
        ),
        Path::new(&schema_path),
    );
    assert_eq!(argv, expected.args, "the driver spawned the fixed argv");
}

#[tokio::test]
async fn resume_continues_the_thread_the_reply_named() {
    let dir = agent::Temp::new("resume");
    let script = codex::replay_script(dir.path(), PIN, "5-resume-workspace-write");
    let (stub, driver) = driver(dir.path(), &script);
    let (reply, consumed) = send(
        &driver,
        1,
        codex::resume_payload(THREAD_HELLO, "Also write bye.txt containing bye"),
    )
    .await;
    assert_eq!(reply.stop, Stop::Done);
    assert_eq!(reply.session.as_deref(), Some(THREAD_HELLO), "the same id");
    assert!(reply.envelope.unwrap().summary.contains("bye.txt"));
    assert_eq!(consumed.get(&DimKey::Tokens), Some(72_300 + 333));
    let argv = stub.argv();
    assert_eq!(&argv[2..5], ["exec", "resume", THREAD_HELLO]);
    assert!(argv.contains(&"sandbox_mode=\"workspace-write\"".to_owned()));
}

#[tokio::test]
async fn a_resume_without_the_cage_reports_the_envelope_the_session_wrote() {
    // Run 4: the recipe's argv, no sandbox on resume, the CLI ran read-only
    // and the session said so. `done` — the run finished — with a `failed`
    // envelope: the task's verdict is the session's, not the driver's.
    let dir = agent::Temp::new("resume-ro");
    let script = codex::replay_script(dir.path(), PIN, "4-resume");
    let (_stub, driver) = driver(dir.path(), &script);
    let (reply, _) = send(&driver, 1, codex::resume_payload(THREAD_HELLO, "x")).await;
    assert_eq!(reply.stop, Stop::Done);
    let envelope = reply.envelope.unwrap();
    assert_eq!(envelope.status, Status::Failed);
    assert!(envelope.error.unwrap().contains("read-only"));
}

/// A `turn.failed` through the driver: `provider`, the backend's text, the
/// thread named, nothing stated so nothing billed, the verdict untouched.
async fn failed_turn_is_provider(pin: &str, run: &str) {
    let dir = agent::Temp::new("failed");
    let script = codex::replay_script(dir.path(), pin, run);
    let (_stub, driver) = driver(dir.path(), &script);
    let (reply, consumed) = send(&driver, 1, codex::run_payload("x")).await;
    let (kind, message) = error_of(&reply);
    assert_eq!(kind, ErrorKind::Provider, "{pin}/{run}");
    assert!(
        message.contains("400 Bad Request") || message.contains("invalid_json_schema"),
        "{pin}/{run}: {message}"
    );
    assert!(reply.envelope.is_none());
    assert!(reply.session.is_some(), "{pin}/{run}: the thread started");
    assert_eq!(
        consumed.get(&DimKey::Tokens),
        Some(0),
        "{pin}/{run}: the terminal event arrived and stated nothing"
    );
    assert!(
        driver.verdict().is_ready(),
        "{pin}/{run}: a backend refusal is not a logout"
    );
}

#[tokio::test]
async fn a_rejected_schema_is_provider_with_the_backends_text() {
    failed_turn_is_provider(PIN, "6-schema-rejected").await;
}

#[tokio::test]
async fn a_rejected_model_is_provider_with_the_backends_text() {
    failed_turn_is_provider("codex-0.46.0", "1-hello").await;
}

#[tokio::test]
async fn a_final_message_that_is_not_an_envelope_is_error_envelope_with_the_transcript() {
    let dir = agent::Temp::new("envelope");
    let script = agent::script(
        dir.path(),
        "prose",
        &[
            json!({ "line": { "type": "thread.started", "thread_id": "t-1" } }),
            json!({ "line": { "type": "turn.started" } }),
            json!({ "line": { "type": "item.completed", "item": { "id": "item_0", "type": "agent_message", "text": "I could not do it, sorry." } } }),
            json!({ "line": { "type": "turn.completed", "usage": { "input_tokens": 100, "cached_input_tokens": 50, "output_tokens": 20 } } }),
            json!({ "exit": 0 }),
        ],
    );
    let (_stub, driver) = driver(dir.path(), &script);
    let (reply, consumed) = send(&driver, 1, codex::run_payload("x")).await;
    let (kind, message) = error_of(&reply);
    assert_eq!(kind, ErrorKind::Envelope);
    assert!(message.contains("no balanced JSON object"), "{message}");
    assert!(
        reply.envelope.is_none(),
        "never a synthesized {{status: failed}}"
    );
    assert_eq!(reply.transcript.len(), 4, "what the CLI actually said");
    assert_eq!(
        consumed.get(&DimKey::Tokens),
        Some(120),
        "somebody paid for it"
    );
}

#[tokio::test]
async fn a_run_that_ends_without_its_turn_end_is_lost_at_the_ceiling() {
    let dir = agent::Temp::new("lost");
    let records = agent::transcript(PIN, "1-hello");
    let (mut directives, _) = agent::script_from_transcript(&records);
    directives.insert(0, json!({ "withhold": "\"type\":\"turn.completed\"" }));
    let script = agent::script(dir.path(), "lost", &directives);
    let (_stub, driver) = driver(dir.path(), &script);
    let (reply, consumed) = send(&driver, 1, codex::run_payload("x")).await;
    let (kind, message) = error_of(&reply);
    assert_eq!(kind, ErrorKind::Lost);
    assert!(message.contains("without its terminal event"), "{message}");
    assert_eq!(reply.usage, Usage::default(), "nothing trustworthy");
    assert_eq!(
        reply.session.as_deref(),
        Some(THREAD_HELLO),
        "thread.started still names it"
    );
    assert_eq!(consumed.get(&DimKey::Tokens), Some(400_000), "the ceiling");
    assert_eq!(
        consumed.get(&DimKey::CostMicroUsd),
        None,
        "no cost ceiling without a cost bound or prices"
    );
}

// --- the error.kind table -----------------------------------------------------

#[tokio::test]
async fn an_unsupported_request_spawns_nothing_and_bills_nothing() {
    let dir = agent::Temp::new("unsupported");
    let script = codex::replay_script(dir.path(), PIN, "1-hello");
    let (stub, driver) = driver(dir.path(), &script);
    for (payload, needle) in [
        (json!({ "v": 2, "op": "run", "task": "x" }), "version 2"),
        (
            json!({ "op": "run", "task": "x", "tools": ["Read"] }),
            "no per-session tool allowlist",
        ),
        (
            json!({ "op": "run", "task": "x", "budget": { "turns": 3 } }),
            "no per-run budget",
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
    let script = codex::replay_script(dir.path(), PIN, "1-hello");
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
    let stub = CodexStub::new(dir.path());
    let script = agent::script(dir.path(), "none", &[json!({ "exit": 0 })]);

    let mut missing = stub.config(&script, dir.path());
    missing.binary = dir.path().join("no-such-codex");
    let err = CodexDriver::new(missing).unwrap_err();
    assert!(matches!(err, ConfigError::Binary { .. }), "{err}");

    let mut pinned = stub.config(&script, dir.path());
    pinned.expect_version = Some("0.46.0".to_owned());
    let err = CodexDriver::new(pinned).unwrap_err();
    assert!(matches!(err, ConfigError::Version { .. }), "{err}");
    assert!(err.to_string().contains("0.157.1"), "{err}");

    let mut matching = stub.config(&script, dir.path());
    matching.expect_version = Some("0.157.1".to_owned());
    CodexDriver::new(matching).unwrap();
}

#[tokio::test]
async fn logged_out_is_a_clean_refusal_that_reprobes_once_per_send() {
    let dir = agent::Temp::new("logged-out");
    let script = codex::replay_script(dir.path(), PIN, "1-hello");
    let stub = CodexStub::new(dir.path());
    let config = stub.config(&script, dir.path());
    stub.set_logged_in(false);

    // Construction does not fail over a login: it is a runtime state.
    let driver = CodexDriver::new(config).unwrap();
    assert_eq!(stub.probes(), 1);
    let Verdict::Unavailable { message } = driver.verdict() else {
        panic!("logged out")
    };
    assert!(
        message.starts_with("codex login status: exit 1: "),
        "{message}"
    );
    assert!(message.contains("Not logged in"), "{message}");

    // Refused, nothing spawned, nothing billed — and re-probed exactly once.
    // For `codex` this gate is the only one that works: an unsigned run
    // retries on 401 and never reports "not authenticated" (#130 §3).
    let (reply, consumed) = send(&driver, 1, codex::run_payload("x")).await;
    let (kind, message) = error_of(&reply);
    assert_eq!(kind, ErrorKind::Unavailable);
    assert!(message.contains("Not logged in"), "{message}");
    assert!(reply.transcript.is_empty() && reply.envelope.is_none());
    assert_eq!(reply.mode, None);
    assert_eq!(consumed, Consumption::none());
    assert!(stub.argv().is_empty(), "the CLI was not run");
    assert_eq!(stub.probes(), 2, "one re-probe, not a loop");

    let (reply, _) = send(&driver, 2, codex::run_payload("x")).await;
    assert_eq!(error_of(&reply).0, ErrorKind::Unavailable);
    assert_eq!(stub.probes(), 3);

    // The human logs in: the next send probes, finds it, and runs.
    stub.set_logged_in(true);
    let (reply, _) = send(&driver, 3, codex::run_payload("x")).await;
    assert_eq!(reply.stop, Stop::Done);
    assert_eq!(reply.mode.as_deref(), Some(MODE), "the fresh probe's line");
    assert_eq!(stub.probes(), 4);
    let (reply, _) = send(&driver, 4, codex::run_payload("x")).await;
    assert_eq!(reply.stop, Stop::Done);
    assert_eq!(stub.probes(), 4, "a ready verdict costs no subprocess");
}

#[tokio::test]
async fn a_failed_turn_that_names_an_auth_failure_is_unavailable_and_flips_the_verdict() {
    let dir = agent::Temp::new("auth-failure");
    let script = codex::failed_script(
        dir.path(),
        "auth",
        "stream error: exceeded retry limit, last status: 401 Unauthorized, request id: x-CDG",
    );
    let (stub, driver) = driver(dir.path(), &script);
    let (reply, consumed) = send(&driver, 1, codex::run_payload("x")).await;
    let (kind, message) = error_of(&reply);
    assert_eq!(kind, ErrorKind::Unavailable);
    assert!(message.contains("401 Unauthorized"), "{message}");
    assert_eq!(
        consumed.get(&DimKey::Tokens),
        Some(0),
        "what the CLI stated"
    );
    assert!(
        !driver.verdict().is_ready(),
        "the run's own events flipped it"
    );

    // The next send re-probes once; the stub is still logged in, so it runs.
    let (_, _) = send(&driver, 2, codex::run_payload("x")).await;
    assert_eq!(stub.probes(), 2);
}

#[tokio::test]
async fn a_request_id_that_happens_to_contain_401_is_not_a_logout() {
    let dir = agent::Temp::new("hex-id");
    let script = codex::failed_script(
        dir.path(),
        "hex",
        "stream error: unexpected status 500 Internal Server Error, request id: 9f401c4293b7-CDG",
    );
    let (_stub, driver) = driver(dir.path(), &script);
    let (reply, _) = send(&driver, 1, codex::run_payload("x")).await;
    let (kind, _) = error_of(&reply);
    assert_eq!(
        kind,
        ErrorKind::Provider,
        "three digits in a hex id are not a status"
    );
    assert!(driver.verdict().is_ready());
}

#[tokio::test]
async fn a_rate_limit_or_a_quota_window_is_throttled_by_name() {
    let dir = agent::Temp::new("throttled");
    for (name, message) in [
        (
            "status",
            "stream error: unexpected status 429 Too Many Requests",
        ),
        (
            "quota",
            "You've hit your usage limit. Upgrade to Pro or try again at 3pm",
        ),
        (
            "rate",
            "Rate limit reached for gpt-6-luna in organization org-x",
        ),
    ] {
        let script = codex::failed_script(dir.path(), name, message);
        let (_stub, driver) = driver(dir.path(), &script);
        let (reply, _) = send(&driver, 1, codex::run_payload("x")).await;
        let (kind, said) = error_of(&reply);
        assert_eq!(kind, ErrorKind::Throttled, "{name}: {said}");
        assert_eq!(said, message, "{name}: verbatim");
        assert!(
            driver.verdict().is_ready(),
            "{name}: a rate limit is not a logout"
        );
    }
}

// --- the ladder rows that do not wait -----------------------------------------

#[tokio::test]
async fn an_abandon_before_the_run_starts_answers_abandoned_and_spawns_nothing() {
    let dir = agent::Temp::new("early");
    let script = codex::replay_script(dir.path(), PIN, "1-hello");
    let (stub, driver) = driver(dir.path(), &script);
    driver.abandon(Corr::new(7));
    let (reply, consumed) = send(&driver, 7, codex::run_payload("x")).await;
    assert_eq!(reply.stop, Stop::Abandoned);
    assert_eq!(reply.usage, Usage::default(), "zeros");
    assert!(reply.transcript.is_empty());
    assert_eq!(consumed, Consumption::none(), "nothing billed");
    assert!(stub.argv().is_empty(), "nothing spawned");
    assert_eq!(driver.in_flight(), 0);
}

#[tokio::test]
async fn sigint_ends_the_run_with_nothing_said_abandoned_at_the_ceiling() {
    // Run 2: SIGINT two seconds into the turn, nothing more printed, exit 1,
    // no terminal event. The first rung answered, but the CLI stated no
    // usage, so the row is "interrupt → exited without a terminal event":
    // the ceiling.
    let dir = agent::Temp::new("sigint");
    let script = codex::replay_script(dir.path(), PIN, "2-sigint");
    let (_stub, driver) = driver(dir.path(), &script);
    let fut = driver.handle(Delivery {
        corr: Corr::new(1),
        from: AgentId::new(1),
        payload: codex::run_payload("write hello.txt containing hello"),
    });
    // Registered at `handle`, so the abandon reaches the run whether or not
    // the CLI has printed anything yet; the ladder starts at once.
    driver.abandon(Corr::new(1));
    let (bytes, consumed) = tokio::time::timeout(Duration::from_secs(4), fut)
        .await
        .expect("SIGINT ends it at once");
    let reply: Reply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reply.stop, Stop::Abandoned, "#128 run 2");
    assert!(
        reply.envelope.is_none(),
        "no interrupt gives the model a turn"
    );
    assert_eq!(reply.usage, Usage::default(), "nothing trustworthy");
    assert!(
        !reply
            .transcript
            .iter()
            .any(|e| e["type"] == "turn.completed"),
        "SIGINT prints nothing further"
    );
    assert_eq!(consumed.get(&DimKey::Tokens), Some(400_000), "the ceiling");
    assert_eq!(driver.in_flight(), 0);
}

#[tokio::test]
async fn dropping_the_driver_mid_run_stops_the_cli() {
    let dir = agent::Temp::new("drop");
    let script = codex::replay_script(dir.path(), PIN, "2-sigint");
    let (_stub, driver) = driver(dir.path(), &script);
    let fut = driver.handle(Delivery {
        corr: Corr::new(1),
        from: AgentId::new(1),
        payload: codex::run_payload("x"),
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
fn describe_offers_run_and_resume_and_neither_tools_nor_budget() {
    let dir = agent::Temp::new("describe");
    let script = agent::script(dir.path(), "none", &[json!({ "exit": 0 })]);
    let (_stub, driver) = driver(dir.path(), &script);
    let schema = driver.describe().unwrap();
    assert_eq!(
        schema.description,
        "Delegate a whole task to a codex session in the workspace. It runs headless, may \
         use up to 400k tokens per call, and answers with a JSON report: status, summary, \
         artifacts, assumptions, events. Pass `session` from a previous report to continue it."
    );
    let input: Value = serde_json::from_slice(&schema.input_schema).unwrap();
    assert_eq!(input, wire::schema(CAPS));
    let branches = input["oneOf"].as_array().unwrap();
    assert_eq!(branches.len(), 2, "run and resume");
    let run = branches
        .iter()
        .find(|b| b["properties"]["op"]["const"] == "run")
        .unwrap();
    assert!(
        run["properties"].get("tools").is_none(),
        "refused, so not offered"
    );
    assert!(run["properties"].get("budget").is_none());
    assert_eq!(driver.ceiling().get(&DimKey::Tokens), Some(400_000));
    assert_eq!(driver.ceiling().get(&DimKey::CostMicroUsd), None);
}
