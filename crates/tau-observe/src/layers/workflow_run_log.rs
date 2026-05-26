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
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{field::Visit, Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// The `target` string that emissions must use to be picked up by this
/// layer. Producers should emit with `tracing::event!(target: TARGET, …)`.
pub const TARGET: &str = "tau::workflow::step";

/// Layer that appends each matching event to a JSONL file.
#[derive(Clone)]
pub struct WorkflowRunLogLayer {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    path: PathBuf,
    file: Option<tokio::fs::File>,
}

impl WorkflowRunLogLayer {
    /// Construct a layer that will append matching events to `path`.
    ///
    /// The file is not opened until the first matching event arrives,
    /// to keep the layer cheap when no workflow run is in progress.
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
        let inner = self.inner.clone();
        // Hand off the write to the runtime so we don't block the
        // emitting task. Best-effort: errors are logged at WARN to a
        // *different* target so they don't recursively re-enter this
        // layer's filter.
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt as _;
            let mut guard = inner.lock().await;
            let path = guard.path.clone();
            if guard.file.is_none() {
                if let Some(parent) = path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                match tokio::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .await
                {
                    Ok(f) => {
                        guard.file = Some(f);
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: "tau_observe::layers::workflow_run_log",
                            path = %path.display(),
                            err = %e,
                            "workflow run-log open failed; dropping event",
                        );
                        return;
                    }
                }
            }
            let file = guard
                .file
                .as_mut()
                .expect("file is Some after the open branch above");
            if let Err(e) = file.write_all(line.as_bytes()).await {
                tracing::warn!(
                    target: "tau_observe::layers::workflow_run_log",
                    err = %e,
                    "workflow run-log write failed",
                );
                return;
            }
            // `sync_data` (fdatasync on Linux) matches the legacy
            // `RunLog` writer — flush data without forcing a metadata
            // sync on every line.
            let _ = file.sync_data().await;
        });
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
