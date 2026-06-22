//! no_std JSON-Schema-subset validator for tool `input_schema`.
//!
//! Replaces the std-only `jsonschema` crate on the run path (EPIC 0).
//! `compile` lowers a schema `Value` to an alloc-backed rule tree, failing
//! closed on any keyword outside the v1 subset; `check` validates runtime
//! args against it. Both are no_std. See
//! `docs/superpowers/specs/2026-06-22-epic-0-destd-run-loop-design.md`.

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
    match obj.get("properties") {
        Some(Value::Object(props)) => {
            for (k, sub) in props {
                let child_ptr = format!("{pointer}/properties/{k}");
                node.properties
                    .insert(k.clone(), compile_node(sub, &child_ptr)?);
            }
        }
        Some(_) => {
            return Err(CompileErr {
                keyword: "properties".to_string(),
                pointer: pointer.to_string(),
                detail: "properties must be an object".to_string(),
            })
        }
        None => {}
    }
    match obj.get("required") {
        Some(Value::Array(req)) => {
            for item in req {
                match item {
                    Value::String(s) => node.required.push(s.clone()),
                    _ => {
                        return Err(CompileErr {
                            keyword: "required".to_string(),
                            pointer: pointer.to_string(),
                            detail: "required entries must be strings".to_string(),
                        })
                    }
                }
            }
        }
        Some(_) => {
            return Err(CompileErr {
                keyword: "required".to_string(),
                pointer: pointer.to_string(),
                detail: "required must be an array".to_string(),
            })
        }
        None => {}
    }

    if let Some(Value::Array(items)) = obj.get("enum") {
        node.enum_values = Some(items.clone());
    }
    if let Some(c) = obj.get("const") {
        node.const_value = Some(c.clone());
    }
    node.minimum = obj.get("minimum").and_then(Value::as_f64);
    node.maximum = obj.get("maximum").and_then(Value::as_f64);
    node.exclusive_minimum = obj.get("exclusiveMinimum").and_then(Value::as_f64);
    node.exclusive_maximum = obj.get("exclusiveMaximum").and_then(Value::as_f64);
    if let Some(mo) = obj.get("multipleOf") {
        match mo.as_f64() {
            Some(m) if m > 0.0 => node.multiple_of = Some(m),
            _ => {
                return Err(CompileErr {
                    keyword: "multipleOf".into(),
                    pointer: pointer.to_string(),
                    detail: "multipleOf must be a positive number".into(),
                })
            }
        }
    }
    node.min_length = obj.get("minLength").and_then(Value::as_u64);
    node.max_length = obj.get("maxLength").and_then(Value::as_u64);
    node.min_items = obj.get("minItems").and_then(Value::as_u64);
    node.max_items = obj.get("maxItems").and_then(Value::as_u64);
    node.unique_items = obj.get("uniqueItems").and_then(Value::as_bool);
    if let Some(items) = obj.get("items") {
        node.items = Some(Box::new(compile_node(items, &format!("{pointer}/items"))?));
    }
    if let Some(ap) = obj.get("additionalProperties") {
        match ap {
            Value::Bool(b) => node.additional_properties = Some(*b),
            _ => {
                return Err(CompileErr {
                    keyword: "additionalProperties".to_string(),
                    pointer: pointer.to_string(),
                    detail: "schema-form additionalProperties is unsupported in v1".to_string(),
                })
            }
        }
    }

    for (key, slot) in [("oneOf", 0u8), ("anyOf", 1), ("allOf", 2)] {
        if let Some(Value::Array(arr)) = obj.get(key) {
            let mut subs = Vec::new();
            for (i, sub) in arr.iter().enumerate() {
                subs.push(compile_node(sub, &format!("{pointer}/{key}/{i}"))?);
            }
            match slot {
                0 => node.one_of = Some(subs),
                1 => node.any_of = Some(subs),
                _ => node.all_of = Some(subs),
            }
        }
    }
    if let Some(sub) = obj.get("not") {
        node.not = Some(Box::new(compile_node(sub, &format!("{pointer}/not"))?));
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
                match item {
                    Value::String(s) => out.push(one(s, pointer)?),
                    _ => {
                        return Err(CompileErr {
                            keyword: "type".to_string(),
                            pointer: pointer.to_string(),
                            detail: "type array must contain only strings".to_string(),
                        })
                    }
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

fn passes(node: &Schema, value: &Value) -> bool {
    let mut scratch = Vec::new();
    check_node(node, value, "", &mut scratch);
    scratch.is_empty()
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
    if let Some(allowed) = &node.enum_values {
        if !allowed.iter().any(|a| a == value) {
            out.push(Violation {
                pointer: pointer.to_string(),
                message: format!("value not in enum {allowed:?}"),
            });
        }
    }
    if let Some(c) = &node.const_value {
        if c != value {
            out.push(Violation {
                pointer: pointer.to_string(),
                message: format!("value must equal const {c}"),
            });
        }
    }
    if let Value::Number(_) = value {
        let n = value.as_f64().unwrap_or(f64::NAN);
        if let Some(m) = node.minimum {
            if n < m {
                out.push(Violation {
                    pointer: pointer.to_string(),
                    message: format!("{n} < minimum {m}"),
                });
            }
        }
        if let Some(m) = node.maximum {
            if n > m {
                out.push(Violation {
                    pointer: pointer.to_string(),
                    message: format!("{n} > maximum {m}"),
                });
            }
        }
        if let Some(m) = node.exclusive_minimum {
            if n <= m {
                out.push(Violation {
                    pointer: pointer.to_string(),
                    message: format!("{n} <= exclusiveMinimum {m}"),
                });
            }
        }
        if let Some(m) = node.exclusive_maximum {
            if n >= m {
                out.push(Violation {
                    pointer: pointer.to_string(),
                    message: format!("{n} >= exclusiveMaximum {m}"),
                });
            }
        }
        if let Some(m) = node.multiple_of {
            let ratio = n / m;
            let rounded = ratio.round();
            if (ratio - rounded).abs() > 1e-9 * ratio.abs().max(1.0) {
                out.push(Violation {
                    pointer: pointer.to_string(),
                    message: format!("{n} not a multiple of {m}"),
                });
            }
        }
    }
    if let Value::String(s) = value {
        let len = s.chars().count() as u64;
        if let Some(m) = node.min_length {
            if len < m {
                out.push(Violation {
                    pointer: pointer.to_string(),
                    message: format!("string shorter than minLength {m}"),
                });
            }
        }
        if let Some(m) = node.max_length {
            if len > m {
                out.push(Violation {
                    pointer: pointer.to_string(),
                    message: format!("string longer than maxLength {m}"),
                });
            }
        }
    }
    if let Value::Array(arr) = value {
        if let Some(m) = node.min_items {
            if (arr.len() as u64) < m {
                out.push(Violation {
                    pointer: pointer.to_string(),
                    message: format!("fewer than minItems {m}"),
                });
            }
        }
        if let Some(m) = node.max_items {
            if (arr.len() as u64) > m {
                out.push(Violation {
                    pointer: pointer.to_string(),
                    message: format!("more than maxItems {m}"),
                });
            }
        }
        if node.unique_items == Some(true) {
            for i in 0..arr.len() {
                if arr[i + 1..].iter().any(|other| other == &arr[i]) {
                    out.push(Violation {
                        pointer: pointer.to_string(),
                        message: "array items not unique".to_string(),
                    });
                    break;
                }
            }
        }
        if let Some(item_schema) = &node.items {
            for (i, item) in arr.iter().enumerate() {
                check_node(item_schema, item, &format!("{pointer}/{i}"), out);
            }
        }
    }
    if let Some(subs) = &node.one_of {
        let n = subs.iter().filter(|s| passes(s, value)).count();
        if n != 1 {
            out.push(Violation {
                pointer: pointer.to_string(),
                message: format!("value must match exactly one oneOf branch, matched {n}"),
            });
        }
    }
    if let Some(subs) = &node.any_of {
        if !subs.iter().any(|s| passes(s, value)) {
            out.push(Violation {
                pointer: pointer.to_string(),
                message: "value matched no anyOf branch".to_string(),
            });
        }
    }
    if let Some(subs) = &node.all_of {
        for (i, s) in subs.iter().enumerate() {
            if !passes(s, value) {
                out.push(Violation {
                    pointer: format!("{pointer}/allOf/{i}"),
                    message: "value failed an allOf branch".to_string(),
                });
            }
        }
    }
    if let Some(sub) = &node.not {
        if passes(sub, value) {
            out.push(Violation {
                pointer: pointer.to_string(),
                message: "value matched a `not` schema".to_string(),
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
        // additionalProperties: false → reject keys not in `properties`.
        if node.additional_properties == Some(false) {
            for k in map.keys() {
                if !node.properties.contains_key(k) {
                    out.push(Violation {
                        pointer: pointer.to_string(),
                        message: format!("unexpected additional property '{k}'"),
                    });
                }
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

    #[test]
    fn malformed_type_array_fails_closed() {
        assert!(compile(&v(serde_json::json!({ "type": [42, "string"] }))).is_err());
    }

    #[test]
    fn malformed_required_entry_fails_closed() {
        assert!(compile(&v(
            serde_json::json!({ "type": "object", "required": [1, "x"] })
        ))
        .is_err());
        assert!(compile(&v(serde_json::json!({ "type": "object", "required": "x" }))).is_err());
    }

    #[test]
    fn non_object_properties_fails_closed() {
        assert!(compile(&v(
            serde_json::json!({ "type": "object", "properties": "oops" })
        ))
        .is_err());
    }

    #[test]
    fn enum_and_const() {
        let s = compile(&v(serde_json::json!({ "enum": ["a", "b"] }))).unwrap();
        assert!(s.check(&v(serde_json::json!("a"))).is_empty());
        assert!(!s.check(&v(serde_json::json!("c"))).is_empty());

        let s = compile(&v(serde_json::json!({ "const": "write" }))).unwrap();
        assert!(s.check(&v(serde_json::json!("write"))).is_empty());
        assert!(!s.check(&v(serde_json::json!("edit"))).is_empty());
    }

    #[test]
    fn numeric_bounds() {
        let s = compile(&v(serde_json::json!({
            "type": "integer", "minimum": 1, "maximum": 10
        })))
        .unwrap();
        assert!(s.check(&v(serde_json::json!(5))).is_empty());
        assert!(!s.check(&v(serde_json::json!(0))).is_empty());
        assert!(!s.check(&v(serde_json::json!(11))).is_empty());

        let s = compile(&v(serde_json::json!({ "type": "number", "multipleOf": 2 }))).unwrap();
        assert!(s.check(&v(serde_json::json!(4))).is_empty());
        assert!(!s.check(&v(serde_json::json!(5))).is_empty());
    }

    #[test]
    fn additional_properties_false() {
        let s = compile(&v(serde_json::json!({
            "type": "object",
            "properties": { "x": { "type": "number" } },
            "additionalProperties": false
        })))
        .unwrap();
        assert!(s.check(&v(serde_json::json!({ "x": 1 }))).is_empty());
        assert!(!s
            .check(&v(serde_json::json!({ "x": 1, "y": 2 })))
            .is_empty());
    }

    #[test]
    fn additional_properties_schema_form_fails_closed() {
        assert!(compile(&v(
            serde_json::json!({ "additionalProperties": { "type": "string" } })
        ))
        .is_err());
    }

    #[test]
    fn exclusive_bounds_and_items() {
        let s = compile(&v(
            serde_json::json!({ "type": "integer", "exclusiveMinimum": 0, "exclusiveMaximum": 10 }),
        ))
        .unwrap();
        assert!(s.check(&v(serde_json::json!(5))).is_empty());
        assert!(!s.check(&v(serde_json::json!(0))).is_empty());
        assert!(!s.check(&v(serde_json::json!(10))).is_empty());

        let s = compile(&v(
            serde_json::json!({ "type": "array", "items": { "type": "integer" } }),
        ))
        .unwrap();
        assert!(s.check(&v(serde_json::json!([1, 2, 3]))).is_empty());
        assert!(!s.check(&v(serde_json::json!([1, "bad"]))).is_empty());
    }

    #[test]
    fn fractional_multiple_of_is_correct() {
        let s = compile(&v(
            serde_json::json!({ "type": "number", "multipleOf": 0.1 }),
        ))
        .unwrap();
        assert!(
            s.check(&v(serde_json::json!(7))).is_empty(),
            "7 is a multiple of 0.1"
        );
        assert!(!s.check(&v(serde_json::json!(7.05))).is_empty());
    }

    #[test]
    fn non_positive_multiple_of_fails_closed() {
        assert!(compile(&v(serde_json::json!({ "multipleOf": 0 }))).is_err());
        assert!(compile(&v(serde_json::json!({ "multipleOf": -2 }))).is_err());
    }

    #[test]
    fn one_of_discriminated_union_like_fs_write() {
        let s = compile(&v(serde_json::json!({
            "type": "object",
            "oneOf": [
                { "properties": { "mode": { "const": "write" }, "path": { "type": "string" } },
                  "required": ["mode", "path"], "additionalProperties": false },
                { "properties": { "mode": { "const": "edit" }, "old": { "type": "string" } },
                  "required": ["mode", "old"], "additionalProperties": false }
            ]
        })))
        .unwrap();
        assert!(s
            .check(&v(serde_json::json!({ "mode": "write", "path": "/a" })))
            .is_empty());
        assert!(s
            .check(&v(serde_json::json!({ "mode": "edit", "old": "x" })))
            .is_empty());
        // matches neither branch (missing required) → violation
        assert!(!s
            .check(&v(serde_json::json!({ "mode": "write" })))
            .is_empty());
        // matches both would also fail oneOf, but const mode makes that impossible here
    }

    #[test]
    fn any_of_all_of_not() {
        let s = compile(&v(
            serde_json::json!({ "anyOf": [ { "type": "string" }, { "type": "integer" } ] }),
        ))
        .unwrap();
        assert!(s.check(&v(serde_json::json!("x"))).is_empty());
        assert!(s.check(&v(serde_json::json!(3))).is_empty());
        assert!(!s.check(&v(serde_json::json!(true))).is_empty());

        let s = compile(&v(
            serde_json::json!({ "allOf": [ { "type": "integer" }, { "minimum": 0 } ] }),
        ))
        .unwrap();
        assert!(s.check(&v(serde_json::json!(5))).is_empty());
        assert!(!s.check(&v(serde_json::json!(-1))).is_empty());

        let s = compile(&v(serde_json::json!({ "not": { "type": "string" } }))).unwrap();
        assert!(s.check(&v(serde_json::json!(3))).is_empty());
        assert!(!s.check(&v(serde_json::json!("x"))).is_empty());
    }

    #[test]
    fn string_and_array_bounds() {
        let s = compile(&v(serde_json::json!({
            "type": "string", "minLength": 2, "maxLength": 4
        })))
        .unwrap();
        assert!(s.check(&v(serde_json::json!("abc"))).is_empty());
        assert!(!s.check(&v(serde_json::json!("a"))).is_empty());
        assert!(!s.check(&v(serde_json::json!("abcde"))).is_empty());

        let s = compile(&v(serde_json::json!({
            "type": "array", "minItems": 1, "uniqueItems": true
        })))
        .unwrap();
        assert!(s.check(&v(serde_json::json!([1, 2]))).is_empty());
        assert!(!s.check(&v(serde_json::json!([]))).is_empty());
        assert!(!s.check(&v(serde_json::json!([1, 1]))).is_empty());
    }

    #[test]
    fn unsupported_keywords_fail_closed() {
        for kw in [
            "pattern",
            "format",
            "$ref",
            "patternProperties",
            "if",
            "dependencies",
        ] {
            let mut m = serde_json::Map::new();
            m.insert("type".to_string(), serde_json::json!("string"));
            m.insert(kw.to_string(), serde_json::json!({}));
            let schema = Value::Object(m);
            let err = compile(&schema).expect_err(kw);
            assert_eq!(err.keyword, kw, "error should name the offending keyword");
        }
    }

    #[test]
    fn one_of_rejects_when_two_branches_match() {
        // 5 satisfies BOTH {minimum:0} and {maximum:10} → matches 2 → oneOf rejects.
        let s = compile(&v(
            serde_json::json!({ "oneOf": [ { "minimum": 0 }, { "maximum": 10 } ] }),
        ))
        .unwrap();
        assert!(
            !s.check(&v(serde_json::json!(5))).is_empty(),
            "value matching two oneOf branches must be rejected"
        );
        // 15 satisfies only {minimum:0} → matches exactly 1 → accepts.
        assert!(s.check(&v(serde_json::json!(15))).is_empty());
    }

    #[test]
    fn annotations_are_ignored_not_errors() {
        let s = compile(&v(serde_json::json!({
            "type": "string",
            "title": "Name", "description": "the name", "default": "x", "examples": ["a"]
        })))
        .expect("annotations must not error");
        assert!(s.check(&v(serde_json::json!("hi"))).is_empty());
    }
}
