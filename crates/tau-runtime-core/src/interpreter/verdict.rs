//! LLM judge verdict (D5). The judge is a one-shot agent whose final
//! text is parsed into this struct; an unparseable verdict is a soft
//! failure (`met = false`) with a diagnostic rationale, not a kernel
//! error — so a flaky judge re-prompts on retry rather than crashing the
//! run.

use alloc::string::String;
use serde::Deserialize;

/// A judge's structured verdict.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct Verdict {
    /// Did the artifact satisfy the criterion?
    pub(crate) met: bool,
    /// Why — fed back into the retry loop as the failure rationale.
    pub(crate) rationale: String,
}

/// Parse a judge's final assistant text into a [`Verdict`]. Tolerant of
/// surrounding prose: extracts the first balanced top-level `{...}` and
/// parses it. On any failure returns a `met = false` verdict carrying the
/// raw text as the rationale.
pub(crate) fn parse_verdict(text: &str) -> Verdict {
    if let Some(json) = first_json_object(text) {
        if let Ok(v) = serde_json::from_str::<Verdict>(json) {
            return v;
        }
    }
    Verdict {
        met: false,
        rationale: alloc::format!("judge returned unparseable verdict: {text}"),
    }
}

/// Return the first balanced top-level `{...}` substring, if any.
fn first_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let mut depth = 0usize;
    for (i, c) in s[start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..start + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_json() {
        let v = parse_verdict(r#"{"met": true, "rationale": "looks good"}"#);
        assert!(v.met);
        assert_eq!(v.rationale, "looks good");
    }

    #[test]
    fn parses_json_embedded_in_prose() {
        let v = parse_verdict("Here is my verdict:\n{\"met\": false, \"rationale\": \"only 1 source\"}\nThanks.");
        assert!(!v.met);
        assert_eq!(v.rationale, "only 1 source");
    }

    #[test]
    fn unparseable_is_soft_fail() {
        let v = parse_verdict("I think it is fine, honestly.");
        assert!(!v.met);
        assert!(v.rationale.contains("unparseable verdict"));
    }

    #[test]
    fn nested_braces_balanced() {
        let v = parse_verdict(r#"{"met": true, "rationale": "ok {nested} fine"}"#);
        assert!(v.met);
    }
}
