# EPIC 2.2 — IR JSON Schema + Conformance Kit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish a drift-tested JSON Schema generated from the `tau-ir` serde types (version-pinned by `ir_format`) plus a portable valid/invalid conformance kit a non-Rust frontend author can validate against.

**Architecture:** Add a std-only `schema` Cargo feature that gates `schemars` `JsonSchema` derives across `tau-ports` + `tau-domain` + `tau-ir` (every type reachable from `IrModule`). Custom-serde types get hand-written `JsonSchema` impls so the schema matches the real wire format. A generator emits `schemas/ir/tau-ir.v2.2.0.schema.json`; a drift test asserts it equals a fresh regeneration; a validate test (via `jsonschema 0.46`) runs the conformance kit. The `schema` feature never compiles in the default no_std/wasm path.

**Tech Stack:** Rust, `schemars` 1.x (JSON Schema draft 2020-12), `serde_json`, `jsonschema` 0.46, mdbook.

## Global Constraints

- **CARGO RULES (repo CLAUDE.md) — every cargo command:** prefix `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl`, scope with `-p <crate>`, never bare/`--workspace`. Timeouts: test 300s, build/check 180s. Use a distinct target dir if another build is active.
- **schemars version:** `schemars` 1.x, configured for **JSON Schema draft 2020-12** (`SchemaSettings::draft2020_12()`). Added to `[workspace.dependencies]`; pulled only by the `schema` feature.
- **`schema` feature is std-only and opt-in.** It must NOT be in any crate's `default` features and must NOT leak into the no_std/wasm build. `tau-ir` default stays `default = []`.
- **IR version is single-sourced:** the published filename segment and `x-tau-ir-format` come from `tau_ir::module::IrFormatVersion::CURRENT` (currently `"v2.2.0"`), never typed by hand.
- **Schema identity (verbatim):**
  - `$schema` = `https://json-schema.org/draft/2020-12/schema`
  - `$id` = `https://lebocqtitouan.github.io/tau/schemas/ir/v2.2.0/tau-ir.schema.json`
  - `title` = `tau IR module (ir_format v2.2.0)`
  - `x-tau-ir-format` = value of `IrFormatVersion::CURRENT`
- **Published file path:** `schemas/ir/tau-ir.v2.2.0.schema.json` (pretty-printed, trailing newline).
- **Conformance kit path:** `schemas/ir/conformance/{README.md, valid/*.json, invalid/*.json}`.
- **Commit identity:** `git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit` (drop `--no-verify` for code tasks so hooks run; keep it only if a hook corrupts identity — see CLAUDE.md).
- **Branch:** `feat/epic-2-2-ir-json-schema` (already checked out). Do not rename.
- **Source-of-truth for custom serde:** `Capability` → `crates/tau-domain/src/package/capability.rs` (`impl Serialize`); `MessageId` → `crates/tau-domain/src/id.rs`; `TargetTriple` → `crates/tau-ports/src/target/triple.rs`.

---

### Task 1: Reachable-type inventory + feature/dependency scaffolding

**Files:**
- Modify: `Cargo.toml` (add `schemars` to `[workspace.dependencies]`)
- Modify: `crates/tau-ports/Cargo.toml`, `crates/tau-domain/Cargo.toml`, `crates/tau-ir/Cargo.toml` (declare `schema` feature)
- Create: `schemas/ir/REACHABLE-TYPES.md` (the inventory artifact)

**Interfaces:**
- Produces: the authoritative list of every type reachable from `IrModule` when serialized, each flagged `derive` (plain serde) or `hand` (custom serde → needs a hand-written `JsonSchema` impl). Tasks 2–4 consume this list.

- [ ] **Step 1: Enumerate reachable types**

Starting from `tau_ir::module::IrModule`, walk every field/variant type transitively (use `crates/tau-ir/src/*.rs` plus the foreign imports). For each foreign type, check whether its crate defines a hand-written `impl Serialize`/`Deserialize` (custom serde) or derives it. Record findings in `schemas/ir/REACHABLE-TYPES.md` as a table:

```markdown
# IR schema — reachable type inventory (from IrModule)

| type | crate | serde | schema strategy |
|---|---|---|---|
| IrModule | tau-ir | derive | cfg_attr derive |
| IrFormatVersion | tau-ir | derive (newtype String) | cfg_attr derive |
| TargetTriple | tau-ports | **custom** (string) | hand impl |
| Capability | tau-domain | **custom** (oneOf by "kind") | hand impl |
| MessageId | tau-domain | **custom** (uuid string) | hand impl |
| ... (complete the walk) ... | | | |
```

Known custom-serde types confirmed present: `TargetTriple`, `Capability`, `MessageId`. Also check the `tau-ir` id newtypes (`crates/tau-ir/src/ids.rs`) and any `tau-domain` ids reachable — flag any with a hand `impl Serialize`.

- [ ] **Step 2: Add `schemars` to the workspace**

In `Cargo.toml` `[workspace.dependencies]`, after the `jsonschema` line, add:

```toml
schemars        = { version = "1", default-features = false }
```

- [ ] **Step 3: Declare the `schema` feature in each crate (no derives yet)**

In `crates/tau-ports/Cargo.toml` add `schemars` as an optional dep and a `schema` feature:

```toml
# under [dependencies]
schemars = { workspace = true, optional = true }

# under [features]
schema = ["dep:schemars"]
```

In `crates/tau-domain/Cargo.toml` likewise, plus turn on no upstream feature (domain has no schema-bearing deps):

```toml
schemars = { workspace = true, optional = true }
# [features]
schema = ["dep:schemars"]
```

In `crates/tau-ir/Cargo.toml` likewise, propagating to the two upstream crates:

```toml
schemars = { workspace = true, optional = true }
# [features]
schema = ["dep:schemars", "tau-domain/schema", "tau-ports/schema"]
```

- [ ] **Step 4: Verify the feature compiles (no derives yet)**

Run:

```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ir --features schema
```

Expected: compiles. `schemars` resolves but is unused so far — if a deny-level `unused_crate_dependencies` lint fires, ignore until Task 4 wires usage (do not add `#[allow]`; the derives land this session). Also confirm the default build is untouched:

```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ir
```

Expected: compiles, no `schemars` in the dependency graph.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/tau-ports/Cargo.toml crates/tau-domain/Cargo.toml crates/tau-ir/Cargo.toml schemas/ir/REACHABLE-TYPES.md
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(epic-2.2): schema feature scaffolding + reachable-type inventory"
```

---

### Task 2: Hand-written `JsonSchema` for `TargetTriple` (tau-ports)

**Files:**
- Modify: `crates/tau-ports/src/target/triple.rs`
- Test: same file (`#[cfg(test)]`)

**Interfaces:**
- Produces: `impl schemars::JsonSchema for TargetTriple` (behind `feature = "schema"`), emitting `{"type":"string"}`. Consumed transitively by Task 4's `schema_for!(IrModule)`.

- [ ] **Step 1: Write the failing test**

Add to `crates/tau-ports/src/target/triple.rs`:

```rust
#[cfg(all(test, feature = "schema"))]
mod schema_tests {
    use super::*;
    #[test]
    fn target_triple_schema_is_string() {
        let schema = schemars::schema_for!(TargetTriple);
        let v = serde_json::to_value(&schema).unwrap();
        assert_eq!(v["type"], "string");
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ports --features schema schema_tests
```

Expected: FAIL to compile — `TargetTriple` does not implement `JsonSchema`.

- [ ] **Step 3: Implement the hand-written schema**

In `crates/tau-ports/src/target/triple.rs`, near the existing `impl serde::Serialize for TargetTriple`:

```rust
#[cfg(feature = "schema")]
impl schemars::JsonSchema for TargetTriple {
    fn schema_name() -> alloc::borrow::Cow<'static, str> {
        "TargetTriple".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "title": "target triple",
            "description": "tau target triple, e.g. \"x86_64-unknown-linux-native-strict\""
        })
    }
}
```

(If the crate is `no_std`, ensure `extern crate alloc;` is in scope — it already is for the serde impls.)

- [ ] **Step 4: Run the test to verify it passes**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ports --features schema schema_tests
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ports/src/target/triple.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(epic-2.2): JsonSchema impl for TargetTriple"
```

---

### Task 3: Hand-written `JsonSchema` for custom-serde tau-domain types

**Files:**
- Modify: `crates/tau-domain/src/id.rs` (`MessageId`, plus any reachable id newtype flagged `hand` in Task 1)
- Modify: `crates/tau-domain/src/package/capability.rs` (`Capability`)
- Test: each file (`#[cfg(test)]`)

**Interfaces:**
- Consumes: the Task 1 inventory (which domain types are `hand`).
- Produces: `impl schemars::JsonSchema` for `MessageId` (uuid string) and `Capability` (`oneOf` keyed by `"kind"`), behind `feature = "schema"`. Consumed by Task 4.

- [ ] **Step 1: Write the failing tests**

In `crates/tau-domain/src/id.rs`:

```rust
#[cfg(all(test, feature = "schema"))]
mod schema_tests {
    use super::*;
    #[test]
    fn message_id_schema_is_uuid_string() {
        let v = serde_json::to_value(&schemars::schema_for!(MessageId)).unwrap();
        assert_eq!(v["type"], "string");
        assert_eq!(v["format"], "uuid");
    }
}
```

In `crates/tau-domain/src/package/capability.rs`:

```rust
#[cfg(all(test, feature = "schema"))]
mod schema_tests {
    use super::*;
    #[test]
    fn capability_schema_is_oneof_tagged_by_kind() {
        let v = serde_json::to_value(&schemars::schema_for!(Capability)).unwrap();
        let variants = v["oneOf"].as_array().expect("oneOf present");
        // every variant pins a const "kind" — the discriminator the serialize impl emits
        assert!(variants.iter().all(|b| b["properties"]["kind"].get("const").is_some()));
        // a real fs.read capability validates against the generated schema (Task 6 expands this)
        assert!(variants.iter().any(|b| b["properties"]["kind"]["const"] == "fs.read"));
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-domain --features schema schema_tests
```

Expected: FAIL to compile — neither type implements `JsonSchema`.

- [ ] **Step 3a: Implement `MessageId`**

In `crates/tau-domain/src/id.rs`, alongside its serde impls:

```rust
#[cfg(feature = "schema")]
impl schemars::JsonSchema for MessageId {
    fn schema_name() -> alloc::borrow::Cow<'static, str> { "MessageId".into() }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "string", "format": "uuid" })
    }
}
```

Apply the same pattern to any other id newtype Task 1 flagged `hand` (string newtypes → `{"type":"string"}`; uuid-backed → add `"format":"uuid"`).

- [ ] **Step 3b: Implement `Capability`**

In `crates/tau-domain/src/package/capability.rs`. The schema is a `oneOf` with **one branch per match arm in `impl Serialize for Capability`** (in this same file). Each branch is an object that pins `kind` to the arm's literal and lists that arm's fields. Worked template (two arms shown — `fs.read` and `net.http`); add the remaining arms by reading the `serialize` match:

```rust
#[cfg(feature = "schema")]
impl schemars::JsonSchema for Capability {
    fn schema_name() -> alloc::borrow::Cow<'static, str> { "Capability".into() }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "title": "capability",
            "oneOf": [
                {
                    "type": "object",
                    "required": ["kind", "paths"],
                    "additionalProperties": false,
                    "properties": {
                        "kind":  { "const": "fs.read" },
                        "paths": { "type": "array", "items": { "type": "string" } }
                    }
                },
                {
                    "type": "object",
                    "required": ["kind", "paths"],
                    "additionalProperties": false,
                    "properties": {
                        "kind":      { "const": "fs.write" },
                        "paths":     { "type": "array", "items": { "type": "string" } },
                        "max_bytes": { "type": "integer", "minimum": 0 }
                    }
                },
                {
                    "type": "object",
                    "required": ["kind", "hosts", "methods"],
                    "additionalProperties": false,
                    "properties": {
                        "kind":    { "const": "net.http" },
                        "hosts":   { "type": "array", "items": { "type": "string" } },
                        "methods": { "type": "array", "items": { "type": "string" } }
                    }
                }
                // ... one branch per remaining Capability::serialize arm (fs.exec, etc.).
                // Completeness is enforced by Task 6: a sample per kind must validate.
            ]
        })
    }
}
```

The `kind` string literals MUST match the `serialize_entry("kind", "...")` values exactly (e.g. `fs.read`, `fs.write`, `fs.exec`, `net.http`, …). Optional fields (e.g. `max_bytes`) are listed in `properties` but omitted from `required`.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-domain --features schema schema_tests
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-domain/src/id.rs crates/tau-domain/src/package/capability.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(epic-2.2): JsonSchema impls for custom-serde domain types (Capability, MessageId)"
```

---

### Task 4: `cfg_attr` derives across `tau-ir` + `schema_for!(IrModule)`

**Files:**
- Modify: every `crates/tau-ir/src/*.rs` defining a type in the Task 1 inventory flagged `derive`
- Test: `crates/tau-ir/src/module.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: Tasks 2–3 (`JsonSchema` for `TargetTriple`, `Capability`, `MessageId`); Task 1 inventory.
- Produces: `schemars::schema_for!(IrModule)` compiles and yields an object schema. Consumed by Task 5's generator.

- [ ] **Step 1: Write the failing test**

In `crates/tau-ir/src/module.rs`:

```rust
#[cfg(all(test, feature = "schema"))]
mod schema_tests {
    use super::*;
    #[test]
    fn ir_module_schema_builds_and_is_object() {
        let v = serde_json::to_value(&schemars::schema_for!(IrModule)).unwrap();
        assert_eq!(v["type"], "object");
        // ir_format is a required top-level property
        let req = v["required"].as_array().expect("required present");
        assert!(req.iter().any(|x| x == "ir_format"));
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ir --features schema schema_tests
```

Expected: FAIL to compile — `IrModule` (and its field types) lack `JsonSchema`.

- [ ] **Step 3: Add `cfg_attr` derives to every `derive`-flagged tau-ir type**

For each `#[derive(...Serialize, Deserialize...)]` on a type in the inventory, add the schemars derive guarded by the feature. Pattern (shown on `IrModule`; apply to all):

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct IrModule { /* unchanged */ }
```

Apply across `module.rs`, `node.rs`, `pipeline.rs`, `capability.rs`, `subflow.rs`, `trigger.rs`, `model_ref.rs`, `ids.rs`, `durable.rs`, `budget.rs`, `template.rs`, `tool_impl.rs`, `message.rs`, `context.rs`, `check.rs` — every type the inventory lists as `derive`. Do NOT add it to types flagged `hand` (they have explicit impls). For any field typed as a custom-serde foreign type, no extra annotation is needed — the hand impl from Tasks 2–3 is picked up automatically.

If a derived type uses `#[serde(...)]` attributes that change the wire shape (e.g. `skip_serializing_if`, `default`, `rename`, `tag`), mirror them with `#[schemars(...)]` where schemars does not read the serde attribute automatically (schemars reads most `#[serde(...)]` attrs natively — only add `#[schemars(...)]` if the generated schema diverges, which Task 6's validate test will reveal).

- [ ] **Step 4: Run the test to verify it passes**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ir --features schema schema_tests
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ir/src
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(epic-2.2): cfg_attr JsonSchema derives across tau-ir IR types"
```

---

### Task 5: Schema generator + checked-in artifact + drift test

**Files:**
- Create: `crates/tau-ir/tests/schema_export.rs`
- Create: `schemas/ir/tau-ir.v2.2.0.schema.json` (generated)

**Interfaces:**
- Consumes: Task 4 (`schema_for!(IrModule)`).
- Produces: the published schema file + a drift test asserting it equals a fresh regeneration. The generation logic is a single function `generate_ir_schema() -> serde_json::Value` reused by both the writer and the drift test (so they cannot diverge).

- [ ] **Step 1: Write the generator + drift test (failing)**

Create `crates/tau-ir/tests/schema_export.rs`:

```rust
//! Generates the published IR JSON Schema and guards it against drift.
//! Regenerate after an intended IR change with: UPDATE_SCHEMA=1 cargo test -p tau-ir --features schema --test schema_export
#![cfg(feature = "schema")]

use std::path::PathBuf;
use tau_ir::module::{IrFormatVersion, IrModule};

fn schema_path() -> PathBuf {
    // CARGO_MANIFEST_DIR = crates/tau-ir ; the repo root is two levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schemas/ir/tau-ir.v2.2.0.schema.json")
}

/// Single source of the schema bytes — used by both the writer and the drift check.
fn generate_ir_schema() -> serde_json::Value {
    let settings = schemars::generate::SchemaSettings::draft2020_12();
    let generator = settings.into_generator();
    let schema = generator.into_root_schema_for::<IrModule>();
    let mut v = serde_json::to_value(&schema).unwrap();
    let obj = v.as_object_mut().unwrap();
    obj.insert("$id".into(),
        "https://lebocqtitouan.github.io/tau/schemas/ir/v2.2.0/tau-ir.schema.json".into());
    obj.insert("title".into(), "tau IR module (ir_format v2.2.0)".into());
    obj.insert("x-tau-ir-format".into(), IrFormatVersion::CURRENT.into());
    v
}

fn pretty(v: &serde_json::Value) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap();
    s.push('\n');
    s
}

#[test]
fn schema_matches_checked_in_file() {
    let generated = pretty(&generate_ir_schema());
    if std::env::var("UPDATE_SCHEMA").is_ok() {
        std::fs::create_dir_all(schema_path().parent().unwrap()).unwrap();
        std::fs::write(schema_path(), &generated).unwrap();
        return;
    }
    let on_disk = std::fs::read_to_string(schema_path())
        .expect("schemas/ir/tau-ir.v2.2.0.schema.json missing — run with UPDATE_SCHEMA=1");
    assert_eq!(generated, on_disk,
        "published IR schema drifted from the serde types; regenerate with UPDATE_SCHEMA=1");
}
```

(Verify the exact schemars 1.x generator API while implementing — `SchemaSettings::draft2020_12()` and `into_root_schema_for` are the 1.x entry points; if the installed 1.x patch renames them, use the equivalent and keep draft 2020-12.)

- [ ] **Step 2: Run it to verify it fails**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ir --features schema --test schema_export
```

Expected: FAIL — the schema file does not exist yet.

- [ ] **Step 3: Generate the checked-in schema**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl UPDATE_SCHEMA=1 cargo test -p tau-ir --features schema --test schema_export
```

Then inspect `schemas/ir/tau-ir.v2.2.0.schema.json` — confirm `$id`, `title`, `x-tau-ir-format` = `v2.2.0`, `$schema` draft 2020-12, and that `ir_format`/`workflow`/`target` appear as properties.

- [ ] **Step 4: Run the drift test (now passing)**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ir --features schema --test schema_export
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ir/tests/schema_export.rs schemas/ir/tau-ir.v2.2.0.schema.json
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(epic-2.2): generate + drift-test published IR JSON schema"
```

---

### Task 6: Conformance kit + validate test

**Files:**
- Create: `schemas/ir/conformance/README.md`
- Create: `schemas/ir/conformance/valid/{minimal,agents-tools,triggers,durable}.json`
- Create: `schemas/ir/conformance/invalid/{missing-ir-format,unknown-node-kind}.json`
- Create: `crates/tau-ir/tests/schema_conformance.rs`

**Interfaces:**
- Consumes: Task 5 (the checked-in schema file).
- Produces: the portable kit + a test validating every `valid/*` and rejecting every `invalid/*`.

- [ ] **Step 1: Author the valid samples**

Build them from real `IrModule` values rather than by hand where possible: a small helper test can serialize a fixture and you copy the JSON. Each must be a complete, schema-valid `IrModule`. `valid/minimal.json` is the smallest legal module:

```json
{
  "ir_format": "v2.2.0",
  "tau_version": "0.0.0",
  "target": "x86_64-unknown-linux-native-strict",
  "workflow": { "agents": {}, "tools": {} }
}
```

`agents-tools.json` must include at least one agent and one tool node exercising the capability table (so a `Capability` value — e.g. `{"kind":"fs.read","paths":["/tmp"]}` — appears and exercises Task 3's hand impl). `triggers.json` includes a non-empty `triggers` array. `durable.json` sets the agent `durable` field. Derive each from a constructed `IrModule` (reuse `tau-ir` / `tau-ir-conformance` fixtures) and confirm it round-trips through `serde_json` before saving. **Cover every `Capability` kind across the valid samples** — that is what enforces Task 3's `oneOf` completeness.

- [ ] **Step 2: Author the invalid samples**

`invalid/missing-ir-format.json` — a module object with the `ir_format` key removed:

```json
{
  "tau_version": "0.0.0",
  "target": "x86_64-unknown-linux-native-strict",
  "workflow": { "agents": {}, "tools": {} }
}
```

`invalid/unknown-node-kind.json` — a capability (or node) with a `kind` not in the schema:

```json
{
  "ir_format": "v2.2.0",
  "tau_version": "0.0.0",
  "target": "x86_64-unknown-linux-native-strict",
  "workflow": {
    "agents": {},
    "tools": {
      "t0": { "id": "t0", "impl": { "kind": "not.a.real.kind" } }
    }
  }
}
```

(Adjust the exact node shape to the real `Tool` schema from Task 5's output so that *only* the `kind` is the violation.)

- [ ] **Step 3: Write the validate test (failing)**

Create `crates/tau-ir/tests/schema_conformance.rs`:

```rust
//! Validates the conformance kit against the published schema.
#![cfg(feature = "schema")]

use std::path::PathBuf;

fn dir() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas/ir") }

fn load(p: &str) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir().join(p)).unwrap()).unwrap()
}

fn compiled() -> jsonschema::Validator {
    jsonschema::validator_for(&load("tau-ir.v2.2.0.schema.json")).expect("schema compiles")
}

#[test]
fn valid_samples_validate() {
    let v = compiled();
    for name in ["minimal", "agents-tools", "triggers", "durable"] {
        let inst = load(&format!("conformance/valid/{name}.json"));
        assert!(v.is_valid(&inst), "valid/{name}.json should validate");
    }
}

#[test]
fn invalid_samples_are_rejected() {
    let v = compiled();
    for name in ["missing-ir-format", "unknown-node-kind"] {
        let inst = load(&format!("conformance/invalid/{name}.json"));
        assert!(!v.is_valid(&inst), "invalid/{name}.json should be rejected");
    }
}
```

- [ ] **Step 4: Run it to verify it fails, then passes**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ir --features schema --test schema_conformance
```

First run: expect failures if any sample is wrong — fix the *sample* (or, if a real schema bug surfaces, the relevant `JsonSchema` impl + regenerate via `UPDATE_SCHEMA=1`) until both tests PASS. This loop is the custom-serde safety net.

- [ ] **Step 5: Write the kit README**

Create `schemas/ir/conformance/README.md`:

```markdown
# tau IR conformance kit

Validate any tool's generated IR against the published schema in any language:

1. Take the schema: `../tau-ir.v2.2.0.schema.json` (JSON Schema draft 2020-12).
2. Validate your generated `IrModule` JSON with any draft-2020-12 validator
   (Rust `jsonschema`, JS `ajv`, Python `jsonschema`, …).
3. `valid/*.json` are modules that MUST validate; `invalid/*.json` MUST be
   rejected. Run your validator over both sets to prove conformance.

The schema is generated from the `tau-ir` Rust serde types and is byte-stable
per `ir_format` version (see ADR-0056). Pin the version segment in the filename.
```

- [ ] **Step 6: Commit**

```bash
git add schemas/ir/conformance crates/tau-ir/tests/schema_conformance.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(epic-2.2): IR schema conformance kit (valid/invalid samples + validate test)"
```

---

### Task 7: Docs reference page + CI lane

**Files:**
- Create: `docs/reference/ir-json-schema.md`
- Modify: `docs/SUMMARY.md`
- Modify: `.github/workflows/<the main CI workflow>` (add a `--features schema` job)

**Interfaces:**
- Consumes: Tasks 5–6 (schema + tests).
- Produces: the documented "published" surface + CI enforcement of drift/validate.

- [ ] **Step 1: Write the reference page**

Create `docs/reference/ir-json-schema.md`:

```markdown
# IR JSON Schema

tau publishes the authoring contract — the IR — as a JSON Schema generated from
the `tau-ir` serde types (ADR-0056). It is version-pinned by `ir_format`.

- **Schema:** [`schemas/ir/tau-ir.v2.2.0.schema.json`](https://github.com/tau-rs/tau/blob/main/schemas/ir/tau-ir.v2.2.0.schema.json)
- **`$id`:** `https://lebocqtitouan.github.io/tau/schemas/ir/v2.2.0/tau-ir.schema.json`
- **Draft:** JSON Schema 2020-12.

The schema is drift-tested byte-equal to a fresh regeneration, so it is provably
the serde types. Frontend / SDK authors validate generated IR against it; the
[conformance kit](https://github.com/tau-rs/tau/tree/main/schemas/ir/conformance)
ships `valid/` and `invalid/` samples for any-language conformance.
```

- [ ] **Step 2: Add it to SUMMARY.md**

In `docs/SUMMARY.md`, under the reference section (find an existing `reference/` entry and add a sibling line):

```
- [IR JSON Schema](reference/ir-json-schema.md)
```

- [ ] **Step 3: Build the book**

```bash
cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build && cd ..
rm -rf docs/book
```

Expected: only `[INFO]` lines, exit 0.

- [ ] **Step 4: Add the CI lane**

Identify the main CI workflow (`ls .github/workflows/`; the test/check workflow, e.g. `ci.yml`). Add a job mirroring the existing check jobs but enabling the feature and running the two new test files:

```yaml
  schema-conformance:
    name: IR schema (drift + conformance)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test -p tau-ir --features schema --test schema_export --test schema_conformance
```

Match the existing workflow's toolchain action, caching, and `CARGO_*` env conventions (copy from a neighboring job — do not invent new ones). Validate YAML with `python -c "import yaml,sys;yaml.safe_load(open('.github/workflows/<file>'))"`.

- [ ] **Step 5: Commit**

```bash
git add docs/reference/ir-json-schema.md docs/SUMMARY.md .github/workflows
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "docs(epic-2.2): IR schema reference page + CI conformance lane"
```

---

## Self-Review

**Spec coverage:** schema feature plumbing (spec unit 1) → Task 1; generator + `$id`/version (unit 2) → Task 5; drift test (unit 3) → Task 5; conformance kit valid/invalid (unit 4) → Task 6; validate test + custom-serde safety net (unit 5) → Task 6; published surface + docs page (unit 6) → Task 7; reachable-type inventory (the sizing gate) → Task 1; hand-written `JsonSchema` for `TargetTriple`/`MessageId`/`Capability` (spec's named risk) → Tasks 2–3; CI lane + no-leak guard (consequences) → Task 7 + Task 1 Step 4. No gaps.

**Placeholder scan:** no TBD/TODO. The two deliberate discovery points — the full reachable-type list (Task 1) and the remaining `Capability` `oneOf` arms (Task 3) — are bounded by an explicit procedure (read the named source file) plus a test that enforces completeness (Task 6 covers every capability kind), not left vague. The CI-workflow filename is resolved in Task 7 Step 4 against `.github/workflows/`.

**Type consistency:** `generate_ir_schema()`, `IrFormatVersion::CURRENT`, the `$id`/`title`/`x-tau-ir-format` literals, and the `schema` feature name are identical across Tasks 4–7. `jsonschema::validator_for` / `is_valid` are the 0.46 API. Schema filename `tau-ir.v2.2.0.schema.json` matches the Global Constraints path everywhere.
