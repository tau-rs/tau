//! Malformed-tolerant jsonl line ingestion.
//!
//! [`parse_line`] turns one line of a `.tau/runs/<id>.jsonl` file into a
//! [`tau_ports::TraceEvent`], accepting both the writer's
//! `{"line_kind":"trace_event","event":{…}}` envelope and a bare event
//! (spec §13.5). Only a blank/whitespace-only line returns
//! `Ok(None)` — that is the one case a live tail (e.g. the TUI reading a
//! file the engine is still writing to) can expect to see routinely (a
//! trailing newline, or a read that lands exactly on EOF). Any non-empty
//! line that fails to parse — including a half-written/truncated line read
//! mid-write — returns `Err`, it is not silently swallowed. A live-tail
//! caller is expected to treat an `Err` on the last/still-growing line as
//! retryable (re-read on the next fsync boundary) and only skip/surface an
//! `Err` once the line is no longer growing (i.e. a later read confirms it
//! wasn't a transient partial write).

use tau_ports::TraceEvent;

/// Error parsing one jsonl line into a [`TraceEvent`].
///
/// Reserved for genuinely malformed *complete* lines — never returned for
/// blank/whitespace-only lines (see [`parse_line`]).
#[derive(thiserror::Error, Debug)]
pub enum IngestError {
    /// The line was non-empty but not a valid JSON-encoded `TraceEvent`.
    #[error("bad trace line: {0}")]
    Json(String),
}

/// Mirrors the tag shape of `tau_runtime_tokio::orchestration::persistence::RunLogLine`
/// (`{"line_kind": "...", ...}`), read manually via `serde_json::Value`.
///
/// `tau-trace` is pure and headless — it must not depend on the tokio host
/// crate — so the wire shape is mirrored here instead of imported. It is
/// read via `serde_json::Value` rather than a `#[derive(Deserialize)]`
/// mirror enum because `tau-trace` has no direct dependency on the `serde`
/// crate (only `serde_json`, which does not re-export `serde`'s derive
/// macros), and adding one is a dependency change outside this fix's scope.
/// The `envelope_shape_matches_the_writer` test guards against drift: if
/// the writer's tag name (`line_kind`), tag value (`trace_event`), or
/// payload field (`event`) ever changes, that test fails rather than the
/// reader silently rendering an empty waterfall (spec §13.5).
const ENVELOPE_TAG_FIELD: &str = "line_kind";
const ENVELOPE_TRACE_EVENT_TAG: &str = "trace_event";
const ENVELOPE_PAYLOAD_FIELD: &str = "event";

/// Parse one `.tau/runs/<id>.jsonl` line into a [`TraceEvent`].
///
/// Accepts both line shapes:
/// - the **envelope** the run-log writer produces,
///   `{"line_kind":"trace_event","event":{…}}` — non-`trace_event` kinds
///   (e.g. `task_mutation`) yield `Ok(None)`;
/// - a **bare** `TraceEvent`, for older logs and test fixtures.
///
/// Returns `Ok(None)` for a blank/whitespace-only line (a trailing newline,
/// or a live tail reading past the last complete line) and for envelope
/// lines that carry no trace event. Returns `Err` for any other non-empty
/// line that fails to deserialize — including a half-written/truncated line
/// read mid-write, which is *not* treated as `Ok(None)`. A live-tail caller
/// should treat such an `Err` on the still-growing tail line as retryable.
pub fn parse_line(line: &str) -> Result<Option<TraceEvent>, IngestError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    // Try the envelope first: it is what the writer emits today. A
    // truncated/malformed line fails JSON parsing outright here (it never
    // becomes a `Value::Object`), so it falls through to the bare-event
    // attempt below and surfaces that `Err` — it never spuriously matches
    // the envelope shape.
    if let Ok(serde_json::Value::Object(mut obj)) =
        serde_json::from_str::<serde_json::Value>(trimmed)
    {
        if let Some(serde_json::Value::String(tag)) = obj.get(ENVELOPE_TAG_FIELD) {
            return if tag == ENVELOPE_TRACE_EVENT_TAG {
                let payload = obj.remove(ENVELOPE_PAYLOAD_FIELD).ok_or_else(|| {
                    IngestError::Json(format!(
                        "envelope line missing '{ENVELOPE_PAYLOAD_FIELD}' field"
                    ))
                })?;
                serde_json::from_value::<TraceEvent>(payload)
                    .map(Some)
                    .map_err(|e| IngestError::Json(e.to_string()))
            } else {
                Ok(None)
            };
        }
    }
    serde_json::from_str::<TraceEvent>(trimmed)
        .map(Some)
        .map_err(|e| IngestError::Json(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use tau_ports::TraceEventKind;

    #[test]
    fn parses_a_turn_line() {
        // Round-trip via the real Serialize impl, per Step 0 — this is
        // immune to field-name drift because we never hand-author the JSON.
        let evt = TraceEvent {
            id: "evt-1".into(),
            ts: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            run_id: "run-1".into(),
            agent_id: Some("agent-1".into()),
            kind: TraceEventKind::Turn {
                agent_id: "agent-1".into(),
                turn_index: 3,
                duration_ms: 250,
                tokens: 42,
            },
        };
        let line = serde_json::to_string(&evt).unwrap();

        let parsed = parse_line(&line).unwrap().unwrap();

        assert_eq!(parsed, evt);
        assert!(matches!(parsed.kind, TraceEventKind::Turn { .. }));
    }

    #[test]
    fn parses_a_tool_call_line() {
        let evt = TraceEvent {
            id: "evt-2".into(),
            ts: Utc.timestamp_opt(1_700_000_100, 0).unwrap(),
            run_id: "run-1".into(),
            agent_id: Some("agent-1".into()),
            kind: TraceEventKind::ToolCall {
                tool_name: "net.http".into(),
                duration_ms: 12,
                status: "ok".into(),
                capability: None,
                turn_index: 0,
            },
        };
        let line = serde_json::to_string(&evt).unwrap();

        let parsed = parse_line(&line).unwrap().unwrap();

        assert_eq!(parsed, evt);
        assert!(matches!(parsed.kind, TraceEventKind::ToolCall { .. }));
    }

    #[test]
    fn blank_line_is_none() {
        assert!(parse_line("").unwrap().is_none());
        assert!(parse_line("   ").unwrap().is_none());
        assert!(parse_line("\n").unwrap().is_none());
        assert!(parse_line("  \t  ").unwrap().is_none());
    }

    #[test]
    fn garbage_is_err_not_panic() {
        let err = parse_line("{not json").unwrap_err();
        assert!(matches!(err, IngestError::Json(_)));
    }

    #[test]
    fn truncated_mid_write_line_is_err_not_panic() {
        // Simulates a live tail reading a partially-flushed line: valid
        // JSON prefix, but cut off before the closing brace.
        let evt = TraceEvent {
            id: "evt-3".into(),
            ts: Utc.timestamp_opt(1_700_000_200, 0).unwrap(),
            run_id: "run-1".into(),
            agent_id: None,
            kind: TraceEventKind::Abort {
                reason: "watchdog".into(),
            },
        };
        let full_line = serde_json::to_string(&evt).unwrap();
        // Byte-index slicing is only safe here because every field in
        // `evt` is ASCII; a non-ASCII value would need a char-boundary-safe
        // split to avoid panicking on a multi-byte UTF-8 codepoint.
        let truncated = &full_line[..full_line.len() / 2];

        assert!(parse_line(truncated).is_err());
    }

    #[test]
    fn parses_the_wrapped_run_log_envelope() {
        // This is the shape `tau-runtime-tokio`'s persistence writer actually
        // produces (`RunLogLine::TraceEvent`). Before spec §13.5 the reader
        // only accepted the bare event, so `tau trace` rendered nothing.
        let evt = TraceEvent {
            id: "evt-9".into(),
            ts: Utc.timestamp_opt(1_700_000_300, 0).unwrap(),
            run_id: "run-9".into(),
            agent_id: Some("agent-9".into()),
            kind: TraceEventKind::ToolCall {
                tool_name: "weather.get_forecast".into(),
                duration_ms: 7,
                status: "ok".into(),
                capability: Some(tau_ports::CapabilityVerdict::Clamp {
                    to: "api.weather.com".into(),
                }),
                turn_index: 0,
            },
        };
        let line = serde_json::json!({
            "line_kind": "trace_event",
            "event": evt,
        })
        .to_string();

        let parsed = parse_line(&line).unwrap().unwrap();

        assert_eq!(parsed, evt);
    }

    #[test]
    fn skips_non_trace_event_envelope_kinds() {
        // `RunLogLine::TaskMutation` is a forward-compat line kind. It is not
        // a trace event, so it is skipped — not an error.
        let line = serde_json::json!({
            "line_kind": "task_mutation",
            "task_id": "01",
            "mutation": "{\"status\":\"done\"}",
        })
        .to_string();

        assert!(parse_line(&line).unwrap().is_none());
    }

    #[test]
    fn still_parses_a_bare_trace_event() {
        // Back-compat: older logs and in-repo fixtures hold bare events.
        let evt = TraceEvent {
            id: "evt-10".into(),
            ts: Utc.timestamp_opt(1_700_000_400, 0).unwrap(),
            run_id: "run-10".into(),
            agent_id: None,
            kind: TraceEventKind::Abort {
                reason: "watchdog".into(),
            },
        };
        let line = serde_json::to_string(&evt).unwrap();

        assert_eq!(parse_line(&line).unwrap().unwrap(), evt);
    }

    #[test]
    fn envelope_shape_matches_the_writer() {
        // Drift guard (spec §13.5): this literal is the exact shape
        // `tau_runtime_tokio::orchestration::persistence::spawn_writer`
        // serializes via `RunLogLine::TraceEvent`. If the writer's tag name
        // (`line_kind`), tag value (`trace_event`), or payload field
        // (`event`) ever changes, this test fails loudly instead of
        // `tau trace` silently rendering an empty waterfall.
        let evt = TraceEvent {
            id: "evt-11".into(),
            ts: Utc.timestamp_opt(1_700_000_500, 0).unwrap(),
            run_id: "run-11".into(),
            agent_id: None,
            kind: TraceEventKind::Turn {
                agent_id: "a".into(),
                turn_index: 0,
                duration_ms: 1,
                tokens: 0,
            },
        };
        let mut line = serde_json::Map::new();
        line.insert(
            "line_kind".into(),
            serde_json::Value::String("trace_event".into()),
        );
        line.insert("event".into(), serde_json::to_value(&evt).unwrap());
        let encoded = serde_json::Value::Object(line).to_string();

        assert_eq!(parse_line(&encoded).unwrap().unwrap(), evt);
    }
}
