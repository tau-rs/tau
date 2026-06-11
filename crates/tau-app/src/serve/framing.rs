//! NDJSON framing for stdin/stdout. One JSON value per line.

use super::idle::Activity;
use super::protocol::Outbound;
use anyhow::{Context, Result};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tracing::warn;

/// Outcome of reading one line from stdin.
#[derive(Debug)]
pub enum Inbound {
    /// Parsed JSON value (validity beyond JSON is the dispatcher's job).
    Json(Value),
    /// Malformed JSON. Includes the original line bytes for logging.
    ParseError(String),
    /// EOF — stdin closed.
    Eof,
}

/// Reader task: read NDJSON lines from stdin, push to channel.
/// Returns when stdin EOF is reached (after sending `Inbound::Eof`).
///
/// `activity` is touched on every non-empty inbound line so the idle
/// timeout (see [`super::idle`]) resets on incoming traffic.
pub async fn reader_task(tx: mpsc::Sender<Inbound>, activity: Activity) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .await
            .context("stdin read failed")?;
        if n == 0 {
            let _ = tx.send(Inbound::Eof).await;
            return Ok(());
        }
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed.is_empty() {
            continue;
        }
        activity.touch();
        let msg = match serde_json::from_str::<Value>(trimmed) {
            Ok(v) => Inbound::Json(v),
            Err(e) => Inbound::ParseError(format!("{}: {}", e, trimmed)),
        };
        if tx.send(msg).await.is_err() {
            return Ok(()); // dispatcher dropped — shutdown
        }
    }
}

/// Writer task: receive `Outbound`s from a channel, serialize as
/// NDJSON to stdout, one line per message.
///
/// stdout is locked once per write to guarantee atomic line writes
/// (concurrent dispatcher tasks send through `mpsc`, but the actual
/// stdout `write_all` happens here single-threaded).
///
/// `activity` is touched on every emitted message so the idle timeout
/// (see [`super::idle`]) resets on outgoing traffic — an in-flight run
/// that keeps streaming output holds the timer open.
pub async fn writer_task(mut rx: mpsc::Receiver<Outbound>, activity: Activity) -> Result<()> {
    let mut stdout = tokio::io::stdout();
    while let Some(out) = rx.recv().await {
        activity.touch();
        let mut line = serde_json::to_string(&out).context("serialize outbound message")?;
        line.push('\n');
        if let Err(e) = stdout.write_all(line.as_bytes()).await {
            warn!(error = %e, "stdout write failed; writer task exiting");
            return Err(e).context("stdout write failed");
        }
        if let Err(e) = stdout.flush().await {
            warn!(error = %e, "stdout flush failed; writer task exiting");
            return Err(e).context("stdout flush failed");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serve::protocol::{RequestId, Response};
    use serde_json::json;

    #[tokio::test]
    async fn writer_emits_ndjson() {
        let (tx, rx) = mpsc::channel(16);
        tx.send(Outbound::Response(Response {
            jsonrpc: "2.0".into(),
            id: RequestId::Int(1),
            result: json!({"ok": true}),
        }))
        .await
        .unwrap();
        drop(tx); // close so writer exits

        // Smoke check: writer task completes without deadlock within timeout.
        let res = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            writer_task(rx, Activity::new()),
        )
        .await;
        assert!(res.is_ok());
    }
}
