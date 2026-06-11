//! Layer 2 — graceful shutdown tests.
//!
//! Tests that the dispatcher loop terminates cleanly:
//!   - Sending `Inbound::Eof` causes the dispatcher loop to exit.
//!   - Dropping the sender (`in_tx`) causes the dispatcher loop to exit
//!     (channel close triggers `in_rx.recv()` → None).
//!
//! Both paths are equivalent to "stdin closed" in the production
//! `lifecycle::run` flow. The test verifies that the dispatcher thread
//! finishes without panicking.
//!
//! Ownership note: destructuring the Harness into separate bindings before
//! moving the `dispatcher_thread` into `spawn_blocking` avoids partial-move
//! compiler errors.

mod common;
use common::Harness;
use std::path::PathBuf;
use tau_app::serve::Inbound;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/handshake-only")
}

/// Sending `Inbound::Eof` causes the dispatcher `run` loop to break and the
/// dispatcher thread to exit cleanly.
#[tokio::test]
async fn eof_triggers_clean_shutdown() {
    let Harness {
        in_tx,
        out_rx: _out,
        cancel_reg: _cr,
        dispatcher_thread: thread,
        ..
    } = Harness::new(fixture_dir()).await;

    // Send EOF signal through the input channel.
    let _ = in_tx.send(Inbound::Eof).await;

    // Drop the sender so the channel is closed after Eof.
    drop(in_tx);

    // Wait for the dispatcher thread to finish.
    // `JoinHandle::join` is blocking; run it on a blocking thread so the
    // tokio executor isn't stalled.
    let join_result = tokio::task::spawn_blocking(move || thread.join())
        .await
        .expect("spawn_blocking panicked");

    assert!(
        join_result.is_ok(),
        "dispatcher thread panicked during shutdown"
    );
}

/// Dropping `in_tx` (closing the mpsc channel) without sending Eof also
/// causes `in_rx.recv()` to return `None`, which breaks the dispatcher loop.
#[tokio::test]
async fn channel_close_triggers_shutdown() {
    let Harness {
        in_tx,
        out_rx: _out,
        cancel_reg: _cr,
        dispatcher_thread: thread,
        ..
    } = Harness::new(fixture_dir()).await;

    // Drop the sender immediately — no messages sent at all.
    drop(in_tx);

    let join_result = tokio::task::spawn_blocking(move || thread.join())
        .await
        .expect("spawn_blocking panicked");

    assert!(
        join_result.is_ok(),
        "dispatcher thread panicked on channel close"
    );
}

/// Confirm that after a successful handshake followed by EOF, the dispatcher
/// still exits cleanly (no in-flight work to drain).
#[tokio::test]
async fn shutdown_after_handshake() {
    let mut h = Harness::new(fixture_dir()).await;
    h.handshake().await;

    // Signal shutdown via Eof then channel close.
    let _ = h.in_tx.send(Inbound::Eof).await;

    let Harness {
        in_tx,
        out_rx: _out,
        cancel_reg: _cr,
        dispatcher_thread: thread,
        ..
    } = h;
    drop(in_tx);

    let join_result = tokio::task::spawn_blocking(move || thread.join())
        .await
        .expect("spawn_blocking panicked");

    assert!(
        join_result.is_ok(),
        "dispatcher thread panicked after handshake+shutdown"
    );
}
