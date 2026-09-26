//! The `codex` adapter (ADR-0013 §7, the `codex` column at 0.154.0):
//! the argv, the event mapping over the eight #128 transcripts, the strict
//! schema projection against its fixture, and one test per row of the
//! `stop` table, the `error.kind` table, and the ladder rows that do not
//! wait on a grace period — each through `CodexDriver` over `tau-fake-cli`
//! behind the `common/agent.rs::CodexStub` wrapper.
//!
//! The rows that wait — SIGTERM, SIGKILL, the wall bound — are
//! `agent_codex_ladder.rs`, in the `ci` profile.

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

use serde_json::{json, Value};
use tau_drivers::agent::codex::{self, CodexDriver, CAPS, CONTRACT};
use tau_drivers::agent::envelope::{Kind, Status};
use tau_drivers::agent::process::{self, Cancel, Ending};
use tau_drivers::agent::wire::Op;
use tau_drivers::agent::wire::{self, ErrorKind, Reply, Stop, Usage};
use tau_drivers::agent::{Accepted, Verdict};
use tau_kernel::abi::{AgentId, Consumption, Corr, DimKey};
use tau_kernel::driver::Driver;
use tau_kernel::kernel::Delivery;

use agent::CodexStub;

const PIN: &str = "codex-0.154.0";
const HELLO_THREAD: &str = "01a0dd5b-4680-7482-a0c0-c96ba75b1f7f";
const STRICT_FIXTURE: &str = "envelope-schema.openai-strict.json";

fn fixture_path(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/agent")
        .join(name)
}

/// One `send` through the driver, outside a kernel.
async fn send(driver: &CodexDriver, payload: Vec<u8>) -> (Reply, Consumption) {
    let (bytes, consumed) = driver
        .handle(Delivery {
            corr: Corr::new(1),
            from: AgentId::new(1),
            payload,
        })
        .await;
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    let reply = serde_json::from_value(value.clone())
        .unwrap_or_else(|e| panic!("not an agent reply: {e}\n{value:#}"));
    (reply, consumed)
}

fn driver_over(dir: &Path, script: &Path) -> (CodexStub, CodexDriver) {
    let stub = CodexStub::new(dir);
    let config = stub.config(script, dir);
    (stub, CodexDriver::new(config).unwrap())
}

fn driver_replaying(dir: &Path, run: &str) -> (CodexStub, CodexDriver) {
    let script = agent::replay_script(dir, PIN, run);
    driver_over(dir, &script)
}

/// A run over `process::run` directly, with the transcript's stdout
/// scripted and its exit as recorded.
fn run_transcript(dir: &Path, run: &str) -> process::Run {
    let script = agent::replay_script(dir, PIN, run);
    let config = agent::fake_config(&script, dir);
    let accepted = Accepted {
        op: Op::Run,
        task: "x".to_owned(),
        session: None,
        workspace: dir.to_path_buf(),
        tools: Vec::new(),
        cost_microusd: None,
        turns: None,
    };
    let mut invocation = codex::invocation(&config, &accepted, Path::new("schema.json"));
    invocation.program = config.binary.clone();
    invocation.args = Vec::new();
    process::run(&invocation, agent::bounds(5_000, 200), &Cancel::default()).unwrap()
}

fn error_of(reply: &Reply) -> &wire::RunError {
    match &reply.stop {
        Stop::Error(error) => error,
        other => panic!("not an error: {other:?}"),
    }
}

// --- the schema the CLI enforces ----------------------------------------------

/// The strict projection is what #128's runs were recorded with, byte for
/// byte in value. `TAU_UPDATE_FIXTURES=1` re-renders it, for the day the
/// envelope moves — and then the transcripts are due a re-recording, since
/// the CLI enforced the old shape.
#[test]
fn the_output_schema_is_the_committed_strict_projection() {
    let projected = codex::output_schema();
    let path = fixture_path(STRICT_FIXTURE);
    if std::env::var_os("TAU_UPDATE_FIXTURES").is_some() {
        std::fs::write(
            &path,
            format!("{}\n", serde_json::to_string_pretty(&projected).unwrap()),
        )
        .unwrap();
    }
    let committed: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(projected, committed, "{}", path.display());
}

#[test]
fn the_strict_projection_closes_every_object_and_has_no_one_of() {
    fn walk(value: &Value, at: &str) {
        match value {
            Value::Object(object) => {
                assert!(
                    object.get("oneOf").is_none(),
                    "{at}: `oneOf` is not permitted by the validator"
                );
                if object.get("type").and_then(Value::as_str) == Some("object") {
                    assert_eq!(
                        object.get("additionalProperties"),
                        Some(&Value::Bool(false)),
                        "{at}: an open object is refused"
                    );
                    let names: Vec<&str> = object["properties"]
                        .as_object()
                        .unwrap()
                        .keys()
                        .map(String::as_str)
                        .collect();
                    let required: Vec<&str> = object["required"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap())
                        .collect();
                    assert_eq!(names, required, "{at}: every property is required");
                }
                for (key, child) in object {
                    walk(child, &format!("{at}/{key}"));
                }
            }
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    walk(item, &format!("{at}[{i}]"));
                }
            }
            _ => {}
        }
    }
    let schema = codex::output_schema();
    walk(&schema, "");
    assert_eq!(
        schema["properties"]["status"]["enum"],
        json!(["ok", "partial", "failed", "cancelled"])
    );
    assert_eq!(
        schema["properties"]["artifacts"]["items"]["properties"]["kind"]["enum"],
        json!(["file", "patch", "report"])
    );
    assert!(
        schema["properties"]["status"]["description"]
            .as_str()
            .unwrap()
            .contains("`partial`: The session stopped short"),
        "the alternatives' descriptions are folded in, not lost"
    );
    // Everything the tolerant parser accepts, the strict shape still names.
    let recorded = agent::transcript(PIN, "1-hello");
    let last = agent::transcript_stdout(&recorded)
        .into_iter()
        .rfind(|e| e["type"] == "item.completed" && e["item"]["type"] == "agent_message")
        .unwrap();
    let envelope: Value = serde_json::from_str(last["item"]["text"].as_str().unwrap()).unwrap();
    let keys: Vec<&str> = envelope
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(keys, required, "the CLI wrote exactly the required keys");
}

// --- the Invocation half of the seam ------------------------------------------

#[test]
fn the_run_argv_is_the_pinned_surface_with_the_contract_in_front_of_the_task() {
    let dir = agent::Temp::new("argv");
    let mut config = agent::fake_config(dir.path(), dir.path());
    config.permission = Some("workspace-write".to_owned());
    config.model = Some("gpt-5.5".to_owned());
    config.effort = Some("high".to_owned());
    let accepted = Accepted {
        op: Op::Run,
        task: "write hello.txt containing hello".to_owned(),
        session: None,
        workspace: dir.path().join("ws"),
        tools: Vec::new(),
        cost_microusd: None,
        turns: None,
    };
    let schema = dir.path().join("schema.json");
    let invocation = codex::invocation(&config, &accepted, &schema);
    let recorded = agent::transcript(PIN, "8-hello-ignore-user-config");
    let recorded_argv: Vec<&str> = recorded[0]["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    // The fixed part is what #128 recorded, flag for flag, in order:
    // `-a never` before the subcommand, then exec's own flags.
    assert_eq!(recorded_argv[1..5], ["-a", "never", "exec", "--json"]);
    assert_eq!(
        invocation.args[..8],
        [
            "-a",
            "never",
            "exec",
            "--json",
            "--output-schema",
            &schema.display().to_string(),
            "--skip-git-repo-check",
            "--ignore-user-config",
        ]
    );
    assert_eq!(
        recorded_argv[7..9],
        ["--skip-git-repo-check", "--ignore-user-config"]
    );
    assert_eq!(recorded_argv[9..11], ["--sandbox", "workspace-write"]);
    assert_eq!(invocation.args[8..10], ["--sandbox", "workspace-write"]);
    assert_eq!(recorded_argv[11], "--cd");
    assert_eq!(
        invocation.args[10..12],
        ["--cd", &dir.path().join("ws").display().to_string()]
    );
    assert_eq!(
        invocation.args[12..16],
        ["-m", "gpt-5.5", "-c", "model_reasoning_effort=high"]
    );
    let prompt = invocation.args.last().unwrap();
    assert!(prompt.starts_with(CONTRACT), "the contract comes first");
    assert!(
        prompt.ends_with("\n\nwrite hello.txt containing hello"),
        "then the task"
    );
    assert!(
        CONTRACT.contains("v1") && !CONTRACT.contains("stdin"),
        "versioned with the wire; nothing is ever written on stdin"
    );
    assert_eq!(invocation.args.len(), 17);
    assert_eq!(invocation.cwd, dir.path().join("ws"));
    assert_eq!(
        invocation.env, config.env,
        "exactly the configured environment"
    );
    assert_eq!(invocation.first_stdin, None);
    assert_eq!(invocation.interrupt, process::Interrupt::Signal);
}

#[test]
fn the_resume_argv_is_the_json_flags_then_resume_session_and_the_amendment_alone() {
    let dir = agent::Temp::new("argv-resume");
    let mut config = agent::fake_config(dir.path(), dir.path());
    config.permission = Some("workspace-write".to_owned());
    config.model = Some("gpt-5.5".to_owned());
    let accepted = Accepted {
        op: Op::Resume,
        task: "Also write bye.txt containing bye".to_owned(),
        session: Some(HELLO_THREAD.to_owned()),
        workspace: dir.path().to_path_buf(),
        tools: Vec::new(),
        cost_microusd: None,
        turns: None,
    };
    let invocation = codex::invocation(&config, &accepted, Path::new("s.json"));
    let recorded = agent::transcript(PIN, "4-resume");
    let recorded_argv: Vec<&str> = recorded[0]["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(
        invocation.args,
        [
            "-a",
            "never",
            "exec",
            "--json",
            "--output-schema",
            "s.json",
            "--skip-git-repo-check",
            "--ignore-user-config",
            "resume",
            HELLO_THREAD,
            "Also write bye.txt containing bye",
        ]
    );
    // As recorded: no sandbox, no `--cd`, no model, no contract — the
    // thread has them. (`--ignore-user-config` was pinned after this run;
    // `exec resume --help` lists it, and it parsed live before `resume`.)
    assert_eq!(
        recorded_argv[8..],
        ["resume", HELLO_THREAD, "Also write bye.txt containing bye"]
    );
    assert!(!recorded_argv.contains(&"--sandbox"));
}

/// The one line of the seam that is `codex`'s own: which event ends the
/// turn. Both endings count, and an `agent_message` quoting one does not.
#[test]
fn the_terminal_line_is_turn_completed_or_turn_failed() {
    let dir = agent::Temp::new("terminal");
    let config = agent::fake_config(dir.path(), dir.path());
    let accepted = Accepted {
        op: Op::Run,
        task: "x".to_owned(),
        session: None,
        workspace: dir.path().to_path_buf(),
        tools: Vec::new(),
        cost_microusd: None,
        turns: None,
    };
    let terminal = codex::invocation(&config, &accepted, Path::new("s")).terminal;
    assert!(terminal(br#"{"type":"turn.completed","usage":{}}"#));
    assert!(terminal(
        br#"{"type":"turn.failed","error":{"message":"x"}}"#
    ));
    assert!(!terminal(br#"{"type":"turn.started"}"#));
    assert!(!terminal(br#"{"type":"thread.started","thread_id":"t"}"#));
    assert!(!terminal(
        br#"{"type":"item.completed","item":{"type":"agent_message","text":"{\"type\":\"turn.completed\"}"}}"#
    ));
    assert!(!terminal(b"not json {\"type\":\"turn.completed\""));
}

/// `6-stdin-held`: `codex exec` reads a piped stdin to end of file before
/// it starts. The supervisor gives a child that is never spoken to no
/// stdin at all, so a CLI that waits on it does not wait on the driver.
#[test]
fn a_child_that_is_never_spoken_to_gets_no_stdin_to_wait_on() {
    let dir = agent::Temp::new("nostdin");
    // `read` returns at once on end of file; on an open pipe it would
    // block until the wall bound, and `got` would never print either way.
    let invocation = agent::sh(
        r#"if read -r line; then printf '{"type":"got"}\n'; else printf '{"type":"eof"}\n'; fi; printf '{"type":"turn.completed"}\n'"#,
        dir.path(),
        agent::never,
    );
    let run = process::run(&invocation, agent::bounds(2_000, 200), &Cancel::default()).unwrap();
    assert_eq!(run.ending, Ending::Completed, "it did not wait on us");
    let transcript = run.transcript();
    assert_eq!(transcript[0]["type"], "eof");
    let held = agent::transcript(PIN, "6-stdin-held");
    let closed_at = held.iter().find(|r| r["tau"] == "stdin_closed").unwrap()["at_ms"]
        .as_u64()
        .unwrap();
    let first_out = held.iter().find(|r| r["tau"] == "stdout").unwrap()["at_ms"]
        .as_u64()
        .unwrap();
    assert!(
        closed_at >= 5_000 && first_out > closed_at,
        "the recording: nothing printed until stdin closed ({closed_at} ms), then {first_out} ms"
    );
}

// --- the Outcome half of the seam, over the transcripts -----------------------

#[test]
fn the_hello_run_maps_to_session_usage_and_the_last_agent_message() {
    let dir = agent::Temp::new("outcome-hello");
    let run = run_transcript(dir.path(), "1-hello");
    assert!(run.terminal_seen());
    assert_eq!(run.code(), Some(0));
    let out = codex::outcome(&run);
    assert_eq!(out.session.as_deref(), Some(HELLO_THREAD));
    assert_eq!(out.model, None, "the event stream never names the model");
    assert_eq!(
        out.mode, None,
        "the driver copies the probe's line, not the adapter"
    );
    assert_eq!(
        out.usage,
        Usage {
            input_tokens: 71_179,
            output_tokens: 490,
            cost_microusd: None,
            turns: None,
        },
        "input_tokens already holds the 64,256 cached; not added twice"
    );
    assert_eq!(out.stop, None, "the envelope decides");
    let text = String::from_utf8(out.final_message.unwrap()).unwrap();
    let envelope = tau_drivers::agent::envelope::parse(text.as_bytes()).unwrap();
    assert_eq!(envelope.status, Status::Ok);
    assert_eq!(
        envelope.artifacts.len(),
        1,
        "the *last* agent_message, not the opening one"
    );
    assert_eq!(envelope.artifacts[0].path, "hello.txt");
    assert_eq!(envelope.artifacts[0].kind, Kind::File);
}

#[test]
fn the_resume_run_re_emits_the_thread_and_reports_the_second_envelope() {
    let dir = agent::Temp::new("outcome-resume");
    let run = run_transcript(dir.path(), "4-resume");
    let out = codex::outcome(&run);
    assert_eq!(
        out.session.as_deref(),
        Some(HELLO_THREAD),
        "the same thread"
    );
    assert_eq!(out.usage.input_tokens, 119_001);
    assert_eq!(out.usage.output_tokens, 764);
    let envelope = tau_drivers::agent::envelope::parse(&out.final_message.unwrap()).unwrap();
    assert_eq!(envelope.artifacts[0].path, "bye.txt");
}

#[test]
fn a_failed_turn_is_a_provider_error_with_the_clis_text() {
    let dir = agent::Temp::new("outcome-400");
    let run = run_transcript(dir.path(), "5-model-rejected");
    assert!(run.terminal_seen(), "turn.failed ends the turn");
    assert_eq!(run.code(), Some(1));
    let out = codex::outcome(&run);
    let Some(Stop::Error(error)) = out.stop else {
        panic!("{:?}", out.stop)
    };
    assert_eq!(error.kind, ErrorKind::Provider);
    assert!(
        error
            .message
            .contains("The 'gpt-5-codex' model is not supported"),
        "{}",
        error.message
    );
    assert_eq!(
        out.session.as_deref(),
        Some("01a0dd5c-59ea-7321-8ba9-e15b9cb2163b")
    );
    assert_eq!(out.usage, Usage::default(), "a failed turn reports none");
    assert_eq!(out.final_message, None);
}

#[test]
fn sigint_and_sigterm_leave_no_end_of_turn_and_exit_1_and_143() {
    let dir = agent::Temp::new("outcome-signals");
    for (run, exit) in [("2-sigint", 1), ("3-sigterm", 143)] {
        let records = agent::transcript(PIN, run);
        let stdout = agent::transcript_stdout(&records);
        assert_eq!(
            stdout.len(),
            2,
            "{run}: thread.started, turn.started, then the signal"
        );
        let after_signal = records
            .iter()
            .skip_while(|r| r["tau"] != "signal")
            .filter(|r| r["tau"] == "stdout")
            .count();
        assert_eq!(after_signal, 0, "{run}: nothing printed after the signal");
        assert_eq!(records.last().unwrap()["code"], exit, "{run}");
        let _ = dir.path();
    }
}

// --- the stop table and the error.kind table, through the driver --------------

#[tokio::test]
async fn done_carries_the_envelope_the_thread_the_probe_line_and_bills_the_usage() {
    let dir = agent::Temp::new("done");
    let (stub, driver) = driver_replaying(dir.path(), "1-hello");
    assert_eq!(driver.version(), "codex-cli 0.154.0");
    assert_eq!(
        driver.verdict(),
        Verdict::Ready {
            mode: Some("Logged in using ChatGPT".to_owned())
        },
        "the one line `codex login status` printed, from stderr"
    );
    assert!(
        driver.schema_file().is_file(),
        "written for --output-schema"
    );
    let (reply, consumed) = send(
        &driver,
        agent::run_payload("write hello.txt containing hello"),
    )
    .await;
    assert_eq!(reply.v, wire::VERSION);
    assert_eq!(reply.stop, Stop::Done);
    let envelope = reply.envelope.unwrap();
    assert_eq!(envelope.status, Status::Ok);
    assert_eq!(envelope.artifacts[0].path, "hello.txt");
    assert_eq!(reply.session.as_deref(), Some(HELLO_THREAD));
    assert_eq!(reply.cli.name, "codex");
    assert_eq!(reply.cli.version, "codex-cli 0.154.0");
    assert_eq!(reply.model, None);
    assert_eq!(reply.mode.as_deref(), Some("Logged in using ChatGPT"));
    assert_eq!(reply.usage.input_tokens, 71_179);
    assert_eq!(reply.usage.output_tokens, 490);
    assert_eq!(reply.usage.cost_microusd, None, "codex states no cost");
    assert_eq!(reply.transcript.len(), 13, "every event, carried unread");
    assert!(!reply.truncated.transcript);
    assert_eq!(consumed.get(&DimKey::Tokens), Some(71_179 + 490));
    assert_eq!(
        consumed.get(&DimKey::CostMicroUsd),
        None,
        "no cost stated and no prices configured: not reported"
    );
    let argv = stub.argv();
    assert_eq!(argv[..4], ["-a", "never", "exec", "--json"]);
    assert_eq!(argv[5], driver.schema_file().display().to_string());
    assert!(argv
        .last()
        .unwrap()
        .ends_with("write hello.txt containing hello"));
    assert_eq!(driver.in_flight(), 0);
}

#[tokio::test]
async fn the_schema_file_goes_with_the_last_driver_handle() {
    let dir = agent::Temp::new("schema-drop");
    let (_stub, driver) = driver_replaying(dir.path(), "1-hello");
    let path = driver.schema_file().to_path_buf();
    let written: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(written, codex::output_schema());
    let other = driver.clone();
    drop(driver);
    assert!(path.is_file(), "a clone still holds it");
    drop(other);
    assert!(!path.exists(), "removed with the last handle");
}

#[tokio::test]
async fn resume_is_served_and_continues_the_named_thread() {
    let dir = agent::Temp::new("resume");
    let (stub, driver) = driver_replaying(dir.path(), "4-resume");
    let (reply, consumed) = send(
        &driver,
        agent::resume_payload(HELLO_THREAD, "Also write bye.txt containing bye"),
    )
    .await;
    assert_eq!(reply.stop, Stop::Done);
    assert_eq!(reply.envelope.unwrap().artifacts[0].path, "bye.txt");
    assert_eq!(reply.session.as_deref(), Some(HELLO_THREAD));
    assert_eq!(consumed.get(&DimKey::Tokens), Some(119_001 + 764));
    let argv = stub.argv();
    assert_eq!(
        argv[8..],
        ["resume", HELLO_THREAD, "Also write bye.txt containing bye"]
    );
}

#[tokio::test]
async fn an_unknown_thread_is_a_provider_error_billed_at_nothing() {
    let dir = agent::Temp::new("unknown");
    let stub = CodexStub::new(dir.path());
    let script = agent::replay_script(dir.path(), PIN, "7-resume-unknown");
    let mut config = stub.config(&script, dir.path());
    let records = agent::transcript(PIN, "7-resume-unknown");
    assert!(
        agent::transcript_stdout(&records).is_empty(),
        "nothing on stdout: no thread was started"
    );
    let stderr = records.last().unwrap()["stderr"]
        .as_str()
        .unwrap()
        .trim()
        .to_owned();
    config
        .env
        .push((agent::REFUSE_VAR.to_owned(), stderr.clone()));
    let driver = CodexDriver::new(config).unwrap();
    let (reply, consumed) = send(
        &driver,
        agent::resume_payload("00000000-0000-0000-0000-000000000000", "hi"),
    )
    .await;
    let error = error_of(&reply);
    assert_eq!(error.kind, ErrorKind::Provider);
    assert!(
        error.message.contains("no rollout found for thread id"),
        "{}",
        error.message
    );
    assert!(error.message.contains("exited 1 before starting a thread"));
    assert_eq!(reply.transcript, Vec::<Value>::new());
    assert_eq!(
        consumed,
        Consumption::none(),
        "nothing reached the provider"
    );
    assert_eq!(
        driver.verdict(),
        Verdict::Ready {
            mode: Some("Logged in using ChatGPT".to_owned())
        },
        "a refused resume says nothing about the login"
    );
}

#[tokio::test]
async fn a_failed_turn_through_the_driver_is_provider_and_bills_what_was_reported() {
    let dir = agent::Temp::new("provider");
    let (_stub, driver) = driver_replaying(dir.path(), "5-model-rejected");
    let (reply, consumed) = send(&driver, agent::run_payload("x")).await;
    let error = error_of(&reply);
    assert_eq!(error.kind, ErrorKind::Provider);
    assert!(error.message.contains("not supported"), "{}", error.message);
    assert!(reply.envelope.is_none());
    assert!(reply.session.is_some(), "the thread was started");
    assert_eq!(
        consumed.get(&DimKey::Tokens),
        Some(0),
        "the turn ended and reported nothing: zero, not the ceiling"
    );
}

#[tokio::test]
async fn a_logged_out_cli_is_unavailable_and_re_probed_once_per_send() {
    let dir = agent::Temp::new("logged-out");
    let stub = CodexStub::new(dir.path());
    let script = agent::replay_script(dir.path(), PIN, "1-hello");
    let config = stub.config(&script, dir.path());
    stub.set_logged_in(false);
    let driver = CodexDriver::new(config).unwrap();
    let Verdict::Unavailable { message } = driver.verdict() else {
        panic!("{:?}", driver.verdict())
    };
    assert!(
        message.starts_with("codex login status: exit 1: Error checking login status"),
        "{message}"
    );
    assert_eq!(stub.probes(), 1);
    let (reply, consumed) = send(&driver, agent::run_payload("x")).await;
    assert_eq!(error_of(&reply).kind, ErrorKind::Unavailable);
    assert_eq!(consumed, Consumption::none());
    assert_eq!(stub.probes(), 2, "one re-probe, never a loop");
    assert!(stub.argv().is_empty(), "nothing was spawned");

    // A human logs in: the next send re-probes and runs.
    stub.set_logged_in(true);
    let (reply, _) = send(&driver, agent::run_payload("x")).await;
    assert_eq!(reply.stop, Stop::Done);
    assert_eq!(reply.mode.as_deref(), Some("Logged in using ChatGPT"));
    assert_eq!(stub.probes(), 3);
}

#[tokio::test]
async fn a_401_in_the_events_is_unavailable_flips_the_verdict_and_bills_nothing() {
    let dir = agent::Temp::new("401");
    // #130 §3's loop, as a failed turn: the probe said ready, the run
    // says otherwise.
    let script = agent::script(
        dir.path(),
        "401",
        &[
            json!({ "line": { "type": "thread.started", "thread_id": "t-401" } }),
            json!({ "line": { "type": "turn.started" } }),
            json!({ "line": { "type": "error", "message": "stream error: exceeded retry limit, last status: 401 Unauthorized, request id: x; retrying 1/5 in 188ms…" } }),
            json!({ "line": { "type": "turn.failed", "error": { "message": "exceeded retry limit, last status: 401 Unauthorized" } } }),
            json!({ "exit": 1 }),
        ],
    );
    let (_stub, driver) = driver_over(dir.path(), &script);
    assert!(driver.verdict().is_ready());
    let (reply, consumed) = send(&driver, agent::run_payload("x")).await;
    let error = error_of(&reply);
    assert_eq!(error.kind, ErrorKind::Unavailable);
    assert!(
        error.message.contains("401 Unauthorized"),
        "{}",
        error.message
    );
    assert_eq!(
        consumed.get(&DimKey::Tokens),
        Some(0),
        "the turn reported nothing"
    );
    assert!(
        matches!(driver.verdict(), Verdict::Unavailable { .. }),
        "the run's own events flipped the verdict"
    );
}

#[tokio::test]
async fn a_rate_limit_is_throttled() {
    let dir = agent::Temp::new("429");
    let script = agent::script(
        dir.path(),
        "429",
        &[
            json!({ "line": { "type": "thread.started", "thread_id": "t-429" } }),
            json!({ "line": { "type": "turn.started" } }),
            json!({ "line": { "type": "turn.failed", "error": { "message": "unexpected status 429 Too Many Requests: usage limit reached" } } }),
            json!({ "exit": 1 }),
        ],
    );
    let (_stub, driver) = driver_over(dir.path(), &script);
    let (reply, _) = send(&driver, agent::run_payload("x")).await;
    let error = error_of(&reply);
    assert_eq!(error.kind, ErrorKind::Throttled);
    assert!(error.message.contains("429"));
    assert!(driver.verdict().is_ready(), "a rate limit is not a logout");
}

#[tokio::test]
async fn a_final_message_that_is_not_an_envelope_is_the_envelope_error_with_the_transcript() {
    let dir = agent::Temp::new("envelope");
    let script = agent::script(
        dir.path(),
        "prose",
        &[
            json!({ "line": { "type": "thread.started", "thread_id": "t-prose" } }),
            json!({ "line": { "type": "turn.started" } }),
            json!({ "line": { "type": "item.completed", "item": { "id": "item_0", "type": "agent_message", "text": "I wrote the file. Done!" } } }),
            json!({ "line": { "type": "turn.completed", "usage": { "input_tokens": 10, "cached_input_tokens": 0, "cache_write_input_tokens": 0, "output_tokens": 5, "reasoning_output_tokens": 0 } } }),
            json!({ "exit": 0 }),
        ],
    );
    let (_stub, driver) = driver_over(dir.path(), &script);
    let (reply, consumed) = send(&driver, agent::run_payload("x")).await;
    let error = error_of(&reply);
    assert_eq!(error.kind, ErrorKind::Envelope);
    assert!(
        error.message.contains("no balanced JSON object"),
        "{}",
        error.message
    );
    assert!(reply.envelope.is_none(), "never synthesized");
    assert_eq!(reply.transcript.len(), 4, "what the CLI actually said");
    assert_eq!(consumed.get(&DimKey::Tokens), Some(15), "real usage");
}

#[tokio::test]
async fn no_final_message_at_all_is_the_envelope_error_too() {
    let dir = agent::Temp::new("silent");
    let script = agent::script(
        dir.path(),
        "silent",
        &[
            json!({ "line": { "type": "thread.started", "thread_id": "t-silent" } }),
            json!({ "line": { "type": "turn.started" } }),
            json!({ "line": { "type": "turn.completed", "usage": { "input_tokens": 1, "output_tokens": 1 } } }),
            json!({ "exit": 0 }),
        ],
    );
    let (_stub, driver) = driver_over(dir.path(), &script);
    let (reply, _) = send(&driver, agent::run_payload("x")).await;
    let error = error_of(&reply);
    assert_eq!(error.kind, ErrorKind::Envelope);
    assert!(
        error.message.contains("no final message"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn a_run_that_ends_without_its_end_of_turn_is_lost_and_bills_the_ceiling() {
    let dir = agent::Temp::new("lost");
    let records = agent::transcript(PIN, "1-hello");
    let (mut directives, _) = agent::script_from_transcript(&records);
    directives.insert(0, json!({ "withhold": "\"type\":\"turn.completed\"" }));
    let script = agent::script(dir.path(), "lost", &directives);
    let (_stub, driver) = driver_over(dir.path(), &script);
    let (reply, consumed) = send(&driver, agent::run_payload("x")).await;
    let error = error_of(&reply);
    assert_eq!(error.kind, ErrorKind::Lost);
    assert!(
        error.message.contains("without its terminal event"),
        "{}",
        error.message
    );
    assert_eq!(
        reply.session.as_deref(),
        Some(HELLO_THREAD),
        "the thread is still named"
    );
    assert_eq!(reply.usage, Usage::default(), "nothing trustworthy");
    assert_eq!(consumed.get(&DimKey::Tokens), Some(400_000), "the ceiling");
    assert_eq!(consumed.get(&DimKey::CostMicroUsd), Some(2_000_000));
}

#[tokio::test]
async fn unsupported_is_tools_budget_a_foreign_version_and_nothing_is_spawned() {
    let dir = agent::Temp::new("unsupported");
    let (stub, driver) = driver_replaying(dir.path(), "1-hello");
    for (payload, expect) in [
        (
            json!({ "op": "run", "task": "x", "tools": ["Bash"] }),
            "no per-session tool allowlist",
        ),
        (
            json!({ "op": "run", "task": "x", "budget": { "turns": 3 } }),
            "enforces no per-run budget",
        ),
        (json!({ "v": 2, "op": "run", "task": "x" }), "version 2"),
        (json!({ "op": "run", "task": "" }), "task is empty"),
        (
            json!({ "op": "run", "task": "x", "workspace": "../out" }),
            "plain path relative",
        ),
    ] {
        let (reply, consumed) = send(&driver, serde_json::to_vec(&payload).unwrap()).await;
        let error = error_of(&reply);
        assert_eq!(error.kind, ErrorKind::Unsupported, "{payload}");
        assert!(
            error.message.contains(expect),
            "{payload}: {}",
            error.message
        );
        assert_eq!(consumed, Consumption::none());
    }
    assert!(stub.argv().is_empty(), "nothing was spawned");
}

#[tokio::test]
async fn a_binary_that_cannot_start_is_a_host_error_billed_at_nothing() {
    let dir = agent::Temp::new("host");
    let (_stub, driver) = driver_replaying(dir.path(), "1-hello");
    // The workspace resolves to a directory the child cannot start in:
    // remove it between decode and spawn by asking for one that is a file.
    std::fs::write(dir.path().join("file"), "").unwrap();
    let (reply, consumed) = send(
        &driver,
        serde_json::to_vec(&json!({ "op": "run", "task": "x", "workspace": "file" })).unwrap(),
    )
    .await;
    assert_eq!(error_of(&reply).kind, ErrorKind::Host);
    assert_eq!(consumed, Consumption::none());
}

#[tokio::test]
async fn an_abandon_that_arrives_first_is_abandoned_with_nothing_billed() {
    let dir = agent::Temp::new("early");
    let (stub, driver) = driver_replaying(dir.path(), "1-hello");
    driver.abandon(Corr::new(1));
    let (reply, consumed) = send(&driver, agent::run_payload("x")).await;
    assert_eq!(reply.stop, Stop::Abandoned);
    assert_eq!(reply.usage, Usage::default());
    assert_eq!(reply.transcript, Vec::<Value>::new());
    assert_eq!(consumed, Consumption::none());
    assert!(stub.argv().is_empty(), "nothing was spawned");
    assert_eq!(driver.in_flight(), 0);
}

/// The SIGINT row of the ladder (`2-sigint`): the CLI prints nothing more
/// and exits 1. No end of turn, so the run is billed at the ceiling; no
/// grace period elapses, because the fake answers the signal at once.
#[tokio::test]
async fn sigint_stops_the_run_at_the_first_rung_and_bills_the_ceiling() {
    let dir = agent::Temp::new("sigint");
    let records = agent::transcript(PIN, "2-sigint");
    let (mut directives, stimulus) = agent::script_from_transcript(&records);
    assert_eq!(stimulus, Some(agent::Stimulus::Signal("SIGINT".to_owned())));
    let ready = dir.path().join("ready");
    directives.insert(2, json!({ "touch": ready }));
    directives.insert(3, agent::still_working());
    let script = agent::script(dir.path(), "2-sigint", &directives);
    let (_stub, driver) = driver_over(dir.path(), &script);
    let fut = driver.handle(Delivery {
        corr: Corr::new(7),
        from: AgentId::new(1),
        payload: agent::run_payload("write hello.txt containing hello"),
    });
    let marker = ready.clone();
    tokio::task::spawn_blocking(move || agent::wait_for(&marker))
        .await
        .unwrap();
    assert_eq!(driver.in_flight(), 1);
    driver.abandon(Corr::new(7));
    let (bytes, consumed) = fut.await;
    let reply: Reply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reply.stop, Stop::Abandoned);
    assert!(reply.envelope.is_none());
    assert_eq!(
        reply.session.as_deref(),
        Some("01a0dd5b-4681-7a13-9be5-abc14190e243")
    );
    assert_eq!(reply.usage, Usage::default());
    assert_eq!(
        reply.transcript.len(),
        2,
        "thread.started, turn.started, silence"
    );
    assert_eq!(consumed.get(&DimKey::Tokens), Some(400_000), "the ceiling");
    assert_eq!(consumed.get(&DimKey::CostMicroUsd), Some(2_000_000));
    assert_eq!(driver.in_flight(), 0);
}

// --- describe() ---------------------------------------------------------------

#[test]
fn describe_omits_tools_and_budget_and_offers_resume() {
    let dir = agent::Temp::new("describe");
    let (_stub, driver) = driver_replaying(dir.path(), "1-hello");
    let schema = driver.describe().unwrap();
    assert_eq!(
        schema.description,
        "Delegate a whole task to a codex session in the workspace. It runs headless, may \
         spend up to $2 (400k tokens) per call, and answers with a JSON report: status, \
         summary, artifacts, assumptions, events. Pass `session` from a previous report to \
         continue it."
    );
    let input: Value = serde_json::from_slice(&schema.input_schema).unwrap();
    assert_eq!(input, wire::schema(CAPS));
    let branches = input["oneOf"].as_array().unwrap();
    assert_eq!(branches.len(), 2, "run and resume");
    let run = &branches[0]["properties"];
    assert!(run.get("tools").is_none(), "a model never writes one");
    assert!(run.get("budget").is_none());
    assert!(run.get("task").is_some() && run.get("workspace").is_some());
    assert_eq!(branches[1]["properties"]["op"]["const"], "resume");
    assert_eq!(input["additionalProperties"], false);
    assert_eq!(driver.ceiling(), driver.config().ceiling());
}
