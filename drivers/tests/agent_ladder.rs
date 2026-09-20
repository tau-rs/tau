//! The cancel ladder and the bounded drain (ADR-0013 §5), against `/bin/sh`
//! children that behave the way #130 observed the real CLIs behaving: one
//! that answers the in-band interrupt, one that ignores every signal, one
//! that never emits its terminal event.
//!
//! This binary is the `ci` profile's: each rung is a grace period long by
//! construction, and the `quick` profile's five-second ceiling is for tests
//! that do not wait on anything.

#![cfg(all(feature = "agent", unix))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "common/agent.rs"]
mod agent;

use std::sync::Arc;
use std::time::Duration;

use tau_drivers::agent::process::{self, Cancel, Ending, Interrupt, Rung};

const INIT: &str = r#"printf '{"type":"system","subtype":"init","session_id":"affe155e"}\n'"#;
const RESULT: &str = r#"printf '{"type":"result","subtype":"success","num_turns":3}\n'"#;

#[test]
fn a_run_that_ends_on_its_own_is_completed_with_its_terminal_event() {
    let dir = agent::Temp::new("done");
    let invocation = agent::sh(
        &format!("{INIT}; {RESULT}; printf 'trailing\\n'"),
        dir.path(),
        agent::result_line,
    );
    let run = process::run(&invocation, agent::bounds(5_000, 200), &Cancel::default()).unwrap();

    assert_eq!(run.ending, Ending::Completed);
    assert!(run.terminal_seen());
    assert_eq!(run.code(), Some(0));
    assert_eq!(run.lines.len(), 3);
    assert_eq!(
        run.terminal_at,
        Some(1),
        "not the last line: #130 run 1 printed a state change after its result"
    );
    assert!(!run.truncated);
    assert!(run.usage_is_reported(), "the CLI said what it used");
    let transcript = run.transcript();
    assert_eq!(transcript[0]["subtype"], "init");
    assert_eq!(
        transcript[2]["raw"], "trailing",
        "a line that is not JSON is carried, not dropped"
    );
}

#[test]
fn the_task_arrives_on_stdin_and_stdin_closes_when_the_terminal_event_lands() {
    let dir = agent::Temp::new("stdin");
    // Echoes the task back, reports, then blocks until stdin is closed: if
    // the driver held it open the child would sit here until the wall bound.
    let mut invocation = agent::sh(
        &format!("read task; printf '{{\"task\":\"%s\"}}\\n' \"$task\"; {RESULT}; while read x; do :; done"),
        dir.path(),
        agent::result_line,
    );
    invocation.first_stdin = Some("write hello.txt containing hello\n".to_owned());

    let run = process::run(&invocation, agent::bounds(5_000, 200), &Cancel::default()).unwrap();
    assert_eq!(
        run.ending,
        Ending::Completed,
        "it exited, it was not stopped"
    );
    assert_eq!(
        run.transcript()[0]["task"],
        "write hello.txt containing hello"
    );
    assert!(run.terminal_seen());
}

#[test]
fn the_in_band_interrupt_is_the_first_rung_and_its_usage_is_real() {
    let dir = agent::Temp::new("interrupt");
    // #130 §5a: the CLI acknowledges the interrupt, emits a `result` with
    // real usage, and exits 1.
    let mut invocation = agent::sh(
        &format!(
            "{INIT}; read control; printf '{{\"type\":\"control_response\"}}\\n'; {RESULT}; exit 1"
        ),
        dir.path(),
        agent::result_line,
    );
    invocation.interrupt = Interrupt::InBand(
        r#"{"type":"control_request","request_id":"tau-cancel-1","request":{"subtype":"interrupt"}}"#
            .to_owned()
            + "\n",
    );
    let cancel = Cancel::default();
    // Abandoned before the driver polls it: the registry's `abandoned_early`
    // path, and the reason the ladder starts at once here.
    cancel.abandon();

    let run = process::run(&invocation, agent::bounds(5_000, 500), &cancel).unwrap();
    assert_eq!(run.ending, Ending::Abandoned(Rung::Interrupt));
    assert!(run.terminal_seen(), "the CLI answered before it left");
    assert_eq!(
        run.terminal_line()
            .map(|line| String::from_utf8_lossy(line).contains("num_turns")),
        Some(true),
        "the terminal line is where an adapter reads the usage"
    );
    assert!(
        run.usage_is_reported(),
        "the first rung was enough, so the CLI's own figures stand"
    );
    assert_eq!(run.code(), Some(1));
}

#[test]
fn a_cli_that_ignores_every_signal_is_killed_and_the_run_is_lost() {
    let dir = agent::Temp::new("stubborn");
    // Ignores SIGINT and SIGTERM, prints nothing more, never reports.
    let invocation = agent::sh(
        &format!("trap '' INT TERM; {INIT}; while :; do sleep 0.05; done"),
        dir.path(),
        agent::result_line,
    );
    let cancel = Arc::new(Cancel::default());
    agent::abandon_after(&cancel, 250);

    let run = process::run(&invocation, agent::bounds(10_000, 300), &cancel).unwrap();
    assert_eq!(
        run.ending,
        Ending::Abandoned(Rung::Kill),
        "interrupt, then SIGTERM, then SIGKILL"
    );
    assert!(!run.terminal_seen(), "nothing was reported");
    assert!(
        !run.usage_is_reported(),
        "unknown consumption is billed up, never down"
    );
}

#[test]
fn the_wall_bound_climbs_the_same_ladder() {
    let dir = agent::Temp::new("wall");
    let invocation = agent::sh(
        &format!("{INIT}; while :; do sleep 0.05; done"),
        dir.path(),
        agent::result_line,
    );
    // A child with default handlers dies on the first rung's SIGINT.
    let run = process::run(&invocation, agent::bounds(300, 500), &Cancel::default()).unwrap();
    assert_eq!(run.ending, Ending::WallLimit(Rung::Interrupt));
    assert!(!run.terminal_seen());
}

#[test]
fn a_run_that_ends_without_its_terminal_event_says_so() {
    let dir = agent::Temp::new("lost");
    let invocation = agent::sh(&format!("{INIT}; exit 143"), dir.path(), agent::result_line);
    let run = process::run(&invocation, agent::bounds(5_000, 200), &Cancel::default()).unwrap();
    assert_eq!(run.ending, Ending::Completed, "it ended by itself");
    assert!(!run.terminal_seen(), "#130 §5c: SIGTERM prints nothing");
    assert!(!run.usage_is_reported(), "billed at the ceiling");
    assert_eq!(run.code(), Some(143));
}

#[test]
fn the_transcript_is_bounded_and_the_terminal_line_is_always_kept() {
    let dir = agent::Temp::new("bound");
    let invocation = agent::sh(
        &format!(
            "i=0; while [ $i -lt 200 ]; do printf '{{\"n\":%s}}\\n' $i; i=$((i+1)); done; {RESULT}"
        ),
        dir.path(),
        agent::result_line,
    );
    let mut bounds = agent::bounds(5_000, 200);
    bounds.transcript_bytes = 100;

    let run = process::run(&invocation, bounds, &Cancel::default()).unwrap();
    assert!(
        run.truncated && run.dropped > 100,
        "dropped {}",
        run.dropped
    );
    assert!(run.lines.len() < 30, "kept {}", run.lines.len());
    assert!(
        run.terminal_seen(),
        "a transcript without the event that ends it would lose the usage \
         the bill is made of"
    );
    let transcript = run.transcript();
    assert_eq!(transcript[0]["n"], 0, "the first events, up to the bound");
    assert_eq!(transcript.last().unwrap()["subtype"], "success");
}

#[test]
fn every_signal_goes_to_the_group_so_no_grandchild_outlives_the_cli() {
    let dir = agent::Temp::new("group");
    let pidfile = dir.path().join("grandchild.pid");
    // The CLIs spawn shells, test runners and sub-agents; a `kill(pid)`
    // would orphan them. This child ignores every signal and leaves one
    // running behind it.
    let invocation = agent::sh(
        &format!(
            "trap '' INT TERM; sleep 30 & printf '%s' $! > {pid}; {INIT}; while :; do sleep 0.05; done",
            pid = pidfile.display()
        ),
        dir.path(),
        agent::result_line,
    );
    let cancel = Arc::new(Cancel::default());
    agent::abandon_after(&cancel, 250);
    let run = process::run(&invocation, agent::bounds(10_000, 300), &cancel).unwrap();
    assert_eq!(run.ending, Ending::Abandoned(Rung::Kill));

    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let target = nix::unistd::Pid::from_raw(pid);
    let gone = (0..100).any(|_| {
        if nix::sys::signal::kill(target, None).is_err() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
        false
    });
    assert!(gone, "the grandchild at {pid} outlived its group");
}

#[test]
fn a_binary_that_cannot_be_spawned_is_an_error_the_caller_reports() {
    let dir = agent::Temp::new("nospawn");
    let mut invocation = agent::sh("true", dir.path(), agent::never);
    invocation.program = dir.path().join("no-such-binary");
    let err = process::run(&invocation, agent::bounds(1_000, 100), &Cancel::default()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}
