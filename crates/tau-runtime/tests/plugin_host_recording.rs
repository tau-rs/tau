//! Integration test: protocol recording via [`RecordingSink::JsonlFile`].
//!
//! Pairs an [`IpcLlmBackend`] with a [`FakeStdioPeer`] over duplex
//! streams (mirroring `plugin_host_ipc_llm.rs::paired_process`),
//! additionally installs a thread-local [`PluginRecordingLayer`]
//! aimed at a tempdir log file, and asserts that one `llm.complete`
//! roundtrip produces both a `dir = "h2p"` (request) and a `dir =
//! "p2h"` (response) entry in the JSONL log with the expected fields.
//!
//! Covers the read- and write-side tap points wired in
//! `plugin_host::process` (Task 17) end-to-end through the Layer
//! pipeline migrated in Logging Sub-project D Task 5.

#![cfg(feature = "test-support")]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tau_observe::layers::plugin_recording::PluginRecordingLayer;
use tau_plugin_protocol::test_support::FakeStdioPeer;
use tau_plugin_protocol::{FramedReader, FramedWriter, FramerOptions};
use tau_ports::fixtures::make_completion_response;
use tau_ports::{CompletionRequest, StopReason};
use tau_runtime::builder::DynLlmBackend;
use tau_runtime::plugin_host::__internals::{
    DynAsyncWriter, IpcLlmBackend, PluginProcess, Recorder,
};
use tokio::io::DuplexStream;
use tracing::subscriber::DefaultGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Registry;

/// Install a `PluginRecordingLayer` aimed at `log_path` as the
/// thread-local subscriber for the duration of the returned guard.
///
/// `Recorder::record` (Sub-project D Task 5) is now a thin
/// `tracing::event!` emitter, so integration tests need a subscriber
/// that actually materializes the event to disk. `set_default`
/// (not `set_global_default`) keeps concurrent tests from fighting
/// over one global subscriber; the `#[tokio::test]` current-thread
/// runtime ensures the same thread-local sees every `tokio::spawn`'d
/// task.
fn install_plugin_recording_layer(log_path: &Path) -> (DefaultGuard, PluginRecordingLayer) {
    let layer = PluginRecordingLayer::new(log_path.to_path_buf());
    let subscriber = Registry::default().with(layer.clone());
    let guard = tracing::subscriber::set_default(subscriber);
    (guard, layer)
}

/// Build a [`PluginProcess`] paired with a [`FakeStdioPeer`] and a
/// stateless [`Recorder`]. Recording events are routed to whatever
/// `PluginRecordingLayer` is active on the calling thread's
/// subscriber.
async fn paired_process_with_recording(
    plugin_name: &str,
) -> (Arc<PluginProcess>, FakeStdioPeer, Arc<Recorder>) {
    let recorder = Arc::new(Recorder::new(plugin_name));

    let (peer_read_half, sut_write_half) = tokio::io::duplex(64 * 1024);
    let (sut_read_half, peer_write_half) = tokio::io::duplex(64 * 1024);
    let peer = FakeStdioPeer {
        reader: FramedReader::new(peer_read_half, FramerOptions::default()),
        writer: FramedWriter::new(peer_write_half),
    };
    let sut_reader: FramedReader<DuplexStream> =
        FramedReader::new(sut_read_half, FramerOptions::default());
    let sut_writer: FramedWriter<DynAsyncWriter> =
        FramedWriter::new(Box::new(sut_write_half) as DynAsyncWriter);

    let process = PluginProcess::new_for_test_with_recorder(
        plugin_name.to_string(),
        sut_reader,
        sut_writer,
        Duration::from_secs(2),
        Some(recorder.clone()),
    );
    (process, peer, recorder)
}

#[tokio::test]
async fn jsonl_file_recording_captures_frames_in_both_directions() {
    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("wire.log");

    // Install the layer BEFORE creating the paired process so the
    // read-loop spawned by `new_for_test_with_recorder` already sees
    // the active subscriber when it begins recording inbound frames.
    let (_guard, layer) = install_plugin_recording_layer(&log_path);

    let (process, mut peer, _recorder) = paired_process_with_recording("test-plugin").await;
    let backend = IpcLlmBackend::new("test-plugin".to_string(), process);

    let req = CompletionRequest::new("test-plugin".to_string());

    // Drive both sides concurrently — same `tokio::join!` pattern as
    // the existing IPC adapter integration tests (the
    // `DynLlmBackend::complete` future isn't `Send`-bounded so we can't
    // `tokio::spawn` it).
    let call_fut = backend.complete(req);
    let peer_fut = async {
        let (msgid, _params) = peer.expect_request("llm.complete").await;
        let canned =
            make_completion_response("hello".into(), Vec::new(), StopReason::EndTurn, None);
        peer.send_response(msgid, &canned).await.unwrap();
    };
    let (call_result, ()) = tokio::join!(call_fut, peer_fut);
    let resp = call_result.expect("complete should succeed");
    assert_eq!(resp.text, "hello");

    // Drop the peer so the read loop sees EOF, then flush the layer
    // — `flush` blocks on the in-flight `tokio::spawn` write tasks
    // and `sync_all`s the file so the subsequent synchronous read
    // observes every line.
    drop(peer);
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    layer.flush().await;

    let log_contents = std::fs::read_to_string(&log_path).unwrap();
    let lines: Vec<&str> = log_contents.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        lines.len() >= 2,
        "expected at least 2 frames, got {} line(s):\n{log_contents}",
        lines.len()
    );

    let mut h2p_count = 0usize;
    let mut p2h_count = 0usize;
    let mut saw_llm_complete = false;
    let mut saw_response_msgid = false;
    for line in &lines {
        let parsed: serde_json::Value =
            serde_json::from_str(line).expect("each line is valid JSON");
        let dir = parsed["dir"].as_str().expect("dir is a string");
        match dir {
            "h2p" => h2p_count += 1,
            "p2h" => p2h_count += 1,
            other => panic!("unexpected dir {other:?} in log line: {line}"),
        }
        assert!(parsed["frame"].is_string(), "frame must be base64 string");
        assert_eq!(
            parsed["plugin"], "test-plugin",
            "plugin field must match the recorder's plugin name"
        );
        assert!(parsed["ts"].is_number(), "ts must be a number");

        if parsed["method"] == "llm.complete" {
            saw_llm_complete = true;
            assert_eq!(dir, "h2p", "llm.complete is the host-side request");
            assert_eq!(parsed["msgid"], 2, "first post-handshake msgid is 2");
        }
        // The response line carries no `method` (only the originating
        // request does); verify it carries the matching msgid.
        if dir == "p2h" && parsed["method"].is_null() && parsed["msgid"] == 2 {
            saw_response_msgid = true;
        }
    }
    assert!(
        h2p_count >= 1,
        "expected at least 1 h2p frame, got {lines:?}"
    );
    assert!(
        p2h_count >= 1,
        "expected at least 1 p2h frame, got {lines:?}"
    );
    assert!(saw_llm_complete, "expected an llm.complete h2p line");
    assert!(
        saw_response_msgid,
        "expected a p2h response line carrying msgid=2 (no method)"
    );
}

#[tokio::test]
async fn jsonl_file_recording_disabled_when_no_sink_configured() {
    // Sanity: when the recorder is `None`, no log file is written and
    // round-trips behave exactly as in the regular ipc_llm tests.
    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("wire.log");

    // Build the SUT side without a recorder.
    let (peer_read_half, sut_write_half) = tokio::io::duplex(64 * 1024);
    let (sut_read_half, peer_write_half) = tokio::io::duplex(64 * 1024);
    let mut peer = FakeStdioPeer {
        reader: FramedReader::new(peer_read_half, FramerOptions::default()),
        writer: FramedWriter::new(peer_write_half),
    };
    let sut_reader: FramedReader<DuplexStream> =
        FramedReader::new(sut_read_half, FramerOptions::default());
    let sut_writer: FramedWriter<DynAsyncWriter> =
        FramedWriter::new(Box::new(sut_write_half) as DynAsyncWriter);
    let process = PluginProcess::new_for_test(
        "test-plugin".to_string(),
        sut_reader,
        sut_writer,
        Duration::from_secs(2),
    );
    let backend = IpcLlmBackend::new("test-plugin".to_string(), process);

    let req = CompletionRequest::new("test-plugin".to_string());
    let call_fut = backend.complete(req);
    let peer_fut = async {
        let (msgid, _params) = peer.expect_request("llm.complete").await;
        let canned =
            make_completion_response("hello".into(), Vec::new(), StopReason::EndTurn, None);
        peer.send_response(msgid, &canned).await.unwrap();
    };
    let (call_result, ()) = tokio::join!(call_fut, peer_fut);
    call_result.expect("complete should succeed");

    assert!(
        !log_path.exists(),
        "no log file should exist when recording is disabled"
    );
}
