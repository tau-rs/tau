//! Layer 2 — O4: a dropped writer is observed, logged, and trips shutdown.

mod common;
use common::Harness;
use std::path::PathBuf;
use std::time::Duration;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/handshake-only")
}

/// When the writer task is gone, the next outbound send trips the
/// `writer_gone` shutdown token (and logs once). Run with `--nocapture`
/// to see the "writer task gone" warning.
#[tokio::test]
async fn writer_gone_is_logged_and_trips_shutdown() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::WARN)
        .try_init();

    let mut h = Harness::new(fixture_dir()).await;
    h.handshake().await; // drains the handshake response

    // Simulate the writer task dying: drop the out-channel receiver.
    h.kill_writer();

    // meta.ping → send_ok → send fails → note_writer_gone trips the token.
    h.send_raw(r#"{"jsonrpc":"2.0","id":7,"method":"meta.ping"}"#)
        .await;

    tokio::time::timeout(Duration::from_secs(2), h.writer_gone.cancelled())
        .await
        .expect("writer_gone token must trip after a send to a dead writer");
}
