//! Built-in goal predicate registry and `StdFsArtifactReader`.
//!
//! Answers all six `FN_BUILTIN_*` predicates from
//! `tau_runtime_core::vocabulary` and exposes a [`BuiltinDeterministicRegistry`]
//! that the production [`super::ir_dispatcher::ForwardingDispatcher`] wires in
//! via `ToolDispatcher::deterministic_registry()`.
//!
//! Only `schema_valid` is implemented HERE — it needs `jsonschema`, which is
//! std-only. The other five live in `tau_native_tools::goal_predicates` so
//! the wasm guest can answer the same predicates from the same source
//! (ADR-0068, #621); this registry delegates to them first and falls through
//! to the local `schema_valid`.
//!
//! The builtins are checked FIRST. Their names are `__tau::`-prefixed which
//! makes collision with user-registered fns structurally impossible.

use std::sync::Arc;

use jsonschema::{Draft, Validator};
use serde_json::{json, Value};
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::artifact::ArtifactReader;
use tau_runtime_core::interpreter::deterministic::DeterministicRegistry;
use tau_runtime_core::vocabulary::FN_BUILTIN_SCHEMA_VALID;

// ---------------------------------------------------------------------------
// StdFsArtifactReader
// ---------------------------------------------------------------------------

/// A [`ArtifactReader`] backed by `std::fs::read`.
///
/// `Ok(None)` is returned for paths that do not exist; other I/O errors
/// surface as [`RuntimeError::Internal`].
pub(crate) struct StdFsArtifactReader;

impl ArtifactReader for StdFsArtifactReader {
    fn read_path(&self, path: &str) -> Result<Option<Vec<u8>>, RuntimeError> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(RuntimeError::Internal {
                message: format!("read {path}: {e}"),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// BuiltinDeterministicRegistry
// ---------------------------------------------------------------------------

/// Production [`DeterministicRegistry`] that answers the six `FN_BUILTIN_*`
/// goal predicate names and returns [`RuntimeError::Internal`] for anything
/// else (user-registered fns are not yet wired here — they compose a separate
/// registry layer outside this struct in β.x).
///
/// Args contract (`args` is the JSON object built by `check.rs`):
/// ```json
/// { "present": bool, "content": string | null, ...predicate_params }
/// ```
pub(crate) struct BuiltinDeterministicRegistry;

impl DeterministicRegistry for BuiltinDeterministicRegistry {
    fn invoke(&self, fn_name: &str, args: &Value) -> Result<Value, RuntimeError> {
        if let Some(result) = tau_native_tools::goal_predicates::invoke(fn_name, args) {
            return result.map_err(|message| RuntimeError::Internal { message });
        }
        match fn_name {
            _ if fn_name == FN_BUILTIN_SCHEMA_VALID => builtin_schema_valid(args),
            other => Err(RuntimeError::Internal {
                message: format!("BuiltinDeterministicRegistry: unknown fn {other:?}"),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Predicate implementations
// ---------------------------------------------------------------------------

/// `__tau::goal::schema_valid` — validate `content` (parsed as JSON) against
/// `args["schema"]` using the `jsonschema` crate (already a workspace dep in
/// `tau-runtime-tokio` and available here via the workspace).
///
/// If `content` does not parse as JSON or fails validation, returns
/// `Value::Bool(false)`. On schema compilation error, returns
/// `{"met": false, "rationale": "<err>"}` defensively.
fn builtin_schema_valid(args: &Value) -> Result<Value, RuntimeError> {
    let present = args["present"].as_bool().unwrap_or(false);
    if !present {
        return Ok(Value::Bool(false));
    }
    let content = match args["content"].as_str() {
        Some(c) => c,
        None => return Ok(Value::Bool(false)),
    };
    let schema = &args["schema"];

    // Parse content as JSON.
    let instance: Value = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(_) => return Ok(Value::Bool(false)),
    };

    // Compile the JSON Schema (Draft 7, matching the tool-args validator elsewhere).
    let compiled: Validator = match jsonschema::options()
        .with_draft(Draft::Draft7)
        .build(schema)
    {
        Ok(c) => c,
        Err(e) => {
            return Ok(json!({
                "met": false,
                "rationale": format!("schema compile error: {e}")
            }));
        }
    };

    Ok(Value::Bool(compiled.is_valid(&instance)))
}

// ---------------------------------------------------------------------------
// Arc constructor helpers (used by ForwardingDispatcher)
// ---------------------------------------------------------------------------

pub(crate) fn make_artifact_reader() -> Arc<StdFsArtifactReader> {
    Arc::new(StdFsArtifactReader)
}

pub(crate) fn make_builtin_registry() -> Arc<BuiltinDeterministicRegistry> {
    Arc::new(BuiltinDeterministicRegistry)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tau_runtime_core::vocabulary::{
        FN_BUILTIN_EQUALS, FN_BUILTIN_EXISTS, FN_BUILTIN_MATCHES, FN_BUILTIN_MIN_COUNT,
        FN_BUILTIN_NON_EMPTY, FN_BUILTIN_SCHEMA_VALID,
    };

    fn reg() -> BuiltinDeterministicRegistry {
        BuiltinDeterministicRegistry
    }

    // -----------------------------------------------------------------------
    // exists
    // -----------------------------------------------------------------------

    #[test]
    fn exists_true_when_present() {
        let v = reg()
            .invoke(
                FN_BUILTIN_EXISTS,
                &json!({ "present": true, "content": "x" }),
            )
            .unwrap();
        assert_eq!(v, json!(true));
    }

    #[test]
    fn exists_false_when_absent() {
        let v = reg()
            .invoke(
                FN_BUILTIN_EXISTS,
                &json!({ "present": false, "content": null }),
            )
            .unwrap();
        assert_eq!(v, json!(false));
    }

    // -----------------------------------------------------------------------
    // non_empty
    // -----------------------------------------------------------------------

    #[test]
    fn non_empty_true_when_present_and_content() {
        let v = reg()
            .invoke(
                FN_BUILTIN_NON_EMPTY,
                &json!({ "present": true, "content": "hello" }),
            )
            .unwrap();
        assert_eq!(v, json!(true));
    }

    #[test]
    fn non_empty_false_when_absent() {
        let v = reg()
            .invoke(
                FN_BUILTIN_NON_EMPTY,
                &json!({ "present": false, "content": null }),
            )
            .unwrap();
        assert_eq!(v, json!(false));
    }

    #[test]
    fn non_empty_false_when_content_is_whitespace_only() {
        let v = reg()
            .invoke(
                FN_BUILTIN_NON_EMPTY,
                &json!({ "present": true, "content": "   \n  " }),
            )
            .unwrap();
        assert_eq!(v, json!(false));
    }

    // -----------------------------------------------------------------------
    // equals
    // -----------------------------------------------------------------------

    #[test]
    fn equals_true_on_match() {
        let v = reg()
            .invoke(
                FN_BUILTIN_EQUALS,
                &json!({ "present": true, "content": "foo", "equals": "foo" }),
            )
            .unwrap();
        assert_eq!(v, json!(true));
    }

    #[test]
    fn equals_false_on_mismatch() {
        let v = reg()
            .invoke(
                FN_BUILTIN_EQUALS,
                &json!({ "present": true, "content": "foo", "equals": "bar" }),
            )
            .unwrap();
        assert_eq!(v, json!(false));
    }

    #[test]
    fn equals_false_when_absent() {
        let v = reg()
            .invoke(
                FN_BUILTIN_EQUALS,
                &json!({ "present": false, "content": null, "equals": "foo" }),
            )
            .unwrap();
        assert_eq!(v, json!(false));
    }

    // -----------------------------------------------------------------------
    // matches
    // -----------------------------------------------------------------------

    #[test]
    fn matches_true_on_pattern_hit() {
        let v = reg()
            .invoke(
                FN_BUILTIN_MATCHES,
                &json!({
                    "present": true,
                    "content": "## Sources\n[a](http://example.com)",
                    "pattern": "(?m)^## Sources"
                }),
            )
            .unwrap();
        assert_eq!(v, json!(true));
    }

    #[test]
    fn matches_false_on_pattern_miss() {
        let v = reg()
            .invoke(
                FN_BUILTIN_MATCHES,
                &json!({
                    "present": true,
                    "content": "no heading here",
                    "pattern": "(?m)^## Sources"
                }),
            )
            .unwrap();
        assert_eq!(v, json!(false));
    }

    #[test]
    fn matches_false_when_absent() {
        let v = reg()
            .invoke(
                FN_BUILTIN_MATCHES,
                &json!({
                    "present": false,
                    "content": null,
                    "pattern": ".*"
                }),
            )
            .unwrap();
        assert_eq!(v, json!(false));
    }

    #[test]
    fn matches_bad_pattern_returns_met_false_with_rationale() {
        let v = reg()
            .invoke(
                FN_BUILTIN_MATCHES,
                &json!({
                    "present": true,
                    "content": "text",
                    "pattern": "(?P<x>unclosed"
                }),
            )
            .unwrap();
        // Should be an object with met=false (not an error).
        let obj = v.as_object().expect("expected object for bad pattern");
        assert_eq!(obj["met"].as_bool(), Some(false));
        assert!(obj.contains_key("rationale"));
    }

    // -----------------------------------------------------------------------
    // min_count
    // -----------------------------------------------------------------------

    #[test]
    fn min_count_true_on_boundary_exact() {
        let v = reg()
            .invoke(
                FN_BUILTIN_MIN_COUNT,
                &json!({
                    "present": true,
                    "content": "line 1\nline 2",
                    "min_count": 2
                }),
            )
            .unwrap();
        assert_eq!(v, json!(true));
    }

    #[test]
    fn min_count_false_below_boundary() {
        let v = reg()
            .invoke(
                FN_BUILTIN_MIN_COUNT,
                &json!({
                    "present": true,
                    "content": "line 1\nline 2",
                    "min_count": 3
                }),
            )
            .unwrap();
        assert_eq!(v, json!(false));
    }

    #[test]
    fn min_count_skips_empty_lines() {
        // 3 lines total but only 2 non-empty → min_count=2 passes, min_count=3 fails
        let v = reg()
            .invoke(
                FN_BUILTIN_MIN_COUNT,
                &json!({
                    "present": true,
                    "content": "line 1\n\nline 2",
                    "min_count": 2
                }),
            )
            .unwrap();
        assert_eq!(v, json!(true));

        let v2 = reg()
            .invoke(
                FN_BUILTIN_MIN_COUNT,
                &json!({
                    "present": true,
                    "content": "line 1\n\nline 2",
                    "min_count": 3
                }),
            )
            .unwrap();
        assert_eq!(v2, json!(false));
    }

    #[test]
    fn min_count_false_when_absent() {
        let v = reg()
            .invoke(
                FN_BUILTIN_MIN_COUNT,
                &json!({ "present": false, "content": null, "min_count": 1 }),
            )
            .unwrap();
        assert_eq!(v, json!(false));
    }

    // -----------------------------------------------------------------------
    // schema_valid (jsonschema crate — full JSON Schema validation)
    // -----------------------------------------------------------------------

    #[test]
    fn schema_valid_true_on_valid_json() {
        let v = reg()
            .invoke(
                FN_BUILTIN_SCHEMA_VALID,
                &json!({
                    "present": true,
                    "content": r#"{"name": "Alice", "age": 30}"#,
                    "schema": {
                        "type": "object",
                        "required": ["name", "age"],
                        "properties": {
                            "name": { "type": "string" },
                            "age":  { "type": "integer" }
                        }
                    }
                }),
            )
            .unwrap();
        assert_eq!(v, json!(true));
    }

    #[test]
    fn schema_valid_false_on_missing_required_key() {
        let v = reg()
            .invoke(
                FN_BUILTIN_SCHEMA_VALID,
                &json!({
                    "present": true,
                    "content": r#"{"name": "Alice"}"#,
                    "schema": {
                        "type": "object",
                        "required": ["name", "age"]
                    }
                }),
            )
            .unwrap();
        assert_eq!(v, json!(false));
    }

    #[test]
    fn schema_valid_false_when_content_is_not_json() {
        let v = reg()
            .invoke(
                FN_BUILTIN_SCHEMA_VALID,
                &json!({
                    "present": true,
                    "content": "not json at all",
                    "schema": { "type": "object" }
                }),
            )
            .unwrap();
        assert_eq!(v, json!(false));
    }

    #[test]
    fn schema_valid_false_when_absent() {
        let v = reg()
            .invoke(
                FN_BUILTIN_SCHEMA_VALID,
                &json!({
                    "present": false,
                    "content": null,
                    "schema": { "type": "object" }
                }),
            )
            .unwrap();
        assert_eq!(v, json!(false));
    }

    // -----------------------------------------------------------------------
    // unknown fn → error
    // -----------------------------------------------------------------------

    #[test]
    fn unknown_fn_returns_internal_error() {
        let err = reg()
            .invoke("user::my_fn", &json!({}))
            .expect_err("unknown fn must error");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("unknown fn"),
            "expected unknown-fn diagnostic in {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // StdFsArtifactReader (filesystem smoke tests)
    // -----------------------------------------------------------------------

    #[test]
    fn std_fs_reader_returns_none_for_missing_path() {
        let reader = StdFsArtifactReader;
        let result = reader
            .read_path("/tmp/__tau_test_definitely_missing_file_xyz_123")
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn std_fs_reader_reads_existing_file() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        tmp.write_all(b"hello tau").expect("write");
        let path = tmp.path().to_str().expect("path str");
        let reader = StdFsArtifactReader;
        let bytes = reader.read_path(path).unwrap().expect("should be Some");
        assert_eq!(bytes, b"hello tau");
    }
}
