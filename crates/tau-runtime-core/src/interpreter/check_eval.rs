//! Deterministic goal-predicate evaluation (C1). Pure over
//! already-materialized locus bytes — the caller (pipeline executor)
//! reads the artifact or named output first, then calls these.

use alloc::format;
use alloc::string::{String, ToString};

use tau_ir::Predicate;

/// Evaluate a goal predicate against the materialized locus bytes.
///
/// `bytes` is `None` when the locus does not exist (e.g. a missing file
/// or an absent named output). Returns `(passed, rationale)`; on pass the
/// rationale is a short confirmation, on fail it explains why.
///
/// Does NOT handle [`Predicate::NativeFn`] — that variant is dispatched
/// through the `DeterministicRegistry` by the pipeline executor (C1.3),
/// never here; this returns `(false, ...)` defensively if reached.
pub(crate) fn eval_predicate(pred: &Predicate, bytes: Option<&[u8]>) -> (bool, String) {
    match pred {
        Predicate::Exists => match bytes {
            Some(_) => (true, "locus exists".to_string()),
            None => (false, "locus does not exist".to_string()),
        },
        Predicate::NonEmpty => match bytes {
            Some(b) if !b.is_empty() => (true, "locus is non-empty".to_string()),
            Some(_) => (false, "locus is empty".to_string()),
            None => (false, "locus does not exist".to_string()),
        },
        Predicate::Equals(expected) => match bytes {
            Some(b) => {
                let actual = String::from_utf8_lossy(b);
                if actual == expected.as_str() {
                    (true, "locus equals expected".to_string())
                } else {
                    (false, "locus does not equal expected value".to_string())
                }
            }
            None => (false, "locus does not exist".to_string()),
        },
        Predicate::Matches(pattern) => match_count(pattern, bytes).map_or_else(
            |e| (false, e),
            |(count, _)| {
                if count >= 1 {
                    (true, "pattern matched".to_string())
                } else {
                    (false, format!("pattern {pattern:?} did not match"))
                }
            },
        ),
        Predicate::MinCount { pattern, min } => match_count(pattern, bytes).map_or_else(
            |e| (false, e),
            |(count, _)| {
                if count as u64 >= *min {
                    (true, format!("{count} matches >= {min}"))
                } else {
                    (false, format!("{count} matches of {pattern:?}, need >= {min}"))
                }
            },
        ),
        Predicate::SchemaValid(schema) => match bytes {
            Some(b) => match serde_json::from_slice::<serde_json::Value>(b) {
                Ok(instance) => match jsonschema::options().build(schema) {
                    Ok(validator) => {
                        if validator.is_valid(&instance) {
                            (true, "instance is schema-valid".to_string())
                        } else {
                            (false, "instance does not satisfy schema".to_string())
                        }
                    }
                    Err(e) => (false, format!("invalid schema: {e}")),
                },
                Err(e) => (false, format!("locus is not valid JSON: {e}")),
            },
            None => (false, "locus does not exist".to_string()),
        },
        Predicate::NativeFn(_) => (
            false,
            "native-fn predicate must be dispatched via the deterministic registry".to_string(),
        ),
    }
}

/// Count regex matches in the (utf8-lossy) bytes. Returns `Err(message)` on
/// missing locus or invalid regex.
fn match_count(pattern: &str, bytes: Option<&[u8]>) -> Result<(usize, ()), String> {
    let b = bytes.ok_or_else(|| "locus does not exist".to_string())?;
    let text = String::from_utf8_lossy(b);
    let re = regex::Regex::new(pattern)
        .map_err(|e| format!("invalid regex {pattern:?}: {e}"))?;
    Ok((re.find_iter(&text).count(), ()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_true_and_false() {
        let (ok, _) = eval_predicate(
            &Predicate::Matches("(?m)^## Sources".to_string()),
            Some(b"## Sources\nfoo"),
        );
        assert!(ok);
        let (ok2, _) = eval_predicate(
            &Predicate::Matches("(?m)^## Sources".to_string()),
            Some(b"no header"),
        );
        assert!(!ok2);
    }

    #[test]
    fn non_empty() {
        assert!(eval_predicate(&Predicate::NonEmpty, Some(b"x")).0);
        assert!(!eval_predicate(&Predicate::NonEmpty, Some(b"")).0);
        assert!(!eval_predicate(&Predicate::NonEmpty, None).0);
    }

    #[test]
    fn min_count_boundary() {
        let p = Predicate::MinCount {
            pattern: "a".to_string(),
            min: 2,
        };
        assert!(!eval_predicate(&p, Some(b"a")).0);
        assert!(eval_predicate(&p, Some(b"aa")).0);
    }

    #[test]
    fn exists_and_missing() {
        assert!(eval_predicate(&Predicate::Exists, Some(b"")).0);
        assert!(!eval_predicate(&Predicate::Exists, None).0);
    }

    #[test]
    fn equals_match_and_mismatch() {
        let p = Predicate::Equals("hello".to_string());
        assert!(eval_predicate(&p, Some(b"hello")).0);
        assert!(!eval_predicate(&p, Some(b"world")).0);
        assert!(!eval_predicate(&p, None).0);
    }

    #[test]
    fn schema_valid_pass_and_fail() {
        let schema = serde_json::json!({ "type": "object", "required": ["name"] });
        let p = Predicate::SchemaValid(schema);
        assert!(eval_predicate(&p, Some(br#"{"name":"tau"}"#)).0);
        assert!(!eval_predicate(&p, Some(br#"{"other":"x"}"#)).0);
        assert!(!eval_predicate(&p, Some(b"not-json")).0);
        assert!(!eval_predicate(&p, None).0);
    }

    #[test]
    fn native_fn_is_rejected() {
        let (ok, msg) = eval_predicate(&Predicate::NativeFn("check_links".to_string()), Some(b"x"));
        assert!(!ok);
        assert!(msg.contains("deterministic registry"));
    }

    #[test]
    fn matches_missing_locus() {
        let (ok, msg) = eval_predicate(&Predicate::Matches("foo".to_string()), None);
        assert!(!ok);
        assert!(msg.contains("does not exist"));
    }

    #[test]
    fn invalid_regex_error() {
        let (ok, msg) = eval_predicate(&Predicate::Matches("[invalid".to_string()), Some(b"text"));
        assert!(!ok);
        assert!(msg.contains("invalid regex"));
    }
}
