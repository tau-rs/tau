//! Malformed-tolerant jsonl line ingestion.
//!
//! [`parse_line`] turns one line of a `.tau/runs/<id>.jsonl` file into a
//! [`tau_ports::TraceEvent`]. A live tail (e.g. the TUI reading a file the
//! engine is still writing to) can observe a blank line or a half-written
//! (truncated) line at EOF — those are not errors, they just haven't
//! arrived yet, so they return `Ok(None)` rather than `Err`. Only a
//! *complete* line that fails to parse as JSON/`TraceEvent` is an error.

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

/// Parse one `.tau/runs/<id>.jsonl` line into a [`TraceEvent`].
///
/// Returns `Ok(None)` for a blank/whitespace-only line — a live tail may
/// read a half-written or trailing-newline line, and that must not be
/// treated as an error. Returns `Err` only for a non-empty line that fails
/// to deserialize as a `TraceEvent`.
pub fn parse_line(line: &str) -> Result<Option<TraceEvent>, IngestError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
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
        let truncated = &full_line[..full_line.len() / 2];

        assert!(parse_line(truncated).is_err());
    }
}
