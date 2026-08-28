//! no_std goal-predicate registry — ported verbatim from
//! `tau-cli/src/cmd/builtin_registry.rs`'s five `builtin_*` bodies that do not
//! require `std` (`schema_valid` needs the `jsonschema` crate, which is std,
//! and stays in `tau-cli`).
//!
//! Behind the `goal-predicates` feature so callers that don't need control
//! flow (e.g. today's `tau-native-tools` consumers) don't pay for
//! `regex-automata`.
//!
//! Args contract (`args` is the JSON object built by the caller): matches the
//! CLI registry byte-for-byte —
//! ```json
//! { "present": bool, "content": string | null, ...predicate_params }
//! ```

use alloc::format;
use alloc::string::String;
use serde_json::{json, Value};

pub const FN_EXISTS: &str = "__tau::goal::exists";
pub const FN_NON_EMPTY: &str = "__tau::goal::non_empty";
pub const FN_EQUALS: &str = "__tau::goal::equals";
pub const FN_MATCHES: &str = "__tau::goal::matches";
pub const FN_MIN_COUNT: &str = "__tau::goal::min_count";

/// The five predicate fn names answerable without std.
pub const SUPPORTED: &[&str; 5] = &[FN_EXISTS, FN_NON_EMPTY, FN_EQUALS, FN_MATCHES, FN_MIN_COUNT];

/// Dispatch a goal-predicate fn by name.
///
/// `None` = this crate does not answer `fn_name` (e.g. `schema_valid`, or an
/// unknown fn). `Some(Err(msg))` = malformed args (missing "pattern" or
/// "min_count").
pub fn invoke(fn_name: &str, args: &Value) -> Option<Result<Value, String>> {
    match fn_name {
        FN_EXISTS => Some(exists(args)),
        FN_NON_EMPTY => Some(non_empty(args)),
        FN_EQUALS => Some(equals(args)),
        FN_MATCHES => Some(matches_(args)),
        FN_MIN_COUNT => Some(min_count(args)),
        _ => None,
    }
}

/// `__tau::goal::exists` → `present`.
fn exists(args: &Value) -> Result<Value, String> {
    let present = args["present"].as_bool().unwrap_or(false);
    Ok(Value::Bool(present))
}

/// `__tau::goal::non_empty` → `present && !content.trim().is_empty()`.
///
/// `content` null or absent → false.
fn non_empty(args: &Value) -> Result<Value, String> {
    let present = args["present"].as_bool().unwrap_or(false);
    let content = args["content"].as_str().unwrap_or("");
    Ok(Value::Bool(present && !content.trim().is_empty()))
}

/// `__tau::goal::equals` → `present && content == args["equals"]`.
fn equals(args: &Value) -> Result<Value, String> {
    let present = args["present"].as_bool().unwrap_or(false);
    let content = args["content"].as_str();
    let expected = args["equals"].as_str();
    Ok(Value::Bool(
        present && content.is_some() && content == expected,
    ))
}

/// `__tau::goal::matches` → `present && Regex::new(pattern)?.is_match(content)`.
///
/// Build-time proves the pattern compiles (Task 6), but if somehow a bad
/// pattern reaches here, return `{"met": false, "rationale": "<err>"}` rather
/// than surfacing an error that aborts the run.
fn matches_(args: &Value) -> Result<Value, String> {
    let present = args["present"].as_bool().unwrap_or(false);
    if !present {
        return Ok(Value::Bool(false));
    }
    let content = match args["content"].as_str() {
        Some(c) => c,
        None => return Ok(Value::Bool(false)),
    };
    let pattern = match args["pattern"].as_str() {
        Some(p) => p,
        None => return Err("FN_MATCHES: missing \"pattern\" field in args".into()),
    };
    match regex_automata::meta::Regex::new(pattern) {
        Ok(re) => Ok(Value::Bool(re.is_match(content))),
        Err(e) => {
            // Defensive: bad pattern at runtime → met=false with rationale.
            // Build-time validation (Task 6) should have caught this earlier.
            Ok(json!({
                "met": false,
                "rationale": format!("regex compile error for pattern {pattern:?}: {e}")
            }))
        }
    }
}

/// `__tau::goal::min_count` → count of non-empty lines in `content >= min_count`.
///
/// "Items" are non-empty lines (lines that are non-whitespace after trim).
/// `content` absent → false.
fn min_count(args: &Value) -> Result<Value, String> {
    let present = args["present"].as_bool().unwrap_or(false);
    if !present {
        return Ok(Value::Bool(false));
    }
    let content = match args["content"].as_str() {
        Some(c) => c,
        None => return Ok(Value::Bool(false)),
    };
    let min_count = args["min_count"]
        .as_u64()
        .ok_or_else(|| String::from("FN_MIN_COUNT: missing or non-integer \"min_count\" field"))?;
    let count = content.lines().filter(|l| !l.trim().is_empty()).count();
    Ok(Value::Bool(count as u64 >= min_count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exists_returns_present() {
        assert_eq!(
            invoke(FN_EXISTS, &json!({"present": true})),
            Some(Ok(json!(true)))
        );
        assert_eq!(
            invoke(FN_EXISTS, &json!({"present": false})),
            Some(Ok(json!(false)))
        );
    }

    #[test]
    fn non_empty_requires_present_and_nonblank() {
        assert_eq!(
            invoke(FN_NON_EMPTY, &json!({"present": true, "content": "hi"})),
            Some(Ok(json!(true)))
        );
        assert_eq!(
            invoke(FN_NON_EMPTY, &json!({"present": true, "content": "  "})),
            Some(Ok(json!(false)))
        );
        assert_eq!(
            invoke(FN_NON_EMPTY, &json!({"present": false, "content": "hi"})),
            Some(Ok(json!(false)))
        );
    }

    #[test]
    fn equals_compares_literal() {
        assert_eq!(
            invoke(
                FN_EQUALS,
                &json!({"present": true, "content": "a", "equals": "a"})
            ),
            Some(Ok(json!(true)))
        );
        assert_eq!(
            invoke(
                FN_EQUALS,
                &json!({"present": true, "content": "a", "equals": "b"})
            ),
            Some(Ok(json!(false)))
        );
    }

    #[test]
    fn matches_supports_case_insensitive_regex() {
        // The north-star fixture's exact patterns.
        assert_eq!(
            invoke(
                FN_MATCHES,
                &json!({"present": true, "content": "URGENT: fan", "pattern": "(?i)urgent"})
            ),
            Some(Ok(json!(true)))
        );
        assert_eq!(
            invoke(
                FN_MATCHES,
                &json!({"present": true, "content": "draft APPROVED", "pattern": "APPROVED"})
            ),
            Some(Ok(json!(true)))
        );
        assert_eq!(
            invoke(
                FN_MATCHES,
                &json!({"present": true, "content": "routine", "pattern": "(?i)urgent"})
            ),
            Some(Ok(json!(false)))
        );
        assert_eq!(
            invoke(
                FN_MATCHES,
                &json!({"present": false, "content": "URGENT", "pattern": "URGENT"})
            ),
            Some(Ok(json!(false)))
        );
    }

    #[test]
    fn matches_bad_pattern_is_met_false_with_rationale() {
        let got = invoke(
            FN_MATCHES,
            &json!({"present": true, "content": "x", "pattern": "("}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(got["met"], json!(false));
        assert!(got["rationale"]
            .as_str()
            .unwrap()
            .contains("regex compile error"));
    }

    #[test]
    fn matches_missing_pattern_is_err() {
        assert!(matches!(
            invoke(FN_MATCHES, &json!({"present": true, "content": "x"})),
            Some(Err(_))
        ));
    }

    #[test]
    fn min_count_counts_nonempty_lines() {
        assert_eq!(
            invoke(
                FN_MIN_COUNT,
                &json!({"present": true, "content": "a\n\nb", "min_count": 2})
            ),
            Some(Ok(json!(true)))
        );
        assert_eq!(
            invoke(
                FN_MIN_COUNT,
                &json!({"present": true, "content": "a", "min_count": 2})
            ),
            Some(Ok(json!(false)))
        );
    }

    #[test]
    fn schema_valid_and_unknown_are_none() {
        assert_eq!(invoke("__tau::goal::schema_valid", &json!({})), None);
        assert_eq!(invoke("nope", &json!({})), None);
    }
}
