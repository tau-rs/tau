//! JSONL run-log writer subscribed to the TraceStream.
//!
//! Writes `<scope>/.tau/runs/<run-id>.jsonl`. Each line is either a
//! TraceEvent or a TaskMutation projection (for replay-ability).
//! Crash-safe: fsync after each line; replay tolerates trailing partial.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::UnboundedReceiver;

use tau_ports::{RunId, TraceEvent};

use crate::orchestration::error::OrchestrationError;

/// Build the JSONL path: `<scope_root>/.tau/runs/<run_id>.jsonl`.
///
/// # Example
///
/// ```
/// use std::path::Path;
/// use tau_runtime_tokio::orchestration::persistence::run_log_path;
///
/// let path = run_log_path(Path::new("/workspace"), &"run-abc".into());
/// assert!(path.ends_with("run-abc.jsonl"));
/// assert!(path.to_str().unwrap().contains(".tau/runs"));
/// ```
pub fn run_log_path(scope_root: &Path, run_id: &RunId) -> PathBuf {
    scope_root
        .join(".tau")
        .join("runs")
        .join(format!("{run_id}.jsonl"))
}

/// Wrapped line shape for the JSONL — tagged union for forward-compat.
///
/// # Example
///
/// ```
/// use tau_runtime_tokio::orchestration::persistence::RunLogLine;
///
/// // TaskMutation is a forward-compat placeholder; ensure it
/// // round-trips through JSON.
/// let line = RunLogLine::TaskMutation {
///     task_id: "01".into(),
///     mutation: r#"{"status":"done"}"#.into(),
/// };
/// let json = serde_json::to_string(&line).expect("serialize RunLogLine");
/// assert!(json.contains("task_mutation"));
/// assert!(json.contains("01"));
/// ```
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "line_kind", rename_all = "snake_case")]
pub enum RunLogLine {
    /// One trace event.
    TraceEvent {
        /// The trace event.
        event: TraceEvent,
    },
    /// Reserved for future use (task-mutation projections).
    TaskMutation {
        /// Task id.
        task_id: String,
        /// Serialized mutation payload.
        mutation: String,
    },
}

/// Spawn a tokio task that drains `rx` and writes each event to the
/// JSONL. Returns immediately; the task runs until `rx` is closed.
///
/// Errors during I/O are logged via `tracing::warn!` and the task
/// continues — partial logs are better than no logs.
pub fn spawn_writer(
    path: PathBuf,
    mut rx: UnboundedReceiver<TraceEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Some(parent) = path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                tracing::warn!("orchestration persistence: mkdir {parent:?}: {e}");
                return;
            }
        }
        let mut file = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
        {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("orchestration persistence: open {path:?}: {e}");
                return;
            }
        };

        while let Some(event) = rx.recv().await {
            let line = RunLogLine::TraceEvent { event };
            let mut json = match serde_json::to_string(&line) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("orchestration persistence: serialize: {e}");
                    continue;
                }
            };
            json.push('\n');
            if let Err(e) = file.write_all(json.as_bytes()).await {
                tracing::warn!("orchestration persistence: write {path:?}: {e}");
                continue;
            }
            if let Err(e) = file.sync_data().await {
                tracing::warn!("orchestration persistence: fsync {path:?}: {e}");
            }
        }
    })
}

/// Replay a run-log JSONL into a vector of trace events. Tolerates a
/// trailing partial line (crash safety).
pub async fn replay(path: &Path) -> Result<Vec<TraceEvent>, OrchestrationError> {
    let file = File::open(path).await?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();
    let mut events = Vec::new();
    while let Some(line) = lines.next_line().await? {
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<RunLogLine>(&line) {
            Ok(RunLogLine::TraceEvent { event }) => events.push(event),
            Ok(RunLogLine::TaskMutation { .. }) => {} // reserved
            Err(_) => break,                          // truncated trailing line; stop here
        }
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tau_ports::TraceEventKind;
    use tokio::sync::mpsc;

    fn make_event(id: &str) -> TraceEvent {
        TraceEvent {
            id: id.into(),
            ts: Utc::now(),
            run_id: "r".into(),
            agent_id: Some("a".into()),
            kind: TraceEventKind::Turn {
                agent_id: "a".into(),
                turn_index: 0,
                duration_ms: 1,
            },
        }
    }

    #[tokio::test]
    async fn writer_and_replay_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.jsonl");
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = spawn_writer(path.clone(), rx);
        tx.send(make_event("e1")).unwrap();
        tx.send(make_event("e2")).unwrap();
        drop(tx);
        handle.await.unwrap();

        let events = replay(&path).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, "e1");
        assert_eq!(events[1].id, "e2");
    }

    #[tokio::test]
    async fn replay_tolerates_trailing_partial() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.jsonl");
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = spawn_writer(path.clone(), rx);
        tx.send(make_event("e1")).unwrap();
        drop(tx);
        handle.await.unwrap();

        // Append garbage without newline.
        let mut f = OpenOptions::new().append(true).open(&path).await.unwrap();
        f.write_all(b"{\"line_kind\":\"truncated").await.unwrap();
        f.sync_data().await.unwrap();
        drop(f);

        let events = replay(&path).await.unwrap();
        assert_eq!(events.len(), 1);
    }
}
