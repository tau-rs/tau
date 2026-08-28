//! Layer that materializes `target = "tau::workflow::step"` events
//! into the `<scope>/.tau/workflow-runs/<workflow>-<run-id>.jsonl`
//! file format previously written by `tau_workflow::persistence::RunLog`.
//!
//! Field schema (must match `StepRecord` in `tau-workflow`):
//! - `ts`, `run_id`, `step_id`, `step_index`, `kind`, `input`, `output`,
//!   `started_at`, `ended_at`, `duration_ms`, `status`
//! - optional: `error`, `detail`
//!
//! The layer writes one line per event and `sync_data`s after each write.
//!
//! ## Why the write is synchronous
//!
//! `on_event` writes the line with blocking `std::fs` I/O rather than
//! handing it to a `tokio::spawn`ed task (tau-rs/tau#650). The detached
//! task never ran to completion: `tau workflow run` returns immediately
//! after the last step, the runtime is dropped, and the task is dropped
//! at its first `.await` — leaving a missing or zero-byte run log on
//! *every* invocation, which in turn made `tau workflow resume` replay
//! nothing and silently re-run already-completed steps.
//!
//! Writing inline instead of adding a `flush()` handshake (the
//! `PluginRecordingLayer` approach) is deliberate:
//!
//! - **Correct for every caller.** A `flush()` only works if the caller
//!   remembers to call it. That is the exact contract #650 broke, and a
//!   layer that silently discards records unless a specific command
//!   handler opts in is a footgun.
//! - **Ordering.** Concurrently-spawned write tasks race on the mutex,
//!   so lines could land out of step order. `tracing` dispatches events
//!   in program order, so an inline write is append-ordered by
//!   construction.
//! - **Works off-runtime.** A bare `tokio::spawn` panics when no reactor
//!   is running, making the layer unusable outside a tokio context.
//! - **The cost is negligible here.** One short line per workflow *step*,
//!   where a step is an LLM call or tool invocation taking orders of
//!   magnitude longer than the write. This is the same shape as
//!   `tracing_subscriber`'s own fmt layer, which writes to its sink
//!   inline. `PluginRecordingLayer` keeps its async path because it sees
//!   one event per protocol frame, a far hotter target.
//!
//! ## String field handling
//!
//! `tracing` dispatches `&str` field values through `Visit::record_str`,
//! and Display/Debug formatted fields (`%x`, `?x`) through
//! `Visit::record_debug`. We implement both so that string-typed fields
//! land in `serde_json::Value::String` *without* `Debug`-quoting — the
//! byte-identical-output guarantee with the legacy direct writer
//! (`tau_workflow::persistence::RunLog`) depends on it. The naive
//! "store `format!("{value:?}")` for everything" approach in the
//! original plan would wrap strings in `"..."` quotes, doubling them
//! after `serde_json` re-quotes the value on serialization.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::{field::Visit, Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// The `target` string that emissions must use to be picked up by this
/// layer. Producers should emit with `tracing::event!(target: TARGET, …)`.
pub const TARGET: &str = "tau::workflow::step";

/// Layer that appends each matching event to a JSONL file.
///
/// ```
/// use std::path::PathBuf;
/// use tau_observe::layers::workflow_run_log::WorkflowRunLogLayer;
///
/// // Construct a layer targeting a workflow run-log file.
/// let path = PathBuf::from("/tmp/tau-workflow-run.jsonl");
/// let layer = WorkflowRunLogLayer::new(path);
/// // Clone is cheap (Arc-backed).
/// let _layer2 = layer.clone();
/// // TARGET must match the expected string to route events correctly.
/// assert_eq!(tau_observe::layers::workflow_run_log::TARGET, "tau::workflow::step");
/// ```
#[derive(Clone)]
pub struct WorkflowRunLogLayer {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    path: PathBuf,
    file: Option<std::fs::File>,
}

impl Inner {
    /// Append one already-serialized JSONL line, opening (and creating)
    /// the file plus its parent directory on first use.
    fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        if self.file.is_none() {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            self.file = Some(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?,
            );
        }
        let file = self
            .file
            .as_mut()
            .expect("file is Some after the open branch above");
        file.write_all(line.as_bytes())?;
        // `sync_data` (fdatasync on Linux) flushes the record without
        // forcing a metadata sync on every line. A run log that is only
        // in the page cache is worthless to `tau workflow resume` after
        // a crash, which is the log's whole reason to exist.
        file.sync_data()
    }
}

impl WorkflowRunLogLayer {
    /// Construct a layer that will append matching events to `path`.
    ///
    /// The file is not opened until the first matching event arrives,
    /// to keep the layer cheap when no workflow run is in progress.
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use tau_observe::layers::workflow_run_log::WorkflowRunLogLayer;
    ///
    /// let path = PathBuf::from("/tmp/run-log.jsonl");
    /// let layer = WorkflowRunLogLayer::new(path);
    /// // Layer is Clone — cheap Arc-backed copy.
    /// let _layer2 = layer.clone();
    /// ```
    pub fn new(path: PathBuf) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner { path, file: None })),
        }
    }
}

impl<S> Layer<S> for WorkflowRunLogLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        if meta.target() != TARGET {
            return;
        }
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        let line = serialize_step_record(&visitor.fields);
        // Write inline — see the module docstring for why this is not
        // handed to a background task. A poisoned mutex means a previous
        // writer panicked mid-line; recover the guard rather than
        // disabling the run log for the rest of the process.
        let failure = {
            let mut guard = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard
                .write_line(&line)
                .err()
                .map(|e| (guard.path.clone(), e))
        };
        // Best-effort: report failures at WARN on a *different* target so
        // they don't recursively re-enter this layer's filter. Emitted
        // after the guard is dropped so the nested dispatch can never
        // re-enter the (non-reentrant) mutex.
        if let Some((path, e)) = failure {
            tracing::warn!(
                target: "tau_observe::layers::workflow_run_log",
                path = %path.display(),
                err = %e,
                "workflow run-log write failed; dropping event",
            );
        }
    }
}

#[derive(Default)]
struct FieldVisitor {
    fields: BTreeMap<String, serde_json::Value>,
}

impl Visit for FieldVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.fields.insert(
            field.name().to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }
    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields.insert(
            field.name().to_string(),
            serde_json::Value::Number(value.into()),
        );
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields.insert(
            field.name().to_string(),
            serde_json::Value::Number(value.into()),
        );
    }
    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), serde_json::Value::Bool(value));
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        // Catches Display-formatted fields (`%x`) via tracing's
        // `DisplayValue::fmt(Debug)` which defers to Display, and any
        // Debug-formatted fields (`?x`) like enums. Result is stored
        // as a JSON string; the `format!("{value:?}")` here is *not*
        // wrapped in extra quotes because `DisplayValue`'s Debug impl
        // emits the bare Display form. For raw `&str` fields tracing
        // dispatches to `record_str` above instead, so this path never
        // double-quotes string literals.
        self.fields.insert(
            field.name().to_string(),
            serde_json::Value::String(format!("{value:?}")),
        );
    }
}

/// Build the JSONL line in the canonical `StepRecord` field order
/// documented in `tau_workflow::persistence::StepRecord`. Missing
/// optional fields (`error`, `detail`) are omitted, matching
/// `#[serde(skip_serializing_if = "Option::is_none")]`.
fn serialize_step_record(fields: &BTreeMap<String, serde_json::Value>) -> String {
    let mut obj = serde_json::Map::new();
    for key in [
        "ts",
        "run_id",
        "step_id",
        "step_index",
        "kind",
        "input",
        "output",
        "started_at",
        "ended_at",
        "duration_ms",
        "status",
    ] {
        if let Some(v) = fields.get(key) {
            obj.insert(key.to_string(), v.clone());
        }
    }
    for key in ["error", "detail"] {
        if let Some(v) = fields.get(key) {
            obj.insert(key.to_string(), v.clone());
        }
    }
    let mut line = serde_json::Value::Object(obj).to_string();
    line.push('\n');
    line
}
