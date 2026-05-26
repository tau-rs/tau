//! Append-only JSONL persistence for workflow runs.
//!
//! One line per step completion. The run log file lives at
//! `<scope>/.tau/workflow-runs/<workflow-name>-<run-id>.jsonl`.
//! Lines are fsync'd after each write so a crash mid-write loses at
//! most the trailing partial line; replay tolerates that.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One step's completion record, serialized as a single JSONL line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StepRecord {
    /// Log-line timestamp (record-emit time).
    pub ts: DateTime<Utc>,
    /// ULID of the run this record belongs to.
    pub run_id: String,
    /// Step id as declared in the workflow TOML.
    pub step_id: String,
    /// Zero-based index of the step in the workflow.
    pub step_index: usize,
    /// `"agent.run"` or `"tool.call"`.
    pub kind: String,
    /// Resolved input string passed to the step.
    pub input: String,
    /// Output text captured from the step.
    pub output: String,
    /// Wall-clock start of the step.
    pub started_at: DateTime<Utc>,
    /// Wall-clock end of the step.
    pub ended_at: DateTime<Utc>,
    /// Duration in milliseconds (`ended_at - started_at`).
    pub duration_ms: u64,
    /// `"ok"` or `"failed"`.
    pub status: StepStatus,
    /// On `status = "failed"`, an opaque error class for matching.
    /// `None` on `status = "ok"`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
    /// On `status = "failed"`, a human-readable detail line.
    /// `None` on `status = "ok"`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub detail: Option<String>,
}

/// Status of a step in a run log.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    /// Step completed successfully.
    Ok,
    /// Step terminated abnormally. Run aborted.
    Failed,
}

impl StepStatus {
    /// Lowercase wire form, matching the `serde(rename_all = "lowercase")`
    /// JSON serialization. Used when emitting the variant through
    /// `tracing::event!` so the on-disk JSONL produced by
    /// `WorkflowRunLogLayer` is byte-identical to the legacy direct
    /// writer in `RunLog::write_direct`.
    pub fn as_str_lowercase(&self) -> &'static str {
        match self {
            StepStatus::Ok => "ok",
            StepStatus::Failed => "failed",
        }
    }
}

/// Builds the canonical run-log path:
/// `<scope_root>/.tau/workflow-runs/<workflow_name>-<run_id>.jsonl`.
pub fn run_log_path(scope_root: &std::path::Path, workflow_name: &str, run_id: &str) -> PathBuf {
    scope_root
        .join(".tau")
        .join("workflow-runs")
        .join(format!("{workflow_name}-{run_id}.jsonl"))
}

use std::path::Path;

use tau_observe::layers::workflow_run_log::TARGET as WF_TARGET;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Append-only run log for a single workflow run.
///
/// Each `append` writes one JSONL line and fsyncs. On crash, the file
/// contains all complete lines plus possibly a truncated trailing line;
/// `replay` skips the trailing partial line.
pub struct RunLog {
    file: File,
    path: PathBuf,
}

impl RunLog {
    /// Open or create the run log for append. The parent directory is
    /// created if missing.
    pub async fn open_for_write(path: &Path) -> Result<Self, crate::WorkflowError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                crate::WorkflowError::PersistenceError {
                    path: path.to_path_buf(),
                    source: e,
                }
            })?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .map_err(|e| crate::WorkflowError::PersistenceError {
                path: path.to_path_buf(),
                source: e,
            })?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
        })
    }

    /// Append one record + fsync.
    pub async fn append(&mut self, record: &StepRecord) -> Result<(), crate::WorkflowError> {
        // Emit a structured tracing event. The `WorkflowRunLogLayer` (when
        // installed) materializes this into the on-disk JSONL line. Sub-
        // project D Task 3 removes the direct-write path below once the
        // layer is wired in `tau workflow run`.
        let error_field = record.error.as_deref().unwrap_or("");
        let detail_field = record.detail.as_deref().unwrap_or("");
        tracing::event!(
            target: WF_TARGET,
            tracing::Level::INFO,
            ts = %record.ts,
            run_id = %record.run_id,
            step_id = %record.step_id,
            step_index = record.step_index as u64,
            kind = %record.kind,
            input = %record.input,
            output = %record.output,
            started_at = %record.started_at,
            ended_at = %record.ended_at,
            duration_ms = record.duration_ms,
            status = %record.status.as_str_lowercase(),
            error = %error_field,
            detail = %detail_field,
        );

        // Existing direct-write path — preserved so RunLog still produces
        // its JSONL output when no WorkflowRunLogLayer is installed (e.g.
        // existing unit tests in this file). Task 3 deletes this path.
        self.write_direct(record).await
    }

    async fn write_direct(&mut self, record: &StepRecord) -> Result<(), crate::WorkflowError> {
        let mut line =
            serde_json::to_string(record).map_err(|e| crate::WorkflowError::PersistenceError {
                path: self.path.clone(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
            })?;
        line.push('\n');
        self.file.write_all(line.as_bytes()).await.map_err(|e| {
            crate::WorkflowError::PersistenceError {
                path: self.path.clone(),
                source: e,
            }
        })?;
        self.file
            .sync_data()
            .await
            .map_err(|e| crate::WorkflowError::PersistenceError {
                path: self.path.clone(),
                source: e,
            })?;
        Ok(())
    }
}

/// Replay a JSONL log into a vector of records. Tolerates a trailing
/// partial line (truncated mid-write on crash) by skipping it.
pub async fn replay(path: &Path) -> Result<Vec<StepRecord>, crate::WorkflowError> {
    let file = File::open(path)
        .await
        .map_err(|e| crate::WorkflowError::PersistenceError {
            path: path.to_path_buf(),
            source: e,
        })?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();
    let mut records = Vec::new();
    while let Some(line) =
        lines
            .next_line()
            .await
            .map_err(|e| crate::WorkflowError::PersistenceError {
                path: path.to_path_buf(),
                source: e,
            })?
    {
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<StepRecord>(&line) {
            Ok(r) => records.push(r),
            Err(_) => {
                // Truncated/corrupt trailing line. Skip silently — the
                // contract is "tolerate the trailing partial". We do NOT
                // continue past a corrupt line in the middle of a file;
                // but BufReader returns lines split by `\n`, so a missing
                // `\n` only affects the final line.
                break;
            }
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_record(idx: usize, id: &str) -> StepRecord {
        let now = Utc::now();
        StepRecord {
            ts: now,
            run_id: "01HKZTEST".into(),
            step_id: id.into(),
            step_index: idx,
            kind: "agent.run".into(),
            input: format!("input-{idx}"),
            output: format!("output-{idx}"),
            started_at: now,
            ended_at: now,
            duration_ms: 1,
            status: StepStatus::Ok,
            error: None,
            detail: None,
        }
    }

    #[tokio::test]
    async fn append_then_replay_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.jsonl");
        {
            let mut log = RunLog::open_for_write(&path).await.unwrap();
            log.append(&make_record(0, "a")).await.unwrap();
            log.append(&make_record(1, "b")).await.unwrap();
        }
        let records = replay(&path).await.unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].step_id, "a");
        assert_eq!(records[1].step_id, "b");
    }

    #[tokio::test]
    async fn replay_tolerates_trailing_partial_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.jsonl");
        {
            let mut log = RunLog::open_for_write(&path).await.unwrap();
            log.append(&make_record(0, "a")).await.unwrap();
        }
        // Append 30 bytes of garbage WITHOUT a trailing newline.
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .unwrap();
        f.write_all(b"{\"step_id\":\"trunc").await.unwrap();
        f.sync_data().await.unwrap();
        drop(f);

        let records = replay(&path).await.unwrap();
        assert_eq!(records.len(), 1, "trailing partial line should be dropped");
        assert_eq!(records[0].step_id, "a");
    }

    #[tokio::test]
    async fn replay_empty_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.jsonl");
        tokio::fs::write(&path, b"").await.unwrap();
        let records = replay(&path).await.unwrap();
        assert!(records.is_empty());
    }
}
