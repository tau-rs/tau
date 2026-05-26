//! Protocol recording: capture every wire frame to the
//! `tau::plugin::frame` tracing target.
//!
//! When [`crate::plugin_host::PluginHostOptions::recording`] is set to
//! [`crate::plugin_host::RecordingSink::JsonlFile`], the read-loop and
//! the writer-mutex tap every frame and emit a [`tracing::event!`]
//! carrying the frame's metadata + base64-encoded body. A
//! [`tau_observe::layers::plugin_recording::PluginRecordingLayer`]
//! installed by the CLI is the actual writer to disk; the [`Recorder`]
//! itself is now a stateless emitter holding only the plugin name.
//!
//! On-disk JSONL shape (unchanged from the pre-D direct-write era):
//!
//! ```text
//! {"ts":1714316451.123,"plugin":"echo-llm","dir":"h2p",
//!  "msgid":1,"method":"meta.handshake","frame":"<base64>"}
//! ```
//!
//! Direction codes are `h2p` (host → plugin) and `p2h` (plugin → host).
//! `method` and `msgid` are decoded from the frame body for indexing
//! convenience but are also redundantly encoded inside `frame` (base64)
//! for lossless replay.
//!
//! Recording is best-effort. The layer logs write errors via
//! `tracing::warn!` and does not propagate — the runtime continues even
//! if the file is full or permission-denied. Likewise, failure to open
//! the file on first event lazy-loads to a no-op for subsequent events.
//!
//! See spec §7.8 (recording), §9.1 (debug tier — protocol recording),
//! and Logging Sub-project D Task 5 (tracing layer migration).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use tau_observe::layers::plugin_recording::TARGET as REC_TARGET;
use tau_plugin_protocol::Frame;

/// Direction of a recorded frame.
///
/// `pub` (rather than `pub(crate)`) so the test-only `__internals`
/// re-exports can surface it; production code only sees the JSONL
/// `dir` string field, never the typed enum.
#[derive(Debug, Clone, Copy)]
pub enum Direction {
    /// Host → plugin (request, host-side notification).
    HostToPlugin,
    /// Plugin → host (response, plugin-side notification).
    PluginToHost,
}

impl Direction {
    fn as_str(&self) -> &'static str {
        match self {
            Direction::HostToPlugin => "h2p",
            Direction::PluginToHost => "p2h",
        }
    }
}

/// A stateless emitter that wraps every recorded frame into a
/// `tracing::event!(target = "tau::plugin::frame", …)`.
///
/// The actual JSONL file is owned by
/// [`tau_observe::layers::plugin_recording::PluginRecordingLayer`],
/// which the CLI installs alongside the global subscriber when
/// `--record-protocol <path>` is passed. The [`Recorder`] keeps only
/// the plugin name so per-plugin attribution can be carried as a field
/// on the emitted event.
///
/// `pub` (rather than `pub(crate)`) because the test-only
/// `__internals` re-exports under [`crate::plugin_host::__internals`]
/// need to surface the type to integration tests; production code
/// reaches the recorder only via
/// [`crate::plugin_host::PluginHostOptions::recording`].
#[derive(Debug)]
pub struct Recorder {
    plugin_name: String,
}

impl Recorder {
    /// Construct a stateless recorder for `plugin_name`. The recorder
    /// emits tracing events on each `record(...)`; the actual file
    /// write happens in a `PluginRecordingLayer` installed elsewhere.
    pub fn new(plugin_name: impl Into<String>) -> Self {
        Self {
            plugin_name: plugin_name.into(),
        }
    }

    /// Record a frame. Emits a `tracing::event!` at INFO level on the
    /// `tau::plugin::frame` target; if no [`PluginRecordingLayer`] is
    /// installed, this is a cheap no-op (tracing's event-emission cost
    /// degrades to a target-string compare when no layer matches).
    ///
    /// INFO (rather than TRACE) is chosen so the default
    /// `tau=info` filter doesn't short-circuit recording events via
    /// `Subscriber::max_level_hint` before the per-layer filter on
    /// `PluginRecordingLayer` has a chance to evaluate them.
    /// Recording is opt-in via `--record-protocol`; the level is an
    /// implementation detail of routing through the tracing
    /// infrastructure.
    ///
    /// `frame_bytes` is the raw MessagePack frame body (post-framing
    /// decode); the function additionally decodes a [`Frame`] to extract
    /// `msgid` + `method` for indexing convenience. Decoding failures
    /// are tolerated: the event is still emitted with the optional
    /// fields omitted (the layer then materializes them as `null`)
    /// and the (lossless) base64-encoded frame body intact.
    pub async fn record(&self, dir: Direction, frame_bytes: &[u8]) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let (msgid, method) = decode_frame_metadata(frame_bytes);
        let encoded = B64.encode(frame_bytes);

        // tracing macros don't support runtime-omitted fields: the four
        // (msgid, method) cases each enumerate which optionals are
        // present. Absent fields surface as `null` in the layer's
        // serialized JSONL (see `PluginRecordingLayer`).
        //
        // - `ts` is emitted as an `f64` so the layer's `record_f64`
        //   path stores a JSON number (not a Display-stringified form).
        // - `msgid` is cast to `u64` (always non-negative) to match the
        //   layer's `record_u64` visitor.
        // - String fields use the `%` display sigil so tracing
        //   dispatches via the `Display` Debug impl (which the layer's
        //   `record_debug` handler maps to a JSON string without
        //   double-quoting).
        match (msgid, method) {
            (Some(id), Some(m)) => tracing::event!(
                target: REC_TARGET,
                tracing::Level::INFO,
                ts = ts,
                plugin = %self.plugin_name,
                dir = %dir.as_str(),
                msgid = id as u64,
                method = %m,
                frame = %encoded,
            ),
            (Some(id), None) => tracing::event!(
                target: REC_TARGET,
                tracing::Level::INFO,
                ts = ts,
                plugin = %self.plugin_name,
                dir = %dir.as_str(),
                msgid = id as u64,
                frame = %encoded,
            ),
            (None, Some(m)) => tracing::event!(
                target: REC_TARGET,
                tracing::Level::INFO,
                ts = ts,
                plugin = %self.plugin_name,
                dir = %dir.as_str(),
                method = %m,
                frame = %encoded,
            ),
            (None, None) => tracing::event!(
                target: REC_TARGET,
                tracing::Level::INFO,
                ts = ts,
                plugin = %self.plugin_name,
                dir = %dir.as_str(),
                frame = %encoded,
            ),
        }
    }
}

/// Best-effort decode of the frame body to extract `msgid` and `method`
/// for the JSONL `msgid` / `method` indexing fields. Decode failures
/// (or future [`Frame`] variants — the type is `#[non_exhaustive]`)
/// map to `(None, None)` — the raw base64-encoded frame is still
/// recorded losslessly under the `frame` field.
fn decode_frame_metadata(frame_bytes: &[u8]) -> (Option<u32>, Option<String>) {
    match Frame::decode(frame_bytes) {
        Ok(Frame::Request { id, method, .. }) => (Some(id), Some(method)),
        Ok(Frame::Response { id, .. }) => (Some(id), None),
        Ok(Frame::Notification { method, .. }) => (None, Some(method)),
        // `Frame` is `#[non_exhaustive]`; a future variant decodes to
        // an indexer-less line rather than panicking.
        Ok(_) => (None, None),
        Err(_) => (None, None),
    }
}

/// Shared recorder handle. `Arc<Recorder>` clones cheaply; tap points
/// share one emitter. See [`Recorder`] for why this is `pub` rather
/// than `pub(crate)`.
pub type RecorderHandle = Arc<Recorder>;

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;
    use std::time::Duration;

    use tau_observe::layers::plugin_recording::PluginRecordingLayer;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::Registry;

    /// Wait until `path` contains at least `expected` JSONL lines, or
    /// panic after `timeout`. The recording layer's writes are
    /// fire-and-forget `tokio::spawn` tasks, so deterministic
    /// observation requires polling rather than a fixed sleep.
    async fn await_layer_flush(path: &Path, expected: usize, timeout: Duration) {
        let start = std::time::Instant::now();
        loop {
            if let Ok(s) = tokio::fs::read_to_string(path).await {
                let count = s.lines().filter(|l| !l.is_empty()).count();
                if count >= expected {
                    return;
                }
            }
            if start.elapsed() >= timeout {
                let actual = std::fs::read_to_string(path)
                    .map(|s| s.lines().filter(|l| !l.is_empty()).count())
                    .unwrap_or(0);
                panic!(
                    "plugin recording layer did not flush {expected} record(s) to {path:?} within {timeout:?} (actual: {actual})"
                );
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn record_appends_line_with_metadata_for_known_request_frame() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rec.jsonl");
        let layer = PluginRecordingLayer::new(path.clone());
        let subscriber = Registry::default().with(layer);

        let frame = Frame::Request {
            id: 7,
            method: "llm.complete".to_string(),
            params: rmp_serde::to_vec::<Vec<()>>(&Vec::new()).unwrap(),
        };
        let bytes = frame.encode().unwrap();

        let _guard = tracing::subscriber::set_default(subscriber);
        let recorder = Recorder::new("plug");
        recorder.record(Direction::HostToPlugin, &bytes).await;

        await_layer_flush(&path, 1, Duration::from_secs(5)).await;

        let contents = std::fs::read_to_string(&path).unwrap();
        let line = contents.lines().next().expect("at least one line");
        let parsed: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(parsed["plugin"], "plug");
        assert_eq!(parsed["dir"], "h2p");
        assert_eq!(parsed["msgid"], 7);
        assert_eq!(parsed["method"], "llm.complete");
        assert!(parsed["frame"].is_string());
        assert!(parsed["ts"].is_number());
    }

    #[tokio::test]
    async fn record_tolerates_undecodable_frame_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rec.jsonl");
        let layer = PluginRecordingLayer::new(path.clone());
        let subscriber = Registry::default().with(layer);

        let _guard = tracing::subscriber::set_default(subscriber);
        let recorder = Recorder::new("plug");
        // Garbage that won't parse as a Frame.
        recorder
            .record(Direction::PluginToHost, b"\xff\xff\xff\xff")
            .await;

        await_layer_flush(&path, 1, Duration::from_secs(5)).await;

        let contents = std::fs::read_to_string(&path).unwrap();
        let line = contents.lines().next().expect("at least one line");
        let parsed: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(parsed["dir"], "p2h");
        assert!(parsed["msgid"].is_null());
        assert!(parsed["method"].is_null());
        assert!(parsed["frame"].is_string());
    }
}
