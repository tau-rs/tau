//! Layer 3 e2e — `runtime.run_streaming` code-path exercise.
//!
//! Full streaming with real TextDelta/TurnCompleted events requires an
//! installed LLM-backend fixture that is not in scope for serve mode
//! v1. Instead, this test exercises the streaming *dispatch path* by
//! sending a `runtime.run_streaming` call for an unknown agent and
//! asserting that the serve process returns the correct JSON-RPC error
//! code `-32010 UNKNOWN_AGENT` (not a crash, not a hang, not silence).
//!
//! Real streaming events are covered in the Layer 2 in-memory tests
//! (`crates/tau-app/tests/serve_run_streaming.rs`) and by manual smoke
//! testing against a project with an installed echo-llm backend.
//!
//! Placed in `crates/tau-cli/tests/` (Option A) so that
//! `CARGO_BIN_EXE_tau` is populated by Cargo's integration-test
//! machinery.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::Value;

#[path = "e2e_common.rs"]
mod e2e_common;

fn tau_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tau"))
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/e2e-handshake-only")
}

fn spawn_serve() -> (
    std::process::Child,
    std::process::ChildStdin,
    BufReader<std::process::ChildStdout>,
) {
    let mut child = Command::new(tau_bin())
        .args(["serve", "--project"])
        .arg(fixture_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn tau serve");

    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    (child, stdin, stdout)
}

fn send_line(stdin: &mut std::process::ChildStdin, line: &str) {
    writeln!(stdin, "{}", line).expect("write to tau serve stdin");
}

fn recv_line(reader: &mut BufReader<std::process::ChildStdout>) -> Value {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("read from tau serve stdout");
    serde_json::from_str(line.trim()).expect("parse JSON-RPC response")
}

/// `runtime.run_streaming` for an unknown agent returns -32010 UNKNOWN_AGENT.
///
/// This ensures the streaming dispatch path is wired end-to-end and
/// that unknown-agent errors propagate correctly from the real serve
/// process (not just the in-memory harness used in Layer 2 tests).
#[cfg_attr(
    windows,
    ignore = "tau serve child can't resolve scope on Windows (no home fallback); see #530"
)]
#[test]
fn unknown_agent_in_streaming_run_returns_32010() {
    e2e_common::ensure_home_env();
    let (mut child, mut stdin, mut reader) = spawn_serve();

    // Handshake first — streaming calls require a completed handshake.
    send_line(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":1,"method":"meta.handshake","params":{"protocol_version":1}}"#,
    );
    let handshake_resp = recv_line(&mut reader);
    assert_eq!(
        handshake_resp["result"]["protocol_version"], 1,
        "handshake failed: {handshake_resp}"
    );

    // Send runtime.run_streaming for an agent that doesn't exist.
    send_line(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":2,"method":"runtime.run_streaming","params":{"agent":"no-such-agent","prompt":"hello"}}"#,
    );
    let resp = recv_line(&mut reader);

    assert_eq!(resp["id"], 2, "unexpected response id: {resp}");
    assert_eq!(
        resp["error"]["code"], -32010,
        "expected -32010 UNKNOWN_AGENT, got: {resp}"
    );

    drop(stdin);
    let _ = child.wait();
}

/// `runtime.run` (non-streaming) for an unknown agent also returns -32010.
///
/// Exercises the batch run code path through the real binary, mirroring
/// the streaming test above.
#[cfg_attr(
    windows,
    ignore = "tau serve child can't resolve scope on Windows (no home fallback); see #530"
)]
#[test]
fn unknown_agent_in_batch_run_returns_32010() {
    e2e_common::ensure_home_env();
    let (mut child, mut stdin, mut reader) = spawn_serve();

    send_line(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":3,"method":"meta.handshake","params":{"protocol_version":1}}"#,
    );
    let _ = recv_line(&mut reader);

    send_line(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":4,"method":"runtime.run","params":{"agent":"no-such-agent","prompt":"hello"}}"#,
    );
    let resp = recv_line(&mut reader);

    assert_eq!(resp["id"], 4, "unexpected response id: {resp}");
    assert_eq!(
        resp["error"]["code"], -32010,
        "expected -32010 UNKNOWN_AGENT, got: {resp}"
    );

    drop(stdin);
    let _ = child.wait();
}
