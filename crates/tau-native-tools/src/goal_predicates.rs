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
use serde_json::Value;

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
/// An uncompilable pattern is an ERROR, not a verdict. Reporting `met: false`
/// (the behaviour before #621's review) let an engine/feature mismatch between
/// this graph and the native one masquerade as "the content did not match",
/// silently sending a Branch down its `otherwise` arm on one target only.
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
        // A pattern that fails to compile HERE already compiled at authoring
        // time (project validation uses the full `regex` crate), so this is a
        // bug — an engine/feature mismatch between this graph and the native
        // one — not a verdict about the content. Reporting `met: false` would
        // silently send a Branch down its `otherwise` arm on one target only
        // (ADR-0068 cross-target parity); surface it as an error instead.
        Err(e) => Err(format!(
            "regex compile error for pattern {pattern:?}: {e} — the pattern \
             compiled at build time, so this build's regex-automata feature \
             set is narrower than the authoring one"
        )),
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

    /// Cross-target parity guard (ADR-0068, #621).
    ///
    /// This crate is the SINGLE source of the `matches` predicate for both
    /// the native registry (`tau-cli`) and the wasm guest — but the engine's
    /// language is decided by cargo FEATURE UNIFICATION, not by this file.
    /// In the `tau-cli` graph, `jsonschema → fancy-regex` (and `regex`) pull
    /// `regex-automata` up to the full Unicode feature set; the wasm guest
    /// links only what THIS crate declares. So a pattern class that is
    /// enabled there and not here compiles natively and fails to compile
    /// in-guest — and a compile failure used to be swallowed as
    /// `met: false`, silently flipping a Branch to its `otherwise` arm on
    /// wasm only.
    ///
    /// These cases are exactly the ones that need a feature beyond the
    /// original `unicode-case`/`unicode-perl` pair (`\b` needs
    /// `unicode-word-boundary`; `\p{…}` needs `unicode-gencat`/
    /// `unicode-script`). Running under `-p tau-native-tools`, the graph is
    /// this crate's own declaration — so trimming the feature list again
    /// fails HERE rather than diverging in production.
    #[test]
    fn matches_parses_the_same_language_the_native_graph_does() {
        for (pattern, content) in [
            (r"\b\w+\b", "two words"),
            (r"\p{L}+", "abc"),
            (r"\p{Greek}", "λ"),
            (r"(?i)\p{Lu}", "a"),
            (r"\d+", "42"),
            (r"\s", " "),
        ] {
            let got = invoke(
                FN_MATCHES,
                &json!({"present": true, "content": content, "pattern": pattern}),
            )
            .unwrap_or_else(|| panic!("matches must answer for {pattern:?}"));
            assert_eq!(
                got,
                Ok(json!(true)),
                "pattern {pattern:?} must compile AND match {content:?} under this \
                 crate's own feature set — otherwise the wasm guest silently \
                 disagrees with the native registry"
            );
        }
    }

    #[test]
    fn matches_bad_pattern_is_err() {
        // A pattern that reaches here uncompilable is a BUG, not a verdict:
        // project validation already compiled it with the full `regex` crate
        // at authoring time. Reporting `met: false` would silently take a
        // Branch's `otherwise` arm; an error surfaces the mismatch instead.
        let got = invoke(
            FN_MATCHES,
            &json!({"present": true, "content": "x", "pattern": "("}),
        )
        .expect("matches answers");
        let msg = got.expect_err("an uncompilable pattern must be an error, not met:false");
        assert!(
            msg.contains("regex compile error"),
            "error must name the compile failure; got {msg:?}"
        );
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
