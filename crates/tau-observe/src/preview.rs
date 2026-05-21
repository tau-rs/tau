//! Sensitive-data preview helpers (ADR-0006 §3.9 discipline).
//!
//! The kernel emits structured logs containing arguments, message
//! payloads, and LLM responses. Per §3.9 these bodies are previewed
//! (256 bytes, UTF-8-boundary-clipped) at `DEBUG` and below; full
//! content is emitted only at `TRACE`. These helpers make that policy
//! mechanical at every call site.
//!
//! Per ADR-0006 NG9 ("tau does not redact for the caller") this module
//! is **kernel-internal**. Plugin authors may use it but are not
//! required to.

use std::fmt::{self, Display, Formatter};

const PREVIEW_LIMIT_BYTES: usize = 256;

/// Render a `&str` truncated to at most 256 bytes ending on a UTF-8
/// boundary, with a `"…"` ellipsis if truncation occurred.
///
/// Use at `DEBUG` (and below) call sites for argument / payload /
/// message content.
pub fn preview(value: &str) -> impl Display + '_ {
    Preview(value)
}

struct Preview<'a>(&'a str);

impl Display for Preview<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let s = self.0;
        if s.len() <= PREVIEW_LIMIT_BYTES {
            f.write_str(s)
        } else {
            let mut cut = PREVIEW_LIMIT_BYTES;
            while cut > 0 && !s.is_char_boundary(cut) {
                cut -= 1;
            }
            f.write_str(&s[..cut])?;
            f.write_str("…")
        }
    }
}

/// Render a `serde_json::Value` as compact JSON, truncated to at most
/// 256 bytes ending on a UTF-8 boundary, with a `"…"` ellipsis if
/// truncation occurred.
///
/// Use at `DEBUG` (and below) call sites for JSON-shaped payloads.
pub fn preview_json(value: &serde_json::Value) -> impl Display + '_ {
    PreviewJson(value)
}

struct PreviewJson<'a>(&'a serde_json::Value);

impl Display for PreviewJson<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let s = serde_json::to_string(&self.0).unwrap_or_else(|_| "<unserializable>".to_string());
        write!(f, "{}", Preview(s.as_str()))
    }
}

/// Render a `&str` in full.
///
/// **Only call this at `tracing::trace!` (or below) sites.** At any
/// higher level, the macros emit the event unconditionally (subject to
/// the filter) and the full content gets persisted. Use [`preview`]
/// instead for DEBUG and above.
pub fn full(value: &str) -> impl Display + '_ {
    Full(value)
}

struct Full<'a>(&'a str);

impl Display for Full<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// Render a `serde_json::Value` as compact JSON in full.
///
/// Same rule as [`full`]: TRACE-only call sites.
pub fn full_json(value: &serde_json::Value) -> impl Display + '_ {
    FullJson(value)
}

struct FullJson<'a>(&'a serde_json::Value);

impl Display for FullJson<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let s = serde_json::to_string(&self.0).unwrap_or_else(|_| "<unserializable>".to_string());
        f.write_str(&s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_string_passes_through_verbatim() {
        let s = "hello";
        assert_eq!(format!("{}", preview(s)), "hello");
    }

    #[test]
    fn empty_string_passes_through() {
        assert_eq!(format!("{}", preview("")), "");
    }

    #[test]
    fn exactly_256_bytes_no_truncation() {
        let s = "a".repeat(256);
        assert_eq!(format!("{}", preview(&s)), s);
    }

    #[test]
    fn over_256_bytes_truncates_with_ellipsis() {
        let s = "a".repeat(300);
        let out = format!("{}", preview(&s));
        assert!(out.ends_with('…'));
        let body = out.trim_end_matches('…');
        assert!(body.len() <= 256, "body was {} bytes", body.len());
    }

    #[test]
    fn truncation_respects_utf8_boundary_at_3_byte_codepoint() {
        let mut s = "a".repeat(254);
        s.push('€');
        s.push_str(&"b".repeat(50));
        let out = format!("{}", preview(&s));
        let body = out.trim_end_matches('…');
        assert!(
            std::str::from_utf8(body.as_bytes()).is_ok(),
            "invalid UTF-8 in preview"
        );
    }

    #[test]
    fn truncation_respects_utf8_boundary_at_4_byte_codepoint() {
        let mut s = "a".repeat(253);
        s.push('𝄞');
        s.push_str(&"b".repeat(50));
        let out = format!("{}", preview(&s));
        let body = out.trim_end_matches('…');
        assert!(
            std::str::from_utf8(body.as_bytes()).is_ok(),
            "invalid UTF-8 in preview"
        );
    }

    #[test]
    fn preview_json_short_value_passes_through() {
        let v = serde_json::json!({"name": "ada"});
        let out = format!("{}", preview_json(&v));
        assert_eq!(out, r#"{"name":"ada"}"#);
    }

    #[test]
    fn preview_json_long_value_truncates() {
        let v = serde_json::json!({"data": "x".repeat(500)});
        let out = format!("{}", preview_json(&v));
        assert!(out.ends_with('…'));
        let body = out.trim_end_matches('…');
        assert!(body.len() <= 256);
    }

    #[test]
    fn full_returns_string_verbatim_regardless_of_length() {
        let s = "x".repeat(1000);
        assert_eq!(format!("{}", full(&s)), s);
    }

    #[test]
    fn full_json_returns_value_verbatim_regardless_of_size() {
        let v = serde_json::json!({"data": "x".repeat(1000)});
        let out = format!("{}", full_json(&v));
        assert!(out.contains(&"x".repeat(1000)));
    }
}
