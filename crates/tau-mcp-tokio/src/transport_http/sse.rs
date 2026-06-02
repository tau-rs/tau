//! Hand-rolled SSE frame parser for Streamable HTTP MCP responses.
//!
//! Per MCP spec rev 2025-03-26: each event is `data: <JSON-RPC>\n\n`.
//! No event names, no IDs, no retries — MCP handles reconnection at
//! the protocol level via session IDs. The parser is therefore tiny:
//! split on blank lines, strip the `data: ` prefix, parse JSON-RPC.

use tau_mcp::protocol::JsonRpcMessage;

use crate::transport_http::error::HttpTransportError;

/// Accumulates SSE bytes and emits complete JSON-RPC messages once
/// `\n\n` event boundaries land.
#[derive(Debug, Default)]
pub struct SseFramer {
    buf: String,
}

impl SseFramer {
    /// Construct a fresh framer.
    pub fn new() -> Self {
        Self {
            buf: String::new(),
        }
    }

    /// Feed a chunk of bytes, returning any complete messages parsed
    /// out of the accumulated buffer.
    pub fn feed_bytes(
        &mut self,
        chunk: &[u8],
    ) -> Result<Vec<JsonRpcMessage>, HttpTransportError> {
        // SSE is text per spec — utf-8 only. Append, then scan for
        // event boundaries.
        let s = std::str::from_utf8(chunk)
            .map_err(|e| HttpTransportError::SseParse(format!("non-utf8 SSE chunk: {e}")))?;
        self.buf.push_str(s);

        let mut messages = Vec::new();
        loop {
            // Look for the next event boundary (`\n\n` or `\r\n\r\n`).
            let boundary = self
                .buf
                .find("\n\n")
                .map(|i| (i, 2))
                .or_else(|| self.buf.find("\r\n\r\n").map(|i| (i, 4)));
            let Some((idx, sep_len)) = boundary else {
                // No complete event yet — keep buffering.
                return Ok(messages);
            };
            let event = self.buf[..idx].to_string();
            self.buf.drain(..idx + sep_len);
            if let Some(msg) = parse_event_block(&event)? {
                messages.push(msg);
            }
        }
    }

    /// Drain the accumulated buffer as a final event (useful after EOF
    /// if the server didn't append a trailing `\n\n`).
    pub fn flush(&mut self) -> Result<Option<JsonRpcMessage>, HttpTransportError> {
        let event = std::mem::take(&mut self.buf);
        let trimmed = event.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            return Ok(None);
        }
        parse_event_block(trimmed)
    }
}

/// Parse one SSE event block (without the trailing `\n\n`) into an
/// optional `JsonRpcMessage`. Returns `Ok(None)` for keep-alive
/// comments (lines starting with `:`).
pub fn parse_event_block(
    block: &str,
) -> Result<Option<JsonRpcMessage>, HttpTransportError> {
    // Collect data: lines (SSE allows multi-line data fields joined by
    // `\n`). MCP only uses one data: line per event, but parse robustly.
    let mut data = String::new();
    for line in block.lines() {
        if line.is_empty() {
            continue;
        }
        if line.starts_with(':') {
            // SSE comment / keep-alive — ignore.
            continue;
        }
        let Some(rest) = line.strip_prefix("data:") else {
            // Non-data field (event:, id:, retry:) — MCP doesn't use
            // these; ignore per SSE spec.
            continue;
        };
        // SSE allows an optional space after `:` — strip one if present.
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        if !data.is_empty() {
            data.push('\n');
        }
        data.push_str(rest);
    }
    if data.is_empty() {
        return Ok(None);
    }
    let msg: JsonRpcMessage = serde_json::from_str(&data)?;
    Ok(Some(msg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_mcp::protocol::jsonrpc::{
        JsonRpcMessage, JsonRpcResponse, RequestId, JSONRPC_VERSION,
    };

    fn response_msg(id: i64) -> JsonRpcMessage {
        JsonRpcMessage::Response(JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::Number(id),
            result: Some(serde_json::json!({"ok": true})),
            error: None,
        })
    }

    #[test]
    fn parses_single_event() {
        let mut f = SseFramer::new();
        let msg = response_msg(1);
        let line = format!("data: {}\n\n", serde_json::to_string(&msg).unwrap());
        let parsed = f.feed_bytes(line.as_bytes()).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0], msg);
    }

    #[test]
    fn parses_two_events_in_one_chunk() {
        let mut f = SseFramer::new();
        let m1 = response_msg(1);
        let m2 = response_msg(2);
        let bytes = format!(
            "data: {}\n\ndata: {}\n\n",
            serde_json::to_string(&m1).unwrap(),
            serde_json::to_string(&m2).unwrap()
        );
        let parsed = f.feed_bytes(bytes.as_bytes()).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], m1);
        assert_eq!(parsed[1], m2);
    }

    #[test]
    fn handles_event_split_across_feed_calls() {
        let mut f = SseFramer::new();
        let msg = response_msg(7);
        let line = format!("data: {}\n\n", serde_json::to_string(&msg).unwrap());
        let (a, b) = line.split_at(line.len() / 2);
        let parsed_a = f.feed_bytes(a.as_bytes()).unwrap();
        assert!(parsed_a.is_empty(), "first chunk should not yield events");
        let parsed_b = f.feed_bytes(b.as_bytes()).unwrap();
        assert_eq!(parsed_b.len(), 1);
        assert_eq!(parsed_b[0], msg);
    }

    #[test]
    fn skips_keepalive_comments() {
        let mut f = SseFramer::new();
        let msg = response_msg(3);
        let bytes = format!(
            ": keepalive\n\ndata: {}\n\n",
            serde_json::to_string(&msg).unwrap()
        );
        let parsed = f.feed_bytes(bytes.as_bytes()).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0], msg);
    }

    #[test]
    fn malformed_json_errors() {
        let mut f = SseFramer::new();
        let bytes = b"data: not json\n\n";
        let err = f.feed_bytes(bytes).expect_err("should error");
        assert!(matches!(err, HttpTransportError::JsonDecode(_)));
    }

    #[test]
    fn flush_drains_buffer_without_trailing_newlines() {
        let mut f = SseFramer::new();
        let msg = response_msg(9);
        let line = format!("data: {}", serde_json::to_string(&msg).unwrap());
        // No \n\n — server abruptly ended the stream.
        let parsed = f.feed_bytes(line.as_bytes()).unwrap();
        assert!(parsed.is_empty());
        let final_msg = f.flush().unwrap().expect("flush yields one message");
        assert_eq!(final_msg, msg);
    }
}
