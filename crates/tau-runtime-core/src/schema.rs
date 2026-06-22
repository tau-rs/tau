//! no_std JSON-Schema-subset validator for tool `input_schema`.
//!
//! Replaces the std-only `jsonschema` crate on the run path (EPIC 0).
//! `compile` lowers a schema `Value` to an alloc-backed rule tree, failing
//! closed on any keyword outside the v1 subset; `check` validates runtime
//! args against it. Both are no_std. See
//! `docs/superpowers/specs/2026-06-22-epic-0-destd-run-loop-design.md`.

// Scaffold: schema fields and helpers for later tasks (T2/T3/T6) are
// intentionally unused here; the module is private until T6 wires it
// into `tool_args`. Suppress the resulting dead-code lint at module scope.
#![allow(dead_code)]

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde_json::Value;

/// A compiled tool input_schema: an alloc-backed rule tree. no_std.
#[derive(Debug, Clone)]
pub struct CompiledSchema {
    root: Schema,
}

/// One node of the rule tree. All constraints are optional and AND-combined.
#[derive(Debug, Clone, Default)]
struct Schema {
    types: Option<Vec<JsonType>>,
    properties: BTreeMap<String, Schema>,
    required: Vec<String>,
    items: Option<Box<Schema>>,
    /// `additionalProperties: false` → Some(false). Schema-form is rejected at compile.
    additional_properties: Option<bool>,
    enum_values: Option<Vec<Value>>,
    const_value: Option<Value>,
    minimum: Option<f64>,
    maximum: Option<f64>,
    exclusive_minimum: Option<f64>,
    exclusive_maximum: Option<f64>,
    multiple_of: Option<f64>,
    min_length: Option<u64>,
    max_length: Option<u64>,
    min_items: Option<u64>,
    max_items: Option<u64>,
    unique_items: Option<bool>,
    one_of: Option<Vec<Schema>>,
    any_of: Option<Vec<Schema>>,
    all_of: Option<Vec<Schema>>,
    not: Option<Box<Schema>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsonType {
    Object,
    Array,
    String,
    Number,
    Integer,
    Boolean,
    Null,
}

/// A single validation failure, formatted for the LLM self-correction message.
#[derive(Debug, Clone)]
pub struct Violation {
    /// JSON Pointer (RFC 6901) to the failing value.
    pub pointer: String,
    /// Human-readable description of the failure.
    pub message: String,
}

/// A schema that failed to compile (malformed, or an out-of-subset keyword).
#[derive(Debug, Clone)]
pub struct CompileErr {
    /// The keyword that caused the failure (empty string for structural errors).
    pub keyword: String,
    /// JSON Pointer to the schema node that failed.
    pub pointer: String,
    /// Human-readable detail about the failure.
    pub detail: String,
}

impl CompiledSchema {
    /// Opt-out validator: accepts every value.
    pub fn accepts_all() -> Self {
        Self {
            root: Schema::default(),
        }
    }

    /// Validate `value`, collecting all violations (empty = valid).
    pub fn check(&self, value: &Value) -> Vec<Violation> {
        let mut out = Vec::new();
        check_node(&self.root, value, "", &mut out);
        out
    }
}

/// Compile a schema `Value` into a `CompiledSchema`, failing closed on any
/// keyword outside the v1 subset.
pub fn compile(schema: &Value) -> Result<CompiledSchema, CompileErr> {
    Ok(CompiledSchema {
        root: compile_node(schema, "")?,
    })
}

const SUPPORTED: &[&str] = &[
    "type",
    "properties",
    "required",
    "items",
    "additionalProperties",
    "enum",
    "const",
    "oneOf",
    "anyOf",
    "allOf",
    "not",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "uniqueItems",
];
const IGNORED: &[&str] = &[
    "title",
    "description",
    "default",
    "$comment",
    "examples",
    "$schema",
    "$id",
];

fn compile_node(schema: &Value, pointer: &str) -> Result<Schema, CompileErr> {
    let obj = match schema {
        Value::Object(m) => m,
        // A bare `true`/empty is "accept anything"; anything else at a schema
        // position is malformed.
        Value::Bool(true) => return Ok(Schema::default()),
        _ => {
            return Err(CompileErr {
                keyword: String::new(),
                pointer: pointer.to_string(),
                detail: "schema node must be an object".to_string(),
            })
        }
    };

    // Fail closed: every key must be supported or an explicitly-ignored annotation.
    for key in obj.keys() {
        if !SUPPORTED.contains(&key.as_str()) && !IGNORED.contains(&key.as_str()) {
            return Err(CompileErr {
                keyword: key.clone(),
                pointer: pointer.to_string(),
                detail: format!("unsupported JSON-Schema keyword '{key}'"),
            });
        }
    }

    let mut node = Schema::default();

    if let Some(t) = obj.get("type") {
        node.types = Some(parse_types(t, pointer)?);
    }
    if let Some(Value::Object(props)) = obj.get("properties") {
        for (k, sub) in props {
            let child_ptr = format!("{pointer}/properties/{k}");
            node.properties
                .insert(k.clone(), compile_node(sub, &child_ptr)?);
        }
    }
    if let Some(Value::Array(req)) = obj.get("required") {
        for item in req {
            if let Value::String(s) = item {
                node.required.push(s.clone());
            }
        }
    }

    Ok(node)
}

fn parse_types(t: &Value, pointer: &str) -> Result<Vec<JsonType>, CompileErr> {
    fn one(name: &str, pointer: &str) -> Result<JsonType, CompileErr> {
        Ok(match name {
            "object" => JsonType::Object,
            "array" => JsonType::Array,
            "string" => JsonType::String,
            "number" => JsonType::Number,
            "integer" => JsonType::Integer,
            "boolean" => JsonType::Boolean,
            "null" => JsonType::Null,
            other => {
                return Err(CompileErr {
                    keyword: "type".to_string(),
                    pointer: pointer.to_string(),
                    detail: format!("unknown type '{other}'"),
                })
            }
        })
    }
    match t {
        Value::String(s) => Ok(alloc::vec![one(s, pointer)?]),
        Value::Array(arr) => {
            let mut out = Vec::new();
            for item in arr {
                if let Value::String(s) = item {
                    out.push(one(s, pointer)?);
                }
            }
            Ok(out)
        }
        _ => Err(CompileErr {
            keyword: "type".to_string(),
            pointer: pointer.to_string(),
            detail: "type must be a string or array of strings".to_string(),
        }),
    }
}

fn type_matches(ty: JsonType, value: &Value) -> bool {
    match (ty, value) {
        (JsonType::Object, Value::Object(_)) => true,
        (JsonType::Array, Value::Array(_)) => true,
        (JsonType::String, Value::String(_)) => true,
        (JsonType::Boolean, Value::Bool(_)) => true,
        (JsonType::Null, Value::Null) => true,
        (JsonType::Number, Value::Number(_)) => true,
        // `integer` accepts an integral number (incl. 2.0).
        (JsonType::Integer, Value::Number(n)) => {
            n.as_i64().is_some()
                || n.as_u64().is_some()
                || n.as_f64().map(|f| f.fract() == 0.0).unwrap_or(false)
        }
        _ => false,
    }
}

fn check_node(node: &Schema, value: &Value, pointer: &str, out: &mut Vec<Violation>) {
    if let Some(types) = &node.types {
        if !types.iter().any(|t| type_matches(*t, value)) {
            out.push(Violation {
                pointer: pointer.to_string(),
                message: format!("value does not match any allowed type {types:?}"),
            });
        }
    }
    if let Value::Object(map) = value {
        for req in &node.required {
            if !map.contains_key(req) {
                out.push(Violation {
                    pointer: pointer.to_string(),
                    message: format!("missing required property '{req}'"),
                });
            }
        }
        for (k, sub) in &node.properties {
            if let Some(child) = map.get(k) {
                let child_ptr = format!("{pointer}/{k}");
                check_node(sub, child, &child_ptr, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(j: serde_json::Value) -> Value {
        j
    }

    #[test]
    fn object_type_accepts_object_rejects_array() {
        let s = compile(&v(serde_json::json!({ "type": "object" }))).unwrap();
        assert!(s.check(&v(serde_json::json!({}))).is_empty());
        assert!(!s.check(&v(serde_json::json!([]))).is_empty());
    }

    #[test]
    fn required_field_missing_is_a_violation() {
        let s = compile(&v(serde_json::json!({
            "type": "object",
            "properties": { "x": { "type": "string" } },
            "required": ["x"]
        })))
        .unwrap();
        assert!(s.check(&v(serde_json::json!({ "x": "hi" }))).is_empty());
        let bad = s.check(&v(serde_json::json!({})));
        assert_eq!(bad.len(), 1);
        assert!(bad[0].message.contains("x"));
    }

    #[test]
    fn property_type_mismatch_is_a_violation() {
        let s = compile(&v(serde_json::json!({
            "type": "object",
            "properties": { "x": { "type": "string" } }
        })))
        .unwrap();
        assert!(!s.check(&v(serde_json::json!({ "x": 42 }))).is_empty());
    }

    #[test]
    fn empty_schema_accepts_everything() {
        let s = compile(&v(serde_json::json!({}))).unwrap();
        assert!(s
            .check(&v(serde_json::json!({ "anything": [1,2,3] })))
            .is_empty());
    }
}
