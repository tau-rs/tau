# EPIC 0 — de-std the run loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `tau-runtime-core`'s run loop run `no_std` with tool-arg validation intact, by replacing the std-only `jsonschema` validator with a no_std JSON-Schema-subset validator enforced fail-closed at build time.

**Architecture:** A new `schema` module in `tau-runtime-core` compiles each tool's `input_schema` (a `serde_json::Value`, alloc-capable) into an alloc-backed rule tree and validates runtime args against it — both `compile` and `check` are `no_std`. `ToolArgsValidator` keeps its public API; only its internals swap from `Arc<jsonschema::Validator>` to the new `Arc<schema::CompiledSchema>`. The `tool-validation` feature stops pulling `std`.

**Tech Stack:** Rust, `no_std` + `alloc`, `serde_json` (alloc feature), `tau-domain::Value`, `tau-ports::ToolError`. Tests: `cargo nextest`.

Spec: [`docs/superpowers/specs/2026-06-22-epic-0-destd-run-loop-design.md`](../specs/2026-06-22-epic-0-destd-run-loop-design.md).

## Global Constraints

- **Branch:** `feat/epic-0-destd-runloop` (already created off `main`; the spec lives on it).
- **CARGO RULES (CLAUDE.md):** every cargo command is `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo <cmd> -p <crate>`. Main agent uses `target/main`. Timeouts: test 300s, build/check 180s, clippy 240s. Use `cargo nextest run` for tests (doctests via `cargo test --doc`). Check `pgrep -af cargo` before launching if a shared target dir might be busy.
- **`no_std` discipline:** `tau-runtime-core` is `#![no_std]` + `#![forbid(unsafe_code)]`. New code uses `alloc::` / `core::`, never `std::`. The new validator module must contain **zero** `std::` and **zero** `use std`.
- **Supported v1 subset (spec §3.1):** `type`, `properties`, `required`, `items`, `enum`, `const`, `additionalProperties` (bool form), `oneOf`, `anyOf`, `allOf`, `not`, `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`, `multipleOf`, `minLength`, `maxLength`, `minItems`, `maxItems`, `uniqueItems`.
- **Ignored annotations (NOT errors):** `title`, `description`, `default`, `$comment`, `examples`, `$schema`, `$id`.
- **Unsupported v1 → fail-closed `SchemaCompileError` (spec §3.2):** `pattern`, `format`, `$ref`, `$defs`/`definitions`, `if`/`then`/`else`, `additionalProperties` (schema form), `patternProperties`, `dependencies`.
- **MANDATORY error template (ADR-0010):** the `BadArgs` reason string MUST contain the substrings `"You sent:"`, `"Expected (input_schema):"`, and `"Specific issue"`. The existing `tool_args.rs` tests assert this — keep them green.
- **Commit identity:** `git -c user.name="LEBOCQ Titouan" -c user.email="75916953+LEBOCQTitouan@users.noreply.github.com" commit -m "..."` ending body with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Rust changes may run the pre-commit hook; `--no-verify` only if the hook's flaky suite blocks (note it in the commit).
- **Two PRs:** PR-0b = Tasks 1–8 (the validator + swap + feature refactor + tau check). PR-0c = Tasks 9–10 (run-loop no_std CI lane + epic close-out). Open PR-0b before starting PR-0c.

---

## File structure

| File | Responsibility | Tasks |
|---|---|---|
| `crates/tau-runtime-core/src/schema.rs` (**new**) | no_std `CompiledSchema` + `compile` + `check`; the entire validator | 1–5 |
| `crates/tau-runtime-core/src/lib.rs` | declare `mod schema`; drop `tool-validation` from the `extern crate std` cfg | 1, 7 |
| `crates/tau-runtime-core/src/tool_args.rs` | swap `ToolArgsValidator` internals jsonschema → `schema::CompiledSchema`; keep public API + template | 6 |
| `crates/tau-runtime-core/Cargo.toml` | drop `dep:jsonschema` + `tau-domain/std` from `tool-validation`; remove `jsonschema` dep | 7 |
| `crates/tau-runtime-core/src/builder.rs`, `src/stream.rs` | comment fixes only (refer to "no_std validator" not "jsonschema"); wiring unchanged | 7 |
| `crates/tau-cli/...` (`tau check`) | confirm/extend `SchemaCompileError` surfacing | 8 |
| `.github/workflows/ci.yml`, wasm-guest build | run-loop no_std lane + enable `tool-validation` on guest for parity | 9 |
| `ROADMAP.md` | mark EPIC 0 status | 10 |

---

## PR-0b — the no_std validator

### Task 1: Scaffold `schema.rs` — structural keywords (`type`, `properties`, `required`)

**Files:**
- Create: `crates/tau-runtime-core/src/schema.rs`
- Modify: `crates/tau-runtime-core/src/lib.rs` (add `mod schema;`)
- Test: unit tests inside `schema.rs`

**Interfaces:**
- Produces:
  - `pub struct CompiledSchema { root: Schema }`
  - `pub fn compile(schema: &serde_json::Value) -> Result<CompiledSchema, CompileErr>` where `pub struct CompileErr { pub keyword: String, pub pointer: String, pub detail: String }`
  - `impl CompiledSchema { pub fn check(&self, value: &serde_json::Value) -> alloc::vec::Vec<Violation> }` where `pub struct Violation { pub pointer: String, pub message: String }`
  - `CompiledSchema::accepts_all()` constructor for the opt-out (empty/null) case.

- [ ] **Step 1: Add the module declaration to `lib.rs`**

`schema` is consumed ONLY by `tool_args`, which is itself gated
`#[cfg(feature = "tool-validation")] pub mod tool_args;` (lib.rs:42–43). Mirror
that gate exactly — gating avoids a dead-code warning (the crate is
`#![deny(...)]` and CI runs clippy `-D warnings`) on the `wasm-interpreter`-only
build where `tool_args` is absent. Add immediately after the `tool_args` line
(lib.rs ~43):

```rust
#[cfg(feature = "tool-validation")]
mod schema;
```

(Private — `tool_args.rs` is its only consumer, same crate.)

- [ ] **Step 2: Write failing tests for structural validation**

Create `crates/tau-runtime-core/src/schema.rs` with imports + the test module first:

```rust
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

#[cfg(test)]
mod tests {
    use super::*;

    fn v(j: serde_json::Value) -> Value { j }

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
        }))).unwrap();
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
        }))).unwrap();
        assert!(!s.check(&v(serde_json::json!({ "x": 42 }))).is_empty());
    }

    #[test]
    fn empty_schema_accepts_everything() {
        let s = compile(&v(serde_json::json!({}))).unwrap();
        assert!(s.check(&v(serde_json::json!({ "anything": [1,2,3] }))).is_empty());
    }
}
```

- [ ] **Step 3: Run tests, verify they fail to compile (no `compile`/`CompiledSchema` yet)**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo build -p tau-runtime-core`
Expected: FAIL — `cannot find function compile`, `cannot find type CompiledSchema`.

- [ ] **Step 4: Implement the structural core**

Add to `schema.rs` (above the test module):

```rust
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
enum JsonType { Object, Array, String, Number, Integer, Boolean, Null }

/// A single validation failure, formatted for the LLM self-correction message.
#[derive(Debug, Clone)]
pub struct Violation {
    pub pointer: String,
    pub message: String,
}

/// A schema that failed to compile (malformed, or an out-of-subset keyword).
#[derive(Debug, Clone)]
pub struct CompileErr {
    pub keyword: String,
    pub pointer: String,
    pub detail: String,
}

impl CompiledSchema {
    /// Opt-out validator: accepts every value.
    pub fn accepts_all() -> Self {
        Self { root: Schema::default() }
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
    Ok(CompiledSchema { root: compile_node(schema, "")? })
}

const SUPPORTED: &[&str] = &[
    "type", "properties", "required", "items", "additionalProperties",
    "enum", "const", "oneOf", "anyOf", "allOf", "not",
    "minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum", "multipleOf",
    "minLength", "maxLength", "minItems", "maxItems", "uniqueItems",
];
const IGNORED: &[&str] = &[
    "title", "description", "default", "$comment", "examples", "$schema", "$id",
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
            node.properties.insert(k.clone(), compile_node(sub, &child_ptr)?);
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
            n.as_i64().is_some() || n.as_u64().is_some()
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
```

- [ ] **Step 5: Run the structural tests, verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-runtime-core schema::`
Expected: 4 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-runtime-core/src/schema.rs crates/tau-runtime-core/src/lib.rs
git -c user.name="LEBOCQ Titouan" -c user.email="75916953+LEBOCQTitouan@users.noreply.github.com" \
  commit -m "feat(runtime-core): no_std schema validator — structural keywords

schema::CompiledSchema + compile/check for type/properties/required.
Fail-closed on out-of-subset keywords; annotations ignored. EPIC 0 #379.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

### Task 2: `enum`, `const`, numeric, string-length, array keywords

**Files:** Modify `crates/tau-runtime-core/src/schema.rs`.

**Interfaces:** Consumes Task 1's `Schema`/`compile_node`/`check_node`. Produces no new public types.

- [ ] **Step 1: Write failing tests**

Add to the `tests` module:

```rust
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
    }))).unwrap();
    assert!(s.check(&v(serde_json::json!(5))).is_empty());
    assert!(!s.check(&v(serde_json::json!(0))).is_empty());
    assert!(!s.check(&v(serde_json::json!(11))).is_empty());

    let s = compile(&v(serde_json::json!({ "type": "number", "multipleOf": 2 }))).unwrap();
    assert!(s.check(&v(serde_json::json!(4))).is_empty());
    assert!(!s.check(&v(serde_json::json!(5))).is_empty());
}

#[test]
fn string_and_array_bounds() {
    let s = compile(&v(serde_json::json!({
        "type": "string", "minLength": 2, "maxLength": 4
    }))).unwrap();
    assert!(s.check(&v(serde_json::json!("abc"))).is_empty());
    assert!(!s.check(&v(serde_json::json!("a"))).is_empty());
    assert!(!s.check(&v(serde_json::json!("abcde"))).is_empty());

    let s = compile(&v(serde_json::json!({
        "type": "array", "minItems": 1, "uniqueItems": true
    }))).unwrap();
    assert!(s.check(&v(serde_json::json!([1, 2]))).is_empty());
    assert!(!s.check(&v(serde_json::json!([]))).is_empty());
    assert!(!s.check(&v(serde_json::json!([1, 1]))).is_empty());
}
```

- [ ] **Step 2: Run, verify failure**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-runtime-core schema::`
Expected: the 3 new tests FAIL (constraints not yet read/checked).

- [ ] **Step 3: Extend `compile_node` to read the keywords**

In `compile_node`, after the `required` block and before `Ok(node)`, add:

```rust
    if let Some(e) = obj.get("enum") {
        if let Value::Array(items) = e {
            node.enum_values = Some(items.clone());
        }
    }
    if let Some(c) = obj.get("const") {
        node.const_value = Some(c.clone());
    }
    node.minimum = obj.get("minimum").and_then(Value::as_f64);
    node.maximum = obj.get("maximum").and_then(Value::as_f64);
    node.exclusive_minimum = obj.get("exclusiveMinimum").and_then(Value::as_f64);
    node.exclusive_maximum = obj.get("exclusiveMaximum").and_then(Value::as_f64);
    node.multiple_of = obj.get("multipleOf").and_then(Value::as_f64);
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
```

- [ ] **Step 4: Extend `check_node` to enforce them**

Add to `check_node`, after the existing `types` block (so all constraints AND together):

```rust
    if let Some(allowed) = &node.enum_values {
        if !allowed.iter().any(|a| a == value) {
            out.push(Violation { pointer: pointer.to_string(),
                message: format!("value not in enum {allowed:?}") });
        }
    }
    if let Some(c) = &node.const_value {
        if c != value {
            out.push(Violation { pointer: pointer.to_string(),
                message: format!("value must equal const {c}") });
        }
    }
    if let Value::Number(_) = value {
        let n = value.as_f64().unwrap_or(f64::NAN);
        if let Some(m) = node.minimum { if n < m {
            out.push(Violation { pointer: pointer.to_string(), message: format!("{n} < minimum {m}") }); } }
        if let Some(m) = node.maximum { if n > m {
            out.push(Violation { pointer: pointer.to_string(), message: format!("{n} > maximum {m}") }); } }
        if let Some(m) = node.exclusive_minimum { if n <= m {
            out.push(Violation { pointer: pointer.to_string(), message: format!("{n} <= exclusiveMinimum {m}") }); } }
        if let Some(m) = node.exclusive_maximum { if n >= m {
            out.push(Violation { pointer: pointer.to_string(), message: format!("{n} >= exclusiveMaximum {m}") }); } }
        if let Some(m) = node.multiple_of { if m != 0.0 && (n / m).fract().abs() > 1e-9 {
            out.push(Violation { pointer: pointer.to_string(), message: format!("{n} not a multiple of {m}") }); } }
    }
    if let Value::String(s) = value {
        let len = s.chars().count() as u64;
        if let Some(m) = node.min_length { if len < m {
            out.push(Violation { pointer: pointer.to_string(), message: format!("string shorter than minLength {m}") }); } }
        if let Some(m) = node.max_length { if len > m {
            out.push(Violation { pointer: pointer.to_string(), message: format!("string longer than maxLength {m}") }); } }
    }
    if let Value::Array(arr) = value {
        if let Some(m) = node.min_items { if (arr.len() as u64) < m {
            out.push(Violation { pointer: pointer.to_string(), message: format!("fewer than minItems {m}") }); } }
        if let Some(m) = node.max_items { if (arr.len() as u64) > m {
            out.push(Violation { pointer: pointer.to_string(), message: format!("more than maxItems {m}") }); } }
        if node.unique_items == Some(true) {
            for i in 0..arr.len() {
                if arr[i + 1..].iter().any(|other| other == &arr[i]) {
                    out.push(Violation { pointer: pointer.to_string(), message: "array items not unique".to_string() });
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
    // additionalProperties: false → reject keys not in `properties`.
    if node.additional_properties == Some(false) {
        if let Value::Object(map) = value {
            for k in map.keys() {
                if !node.properties.contains_key(k) {
                    out.push(Violation { pointer: pointer.to_string(),
                        message: format!("unexpected additional property '{k}'") });
                }
            }
        }
    }
```

- [ ] **Step 5: Run, verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-runtime-core schema::`
Expected: all schema tests pass.

- [ ] **Step 6: Commit** (`feat(runtime-core): no_std schema validator — enum/const/numeric/string/array`).

### Task 3: Combinators (`oneOf`, `anyOf`, `allOf`, `not`)

**Files:** Modify `crates/tau-runtime-core/src/schema.rs`.

**Interfaces:** Consumes Task 1–2. The fs-write-shaped test is the acceptance proof.

- [ ] **Step 1: Write failing tests (incl. an fs-write-shaped `oneOf`)**

```rust
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
    }))).unwrap();
    assert!(s.check(&v(serde_json::json!({ "mode": "write", "path": "/a" }))).is_empty());
    assert!(s.check(&v(serde_json::json!({ "mode": "edit", "old": "x" }))).is_empty());
    // matches neither branch (missing required) → violation
    assert!(!s.check(&v(serde_json::json!({ "mode": "write" }))).is_empty());
    // matches both would also fail oneOf, but const mode makes that impossible here
}

#[test]
fn any_of_all_of_not() {
    let s = compile(&v(serde_json::json!({ "anyOf": [ { "type": "string" }, { "type": "integer" } ] }))).unwrap();
    assert!(s.check(&v(serde_json::json!("x"))).is_empty());
    assert!(s.check(&v(serde_json::json!(3))).is_empty());
    assert!(!s.check(&v(serde_json::json!(true))).is_empty());

    let s = compile(&v(serde_json::json!({ "allOf": [ { "type": "integer" }, { "minimum": 0 } ] }))).unwrap();
    assert!(s.check(&v(serde_json::json!(5))).is_empty());
    assert!(!s.check(&v(serde_json::json!(-1))).is_empty());

    let s = compile(&v(serde_json::json!({ "not": { "type": "string" } }))).unwrap();
    assert!(s.check(&v(serde_json::json!(3))).is_empty());
    assert!(!s.check(&v(serde_json::json!("x"))).is_empty());
}
```

- [ ] **Step 2: Run, verify failure.** (`schema::` filter.)

- [ ] **Step 3: Compile the combinators**

In `compile_node`, before `Ok(node)`:

```rust
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
```

- [ ] **Step 4: Check the combinators**

Add a helper and calls in `check_node` (combinators evaluate sub-schemas with a throwaway buffer to count passes):

```rust
fn passes(node: &Schema, value: &Value) -> bool {
    let mut scratch = Vec::new();
    check_node(node, value, "", &mut scratch);
    scratch.is_empty()
}
```

In `check_node`, after the `additionalProperties` block:

```rust
    if let Some(subs) = &node.one_of {
        let n = subs.iter().filter(|s| passes(s, value)).count();
        if n != 1 {
            out.push(Violation { pointer: pointer.to_string(),
                message: format!("value must match exactly one oneOf branch, matched {n}") });
        }
    }
    if let Some(subs) = &node.any_of {
        if !subs.iter().any(|s| passes(s, value)) {
            out.push(Violation { pointer: pointer.to_string(),
                message: "value matched no anyOf branch".to_string() });
        }
    }
    if let Some(subs) = &node.all_of {
        for (i, s) in subs.iter().enumerate() {
            if !passes(s, value) {
                out.push(Violation { pointer: format!("{pointer}/allOf/{i}"),
                    message: "value failed an allOf branch".to_string() });
            }
        }
    }
    if let Some(sub) = &node.not {
        if passes(sub, value) {
            out.push(Violation { pointer: pointer.to_string(),
                message: "value matched a `not` schema".to_string() });
        }
    }
```

- [ ] **Step 5: Run, verify pass.**

- [ ] **Step 6: Commit** (`feat(runtime-core): no_std schema validator — oneOf/anyOf/allOf/not`).

### Task 4: Fail-closed on unsupported keywords

**Files:** Modify `crates/tau-runtime-core/src/schema.rs`.

The fail-closed logic already exists (the `SUPPORTED`/`IGNORED` check in `compile_node`). This task locks it with tests so a future keyword can't silently slip in.

- [ ] **Step 1: Write failing/locking tests**

```rust
#[test]
fn unsupported_keywords_fail_closed() {
    for kw in ["pattern", "format", "$ref", "patternProperties", "if", "dependencies"] {
        let schema = serde_json::json!({ "type": "string", kw: {} });
        let err = compile(&v(schema)).expect_err(kw);
        assert_eq!(err.keyword, kw, "error should name the offending keyword");
    }
}

#[test]
fn annotations_are_ignored_not_errors() {
    let s = compile(&v(serde_json::json!({
        "type": "string",
        "title": "Name", "description": "the name", "default": "x", "examples": ["a"]
    }))).expect("annotations must not error");
    assert!(s.check(&v(serde_json::json!("hi"))).is_empty());
}
```

- [ ] **Step 2: Run.** Expected: both pass already (Task 1 logic). If `unsupported_keywords_fail_closed` fails for `pattern`/`format` because they're not caught, confirm they're absent from `SUPPORTED`/`IGNORED` — they are, so the generic check fires. Expected: PASS.

- [ ] **Step 3: Commit** (`test(runtime-core): lock fail-closed unsupported-keyword behavior`).

### Task 5: `no_std` purity check of the new module

**Files:** none (verification only).

- [ ] **Step 1: Confirm zero `std` in the new module**

Run: `grep -nE '(^|[^a-zA-Z_])std::|use std' crates/tau-runtime-core/src/schema.rs`
Expected: no output (no matches). The module uses only `alloc::` / `core::` /
`serde_json`, so it is `no_std`-clean by construction.

- [ ] **Step 2: Confirm the crate still compiles with the module present (default features)**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-core`
Expected: clean (default features include `tool-validation`, so `schema` compiles and is exercised by the Task 1–4 tests).

NOTE: the definitive *no_std-with-validation* build proof is deferred to **Task 7 Step 4**
(`--no-default-features --features wasm-interpreter,tool-validation`), because until Task 7
removes `jsonschema`, the `tool-validation` feature still pulls std. At this task `schema`
is gated behind `tool-validation` (mirroring `tool_args`), so the `wasm-interpreter`-only
build simply omits it — no dead-code warning, nothing to prove yet.

### Task 6: Swap `ToolArgsValidator` internals + differential test

**Files:**
- Modify: `crates/tau-runtime-core/src/tool_args.rs`
- Modify: `crates/tau-runtime-core/Cargo.toml` (add `jsonschema` dev-dependency, temporary)
- Test: the existing `tool_args.rs` tests (regression net) + a new differential test.

**Interfaces:**
- Consumes: `schema::{compile, CompiledSchema, CompileErr}`.
- Produces: `ToolArgsValidator` with the SAME public API (`compile`, `validate`, `validate_tool_args`, `SchemaCompileError { kind, schema_excerpt }`).

- [ ] **Step 1: Add the differential test FIRST (jsonschema still present)**

Add `jsonschema` to `[dev-dependencies]` in `crates/tau-runtime-core/Cargo.toml`:

```toml
# TEMPORARY (EPIC 0 PR-0b): differential test only; removed in Task 7.
jsonschema = { workspace = true }
```

Add to `tool_args.rs` `tests` module:

```rust
/// Differential: the new validator must agree with jsonschema on accept/reject
/// for every (schema, args) pair. Deleted with jsonschema in Task 7.
#[test]
fn differential_against_jsonschema() {
    let cases = [
        (serde_json::json!({"type":"object","properties":{"x":{"type":"string"}},"required":["x"]}),
         serde_json::json!({"x":"ok"})),
        (serde_json::json!({"type":"object","properties":{"x":{"type":"string"}},"required":["x"]}),
         serde_json::json!({})),
        (serde_json::json!({"type":"integer","minimum":1,"maximum":10}), serde_json::json!(5)),
        (serde_json::json!({"type":"integer","minimum":1,"maximum":10}), serde_json::json!(99)),
        (serde_json::json!({"enum":["a","b"]}), serde_json::json!("c")),
        (serde_json::json!({"oneOf":[{"type":"string"},{"type":"integer"}]}), serde_json::json!("s")),
        (serde_json::json!({"type":"array","items":{"type":"integer"},"minItems":1}), serde_json::json!([1,2])),
        (serde_json::json!({"type":"array","items":{"type":"integer"}}), serde_json::json!(["bad"])),
    ];
    for (schema_json, args_json) in cases {
        let js = jsonschema::options().with_draft(jsonschema::Draft::Draft7)
            .build(&schema_json).expect("jsonschema compiles");
        let js_ok = js.is_valid(&args_json);
        let ours = super::super::schema::compile(&schema_json).expect("ours compiles");
        let our_ok = ours.check(&args_json).is_empty();
        assert_eq!(js_ok, our_ok, "divergence on schema={schema_json} args={args_json}");
    }
}
```

(Path note: `tool_args.rs` `tests` already does `use super::*`; reach the sibling module via `crate::schema`. Replace `super::super::schema` with `crate::schema` — use whichever resolves; `crate::schema` is correct.)

- [ ] **Step 2: Run the differential test against the CURRENT jsonschema-backed validator**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-runtime-core tool_args::tests::differential`
Expected: PASS (both paths exist; proves the harness itself is correct before the swap).

- [ ] **Step 3: Swap `ToolArgsValidator` internals**

In `tool_args.rs`:
- Remove `use jsonschema::{Draft, ValidationOptions, Validator};` and the `draft7_options()` fn.
- Change the field: `compiled: Option<Arc<Validator>>` → `compiled: Option<Arc<crate::schema::CompiledSchema>>`.
- Rewrite `compile` to use `crate::schema::compile`, mapping `CompileErr` into `SchemaCompileError`:

```rust
        let compiled = crate::schema::compile(&schema_json).map_err(|err| SchemaCompileError {
            kind: if err.keyword.is_empty() {
                format!("schema invalid at {}: {}", err.pointer, err.detail)
            } else {
                format!("unsupported keyword '{}' at {}: {}", err.keyword, err.pointer, err.detail)
            },
            schema_excerpt: declared_schema_json.chars().take(200).collect(),
        })?;
        Ok(Self { compiled: Some(Arc::new(compiled)), declared_schema_json })
```

- Rewrite the `validate` issue collection to use `check`, preserving the template:

```rust
        let issues: Vec<String> = compiled
            .check(&args_json)
            .into_iter()
            .map(|vio| format!("  {}: {}", vio.pointer, vio.message))
            .collect();
```

Keep the rest of `validate` (the `format!("...You sent:...Expected (input_schema):...Specific issue(s):...")` block) byte-for-byte so the MANDATORY template is unchanged.

- [ ] **Step 4: Run the full `tool_args` suite + the differential test**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-runtime-core tool_args::`
Expected: ALL pass — the pre-existing behavioral tests (missing-required, type-mismatch, integer-range, malformed-type, opt-out, wrapper) AND `differential_against_jsonschema`. This is the proof the swap is behavior-preserving.

NOTE on `compile_malformed_schema_returns_error`: it feeds `{"type":"objectt"}`. The new `parse_types` rejects unknown type names with `CompileErr{keyword:"type"}`, so `compile` returns `Err` and `err.kind.contains("compile")`... the new `kind` says `"unsupported keyword 'type'..."` — it does NOT contain "compile". **Update that test's assertion** to `assert!(err.kind.contains("type") || err.kind.contains("unsupported"))` (the test owns the contract; the schema is genuinely invalid, which is what matters).

- [ ] **Step 5: Commit** (`feat(runtime-core): back ToolArgsValidator with the no_std schema validator + differential test`).

### Task 7: Remove `jsonschema` + de-std the `tool-validation` feature

**Files:**
- Modify: `crates/tau-runtime-core/Cargo.toml`
- Modify: `crates/tau-runtime-core/src/lib.rs`
- Modify: `crates/tau-runtime-core/src/tool_args.rs` (delete differential test + module doc tweak)
- Modify: `crates/tau-runtime-core/src/{builder.rs,stream.rs}` (comment accuracy only)

- [ ] **Step 1: Delete the differential test and its dev-dep**

Remove `differential_against_jsonschema` from `tool_args.rs` and the temporary `jsonschema` line from `[dev-dependencies]`.

- [ ] **Step 2: De-std the feature + drop the dep**

In `crates/tau-runtime-core/Cargo.toml`:
- Change `tool-validation = ["wasm-interpreter", "dep:jsonschema", "tau-domain/std"]` → `tool-validation = ["wasm-interpreter"]`.
- Delete the `jsonschema = { workspace = true, optional = true }` line and its comment.

In `crates/tau-runtime-core/src/lib.rs`:
- Change `#[cfg(any(test, feature = "host-fs", feature = "tool-validation"))]` → `#[cfg(any(test, feature = "host-fs"))]` on the `extern crate std;` line.

- [ ] **Step 3: Fix now-stale comments**

- `tool_args.rs` line ~19: `//! Gated behind feature = "tool-validation" (jsonschema is std-only).` → `//! Gated behind feature = "tool-validation"; the validator is no_std (EPIC 0).`
- `stream.rs` lines ~42-46: replace the "pre-compiled jsonschema" wording with "pre-compiled no_std `ToolArgsValidator`".

- [ ] **Step 4: Verify no_std build WITH tool-validation on**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo check -p tau-runtime-core --no-default-features --features wasm-interpreter,tool-validation`
Expected: clean — proves tool validation now compiles no_std.

- [ ] **Step 5: Confirm jsonschema is gone from the crate's tree**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo tree -p tau-runtime-core -e features -i jsonschema`
Expected: errors with "package ID specification `jsonschema` did not match" or empty — i.e. jsonschema is no longer a dependency of tau-runtime-core. (It remains in the workspace for tau-cli — that's expected.)

- [ ] **Step 6: Full default-feature test run (regression)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-runtime-core`
Expected: all pass.

- [ ] **Step 7: Commit** (`refactor(runtime-core): drop jsonschema + tau-domain/std from the run path (EPIC 0 #380)`).

### Task 8: `tau check` surfaces `SchemaCompileError` (incl. unsupported keyword)

**Files:**
- Test: `crates/tau-cli/tests/` (find the existing `tau check` / build error test; add a case) — exact file located in Step 1.
- Modify: only if a gap is found.

- [ ] **Step 1: Locate where `tau check`/`build` surfaces `BuildError::ToolSchemaInvalid`**

Run: `grep -rn "ToolSchemaInvalid\|input_schema is not a valid" crates/tau-cli/`
The builder already maps `SchemaCompileError` → `BuildError::ToolSchemaInvalid { tool_name, detail }` (builder.rs:735), and `detail` now carries the unsupported-keyword message (Task 6 Step 3). Confirm `tau check` renders `BuildError` to the user.

- [ ] **Step 2: Add a test that a tool schema using an unsupported keyword fails the build with a clear message**

Add a unit test in `builder.rs` `tests` (mirrors the existing `tool_schema_invalid_*` test at builder.rs:964) that registers a MockTool whose `input_schema` contains `{"type":"string","pattern":"x"}` and asserts `build()` returns `BuildError::ToolSchemaInvalid` whose `detail` contains `pattern`:

```rust
    #[cfg(feature = "tool-validation")]
    #[test]
    fn unsupported_schema_keyword_fails_build_with_named_keyword() {
        // Build a MockTool whose input_schema uses `pattern` (unsupported v1).
        // Use the same MockTool harness as `tool_schema_invalid_*` above.
        let err = /* RuntimeBuilder with the tool */ .build().expect_err("pattern unsupported");
        let BuildError::ToolSchemaInvalid { detail, .. } = err else { panic!("got: {err:?}") };
        assert!(detail.contains("pattern"), "detail should name the keyword; got: {detail}");
    }
```

(Copy the exact MockTool construction from the neighbouring `tool_schema_invalid_*` test so the harness matches.)

- [ ] **Step 3: Run** the new test (`builder::tests::unsupported_schema_keyword`); expected PASS.

- [ ] **Step 4: Commit** (`test(runtime-core): tau check names unsupported schema keywords (#379)`).

### Task 8.5: PR-0b gate — clippy + push + PR

- [ ] **Step 1: Clippy clean (default + no_std feature sets)**

```bash
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo clippy -p tau-runtime-core --all-targets -- -D warnings
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo clippy -p tau-runtime-core --no-default-features --features wasm-interpreter,tool-validation -- -D warnings
```
Expected: clean both.

- [ ] **Step 2: Push + open PR-0b** against `main`, body closing #378/#379/#380/#381, summarizing the swap and the no_std-with-validation proof. Enroll in merge queue (`gh pr merge <#> --auto`). Wait for green before PR-0c.

---

## PR-0c — run-loop no_std CI lane + epic close-out

### Task 9: CI lane that RUNS the loop no_std with validation parity

**Files:**
- Modify: `.github/workflows/ci.yml` (the existing no_std lane, ~lines 349–400)
- Modify: the `tau-wasm-guest` build to enable `tool-validation` (locate its Cargo features / the build invocation)
- Test: a conformance/integration scenario exercising tool-arg rejection.

**Interfaces:** Consumes the now-no_std `tool-validation` feature.

- [ ] **Step 1: Enable `tool-validation` on the wasm guest**

Run: `grep -rn "wasm-interpreter\|tool-validation\|tau-runtime-core" crates/tau-wasm-guest/Cargo.toml`
Add `tool-validation` to the guest's `tau-runtime-core` feature set (now no_std-safe), so the guest validates tool args identically to the host. Verify the guest still links:

`timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo build -p tau-wasm-guest --target wasm32-wasip2 --release`
Expected: links clean (no std leakage — the forbidden-imports grep + linker would catch it).

- [ ] **Step 2: Add a no_std run-loop test that exercises validation**

Find the existing no_std run test (`grep -rn "run_ir_streaming" crates/tau-runtime-core/tests/`). Add a test (gated to run under `--no-default-features --features wasm-interpreter,tool-validation`) that drives `run_ir_streaming` with a tool whose schema rejects the LLM's first args, asserting a `BadArgs` self-correction turn appears — proving validation runs no_std.

- [ ] **Step 3: Add the CI lane**

In `.github/workflows/ci.yml`, extend the no_std job: a step that runs the run-loop test under the no_std feature set:

```yaml
      - name: run-loop no_std (with tool-validation)
        run: |
          CARGO_INCREMENTAL=0 cargo nextest run -p tau-runtime-core \
            --no-default-features --features wasm-interpreter,tool-validation \
            run_loop_no_std_validation
```

(Match the exact test name from Step 2 and the job's existing env/runner conventions.)

- [ ] **Step 4: Run the lane's commands locally**, expected green.

- [ ] **Step 5: Commit** (`ci: run the agent loop no_std with tool-validation (EPIC 0 #382)`).

### Task 10: Close out EPIC 0

**Files:** Modify `ROADMAP.md`.

- [ ] **Step 1: Update the EPIC 0 status** in `ROADMAP.md` (and the Phase 2 §C.1 / no_std readiness notes if present) to reflect the run loop now compiling AND running no_std with validation intact; reference the merged PRs.

- [ ] **Step 2: mdbook build clean** (`cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build`; then `rm -rf docs/book`). Expected `[INFO]`-only.

- [ ] **Step 3: Commit** (`docs(roadmap): EPIC 0 de-std run loop shipped`).

- [ ] **Step 4: Push + open PR-0c**, body closing #382 and noting the epic DoD met; enroll in merge queue.

---

## Self-Review

**1. Spec coverage:**
- §2 feature/attribute changes (drop jsonschema + tau-domain/std; lib.rs cfg) → Task 7. ✓
- §3 validator types/interfaces (`compile`/`validate`, preserved `ToolArgsValidator`/`SchemaCompileError`) → Tasks 1–6 (note: spec named `CompiledSchema`/`ArgsViolation` as illustrative; the plan keeps the actual existing public names `ToolArgsValidator`/`SchemaCompileError` and uses `CompiledSchema`/`Violation` only as the internal no_std type — documented in Task 6). ✓
- §3.1 full subset incl. combinators + ignored annotations → Tasks 1–3. ✓
- §3.2 fail-closed unsupported keywords → Tasks 1 (logic) + 4 (lock). ✓
- §4 audit → already done (spec §4); no plan task needed. ✓
- §5 differential test + per-keyword tests + conformance parity → Tasks 2/3 (per-keyword), 6 (differential), 9 (conformance parity). ✓
- §6 PR sequencing 0b/0c → task grouping. ✓
- Stories 0.1 (done), 0.4 (done) → closed by reference in PR-0b body (Task 8.5). 0.3 → Task 7. 0.5 → Task 9. ✓

**2. Placeholder scan:** Task 8 Step 2 and Task 9 Steps 1–3 reference "copy the exact MockTool harness from the neighbouring test" / "match the exact test name" — these point at concrete existing code located by a grep in the same step, not vague instructions. All validator code is complete and verbatim. No TBD/TODO.

**3. Type consistency:** `compile` / `check` / `CompiledSchema` / `Violation` / `CompileErr` (internal, schema.rs) and `ToolArgsValidator` / `SchemaCompileError { kind, schema_excerpt }` / `validate_tool_args` (public, tool_args.rs) are used consistently across Tasks 1–8. The internal `CompileErr` is mapped to the public `SchemaCompileError` in Task 6 Step 3. `Schema` field names match between `compile_node` (writes) and `check_node` (reads).

**Known follow-up:** the spec's §3 illustrative names (`CompiledSchema::validate`, `ArgsViolation`) differ from the implemented names (`CompiledSchema::check`, `Violation`); the public `ToolArgsValidator::validate` is unchanged. This is intentional (reuse the existing public surface) and noted in Task 6.
