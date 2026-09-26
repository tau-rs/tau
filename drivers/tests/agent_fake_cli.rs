//! `tau-fake-cli` (ADR-0013 §10): the scripted stand-in for an agent CLI,
//! spawned through `std::process::Command` and through the driver's own
//! `process::run`. #130's cancel runs, committed under
//! `cassettes/cli/claude-2.1.272/`, are its first scripts: the in-band
//! interrupt is answered with a `control_response` and a `result` and exit
//! 1, `SIGINT` with a `result` and exit 0, `SIGTERM` with silence and exit
//! 143.
//!
//! Every test here is the `quick` profile's: nothing waits on a grace
//! period. Zero-delay lines are printed before any stimulus is looked at,
//! so the order of what a test reads is fixed by the script, not by timing.

#![cfg(all(feature = "agent", unix))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "common/agent.rs"]
mod agent;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};

use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use serde_json::{json, Value};
use tau_drivers::agent::process::{self, Cancel, Ending, Interrupt, Rung};

use agent::Stimulus;

const PIN: &str = "claude-2.1.272";

/// The fake, spawned the way a test that is not the driver spawns it:
/// stdin and stdout piped, its script named in the environment.
fn spawn(script: &Path, stdin: Stdio) -> (Child, BufReader<ChildStdout>) {
    let mut child = Command::new(agent::FAKE_CLI)
        .env_clear()
        .envs(agent::fake_env(script))
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    (child, stdout)
}

/// The next stdout line, parsed; `None` at end of file.
fn next(stdout: &mut BufReader<ChildStdout>) -> Option<Value> {
    let mut line = String::new();
    match stdout.read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(serde_json::from_str(line.trim_end()).unwrap()),
    }
}

/// Every remaining stdout line, to end of file.
fn rest(stdout: &mut BufReader<ChildStdout>) -> Vec<Value> {
    let mut lines = Vec::new();
    while let Some(line) = next(stdout) {
        lines.push(line);
    }
    lines
}

fn signal(child: &Child, signal: Signal) {
    kill(Pid::from_raw(i32::try_from(child.id()).unwrap()), signal).unwrap();
}

/// A committed transcript as a script on disk, with the lines it printed
/// before and after its stimulus.
struct Replay {
    path: std::path::PathBuf,
    before: Vec<Value>,
    after: Vec<Value>,
    stimulus: Option<Stimulus>,
    exit: i64,
}

fn replay(dir: &Path, run: &str) -> Replay {
    let records = agent::transcript(PIN, run);
    let (directives, stimulus) = agent::script_from_transcript(&records);
    let stdout = agent::transcript_stdout(&records);
    let split = records
        .iter()
        .take_while(|record| {
            !(record["tau"] == "signal"
                || (record["tau"] == "stdin" && record["line"]["type"] == "control_request"))
        })
        .filter(|record| record["tau"] == "stdout")
        .count();
    let (before, after) = stdout.split_at(split);
    Replay {
        path: agent::script(dir, run, &directives),
        before: before.to_vec(),
        after: after.to_vec(),
        stimulus,
        exit: records.last().unwrap()["code"].as_i64().unwrap(),
    }
}

#[test]
fn the_hello_run_replays_line_for_line_and_exits_as_recorded() {
    let dir = agent::Temp::new("hello");
    let run = replay(dir.path(), "1-hello");
    assert_eq!(run.stimulus, None, "#130 §4 ran to its end");
    assert_eq!(run.before.len(), 11, "eleven events, hook to idle");

    let (mut child, mut stdout) = spawn(&run.path, Stdio::piped());
    let stdin = child.stdin.take().unwrap();
    let lines = rest(&mut stdout);
    drop(stdin);
    assert_eq!(lines, run.before, "every line, in order, value for value");
    assert_eq!(lines[3]["subtype"], "init");
    assert!(
        lines[3].get("plugins").is_none(),
        "the replay is the scrubbed transcript"
    );
    assert_eq!(lines[9]["type"], "result");
    assert_eq!(
        child.wait().unwrap().code(),
        Some(i32::try_from(run.exit).unwrap())
    );
}

#[test]
fn the_in_band_interrupt_is_answered_then_the_fake_exits_1() {
    let dir = agent::Temp::new("interrupt");
    let run = replay(dir.path(), "2-stdin-cancel");
    assert_eq!(run.stimulus, Some(Stimulus::Interrupt));
    assert_eq!(run.exit, 1);

    let (mut child, mut stdout) = spawn(&run.path, Stdio::piped());
    let mut stdin = child.stdin.take().unwrap();
    // The task, then the interrupt, as the driver writes them.
    stdin
        .write_all(
            b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"a.txt..f.txt\"}}\n",
        )
        .unwrap();
    stdin.write_all(agent::INTERRUPT.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let lines = rest(&mut stdout);
    let (before, after) = lines.split_at(run.before.len());
    assert_eq!(before, run.before, "the turn, up to the tool_use");
    assert_eq!(after, run.after, "#130 §5a: what an interrupt prints");
    assert_eq!(after[0]["type"], "control_response");
    assert_eq!(after[0]["response"]["request_id"], "tau-cancel-1");
    let result = after.iter().find(|line| line["type"] == "result").unwrap();
    assert_eq!(result["terminal_reason"], "aborted_tools");
    assert_eq!(result["num_turns"], 3);
    assert_eq!(child.wait().unwrap().code(), Some(1));
}

#[test]
fn sigint_prints_a_result_then_the_fake_exits_0() {
    let dir = agent::Temp::new("sigint");
    let run = replay(dir.path(), "3-sigint");
    assert_eq!(run.stimulus, Some(Stimulus::Signal("SIGINT".to_owned())));

    let (mut child, mut stdout) = spawn(&run.path, Stdio::piped());
    let stdin = child.stdin.take().unwrap();
    for expected in &run.before {
        assert_eq!(next(&mut stdout).as_ref(), Some(expected));
    }
    // The handler is installed before the first line is printed, so a
    // signal sent after the last timeline line is caught, never fatal.
    signal(&child, Signal::SIGINT);
    let after = rest(&mut stdout);
    drop(stdin);
    assert_eq!(after, run.after, "#130 §5b: what SIGINT prints");
    let result = after.last().unwrap();
    assert_eq!(result["type"], "result");
    assert_eq!(result["terminal_reason"], "aborted_streaming");
    assert_eq!(child.wait().unwrap().code(), Some(0));
}

#[test]
fn sigterm_prints_nothing_and_the_fake_exits_143() {
    let dir = agent::Temp::new("sigterm");
    let run = replay(dir.path(), "4-sigterm");
    assert_eq!(run.stimulus, Some(Stimulus::Signal("SIGTERM".to_owned())));
    assert!(run.after.is_empty(), "#130 §5c: silence");

    let (mut child, mut stdout) = spawn(&run.path, Stdio::piped());
    let stdin = child.stdin.take().unwrap();
    for expected in &run.before {
        assert_eq!(next(&mut stdout).as_ref(), Some(expected));
    }
    signal(&child, Signal::SIGTERM);
    assert_eq!(rest(&mut stdout), Vec::<Value>::new());
    drop(stdin);
    let status = child.wait().unwrap();
    assert_eq!(
        status.code(),
        Some(143),
        "exited, as `claude` does, not killed"
    );
}

#[test]
fn an_unscripted_signal_keeps_its_default_disposition() {
    let dir = agent::Temp::new("default");
    let path = agent::script(
        dir.path(),
        "init-only",
        &[json!({ "line": { "type": "system", "subtype": "init" } })],
    );
    let (mut child, mut stdout) = spawn(&path, Stdio::piped());
    let stdin = child.stdin.take().unwrap();
    assert_eq!(next(&mut stdout).unwrap()["subtype"], "init");
    signal(&child, Signal::SIGTERM);
    assert_eq!(rest(&mut stdout), Vec::<Value>::new());
    drop(stdin);
    let status = child.wait().unwrap();
    assert_eq!(
        status.signal(),
        Some(Signal::SIGTERM as i32),
        "killed, not exited"
    );
}

#[test]
fn an_ignored_signal_leaves_the_fake_running_for_sigkill() {
    let dir = agent::Temp::new("stubborn");
    // SIGINT is answered with a line and no exit: the proof of life. SIGTERM
    // is swallowed. Only SIGKILL ends it.
    let path = agent::script(
        dir.path(),
        "stubborn",
        &[
            json!({ "line": { "type": "system", "subtype": "init" } }),
            json!({ "on": { "signal": "SIGINT" }, "lines": [{ "type": "ping" }] }),
            json!({ "on": { "signal": "SIGTERM" }, "ignore": true }),
        ],
    );
    let (mut child, mut stdout) = spawn(&path, Stdio::piped());
    let stdin = child.stdin.take().unwrap();
    assert_eq!(next(&mut stdout).unwrap()["subtype"], "init");
    signal(&child, Signal::SIGINT);
    assert_eq!(next(&mut stdout).unwrap()["type"], "ping");
    signal(&child, Signal::SIGTERM);
    signal(&child, Signal::SIGINT);
    assert_eq!(
        next(&mut stdout).unwrap()["type"],
        "ping",
        "still alive after SIGTERM"
    );
    signal(&child, Signal::SIGKILL);
    assert_eq!(rest(&mut stdout), Vec::<Value>::new());
    drop(stdin);
    assert_eq!(child.wait().unwrap().signal(), Some(Signal::SIGKILL as i32));
}

#[test]
fn a_stimulus_during_a_delay_replaces_what_was_pending() {
    let dir = agent::Temp::new("delay");
    let path = agent::script(
        dir.path(),
        "delay",
        &[
            json!({ "line": { "n": 1 } }),
            json!({ "line": { "n": 2 }, "delay_ms": 60_000 }),
            json!({ "on": { "stdin": "go" }, "lines": [{ "n": 3 }], "exit": 7 }),
        ],
    );
    let (mut child, mut stdout) = spawn(&path, Stdio::piped());
    let mut stdin = child.stdin.take().unwrap();
    assert_eq!(next(&mut stdout).unwrap()["n"], 1);
    stdin.write_all(b"go\n").unwrap();
    stdin.flush().unwrap();
    assert_eq!(
        rest(&mut stdout),
        vec![json!({ "n": 3 })],
        "the held-back line was abandoned with the turn"
    );
    assert_eq!(child.wait().unwrap().code(), Some(7));
}

#[test]
fn a_delayed_line_still_arrives_and_eof_ends_a_spent_timeline() {
    let dir = agent::Temp::new("eof");
    let path = agent::script(
        dir.path(),
        "eof",
        &[
            json!({ "line": "first" }),
            json!({ "line": { "n": 2 }, "delay_ms": 20 }),
        ],
    );
    // stdin is `/dev/null`: end of file at once, which must not cut the
    // timeline short.
    let (mut child, mut stdout) = spawn(&path, Stdio::null());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert_eq!(line, "first\n", "a string is printed verbatim");
    assert_eq!(rest(&mut stdout), vec![json!({ "n": 2 })]);
    assert_eq!(
        child.wait().unwrap().code(),
        Some(0),
        "spent, and stdin closed"
    );
}

#[test]
fn a_missing_or_malformed_script_exits_2_with_nothing_on_stdout() {
    let dir = agent::Temp::new("bad");
    let unset = Command::new(agent::FAKE_CLI)
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(unset.status.code(), Some(2));
    assert!(
        unset.stdout.is_empty(),
        "nothing a driver could mistake for an event"
    );
    assert!(String::from_utf8_lossy(&unset.stderr).contains(agent::SCRIPT_VAR));

    let path = dir.path().join("bad.jsonl");
    std::fs::write(&path, "{\"line\": 1}\n{\"delay_ms\": 5}\n").unwrap();
    let malformed = Command::new(agent::FAKE_CLI)
        .env_clear()
        .env(agent::SCRIPT_VAR, &path)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(malformed.status.code(), Some(2));
    assert!(malformed.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&malformed.stderr).contains("line 2"),
        "{}",
        String::from_utf8_lossy(&malformed.stderr)
    );
}

// Through the driver's own supervisor: the fake is what every agent test
// points `AgentConfig::binary` at.

#[test]
fn the_driver_climbs_the_first_rung_against_the_fake_and_bills_the_reported_usage() {
    let dir = agent::Temp::new("ladder");
    let run = replay(dir.path(), "2-stdin-cancel");
    let mut invocation = agent::fake(&run.path, dir.path(), agent::result_line);
    invocation.first_stdin = Some(
        "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"a.txt..f.txt\"}}\n"
            .to_owned(),
    );
    invocation.interrupt = Interrupt::InBand(agent::INTERRUPT.to_owned());
    let cancel = Cancel::default();
    cancel.abandon();

    let outcome = process::run(&invocation, agent::bounds(5_000, 500), &cancel).unwrap();
    assert_eq!(outcome.ending, Ending::Abandoned(Rung::Interrupt));
    assert!(outcome.terminal_seen());
    assert!(outcome.usage_is_reported(), "the CLI's own figures stand");
    assert_eq!(outcome.code(), Some(1));
    let transcript = outcome.transcript();
    assert_eq!(transcript, [run.before, run.after].concat());
    assert_eq!(
        transcript[outcome.terminal_at.unwrap()]["terminal_reason"],
        "aborted_tools"
    );
}

#[test]
fn withholding_the_terminal_event_makes_the_run_lost() {
    let dir = agent::Temp::new("lost");
    let records = agent::transcript(PIN, "1-hello");
    let (mut directives, _) = agent::script_from_transcript(&records);
    directives.insert(0, json!({ "withhold": "\"type\":\"result\"" }));
    let path = agent::script(dir.path(), "lost", &directives);

    let invocation = agent::fake(&path, dir.path(), agent::result_line);
    let outcome = process::run(&invocation, agent::bounds(5_000, 200), &Cancel::default()).unwrap();
    assert_eq!(outcome.ending, Ending::Completed, "it ended by itself");
    assert!(!outcome.terminal_seen(), "no result: billed at the ceiling");
    assert!(!outcome.usage_is_reported());
    assert_eq!(outcome.code(), Some(0));
    assert_eq!(outcome.lines.len(), 10, "everything but the result");
}

#[test]
fn the_config_helper_points_the_registration_at_the_fake() {
    let dir = agent::Temp::new("config");
    let path = agent::script(dir.path(), "empty", &[json!({ "exit": 0 })]);
    let config = agent::fake_config(&path, dir.path());
    assert_eq!(config.binary, Path::new(agent::FAKE_CLI));
    assert_eq!(config.name, "claude");
    assert!(config
        .env
        .iter()
        .any(|(k, v)| k == agent::SCRIPT_VAR && Path::new(v) == path));
    assert!(Path::new(agent::FAKE_CLI).is_file());
}

#[test]
fn a_touch_directive_marks_readiness_after_the_handlers_are_installed() {
    let dir = agent::Temp::new("touch");
    let ready = dir.path().join("ready");
    // The marker lands after the handlers are installed, so a signal sent
    // once it exists is caught, however slowly the binary started.
    let path = agent::script(
        dir.path(),
        "touch",
        &[
            json!({ "touch": ready }),
            json!({ "on": { "signal": "SIGTERM" }, "lines": [{ "type": "caught" }], "exit": 3 }),
        ],
    );
    let (mut child, mut stdout) = spawn(&path, Stdio::piped());
    let stdin = child.stdin.take().unwrap();
    agent::wait_for(&ready);
    signal(&child, Signal::SIGTERM);
    assert_eq!(rest(&mut stdout), vec![json!({ "type": "caught" })]);
    drop(stdin);
    assert_eq!(child.wait().unwrap().code(), Some(3));

    let bad = dir.path().join("bad.jsonl");
    std::fs::write(&bad, "{\"touch\": 7}\n").unwrap();
    let malformed = Command::new(agent::FAKE_CLI)
        .env_clear()
        .env(agent::SCRIPT_VAR, &bad)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(malformed.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&malformed.stderr).contains("touch must be a string"));
}
