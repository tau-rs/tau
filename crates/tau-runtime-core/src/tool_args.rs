//! Tool-args schema validation at the kernel boundary.
//!
//! Validates every tool-call's arguments against the tool's declared
//! `ToolSpec.input_schema` (JSON Schema Draft 7) before invoking
//! `Tool::invoke`. Validators are pre-compiled once at
//! `RuntimeBuilder::build()` so per-invoke cost stays in the tens of
//! microseconds.
//!
//! Two failure paths:
//!   - Schema malformed at registration → `BuildError::ToolSchemaInvalid`
//!     (returned by `compile`; surfaced by the builder).
//!   - Args mismatch at invoke → `ToolError::BadArgs` carrying a
//!     self-instructive reason that includes the full declared
//!     `input_schema` plus the original args plus the specific issue.
//!
//! See `docs/superpowers/specs/2026-04-30-tool-args-schema-design.md`
//! and ADR-0010.
//!
//! Gated behind `feature = "tool-validation"`; the validator is no_std (EPIC 0).

use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use tau_domain::Value;
use tau_ports::ToolError;

/// Compiled validator for a single tool's `input_schema`.
///
/// Built once at `RuntimeBuilder::build()` per registered tool.
/// `compile()` rejects malformed schemas; `validate()` rejects
/// non-conforming runtime args with a self-instructive error string.
///
/// # Cloneability
///
/// `ToolArgsValidator` is `Clone`: the compiled `CompiledSchema`
/// is wrapped in `Arc` so cloning is a reference-count bump rather than
/// a schema recompile. This is required by `Runtime::run_streaming_with_history`,
/// which snapshots the `tool_validators` registry into an owned HashMap
/// for the `'static` stream.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ToolArgsValidator {
    /// Compiled no_std schema instance. `None` = tool opted out (empty
    /// schema or `Value::Null`); validation is a no-op.
    ///
    /// Wrapped in `Arc` so that `ToolArgsValidator` can be `Clone`
    /// without recompiling the schema — clone is an Arc reference-count
    /// bump.
    compiled: Option<Arc<crate::schema::CompiledSchema>>,
    /// The original input_schema as declared, kept for inclusion in
    /// BadArgs error messages (the MANDATORY rule).
    declared_schema_json: String,
}

impl ToolArgsValidator {
    /// Compile from a tool's declared `input_schema`. Returns
    /// `Err(SchemaCompileError)` if the schema is not a valid Draft 7
    /// schema.
    ///
    /// Empty (`Value::Null` or empty `Value::Object`) opts out — the
    /// returned validator passes everything.
    pub fn compile(input_schema: &Value) -> Result<Self, SchemaCompileError> {
        let schema_json = serde_json::to_value(input_schema).map_err(|e| SchemaCompileError {
            kind: format!("internal: schema serialization failed: {e}"),
            schema_excerpt: String::new(),
        })?;
        let declared_schema_json = serde_json::to_string(&schema_json).unwrap_or_default();

        // Opt-out: null or empty object schema accepts everything.
        let is_opt_out = match &schema_json {
            serde_json::Value::Null => true,
            serde_json::Value::Object(map) => map.is_empty(),
            _ => false,
        };
        if is_opt_out {
            return Ok(Self {
                compiled: None,
                declared_schema_json,
            });
        }

        let compiled = crate::schema::compile(&schema_json).map_err(|err| SchemaCompileError {
            kind: if err.keyword.is_empty() {
                format!("schema invalid at {}: {}", err.pointer, err.detail)
            } else {
                format!(
                    "unsupported keyword '{}' at {}: {}",
                    err.keyword, err.pointer, err.detail
                )
            },
            schema_excerpt: declared_schema_json.chars().take(200).collect(),
        })?;
        Ok(Self {
            compiled: Some(Arc::new(compiled)),
            declared_schema_json,
        })
    }

    /// Validate runtime args against the compiled schema. Returns
    /// `Err(reason)` on failure where `reason` follows the MANDATORY
    /// template from ADR-0010 §4: every error contains `"You sent:"`,
    /// `"Expected (input_schema):"`, and `"Specific issue"` substrings.
    ///
    /// On opt-out (empty/null schema), validation is a no-op and
    /// returns `Ok(())`.
    pub fn validate(&self, args: &Value, tool_name: &str) -> Result<(), String> {
        let Some(compiled) = &self.compiled else {
            return Ok(());
        };
        let args_json = serde_json::to_value(args)
            .map_err(|e| format!("internal: args serialization failed: {e}"))?;
        let issues: Vec<String> = compiled
            .check(&args_json)
            .into_iter()
            .map(|vio| format!("  {}: {}", vio.pointer, vio.message))
            .collect();
        if !issues.is_empty() {
            let args_repr =
                serde_json::to_string(&args_json).unwrap_or_else(|_| "<unprintable>".into());
            return Err(format!(
                "{tool_name}: args validation failed.\n\n\
                 You sent: {args_repr}\n\n\
                 Expected (input_schema):\n{schema}\n\n\
                 Specific issue(s):\n{issues}",
                schema = self.declared_schema_json,
                issues = issues.join("\n"),
            ));
        }
        Ok(())
    }
}

/// Error returned when a tool's `input_schema` fails to compile as a
/// valid Draft 7 schema. Surfaced by `RuntimeBuilder::build()` as
/// `BuildError::ToolSchemaInvalid`.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct SchemaCompileError {
    /// Human-readable compile diagnostic from the no_std schema validator.
    pub kind: String,
    /// First 200 chars of the schema for context.
    pub schema_excerpt: String,
}

/// Validate runtime args via the kernel's pre-compiled validator.
/// Replaces the v0.1 `deserialize_tool_args` passthrough at `run.rs:417`.
///
/// Returns `Err(ToolError::BadArgs { reason })` on validation failure.
/// The caller in `run.rs` is responsible for routing this through the
/// validation-failure short-circuit (write MessagePayload::ToolError
/// to the conversation; `continue` the inner loop) — DO NOT propagate
/// via `?`, which would terminate the run.
pub fn validate_tool_args<'a>(
    value: &'a Value,
    tool_name: &str,
    validator: &ToolArgsValidator,
) -> Result<&'a Value, ToolError> {
    match validator.validate(value, tool_name) {
        Ok(()) => Ok(value),
        Err(reason) => Err(ToolError::BadArgs { reason }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;

    /// Build a tau_domain::Value JSON Schema from a `serde_json::json!` literal.
    fn schema(json: serde_json::Value) -> Value {
        let s = serde_json::to_string(&json).expect("schema serializes");
        serde_json::from_str(&s).expect("schema round-trips through tau_domain::Value")
    }

    /// Build a tau_domain::Value args object from a `serde_json::json!` literal.
    fn args(json: serde_json::Value) -> Value {
        let s = serde_json::to_string(&json).expect("args serialize");
        serde_json::from_str(&s).expect("args round-trip through tau_domain::Value")
    }

    // -------- compile --------

    #[test]
    fn compile_happy_path_returns_some_compiled() {
        let s = schema(serde_json::json!({
            "type": "object",
            "properties": { "x": { "type": "string" } },
            "required": ["x"]
        }));
        let v = ToolArgsValidator::compile(&s).expect("valid schema");
        assert!(v.compiled.is_some());
    }

    #[test]
    fn compile_null_schema_opts_out() {
        let v = ToolArgsValidator::compile(&Value::Null).expect("null opts out");
        assert!(v.compiled.is_none());
    }

    #[test]
    fn compile_empty_object_schema_opts_out() {
        let s = Value::Object(BTreeMap::new());
        let v = ToolArgsValidator::compile(&s).expect("empty opts out");
        assert!(v.compiled.is_none());
    }

    #[test]
    fn compile_malformed_schema_returns_error() {
        // "type": "objectt" is a typo — not a valid Draft 7 type.
        let s = schema(serde_json::json!({ "type": "objectt" }));
        let err = ToolArgsValidator::compile(&s).expect_err("malformed");
        assert!(
            err.kind.contains("type") || err.kind.contains("unsupported"),
            "expected type/unsupported failure kind; got: {}",
            err.kind
        );
        assert!(
            !err.schema_excerpt.is_empty(),
            "schema_excerpt should be populated"
        );
    }

    // -------- validate happy path / opt-out --------

    #[test]
    fn validate_happy_path() {
        let s = schema(serde_json::json!({
            "type": "object",
            "properties": { "x": { "type": "string" } },
            "required": ["x"]
        }));
        let v = ToolArgsValidator::compile(&s).unwrap();
        let a = args(serde_json::json!({ "x": "hello" }));
        v.validate(&a, "test-tool").expect("matches");
    }

    #[test]
    fn validate_opt_out_passes_anything() {
        let v = ToolArgsValidator::compile(&Value::Null).unwrap();
        let a = args(serde_json::json!({ "literally": "anything" }));
        v.validate(&a, "test-tool").expect("opt-out passes");
    }

    // -------- validate failure cases (each asserts the MANDATORY template) --------

    fn assert_mandatory_template(reason: &str, tool_name: &str) {
        assert!(
            reason.contains(tool_name),
            "reason should contain tool name; got: {reason}"
        );
        assert!(
            reason.contains("You sent:"),
            "reason MUST contain 'You sent:'; got: {reason}"
        );
        assert!(
            reason.contains("Expected (input_schema):"),
            "reason MUST contain 'Expected (input_schema):'; got: {reason}"
        );
        assert!(
            reason.contains("Specific issue"),
            "reason MUST contain 'Specific issue'; got: {reason}"
        );
    }

    #[test]
    fn validate_missing_required_field_includes_template() {
        let s = schema(serde_json::json!({
            "type": "object",
            "properties": { "x": { "type": "string" } },
            "required": ["x"]
        }));
        let v = ToolArgsValidator::compile(&s).unwrap();
        let a = args(serde_json::json!({})); // missing "x"
        let reason = v.validate(&a, "test-tool").expect_err("missing required");
        assert_mandatory_template(&reason, "test-tool");
    }

    #[test]
    fn validate_type_mismatch_includes_template() {
        let s = schema(serde_json::json!({
            "type": "object",
            "properties": { "x": { "type": "string" } },
            "required": ["x"]
        }));
        let v = ToolArgsValidator::compile(&s).unwrap();
        let a = args(serde_json::json!({ "x": 42 })); // integer, not string
        let reason = v.validate(&a, "test-tool").expect_err("type mismatch");
        assert_mandatory_template(&reason, "test-tool");
    }

    #[test]
    fn validate_integer_out_of_range_includes_template() {
        let s = schema(serde_json::json!({
            "type": "object",
            "properties": { "n": { "type": "integer", "minimum": 1, "maximum": 10 } },
            "required": ["n"]
        }));
        let v = ToolArgsValidator::compile(&s).unwrap();
        let a = args(serde_json::json!({ "n": 999 }));
        let reason = v.validate(&a, "test-tool").expect_err("out of range");
        assert_mandatory_template(&reason, "test-tool");
    }

    // -------- validate_tool_args wrapper --------

    #[test]
    fn validate_tool_args_returns_bad_args_on_failure() {
        let s = schema(serde_json::json!({
            "type": "object",
            "properties": { "x": { "type": "string" } },
            "required": ["x"]
        }));
        let v = ToolArgsValidator::compile(&s).unwrap();
        let a = args(serde_json::json!({}));
        let err = validate_tool_args(&a, "test-tool", &v).expect_err("missing required");
        let ToolError::BadArgs { reason } = err else {
            panic!("expected ToolError::BadArgs, got: {err:?}");
        };
        assert_mandatory_template(&reason, "test-tool");
    }

    #[test]
    fn validate_tool_args_returns_value_on_success() {
        let s = schema(serde_json::json!({
            "type": "object",
            "properties": { "x": { "type": "string" } }
        }));
        let v = ToolArgsValidator::compile(&s).unwrap();
        let a = args(serde_json::json!({ "x": "hello" }));
        let got = validate_tool_args(&a, "test-tool", &v).expect("happy path");
        assert_eq!(got, &a);
    }
}
