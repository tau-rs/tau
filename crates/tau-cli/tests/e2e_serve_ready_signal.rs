//! Layer 3 e2e — `--ready-on-stderr` signal test.
//!
//! Spawns the real `tau` binary and verifies that `tau serve
//! --ready-on-stderr` writes `tau-serve ready` to stderr before any
//! protocol output appears on stdout. No JSON-RPC traffic is needed.
//!
//! Placed in `crates/tau-cli/tests/` (Option A) so that
//! `CARGO_BIN_EXE_tau` is populated by Cargo's integration-test
//! machinery, which only works when the test resides in the same crate
//! as the binary target (`[[bin]] name = "tau"`).

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[path = "e2e_common.rs"]
mod e2e_common;

fn tau_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tau"))
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/e2e-handshake-only")
}

#[test]
fn ready_on_stderr_marker() {
    e2e_common::ensure_home_env();
    let bin = tau_bin();
    let fixture = fixture_dir();

    let mut child = Command::new(&bin)
        .args(["serve", "--ready-on-stderr", "--project"])
        .arg(&fixture)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn tau serve");

    let stderr = child.stderr.take().unwrap();
    let reader = BufReader::new(stderr);
    let mut saw_ready = false;
    // Retain the stderr lines we read so a failure shows *why* serve died
    // (e.g. a sandbox-adapter resolution error) instead of a bare miss.
    let mut collected: Vec<String> = Vec::new();

    // Read up to 50 lines from stderr — the ready signal should appear
    // very early (before any protocol traffic). Use map_while to bail
    // on first read error rather than looping on a repeated Err
    // (clippy::lines_filter_map_ok).
    for line in reader.lines().map_while(Result::ok).take(50) {
        if line.contains("tau-serve ready") {
            saw_ready = true;
            break;
        }
        collected.push(line);
    }

    assert!(
        saw_ready,
        "did not observe 'tau-serve ready' on stderr within first 50 lines.\n\
         --- tau serve child stderr ---\n{}\n--- end child stderr ---",
        collected.join("\n")
    );

    // Close stdin → child exits cleanly (EOF triggers shutdown).
    drop(child.stdin.take());
    let _ = child.wait();
}
