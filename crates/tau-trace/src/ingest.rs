//! Malformed-tolerant jsonl line ingestion.
//!
//! [`parse_line`] turns one line of a `.tau/runs/<id>.jsonl` file into a
//! [`tau_ports::TraceEvent`]. Only a blank/whitespace-only line returns
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

/// Parse one `.tau/runs/<id>.jsonl` line into a [`TraceEvent`].
///
/// Returns `Ok(None)` only for a blank/whitespace-only line (e.g. a
/// trailing newline, or a live tail reading past the last complete line).
/// Returns `Err` for any non-empty line that fails to deserialize as a
/// `TraceEvent` — this includes a half-written/truncated line read
/// mid-write, which is *not* treated as `Ok(None)`. A live-tail caller
/// should treat such an `Err` on the still-growing tail line as retryable.
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
}
