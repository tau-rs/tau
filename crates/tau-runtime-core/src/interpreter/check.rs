//! Check evaluation: `evaluate_goal` (deterministic predicate via registry).
//!
//! Task 18 will extend this file with `evaluate_deliverable` (existence floor
//! + LLM judge). Task 19 wires both into `run_pipeline`.

use alloc::format;
use alloc::string::{String, ToString};

use serde_json::{json, Value};
use tau_ir::check::{GoalPredicate, Locus};

use crate::error::RuntimeError;
use crate::interpreter::artifact::ArtifactReader;
use crate::interpreter::deterministic::DeterministicRegistry;
use crate::interpreter::output_store::OutputStore;
use crate::vocabulary::{
    FN_BUILTIN_EQUALS, FN_BUILTIN_EXISTS, FN_BUILTIN_MATCHES, FN_BUILTIN_MIN_COUNT,
    FN_BUILTIN_NON_EMPTY, FN_BUILTIN_SCHEMA_VALID,
};

/// Outcome of evaluating a postcondition check.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckVerdict {
    /// Whether the postcondition held.
    pub met: bool,
    /// Human-readable rationale (load-bearing — fed into the retry loop).
    pub rationale: String,
}

/// Resolve a locus to `(present, content)`.
///
/// Returns `(true, Some(string))` when the locus resolves to content,
/// or `(false, None)` when the path/step-output does not exist.
pub(crate) fn resolve_locus(
    locus: &Locus,
    store: &OutputStore,
    reader: Option<&dyn ArtifactReader>,
) -> Result<(bool, Option<String>), RuntimeError> {
    match locus {
        Locus::Output(step_id) => Ok(match store.get(&step_id.0) {
            Some(v) => (true, Some(value_to_string(v))),
            None => (false, None),
        }),
        Locus::Path(p) => {
            let r = reader.ok_or_else(|| RuntimeError::Internal {
                message: format!("check needs an artifact reader to read {p}"),
            })?;
            Ok(match r.read_path(p)? {
                Some(bytes) => (true, Some(String::from_utf8_lossy(&bytes).into_owned())),
                None => (false, None),
            })
        }
    }
}

/// Map a `GoalPredicate` to `(fn_name, extra_args_object)`.
fn predicate_call(p: &GoalPredicate) -> (&str, Value) {
    match p {
        GoalPredicate::Exists => (FN_BUILTIN_EXISTS, json!({})),
        GoalPredicate::NonEmpty => (FN_BUILTIN_NON_EMPTY, json!({})),
        GoalPredicate::Equals(x) => (FN_BUILTIN_EQUALS, json!({ "equals": x })),
        GoalPredicate::Matches(x) => (FN_BUILTIN_MATCHES, json!({ "pattern": x })),
        GoalPredicate::MinCount(n) => (FN_BUILTIN_MIN_COUNT, json!({ "min_count": n })),
        GoalPredicate::SchemaValid(v) => (FN_BUILTIN_SCHEMA_VALID, json!({ "schema": v })),
        GoalPredicate::NativeFn(fn_ref) => (fn_ref.name.as_str(), json!({})),
    }
}

/// Convert an `OutputStore` value to a string, mirroring `OutputStore::template_map`.
///
/// `Value::String` passes through; all other values are compact-JSON encoded.
pub(crate) fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Normalize a registry response into a `CheckVerdict`.
///
/// Accepts:
/// - `Value::Bool(b)` — synthesizes a rationale.
/// - An object `{"met": bool, "rationale": "..."}` — uses those fields.
/// - Anything else — treat as not-met with a synthesized rationale.
fn normalize_verdict(raw: Value, predicate: &GoalPredicate) -> CheckVerdict {
    match raw {
        Value::Bool(b) => CheckVerdict {
            met: b,
            rationale: if b {
                format!("goal predicate {predicate:?} returned true")
            } else {
                format!("goal predicate {predicate:?} returned false")
            },
        },
        Value::Object(ref map) => {
            let met = map.get("met").and_then(|v| v.as_bool()).unwrap_or(false);
            let rationale = map
                .get("rationale")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    if met {
                        format!("goal predicate {predicate:?} returned true")
                    } else {
                        format!("goal predicate {predicate:?} returned false")
                    }
                });
            CheckVerdict { met, rationale }
        }
        other => CheckVerdict {
            met: false,
            rationale: format!("goal predicate {predicate:?} returned unexpected shape: {other}"),
        },
    }
}

/// Evaluate a goal predicate via the deterministic registry.
///
/// Resolves the locus to `(present, content)`, builds a JSON args object
/// `{ present, content, ...predicate_params }`, invokes the predicate fn
/// through `registry`, and normalizes the result into a [`CheckVerdict`].
pub fn evaluate_goal(
    evaluates: &Locus,
    predicate: &GoalPredicate,
    store: &OutputStore,
    reader: Option<&dyn ArtifactReader>,
    registry: &dyn DeterministicRegistry,
) -> Result<CheckVerdict, RuntimeError> {
    let (present, content) = resolve_locus(evaluates, store, reader)?;
    let (fn_name, mut args) = predicate_call(predicate);

    // Inject the standard `present` and `content` fields into the args object.
    if let Value::Object(ref mut m) = args {
        m.insert("present".into(), Value::Bool(present));
        m.insert(
            "content".into(),
            content.map(Value::String).unwrap_or(Value::Null),
        );
    }

    let raw = registry.invoke(fn_name, &args)?;
    Ok(normalize_verdict(raw, predicate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::artifact::InMemoryArtifactReader;

    /// Minimal test registry: answers `FN_BUILTIN_NON_EMPTY` by inspecting
    /// `present` and `content` fields from the args object.
    struct TestRegistry;

    impl DeterministicRegistry for TestRegistry {
        fn invoke(&self, fn_name: &str, args: &Value) -> Result<Value, RuntimeError> {
            if fn_name == FN_BUILTIN_NON_EMPTY {
                let present = args["present"].as_bool().unwrap_or(false);
                let content = args["content"].as_str().unwrap_or("");
                Ok(json!(present && !content.is_empty()))
            } else {
                Err(RuntimeError::Internal {
                    message: format!("TestRegistry: unknown fn {fn_name}"),
                })
            }
        }
    }

    #[test]
    fn goal_non_empty_passes_on_present_content() {
        let reg = TestRegistry;
        let store = OutputStore::new();
        let reader = InMemoryArtifactReader::new().with_file("/r.md", b"hello");
        let verdict = evaluate_goal(
            &Locus::Path("/r.md".into()),
            &GoalPredicate::NonEmpty,
            &store,
            Some(&reader as &dyn ArtifactReader),
            &reg,
        )
        .unwrap();
        assert!(verdict.met);
    }

    #[test]
    fn goal_non_empty_fails_when_absent() {
        let reg = TestRegistry;
        let store = OutputStore::new();
        let reader = InMemoryArtifactReader::new();
        let verdict = evaluate_goal(
            &Locus::Path("/missing".into()),
            &GoalPredicate::NonEmpty,
            &store,
            Some(&reader as &dyn ArtifactReader),
            &reg,
        )
        .unwrap();
        assert!(!verdict.met);
    }
}
