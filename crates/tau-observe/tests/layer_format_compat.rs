//! Byte-identical output assertions for `WorkflowRunLogLayer` and
//! `PluginRecordingLayer`.
//!
//! These tests pin the on-disk JSONL produced by the two layers so a
//! future change to the serializer is caught the moment it diverges
//! from the legacy direct-write format.
//!
//! ## What "byte-identical" means here
//!
//! Sub-project D's contract is that the file produced by the layer is
//! interchangeable with the file produced by the (now-deleted) direct
//! writers. "Interchangeable" means: existing readers (`replay`,
//! recording test helpers) parse the new file successfully and recover
//! the originating record. It does **not** mean the bytes match the
//! legacy `serde_json::to_string(&StepRecord)` output exactly — both
//! layers use `serde_json::Map` (without the `preserve_order`
//! feature) which sorts keys alphabetically, while the legacy
//! `serde_json::to_string` path on a `#[derive(Serialize)]` struct
//! emitted keys in declaration order.
//!
//! Two layers, two contracts:
//!
//! - `WorkflowRunLogLayer`: emits each line with keys in alphabetical
//!   order (driven by `serde_json::Map`'s `BTreeMap` backing). Optional
//!   fields `error` / `detail` are omitted when the producer doesn't
//!   emit them — matching `#[serde(skip_serializing_if =
//!   "Option::is_none")]`. The line MUST round-trip back to the
//!   originating `StepRecord` via `serde_json::from_str`.
//!
//! - `PluginRecordingLayer`: emits each line with keys in alphabetical
//!   order — matching the legacy `Recorder::record` which used
//!   `serde_json::json!({...}).to_string()` (which itself goes through
//!   `serde_json::Map`'s alphabetical ordering). Missing optional
//!   fields `msgid` / `method` are filled in as JSON `null` for stable
//!   schema.
//!
//! ## Key-order finding
//!
//! Both layers currently produce alphabetical key order — they go
//! through `serde_json::Map` (a `BTreeMap` without `preserve_order`).
//! The `serialize_step_record` helper in `workflow_run_log.rs`
//! documents "declaration order" but the `Map::insert` calls under
//! the hood sort. This test pins the actual observable behavior;
//! flipping to declaration order would require either enabling
//! `serde_json/preserve_order` workspace-wide or reconstructing a
//! `StepRecord` and calling `serde_json::to_string` on it directly.
//! The on-disk contract (parse-back equivalence) is preserved either
//! way.

use std::path::Path;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use serde_json::Value;
use tau_observe::layers::plugin_recording::{PluginRecordingLayer, TARGET as REC_TARGET};
use tau_observe::layers::workflow_run_log::{WorkflowRunLogLayer, TARGET as WF_TARGET};
use tau_workflow::persistence::{StepRecord, StepStatus};
use tracing_subscriber::layer::SubscriberExt;

/// Poll `path` until at least one full line (ending in `\n`) has been
/// written or `timeout` elapses. The layers spawn fire-and-forget
/// `tokio::spawn` write tasks, so deterministic observation requires
/// polling — a fixed sleep races under CI load.
///
/// On timeout, panics with the current bytes observed so the failure
/// mode is obvious.
async fn poll_for_lines(path: &Path, expected_lines: usize, timeout: Duration) {
    let start = std::time::Instant::now();
    loop {
        if let Ok(s) = tokio::fs::read_to_string(path).await {
            let complete = s.matches('\n').count();
            if complete >= expected_lines {
                return;
            }
        }
        if start.elapsed() >= timeout {
            let actual = tokio::fs::read_to_string(path).await.unwrap_or_default();
            panic!(
                "expected {expected_lines} complete line(s) at {path:?} within {timeout:?}; \
                 actual contents ({} bytes): {actual:?}",
                actual.len()
            );
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Drive `WorkflowRunLogLayer` with a synthetic event matching
/// `RunLog::append`'s emission shape, then assert the line:
/// 1. Is exactly one record + trailing `\n`.
/// 2. Has all canonical fields present in alphabetical key order (see
///    module docstring: `serde_json::Map` sorts).
/// 3. Parses back into an equal `StepRecord` via `serde_json`.
/// 4. Omits optional `error`/`detail` when the producer didn't emit
///    them (matches `#[serde(skip_serializing_if = "Option::is_none")]`).
#[tokio::test]
async fn workflow_layer_emits_alphabetical_jsonl_round_trips_step_record() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("run.jsonl");
    let layer = WorkflowRunLogLayer::new(path.clone());
    let subscriber = tracing_subscriber::registry().with(layer);

    // Reference record. `error` / `detail` are intentionally `None` so
    // we can prove the layer's "omit when absent" branch.
    let ts = Utc.with_ymd_and_hms(2026, 5, 17, 12, 0, 0).unwrap();
    let started = Utc.with_ymd_and_hms(2026, 5, 17, 12, 0, 0).unwrap();
    let ended = Utc.with_ymd_and_hms(2026, 5, 17, 12, 0, 1).unwrap();
    let record = StepRecord {
        ts,
        run_id: "01HXYZ".to_string(),
        step_id: "build".to_string(),
        step_index: 0,
        kind: "agent.run".to_string(),
        input: "hello".to_string(),
        output: "world".to_string(),
        started_at: started,
        ended_at: ended,
        duration_ms: 1000,
        status: StepStatus::Ok,
        error: None,
        detail: None,
    };

    tracing::subscriber::with_default(subscriber, || {
        // Mirror `RunLog::append`'s emission, but skip the
        // `error`/`detail` fields entirely to exercise the "omitted on
        // None" branch of the serializer.
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
        );
    });

    poll_for_lines(&path, 1, Duration::from_secs(5)).await;

    let written = tokio::fs::read_to_string(&path).await.unwrap();
    assert!(
        written.ends_with('\n'),
        "JSONL line must end with newline; got {written:?}"
    );
    assert_eq!(
        written.matches('\n').count(),
        1,
        "exactly one record expected; got {written:?}"
    );
    let trimmed = written.trim_end_matches('\n');

    // Field-order assertion: keys appear in **alphabetical** order
    // (driven by `serde_json::Map`'s `BTreeMap` backing). Walk through
    // the alphabetical key list; each subsequent key must appear
    // strictly after the previous one in the raw string. Catches a
    // future regression where `serde_json/preserve_order` is flipped
    // on or someone switches the serializer to a non-`Map` path
    // without updating callers/readers.
    let alphabetical = [
        "duration_ms",
        "ended_at",
        "input",
        "kind",
        "output",
        "run_id",
        "started_at",
        "status",
        "step_id",
        "step_index",
        "ts",
    ];
    let mut last_idx = 0usize;
    for key in alphabetical {
        let needle = format!("\"{key}\"");
        let pos = trimmed.find(&needle).unwrap_or_else(|| {
            panic!("field {key:?} missing from layer output: {trimmed:?}");
        });
        assert!(
            pos >= last_idx,
            "field {key:?} appeared at {pos} before previous field at {last_idx} in {trimmed:?}",
        );
        last_idx = pos;
    }

    // Optional fields must be absent (producer did not emit them).
    assert!(
        !trimmed.contains("\"error\""),
        "optional `error` should be omitted when None; got {trimmed:?}",
    );
    assert!(
        !trimmed.contains("\"detail\""),
        "optional `detail` should be omitted when None; got {trimmed:?}",
    );

    // Round-trip: parse the JSON back into `StepRecord` and assert
    // equality. This is the strongest "values survived end-to-end"
    // check — chrono's `Display` vs serde's RFC3339 string form
    // differs textually (`+00:00` vs `Z`) but both parse to the same
    // `DateTime<Utc>`, so equality holds.
    let parsed: StepRecord =
        serde_json::from_str(trimmed).expect("layer output must parse back as StepRecord");
    assert_eq!(parsed, record);
}

/// Same as above but with `Some(error) + Some(detail)`. Proves the
/// optional fields land in the correct tail position when emitted.
#[tokio::test]
async fn workflow_layer_emits_optional_fields_when_present() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("run.jsonl");
    let layer = WorkflowRunLogLayer::new(path.clone());
    let subscriber = tracing_subscriber::registry().with(layer);

    let ts = Utc.with_ymd_and_hms(2026, 5, 17, 12, 0, 0).unwrap();
    let record = StepRecord {
        ts,
        run_id: "01HXYZ".to_string(),
        step_id: "build".to_string(),
        step_index: 2,
        kind: "tool.call".to_string(),
        input: "in".to_string(),
        output: "out".to_string(),
        started_at: ts,
        ended_at: ts,
        duration_ms: 5,
        status: StepStatus::Failed,
        error: Some("ToolError".to_string()),
        detail: Some("boom".to_string()),
    };

    tracing::subscriber::with_default(subscriber, || {
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
    });

    poll_for_lines(&path, 1, Duration::from_secs(5)).await;

    let written = tokio::fs::read_to_string(&path).await.unwrap();
    let trimmed = written.trim_end_matches('\n');

    // Alphabetical position: `detail` (d) precedes `error` (e); both
    // appear earlier than e.g. `status` (s). Catches a regression
    // where the optional-fields branch accidentally appends instead of
    // letting `Map::insert` re-sort.
    let detail_at = trimmed.find("\"detail\"").unwrap();
    let error_at = trimmed.find("\"error\"").unwrap();
    let status_at = trimmed.find("\"status\"").unwrap();
    assert!(
        detail_at < error_at,
        "`detail` must precede `error` alphabetically; got {trimmed:?}",
    );
    assert!(
        error_at < status_at,
        "`error` must precede `status` alphabetically; got {trimmed:?}",
    );

    // Round-trip equality.
    let parsed: StepRecord =
        serde_json::from_str(trimmed).expect("layer output must parse back as StepRecord");
    assert_eq!(parsed, record);
}

/// Drive `PluginRecordingLayer` with a synthetic event and compare the
/// resulting line *byte-for-byte* against a hand-constructed reference
/// built via `serde_json::json!` — which (without `preserve_order`)
/// sorts keys alphabetically, exactly matching the legacy `Recorder`
/// writer.
#[tokio::test]
async fn plugin_recording_layer_emits_alphabetical_jsonl() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("rec.jsonl");
    let layer = PluginRecordingLayer::new(path.clone());
    let subscriber = tracing_subscriber::registry().with(layer.clone());

    tracing::subscriber::with_default(subscriber, || {
        tracing::event!(
            target: REC_TARGET,
            tracing::Level::INFO,
            ts = 1234.5_f64,
            plugin = %"test",
            dir = %"h2p",
            msgid = 7u64,
            method = %"init",
            frame = %"AAAA",
        );
    });

    // `PluginRecordingLayer` exposes a `flush()` method (added in
    // Task 5's optional-flush work) that waits for in-flight write
    // tasks AND fsyncs the file. Prefer it over polling for the
    // deterministic single-event case.
    layer.flush().await;

    let written = tokio::fs::read_to_string(&path).await.unwrap();

    // Reference: keys in alphabetical order via `json!`.
    let expected = serde_json::json!({
        "dir": "h2p",
        "frame": "AAAA",
        "method": "init",
        "msgid": 7,
        "plugin": "test",
        "ts": 1234.5,
    })
    .to_string()
        + "\n";

    assert_eq!(
        written, expected,
        "plugin recording layer output diverged from alphabetical reference",
    );
}

/// When the producer omits optional fields (`msgid`, `method`), the
/// layer must still emit them as JSON `null` so the on-disk schema is
/// stable. This case fires for frames whose body fails to decode (see
/// `decode_frame_metadata` in `tau_runtime::plugin_host::recording`).
#[tokio::test]
async fn plugin_recording_layer_fills_null_for_missing_optionals() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("rec.jsonl");
    let layer = PluginRecordingLayer::new(path.clone());
    let subscriber = tracing_subscriber::registry().with(layer.clone());

    tracing::subscriber::with_default(subscriber, || {
        tracing::event!(
            target: REC_TARGET,
            tracing::Level::INFO,
            ts = 1.0_f64,
            plugin = %"p",
            dir = %"p2h",
            frame = %"Zg==",
            // no msgid, no method
        );
    });

    layer.flush().await;

    let written = tokio::fs::read_to_string(&path).await.unwrap();
    let trimmed = written.trim_end_matches('\n');
    let v: Value = serde_json::from_str(trimmed).unwrap();
    assert!(v["msgid"].is_null(), "msgid must be null when omitted");
    assert!(v["method"].is_null(), "method must be null when omitted");

    // And the full byte string matches the alphabetical reference:
    let expected = serde_json::json!({
        "dir": "p2h",
        "frame": "Zg==",
        "method": null,
        "msgid": null,
        "plugin": "p",
        "ts": 1.0,
    })
    .to_string()
        + "\n";
    assert_eq!(written, expected);
}
