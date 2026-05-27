//! Layer that materializes `target = "tau::plugin::frame"` events
//! into the recording JSONL format previously written by
//! `tau_runtime::plugin_host::recording::Recorder`.
//!
//! Field schema (must match `recording.rs` output):
//! - `ts` (f64 unix seconds)
//! - `plugin` (string)
//! - `dir` (string: "h2p" | "p2h")
//! - `msgid` (u32 | null)
//! - `method` (string | null)
//! - `frame` (string: base64-encoded raw frame bytes)
//!
//! Unlike `WorkflowRunLogLayer`, this layer does NOT call `sync_data`
//! after each line — that matches the legacy `Recorder::record` writer,
//! which only `write_all`s. Tests that read the file synchronously
//! must drain the buffer explicitly (see `recording.rs` for the
//! `flush_recorder` pattern).
//!
//! ## Null handling for optional fields
//!
//! `msgid` and `method` are `Option`s in the recorder. `tracing` has no
//! clean way to emit "this field is null", so the producer (Task 5)
//! emits these fields only when `Some`. The serializer here fills in
//! `null` for missing optional fields so the output schema is stable.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};
use tracing::{field::Visit, Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// The `target` string that emissions must use to be picked up by this
/// layer. Producers should emit with `tracing::event!(target: TARGET, …)`.
pub const TARGET: &str = "tau::plugin::frame";

/// Layer that appends each matching event to a JSONL file.
///
/// Writes are dispatched to `tokio::spawn`'d tasks so the producing
/// thread is never blocked on disk IO. A pending-write counter +
/// [`Notify`] let callers `flush()` and await drainage at process
/// exit (see `PluginRecordingLayer::flush`); the layer also locks +
/// `flush`/`sync_all`s the underlying file at that point so a
/// subsequent synchronous read observes every recorded line.
///
/// ```
/// use std::path::PathBuf;
/// use tau_observe::layers::plugin_recording::PluginRecordingLayer;
///
/// // Construct a layer targeting a recording file.
/// let path = PathBuf::from("/tmp/tau-plugin-recording.jsonl");
/// let layer = PluginRecordingLayer::new(path.clone());
/// // Clone is cheap (Arc-backed).
/// let layer2 = layer.clone();
/// // TARGET must match the expected string to route events correctly.
/// assert_eq!(tau_observe::layers::plugin_recording::TARGET, "tau::plugin::frame");
/// ```
#[derive(Clone)]
pub struct PluginRecordingLayer {
    inner: Arc<Mutex<Inner>>,
    /// Number of in-flight write tasks (spawned but not yet finished).
    /// Producers increment before `tokio::spawn`; the spawned task
    /// decrements and notifies after either writing or aborting.
    pending: Arc<AtomicUsize>,
    /// Pulsed every time `pending` decrements toward zero so
    /// [`PluginRecordingLayer::flush`] can wake without busy-polling.
    pending_notify: Arc<Notify>,
}

struct Inner {
    path: PathBuf,
    file: Option<tokio::fs::File>,
}

impl PluginRecordingLayer {
    /// Construct a layer that will append matching events to `path`.
    ///
    /// The file is not opened until the first matching event arrives,
    /// to keep the layer cheap when no plugin recording is active.
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use tau_observe::layers::plugin_recording::PluginRecordingLayer;
    ///
    /// let path = PathBuf::from("/tmp/recording.jsonl");
    /// let layer = PluginRecordingLayer::new(path);
    /// // Layer is Clone — cheap Arc-backed copy.
    /// let _layer2 = layer.clone();
    /// ```
    pub fn new(path: PathBuf) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner { path, file: None })),
            pending: Arc::new(AtomicUsize::new(0)),
            pending_notify: Arc::new(Notify::new()),
        }
    }

    /// Wait until every in-flight `on_event`-spawned write task has
    /// finished, then `flush` + `sync_all` the underlying file so a
    /// subsequent synchronous reader observes every recorded line.
    ///
    /// Called by the CLI's `cmd::run` / `cmd::chat` / `cmd::plugin
    /// describe` on exit when `--record-protocol` is set. Best-effort:
    /// IO errors are logged at WARN, never propagated.
    ///
    /// `no_run`: requires an active tokio runtime (`tokio::spawn` must be
    /// reachable) and a live file write cycle to exercise meaningfully;
    /// doctest execution would block without a runtime context.
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use tau_observe::layers::plugin_recording::PluginRecordingLayer;
    /// # #[tokio::main]
    /// # async fn main() {
    /// let layer = PluginRecordingLayer::new(PathBuf::from("/tmp/rec.jsonl"));
    /// // Call flush before process exit to ensure durability.
    /// layer.flush().await;
    /// # }
    /// ```
    pub async fn flush(&self) {
        // Wait for pending writes to drain. Each finishing task
        // notifies all waiters; we re-check after each wake. The
        // `notified()` future MUST be created before the `pending`
        // check, otherwise a task that finishes between the check and
        // the wait would leave us hanging — `Notify::notify_waiters`
        // wakes only futures registered at the time of the call.
        loop {
            let notified = self.pending_notify.notified();
            tokio::pin!(notified);
            // `enable()` arms the future so subsequent notify_waiters
            // calls are observed even before this future is polled.
            notified.as_mut().enable();
            if self.pending.load(Ordering::Acquire) == 0 {
                break;
            }
            notified.await;
        }
        use tokio::io::AsyncWriteExt as _;
        let mut guard = self.inner.lock().await;
        if let Some(file) = guard.file.as_mut() {
            if let Err(e) = file.flush().await {
                tracing::warn!(
                    target: "tau_observe::layers::plugin_recording",
                    err = %e,
                    "plugin recording flush failed",
                );
            }
            if let Err(e) = file.sync_all().await {
                tracing::warn!(
                    target: "tau_observe::layers::plugin_recording",
                    err = %e,
                    "plugin recording sync_all failed",
                );
            }
        }
    }
}

impl<S> Layer<S> for PluginRecordingLayer
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
        let line = serialize_frame_record(&visitor.fields);
        let inner = self.inner.clone();
        let pending = self.pending.clone();
        let pending_notify = self.pending_notify.clone();
        // Increment BEFORE spawn so `flush` can observe the in-flight
        // work even if the spawned task is queued behind the flush
        // call.
        pending.fetch_add(1, Ordering::AcqRel);
        // Hand off the write to the runtime so we don't block the
        // emitting task. Best-effort: errors are logged at WARN to a
        // *different* target so they don't recursively re-enter this
        // layer's filter.
        tokio::spawn(async move {
            // Always decrement + notify on exit (success or early
            // return). A guard struct keeps the bookkeeping
            // panic-safe without an explicit `defer!` macro.
            struct PendingGuard {
                pending: Arc<AtomicUsize>,
                notify: Arc<Notify>,
            }
            impl Drop for PendingGuard {
                fn drop(&mut self) {
                    self.pending.fetch_sub(1, Ordering::AcqRel);
                    self.notify.notify_waiters();
                }
            }
            let _guard = PendingGuard {
                pending,
                notify: pending_notify,
            };

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
                            target: "tau_observe::layers::plugin_recording",
                            path = %path.display(),
                            err = %e,
                            "plugin recording open failed; dropping event",
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
                    target: "tau_observe::layers::plugin_recording",
                    err = %e,
                    "plugin recording write failed",
                );
            }
            // NOTE: no `sync_data` here — the legacy `Recorder::record`
            // only `write_all`s, and matching that behavior is required
            // for the byte-identical-output assertion in Task 6. The
            // CLI calls `PluginRecordingLayer::flush` on exit to ensure
            // durability before the process terminates.
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
    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        // `ts` is an `f64` unix-seconds value; tracing 0.1.40+ dispatches
        // `f64` field values through `record_f64`. If the conversion to a
        // JSON number fails (NaN / infinity), fall back to `null` rather
        // than panicking — the recorder treats `ts` as best-effort.
        let v = serde_json::Number::from_f64(value)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null);
        self.fields.insert(field.name().to_string(), v);
    }
    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), serde_json::Value::Bool(value));
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        // Catches Display-formatted fields (`%x`) and Debug-formatted
        // fields (`?x`). Stored as a JSON string; for `&str` fields
        // tracing dispatches to `record_str` above instead, so this path
        // never double-quotes string literals.
        self.fields.insert(
            field.name().to_string(),
            serde_json::Value::String(format!("{value:?}")),
        );
    }
}

/// Build the JSONL line matching the byte-identical output produced by
/// `tau_runtime::plugin_host::recording::Recorder::record`.
///
/// The legacy `Recorder` uses `serde_json::json!({...})` which without
/// the `preserve_order` feature flag stores keys in a `BTreeMap` and
/// serializes them **alphabetically**. Producing byte-identical output
/// (required for the Task 6 assertion) therefore means going through
/// the same `serde_json::Map`/`Value::Object` path here so both writers
/// land on the same alphabetical key order: `dir, frame, method, msgid,
/// plugin, ts`.
///
/// Missing `msgid` / `method` fields are written as JSON `null` so the
/// output shape is stable regardless of whether the producer chose to
/// omit the field (the typical case when the underlying frame couldn't
/// be decoded — see `decode_frame_metadata` in `recording.rs`).
fn serialize_frame_record(fields: &BTreeMap<String, serde_json::Value>) -> String {
    let mut obj = serde_json::Map::new();
    // Pass through required fields if present.
    for key in ["ts", "plugin", "dir", "frame"] {
        if let Some(v) = fields.get(key) {
            obj.insert(key.to_string(), v.clone());
        }
    }
    // Optional fields: emit explicit `null` if the producer omitted them
    // so the output schema is stable.
    for key in ["msgid", "method"] {
        let v = fields.get(key).cloned().unwrap_or(serde_json::Value::Null);
        obj.insert(key.to_string(), v);
    }
    // `Value::Object` serializes its `Map` in `BTreeMap` order
    // (alphabetical) unless the `preserve_order` feature is enabled —
    // matches `Recorder::record`'s `json!({…}).to_string()`.
    let mut line = serde_json::Value::Object(obj).to_string();
    line.push('\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serialize_matches_recorder_alphabetical_key_order() {
        // Mirror the byte layout produced by `Recorder::record` so the
        // Task 6 assertion has a fixed reference. `Recorder` uses
        // `serde_json::json!({...})` without `preserve_order`, which
        // sorts keys alphabetically: dir, frame, method, msgid,
        // plugin, ts.
        let mut fields = BTreeMap::new();
        fields.insert("frame".to_string(), json!("ZnJhbWU="));
        fields.insert("method".to_string(), json!("llm.complete"));
        fields.insert("msgid".to_string(), json!(7));
        fields.insert("dir".to_string(), json!("h2p"));
        fields.insert("plugin".to_string(), json!("echo-llm"));
        fields.insert("ts".to_string(), json!(1714316451.123));

        let line = serialize_frame_record(&fields);
        assert!(line.ends_with('\n'));
        let trimmed = line.trim_end_matches('\n');
        let dir_at = trimmed.find("\"dir\"").unwrap();
        let frame_at = trimmed.find("\"frame\"").unwrap();
        let method_at = trimmed.find("\"method\"").unwrap();
        let msgid_at = trimmed.find("\"msgid\"").unwrap();
        let plugin_at = trimmed.find("\"plugin\"").unwrap();
        let ts_at = trimmed.find("\"ts\"").unwrap();
        assert!(dir_at < frame_at);
        assert!(frame_at < method_at);
        assert!(method_at < msgid_at);
        assert!(msgid_at < plugin_at);
        assert!(plugin_at < ts_at);
        // Round-trip parses as JSON.
        let v: serde_json::Value = serde_json::from_str(trimmed).unwrap();
        assert_eq!(v["plugin"], "echo-llm");
        assert_eq!(v["msgid"], 7);
        assert_eq!(v["method"], "llm.complete");
    }

    #[test]
    fn serialize_emits_null_for_missing_optional_fields() {
        let mut fields = BTreeMap::new();
        fields.insert("ts".to_string(), json!(1714316451.123));
        fields.insert("plugin".to_string(), json!("plug"));
        fields.insert("dir".to_string(), json!("p2h"));
        fields.insert("frame".to_string(), json!("Zg=="));
        // No msgid, no method.

        let line = serialize_frame_record(&fields);
        let trimmed = line.trim_end_matches('\n');
        let v: serde_json::Value = serde_json::from_str(trimmed).unwrap();
        assert!(v["msgid"].is_null());
        assert!(v["method"].is_null());
        assert_eq!(v["dir"], "p2h");
    }
}
