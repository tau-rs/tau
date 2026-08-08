//! Layer 3 e2e — child exits when stdin is closed (parent-death proxy).
//!
//! Verifies that `tau serve` shuts itself down within a reasonable
//! timeout after its stdin pipe is closed (simulating the parent
//! process exiting). The framing reader detects stdin EOF and sends
//! `Inbound::Eof`, which propagates through the dispatcher to a clean
//! shutdown.
//!
//! Placed in `crates/tau-cli/tests/` (Option A) so that
//! `CARGO_BIN_EXE_tau` is populated by Cargo's integration-test
//! machinery.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[path = "e2e_common.rs"]
mod e2e_common;

fn tau_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tau"))
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/e2e-handshake-only")
}

/// Closing stdin should cause the serve child to exit within 10 s.
///
/// Uses `--ready-on-stderr` to synchronize: we wait for the ready
/// marker before closing stdin, so we know the process is fully up
/// and listening (not still starting up) when we trigger shutdown.
#[cfg_attr(
    windows,
    ignore = "tau serve child can't resolve scope on Windows (no home fallback); see #530"
)]
#[test]
fn child_exits_on_stdin_eof() {
    e2e_common::ensure_home_env();
    let mut child = Command::new(tau_bin())
        .args(["serve", "--ready-on-stderr", "--project"])
        .arg(fixture_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn tau serve");

    // Wait for ready signal so the process is fully up.
    {
        use std::io::BufRead;
        let stderr = child.stderr.take().unwrap();
        let reader = std::io::BufReader::new(stderr);
        let mut ready = false;
        for line in reader.lines().map_while(Result::ok).take(50) {
            if line.contains("tau-serve ready") {
                ready = true;
                break;
            }
        }
        assert!(ready, "tau serve never printed 'tau-serve ready' on stderr");
        // stderr BufReader + ChildStderr are dropped here, closing the
        // read end. This is fine — we only need to know startup completed.
    }

    // Now close stdin → EOF propagates to the reader task → Inbound::Eof
    // → dispatcher shuts down → process exits.
    drop(child.stdin.take());

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait().expect("try_wait") {
            Some(_status) => return, // success — child exited
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                panic!("tau serve still alive 10 s after stdin EOF");
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}
