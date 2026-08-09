//! Layer 3 e2e — handshake smoke test over a real pipe.
//!
//! Spawns the real `tau` binary and performs a full `meta.handshake`
//! roundtrip over its stdio using the NDJSON framing that serve mode
//! defines. Also exercises `meta.ping` before and after handshake.
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

fn recv_line(
    reader: &mut BufReader<std::process::ChildStdout>,
    child: &mut std::process::Child,
) -> Value {
    let mut line = String::new();
    let n = reader
        .read_line(&mut line)
        .expect("read from tau serve stdout");
    if n == 0 || line.trim().is_empty() {
        panic!(
            "tau serve produced no stdout line (child likely exited during startup).{}",
            e2e_common::drain_child_stderr(child)
        );
    }
    serde_json::from_str(line.trim()).expect("parse JSON-RPC response")
}

/// `meta.ping` works before the handshake.
#[test]
fn ping_before_handshake() {
    e2e_common::ensure_home_env();
    let (mut child, mut stdin, mut reader) = spawn_serve();

    send_line(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":1,"method":"meta.ping"}"#,
    );
    let resp = recv_line(&mut reader, &mut child);

    assert_eq!(resp["id"], 1, "unexpected response: {resp}");
    assert_eq!(
        resp["result"]["ok"], true,
        "ping returned unexpected body: {resp}"
    );

    drop(stdin);
    let _ = child.wait();
}

/// Full `meta.handshake` roundtrip: request → response with matching protocol_version.
#[test]
fn handshake_response_over_real_pipe() {
    e2e_common::ensure_home_env();
    let (mut child, mut stdin, mut reader) = spawn_serve();

    send_line(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":2,"method":"meta.handshake","params":{"client_name":"e2e-test","client_version":"0.1.0","protocol_version":1}}"#,
    );
    let resp = recv_line(&mut reader, &mut child);

    assert_eq!(resp["id"], 2, "unexpected response: {resp}");
    assert_eq!(
        resp["result"]["protocol_version"], 1,
        "unexpected protocol_version in response: {resp}"
    );
    assert_eq!(
        resp["result"]["server_name"], "tau",
        "unexpected server_name in response: {resp}"
    );
    // agents field should be an array (empty for this fixture).
    assert!(
        resp["result"]["agents"].is_array(),
        "expected agents array in handshake response: {resp}"
    );

    drop(stdin);
    let _ = child.wait();
}

/// After a successful handshake, `meta.ping` still works.
#[test]
fn ping_after_handshake() {
    e2e_common::ensure_home_env();
    let (mut child, mut stdin, mut reader) = spawn_serve();

    // Handshake first.
    send_line(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":10,"method":"meta.handshake","params":{"protocol_version":1}}"#,
    );
    let _ = recv_line(&mut reader, &mut child);

    // Now ping.
    send_line(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":11,"method":"meta.ping"}"#,
    );
    let resp = recv_line(&mut reader, &mut child);

    assert_eq!(resp["id"], 11, "unexpected response: {resp}");
    assert_eq!(
        resp["result"]["ok"], true,
        "ping returned unexpected body: {resp}"
    );

    drop(stdin);
    let _ = child.wait();
}
