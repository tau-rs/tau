# Agent `output_schema` Field Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an optional `output_schema: Option<serde_json::Value>` field to agents, plumbed end-to-end through tau-pkg → tau-ir → tau-ts-extract → conformance, so a later "judge-compat build-time warning" task has the data to cross-check. This task only adds the field; it does NOT add the judge cross-check.

**Architecture:** Additive, byte-stable plumbing. Each layer mirrors how the existing `output_schema` on `[steps.*]` is handled: a pass-through `serde_json::Value` with no deep validation. The IR `Agent` node gains the field behind `#[serde(default, skip_serializing_if = "Option::is_none")]` so trigger-less / schema-less modules serialize byte-identically to before. The IR format version bumps `v1.2.0` → `v1.3.0` (MINOR / additive per ADR-0006 semver discipline — old readers ignore the new key, new readers handle both).

**Tech Stack:** Rust (workspace of 8 crates), serde / serde_json, swc (TS AST), `cargo nextest`.

**CARGO RULES (from CLAUDE.md — every cargo command):**
`timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <cmd> -p <crate>` — never bare cargo, always `-p` single crate, always `timeout`, prefer `cargo nextest run`. Timeouts: test 300s, build/check 180s, clippy 240s.

---

## Design Decisions (locked)

1. **`Agent` KEEPS its `Eq` derive.** (Corrected during Task 2 review — the original plan wrongly claimed `Eq` had to be dropped.) `serde_json::Value` *does* implement `Eq` (serde_json provides `impl Eq for Value`), so `Agent` with an `Option<serde_json::Value>` field compiles unchanged with `#[derive(..., Eq, PartialEq, ...)]`. Verified by compilation. Dropping `Eq` would have been a needless public-API regression, so we leave the derive intact.

2. **`Agent` is NOT `#[non_exhaustive]`** and is constructed by struct-literal in ~12 sites across the workspace. Every literal must gain `output_schema: <value>`. The full list is enumerated in Task 2, Step 5. The lowering site (`parse.rs`) gets the real value; every test/other site gets `None`.

3. **Byte-stability:** `#[serde(default, skip_serializing_if = "Option::is_none")]` on the IR field means every existing fixture's canonical bytes are unchanged (the `output_schema` key is absent when `None`). This is what makes the version bump a legitimate MINOR.

4. **TS emission via TOML text:** `tau-ts-extract` deliberately routes through `ProjectConfig::parse_str` (never constructs `#[non_exhaustive]` tau-pkg structs). So the extractor emits `output_schema = { … }` as a TOML inline table. We hand-roll a small `json_to_toml_inline(&serde_json::Value) -> String` (serde_json `Map` is a `BTreeMap` without `preserve_order`, so key order is deterministic/alphabetical → stable output). The conformance gate compares **canonical IR bytes**, not TOML text, so the only requirement on the emitted TOML is that `parse_str` reads it back to the identical `serde_json::Value`.

5. **`expected_report.json` is not read by any code** (verified: `grep -rn expected_report` finds only the fixture files themselves). The real conformance gate is the inline assertions in `tests/conformance.rs` + cross-mode `assert_conform`. We still add a fresh `expected_report.json` to the new fixture for documentation consistency.

---

## File Structure

| File | Change |
|---|---|
| `crates/tau-pkg/src/project/project.rs` | `UncheckedAgent` + `AgentEntry` + `validate_agent` gain `output_schema` |
| `crates/tau-ir/src/node.rs` | `Agent` gains field; drop `Eq`; update 2 in-file tests; add 2 tests |
| `crates/tau-ir/src/module.rs` | `IrFormatVersion::CURRENT` → `v1.3.0`; update version unit test |
| `crates/tau-ir/src/lower/parse.rs` | map `entry.output_schema` → `agent.output_schema` |
| `crates/tau-ir/src/lower/typecheck.rs` | add `output_schema: None` to 2 test literals |
| `crates/tau-ir/tests/lower_e2e.rs` | new assertion: agent output_schema survives lowering |
| `crates/tau-runtime-core/tests/*.rs` | add `output_schema: None` to 5 test literals |
| `crates/tau-runtime-core/src/interpreter/check.rs` | add `output_schema: None` to 1 literal |
| `crates/tau-runtime-tokio/tests/ir_smoke.rs` | add `output_schema: None` to 1 literal |
| `crates/tau-ts-extract/src/lower.rs` | `IrAgent.output_schema`; extract `outputSchema`; emit inline TOML; `json_to_toml_inline` + `expr_to_json` helpers |
| `crates/tau-ts-extract/tests/output_schema_conformance.rs` | new TOML↔TS byte-equal test |
| `crates/tau-ir-conformance/fixtures/14_agent_output_schema/` | new fixture (workflow.toml + mock_llm.jsonl + expected_report.json) |
| `crates/tau-ir-conformance/tests/conformance.rs` | new fixture 14 dev-mode + cross-mode tests |
| `docs/decisions/0049-agent-output-schema.md` | ADR noting the additive `v1.3.0` bump |
| `docs/SUMMARY.md` | link the new ADR |

---

## Task 1: tau-pkg — `output_schema` on agent config

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs` (`UncheckedAgent` ~:54, `AgentEntry` ~:719, `AgentEntry::new` ~:765, `validate_agent` ~:1192/1386)
- Test: inline `#[cfg(test)]` module in the same file (follow the existing `mod tests` there)

- [ ] **Step 1: Write the failing test**

Add to the existing test module in `crates/tau-pkg/src/project/project.rs` (find `mod tests` — append there):

```rust
#[test]
fn agent_output_schema_parses_and_passes_through() {
    let toml = r#"
[project]
name = "p"

[agents.judge]
display_name = "Judge"
package = "p@^0.1"
llm_backend = "mock"
output_schema = { type = "object", required = ["verdict"] }
"#;
    let cfg = ProjectConfig::parse_str(toml).expect("parse");
    let agent = cfg.agents.get("judge").expect("agent present");
    let schema = agent.output_schema.as_ref().expect("output_schema present");
    assert_eq!(schema["type"], serde_json::json!("object"));
    assert_eq!(schema["required"], serde_json::json!(["verdict"]));
}

#[test]
fn agent_without_output_schema_is_none() {
    let toml = r#"
[project]
name = "p"

[agents.plain]
display_name = "Plain"
package = "p@^0.1"
llm_backend = "mock"
"#;
    let cfg = ProjectConfig::parse_str(toml).expect("parse");
    assert!(cfg.agents.get("plain").unwrap().output_schema.is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg agent_output_schema_parses_and_passes_through agent_without_output_schema_is_none`
Expected: FAIL — `no field 'output_schema' on type '&AgentEntry'` (compile error).

- [ ] **Step 3: Add the field to `UncheckedAgent`**

In `UncheckedAgent` (~:100, after the `credentials` field, keep it grouped with the IR-lowering fields), add:

```rust
    /// JSON schema describing this agent's structured output. Pass-through
    /// (no deep validation) — mirrors `[steps.<name>].output_schema`. Used
    /// by the IR lowering pass and a later judge-compat build-time check.
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
```

- [ ] **Step 4: Add the field to `AgentEntry` + `AgentEntry::new`**

In `AgentEntry` (~:757, after `credentials`), add:

```rust
    /// JSON schema describing this agent's structured output (IR lowering
    /// use). `None` = unspecified. Pass-through, validated only for shape
    /// (any valid JSON value).
    pub output_schema: Option<serde_json::Value>,
```

In `AgentEntry::new` (~:790, inside the returned `Self { … }`, after `credentials: Vec::new(),`), add:

```rust
            output_schema: None,
```

- [ ] **Step 5: Map the field in `validate_agent`**

In the `Ok(AgentEntry { … })` construction at the end of `validate_agent` (~:1400, after `credentials,`), add:

```rust
        output_schema: raw.output_schema,
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg agent_output_schema_parses_and_passes_through agent_without_output_schema_is_none`
Expected: PASS (2 tests).

- [ ] **Step 7: Full tau-pkg test run (no regressions)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg`
Expected: PASS (existing AgentEntry literals are all built via `validate_agent` or `::new`, so none break — the struct is `#[non_exhaustive]`, no external literal construction).

- [ ] **Step 8: Commit**

```bash
git add crates/tau-pkg/src/project/project.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-pkg): add optional output_schema to agents"
```

---

## Task 2: tau-ir — `Agent.output_schema` + format-version bump + lowering

**Files:**
- Modify: `crates/tau-ir/src/node.rs` (`Agent` ~:30, in-file tests ~:102/123)
- Modify: `crates/tau-ir/src/module.rs` (`IrFormatVersion::CURRENT` ~:29, version test ~:94)
- Modify: `crates/tau-ir/src/lower/parse.rs` (~:113)
- Modify: `crates/tau-ir/src/lower/typecheck.rs` (test literals ~:280, ~:550)
- Modify: `crates/tau-ir/tests/lower_e2e.rs`

- [ ] **Step 1: Write the failing IR-node tests**

In `crates/tau-ir/src/node.rs`, inside the existing `#[cfg(test)] mod tests`, add:

```rust
    /// An `Agent` with `output_schema` set round-trips through serde.
    #[test]
    fn agent_output_schema_round_trips() {
        let agent = Agent {
            id: AgentId("judge".into()),
            prompt: String::new(),
            model: "claude-haiku-4-5".into(),
            tool_refs: alloc::vec::Vec::new(),
            context: None,
            budget: AgentBudget { max_turns: None, max_tokens: None },
            produces: alloc::vec::Vec::new(),
            output_schema: Some(serde_json::json!({"type": "object"})),
        };
        let json = serde_json::to_string(&agent).expect("serialize");
        let back: Agent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(agent, back);
    }

    /// `output_schema = None` serializes WITHOUT an `"output_schema"` key
    /// (guards byte-stability for schema-less agents).
    #[test]
    fn agent_empty_output_schema_omitted_from_json() {
        let agent = Agent {
            id: AgentId("gatherer".into()),
            prompt: String::new(),
            model: "claude-haiku-4-5".into(),
            tool_refs: alloc::vec::Vec::new(),
            context: None,
            budget: AgentBudget { max_turns: None, max_tokens: None },
            produces: alloc::vec::Vec::new(),
            output_schema: None,
        };
        let json = serde_json::to_string(&agent).expect("serialize");
        assert!(
            !json.contains("\"output_schema\""),
            "expected 'output_schema' key absent for None; got: {json}"
        );
    }
```

(Note: `serde_json` is already a dep of tau-ir; in `no_std` test code reference it as `serde_json::json!`.)

- [ ] **Step 2: Run to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir agent_output_schema_round_trips agent_empty_output_schema_omitted_from_json`
Expected: FAIL — `missing field 'output_schema'` / `no field` (compile error).

- [ ] **Step 3: Add the field to `Agent` and drop `Eq`**

In `crates/tau-ir/src/node.rs`, change the derive on `Agent` (~:29) from:

```rust
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct Agent {
```

to (drop `Eq`):

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Agent {
```

Then add the field after `produces` (~:45):

```rust
    /// Optional JSON schema describing the agent's structured output.
    /// Plumbed from `[agents.<id>].output_schema`; consumed by a later
    /// judge-compat build-time check. `skip_serializing_if` keeps
    /// schema-less agents byte-stable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
```

(`Value` is already imported at the top of `node.rs` via `use serde_json::Value;`.)

- [ ] **Step 4: Fix the two in-file Agent literals**

In `node.rs`, the existing tests `agent_produces_round_trips` (~:102) and `agent_empty_produces_omitted_from_json` (~:123) each construct `Agent { … }`. Add `output_schema: None,` after their `produces:` field in BOTH.

- [ ] **Step 5: Fix every remaining `Agent { … }` literal in the workspace**

These crates construct `Agent` by struct-literal and will not compile until each gains the field. Add `output_schema: None,` after the `produces:` field in each:

- `crates/tau-ir/src/lower/typecheck.rs:280` (`agent_with_tool_refs`)
- `crates/tau-ir/src/lower/typecheck.rs:550`
- `crates/tau-runtime-core/tests/pipeline_check.rs:148` (`writer_agent`)
- `crates/tau-runtime-core/tests/pipeline_retry.rs:190` (`writer_agent`)
- `crates/tau-runtime-core/tests/pipeline_executor.rs:186` (`agent`)
- `crates/tau-runtime-core/tests/context_pipeline.rs:225`
- `crates/tau-runtime-core/tests/run_ir_streaming.rs:115` (`agent`)
- `crates/tau-runtime-core/src/interpreter/check.rs:227` (the `JudgeRef::Builtin { model } => Agent { … }` arm)
- `crates/tau-runtime-tokio/tests/ir_smoke.rs:102`

(If any literal already lacks a `produces` field — none should, but if so — place `output_schema: None,` before the closing brace.)

The lowering site `crates/tau-ir/src/lower/parse.rs:113` gets the real value in Step 7, not `None`.

- [ ] **Step 6: Bump the format version**

In `crates/tau-ir/src/module.rs`, change `CURRENT` (~:29):

```rust
    pub const CURRENT: &'static str = "v1.3.0";
```

Update the version unit test (~:94) — change both `"v1.2.0"` literals to `"v1.3.0"`:

```rust
        assert_eq!(IrFormatVersion::CURRENT, "v1.3.0");
        assert_eq!(IrFormatVersion::current().0, "v1.3.0");
```

(The tests in `lower_e2e.rs`, `lower_triggers.rs`, `trigger_hash_preservation.rs` reference `IrFormatVersion::CURRENT` symbolically — they auto-track, no edit needed.)

- [ ] **Step 7: Wire lowering**

In `crates/tau-ir/src/lower/parse.rs`, in the `Agent { … }` construction (~:113, after `produces: entry.produces.clone(),`), add:

```rust
                output_schema: entry.output_schema.clone(),
```

- [ ] **Step 8: Run the tau-ir + module + lowering tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir`
Expected: PASS (new node tests pass, version test passes, lowering compiles).

- [ ] **Step 9: Add the lowering-survival assertion in `lower_e2e.rs`**

First inspect `crates/tau-ir/tests/lower_e2e.rs` to find an existing test that lowers a `ProjectConfig` with an agent. Add a focused test that builds a minimal `ProjectConfig` via `tau_pkg::project::project::ProjectConfig::parse_str` with an agent carrying `output_schema = { type = "object" }`, lowers it via `lower_project`, and asserts the resulting IR agent's `output_schema` equals `Some(json!({"type":"object"}))`:

```rust
#[test]
fn agent_output_schema_survives_lowering() {
    let toml = r#"
[project]
name = "p"

[agents.judge]
display_name = "Judge"
package = "p@^0.1"
llm_backend = "mock"
model = "mock-1"
output_schema = { type = "object" }
"#;
    let config = tau_pkg::project::project::ProjectConfig::parse_str(toml).expect("parse");
    let target = tau_ports::target::TargetTriple::PASSTHROUGH;
    let caches = tau_ir::lower::Caches {
        native_tool: &|_| Some([0u8; 32]),
        mcp_contract: &|_| None,
        skill: &|_| None,
    };
    let module = tau_ir::lower::lower_project(&config, &target, &caches).expect("lower");
    let agent = module.workflow.agents.get(&tau_ir::ids::AgentId("judge".into())).expect("agent");
    assert_eq!(agent.output_schema, Some(serde_json::json!({"type": "object"})));
}
```

(Confirm the exact import paths/`Caches` shape against the existing top-of-file imports in `lower_e2e.rs` before writing; copy that file's conventions if they differ — e.g. it may already `use` `lower_project`, `Caches`, `ProjectConfig`.)

- [ ] **Step 10: Run that test**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir agent_output_schema_survives_lowering`
Expected: PASS.

- [ ] **Step 11: Build the downstream crates that construct `Agent`**

Run each (they must compile with the new field):
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-tokio
```
Expected: PASS for both.

- [ ] **Step 12: Commit**

```bash
git add crates/tau-ir crates/tau-runtime-core crates/tau-runtime-tokio
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-ir): carry output_schema on Agent; bump IR format to v1.3.0"
```

---

## Task 3: tau-ts-extract — `outputSchema` parity + byte-equal conformance

**Files:**
- Modify: `crates/tau-ts-extract/src/lower.rs` (`IrAgent` ~:28, emitter ~:297-321, `extract_agent` ~:782, add helpers)
- Test: `crates/tau-ts-extract/tests/output_schema_conformance.rs` (new) + fixture dir

- [ ] **Step 1: Write the failing conformance test + fixture**

Create `crates/tau-ts-extract/tests/fixtures/output_schema_conformance/tau.toml`:

```toml
[project]
name = "output-schema-fixture"

[agents.judge]
display_name = "Judge"
package = "p@^0.1"
llm_backend = "mock-llm"
model = "mock-1"
output_schema = { required = ["verdict"], type = "object" }
```

(Key order `required` before `type` is alphabetical — matches serde_json `Map`'s BTreeMap ordering so the TS path's emitted inline table parses to the identical `Value`.)

Create `crates/tau-ts-extract/tests/fixtures/output_schema_conformance/project.ts`:

```typescript
import { agent } from "tau";

export const judge = agent({
  display_name: "Judge",
  package: "p@^0.1",
  llm_backend: "mock-llm",
  model: "mock-1",
  outputSchema: { type: "object", required: ["verdict"] },
});
```

Create `crates/tau-ts-extract/tests/output_schema_conformance.rs` (mirror `deliverables_goals_conformance.rs` exactly, only the fixture dir name differs):

```rust
//! TOML↔TS conformance: an agent's `output_schema` (TOML) /
//! `outputSchema` (TS) must produce byte-equal canonical IR.

use std::path::Path;

#[test]
fn toml_and_ts_produce_byte_equal_canonical_ir_with_agent_output_schema() {
    let fixture_dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/output_schema_conformance");

    let toml_str = std::fs::read_to_string(fixture_dir.join("tau.toml")).expect("read tau.toml");
    let toml_project =
        tau_pkg::project::project::ProjectConfig::parse_str(&toml_str).expect("parse tau.toml");

    let ts_src = std::fs::read_to_string(fixture_dir.join("project.ts")).expect("read project.ts");
    let ts_project = tau_ts_extract::extract_project(&ts_src, &fixture_dir.join("project.ts"))
        .expect("extract project.ts");

    let target = tau_ports::target::TargetTriple::PASSTHROUGH;
    let caches = tau_ir::lower::Caches {
        native_tool: &|fn_name| {
            let seed = fn_name.as_bytes().first().copied().unwrap_or(1);
            let mut h = [0u8; 32];
            for b in h.iter_mut() {
                *b = seed;
            }
            Some(h)
        },
        mcp_contract: &|_| None,
        skill: &|_| None,
    };

    let toml_ir =
        tau_ir::lower::lower_project(&toml_project, &target, &caches).expect("lower TOML to IR");
    let ts_ir =
        tau_ir::lower::lower_project(&ts_project, &target, &caches).expect("lower TS to IR");

    let toml_bytes = tau_ir::canonical::to_canonical_bytes(&toml_ir);
    let ts_bytes = tau_ir::canonical::to_canonical_bytes(&ts_ir);

    if toml_bytes != ts_bytes {
        panic!(
            "TOML↔TS canonical IRs differ:\n--- TOML ---\n{}\n--- TS ---\n{}\n",
            String::from_utf8_lossy(&toml_bytes),
            String::from_utf8_lossy(&ts_bytes)
        );
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ts-extract toml_and_ts_produce_byte_equal_canonical_ir_with_agent_output_schema`
Expected: FAIL — the TS path drops `outputSchema` (no extraction yet), so the TOML IR has `output_schema: Some(...)` and the TS IR has `None` → canonical bytes differ.

- [ ] **Step 3: Add the field to `IrAgent` + a JSON-value extractor**

In `crates/tau-ts-extract/src/lower.rs`, add to `struct IrAgent` (~:28, after `produces`):

```rust
    output_schema: Option<serde_json::Value>,
```

Add a helper that converts a TS literal expression to a `serde_json::Value` (place near `expr_as_string` ~:468):

```rust
/// Convert a TS literal expression (object / array / string / number /
/// bool / null) into a `serde_json::Value`. Returns `None` for any
/// non-literal (identifier, call, template, etc.) — schemas must be
/// literal per the declarations-only contract (ADR-0041).
fn expr_to_json(expr: &Expr) -> Option<serde_json::Value> {
    match expr {
        Expr::Object(obj) => {
            let mut map = serde_json::Map::new();
            for p in &obj.props {
                if let PropOrSpread::Prop(prop) = p {
                    if let Prop::KeyValue(KeyValueProp { key, value }) = prop.as_ref() {
                        let k = match key {
                            PropName::Ident(i) => i.sym.to_string(),
                            PropName::Str(s) => s.value.to_string(),
                            _ => return None,
                        };
                        map.insert(k, expr_to_json(value)?);
                    } else {
                        return None;
                    }
                } else {
                    return None;
                }
            }
            Some(serde_json::Value::Object(map))
        }
        Expr::Array(arr) => {
            let mut out = Vec::new();
            for elem in arr.elems.iter().flatten() {
                out.push(expr_to_json(&elem.expr)?);
            }
            Some(serde_json::Value::Array(out))
        }
        Expr::Lit(Lit::Str(s)) => Some(serde_json::Value::String(s.value.to_string())),
        Expr::Lit(Lit::Bool(b)) => Some(serde_json::Value::Bool(b.value)),
        Expr::Lit(Lit::Null(_)) => Some(serde_json::Value::Null),
        Expr::Lit(Lit::Num(n)) => serde_json::Number::from_f64(n.value).map(serde_json::Value::Number),
        _ => None,
    }
}
```

(`PropOrSpread`, `Prop`, `KeyValueProp`, `PropName`, `Lit` are already imported at the top of `lower.rs`. `Expr::Array` / `ArrayLit` too. Confirm `serde_json` import — it's a dep; add `use serde_json;` only if not already pathed; the snippet uses fully-qualified `serde_json::` so no `use` is needed.)

- [ ] **Step 4: Extract `outputSchema` in `extract_agent`**

In `extract_agent` (~:771, alongside the `produces` extraction), add:

```rust
    let output_schema = props.get("outputSchema").and_then(|e| expr_to_json(e));
```

Then add `output_schema,` to the `IrAgent { … }` literal (~:782).

- [ ] **Step 5: Add an inline-TOML emitter + emit the field**

Add a helper (place near `toml_str` / `toml_key`):

```rust
/// Serialize a `serde_json::Value` as a TOML inline value (table /
/// array / string / int / float / bool). JSON `null` has no TOML
/// representation, so it is emitted as the empty string (schemas should
/// not contain null; this is a defensive fallback). Object key order is
/// the value's own (serde_json `Map` = `BTreeMap`, alphabetical) →
/// deterministic output.
fn json_to_toml_inline(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let entries: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{} = {}", toml_key(k), json_to_toml_inline(v)))
                .collect();
            format!("{{ {} }}", entries.join(", "))
        }
        serde_json::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(json_to_toml_inline).collect();
            format!("[{}]", items.join(", "))
        }
        serde_json::Value::String(s) => toml_str(s),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Null => toml_str(""),
    }
}
```

In the agent-emission block (`for (name, agent) in agents` ~:297), after the `produces` block (~:315) and BEFORE the `[agents.<id>.prompt]` sub-table block (the prompt sub-table must stay last, since once a `[…]` header is emitted no more bare keys can follow), add:

```rust
        if let Some(schema) = &agent.output_schema {
            out.push_str(&format!("output_schema = {}\n", json_to_toml_inline(schema)));
        }
```

- [ ] **Step 6: Run the conformance test**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ts-extract toml_and_ts_produce_byte_equal_canonical_ir_with_agent_output_schema`
Expected: PASS. If it fails on a byte diff, print the two canonical strings (the test does) and check key ordering / number formatting — adjust the fixture so both paths produce the same `Value`.

- [ ] **Step 7: Full tau-ts-extract test run**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ts-extract`
Expected: PASS (existing `agent` extraction tests still pass — `IrAgent` gained a field but all literals are inside this file and updated; the emitter only adds output when `Some`).

- [ ] **Step 8: Commit**

```bash
git add crates/tau-ts-extract
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-ts-extract): extract agent outputSchema with byte-equal conformance"
```

---

## Task 4: conformance fixture + ADR

**Files:**
- Create: `crates/tau-ir-conformance/fixtures/14_agent_output_schema/{workflow.toml,mock_llm.jsonl,expected_report.json}`
- Modify: `crates/tau-ir-conformance/tests/conformance.rs`
- Create: `docs/decisions/0049-agent-output-schema.md`
- Modify: `docs/SUMMARY.md`

- [ ] **Step 1: Create the fixture files**

`crates/tau-ir-conformance/fixtures/14_agent_output_schema/workflow.toml` (mirrors fixture 01 + an `output_schema` on the agent):

```toml
[project]
name = "fixture-14"

[agents.fan]
display_name = "Fan Controller"
package      = "fan-ctrl@^0.1"
llm_backend  = "mock-llm"
model        = "mock-1"
tool_refs    = ["read_temp"]
max_turns    = 2
output_schema = { type = "object", required = ["temperature"] }

[tools.read_temp]
native      = "ReadTemp"
description = "Read the current temperature."
capabilities = []
```

`crates/tau-ir-conformance/fixtures/14_agent_output_schema/mock_llm.jsonl` (identical to fixture 01):

```
{"turn": 0, "response": {"tool_uses": [{"id": "1", "name": "read_temp", "input": {}}], "stop_reason": "tool_use"}}
{"turn": 1, "response": {"text": "ok", "stop_reason": "end_turn"}}
```

`crates/tau-ir-conformance/fixtures/14_agent_output_schema/expected_report.json` (documentation only — not read by the harness):

```json
{
  "run_outcome_kind": "Completed",
  "tool_calls": { "read_temp:{}": 1 },
  "message_added_count": 4
}
```

- [ ] **Step 2: Write the failing fixture-14 tests**

In `crates/tau-ir-conformance/tests/conformance.rs`, append (after fixture 13's block):

```rust
// ---------------------------------------------------------------------------
// Fixture 14 — agent output_schema is additive + byte-stable (v1.3.0)
// ---------------------------------------------------------------------------

/// Fixture 14: mirrors fixture 01 (agent + one native tool, two turns) with
/// an additional `output_schema` on the agent. The schema does not affect
/// execution — it is carried verbatim on the IR `Agent` node. This fixture
/// proves the v1.2.0→v1.3.0 additive field lowers, round-trips through the
/// canonical bundle encoder (BundleMode), and produces the same side-effect
/// multiset as the schema-less fixture 01.
#[tokio::test(flavor = "current_thread")]
async fn fixture_14_dev_mode_completed_with_output_schema() {
    let dir = fixture_dir("14_agent_output_schema");
    let report = DevMode.run(&dir).await;

    assert!(
        report.build_refused.is_none(),
        "expected an executed run, got build_refused: {:?}",
        report.build_refused
    );
    assert!(
        matches!(report.run_outcome, Some(RunOutcome::Completed { .. })),
        "expected RunOutcome::Completed, got: {:?}",
        report.run_outcome
    );
    let total = count_tool_calls(&report, "read_temp");
    assert_eq!(total, 1, "expected exactly 1 read_temp call; got {total}");
}

/// Cross-mode conformance for fixture 14: the agent's `output_schema`
/// round-trips through the bundle's canonical encoder/decoder (BundleMode
/// asserts `canonical_hash` equality internally), and both modes produce the
/// same side-effect multiset.
#[tokio::test(flavor = "current_thread")]
async fn fixture_14_cross_mode_conformance() {
    let dir = fixture_dir("14_agent_output_schema");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}
```

Also update the doc comment near the top of the file (the "All fixtures are live" list ~:7-14) to append `14_agent_output_schema`.

- [ ] **Step 3: Run the fixture-14 tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-conformance fixture_14`
Expected: PASS (2 tests). The cross-mode test proves the new field survives the canonical bundle round-trip without changing the hash relationship.

- [ ] **Step 4: Full conformance run (existing fixtures still byte-stable)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-conformance`
Expected: PASS — fixtures 01-13 are unchanged (schema-less agents omit the key → identical canonical bytes), proving the bump is additive.

- [ ] **Step 5: Write the ADR**

Create `docs/decisions/0049-agent-output-schema.md`. Read `docs/decisions/template.md` and `docs/decisions/0044-deliverables-and-goals.md` (its "Decision 7 — Bundle format v1.1.0 → v1.2.0 (additive MINOR bump)" section) first and match their structure. Content:

- **Status:** Accepted.
- **Context:** Deterministic steps carry `output_schema`; agents did not. A later judge-compat build-time check needs the agent's declared output schema to cross-reference against a deliverable's judge. This ADR records only the additive field plumbing — not the cross-check itself.
- **Decision:** Add `output_schema: Option<serde_json::Value>` to `UncheckedAgent`/`AgentEntry` (tau-pkg), to the IR `Agent` node behind `#[serde(default, skip_serializing_if = "Option::is_none")]`, and to the TS `outputSchema` extraction. Pass-through (no deep JSON-schema validation), mirroring `[steps.*].output_schema`.
- **IR format version:** `v1.2.0` → `v1.3.0` — MINOR/additive per ADR-0006 semver discipline. A v1.2.0 reader ignores the absent-when-`None` key; a v1.3.0 reader handles both. All pre-existing fixtures' canonical bytes are unchanged (verified by the full `tau-ir-conformance` suite).
- **Consequences:** No public-API regression — `tau_ir::node::Agent` keeps its `Eq` derive (`serde_json::Value` implements `Eq`). No runtime behavior change — the schema is inert until the downstream judge-compat task consumes it.

- [ ] **Step 6: Link the ADR in `docs/SUMMARY.md`**

Find the decisions/ADR section in `docs/SUMMARY.md` (where 0047/0048 are listed) and add a line for `0049-agent-output-schema.md` following the existing format exactly.

- [ ] **Step 7: Build the book (docs gate)**

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build && cd .. && rm -rf docs/book`
Expected: only `[INFO]` lines, no warnings/errors (linkcheck `warning-policy = "error"`). If `mdbook`/`mdbook-linkcheck` are missing, note it and skip — CI's `docs-deploy` is the authoritative gate.

- [ ] **Step 8: Commit**

```bash
git add crates/tau-ir-conformance docs/decisions/0049-agent-output-schema.md docs/SUMMARY.md
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(conformance): fixture 14 agent output_schema + ADR-0049"
```

---

## Task 5: Drift check + final verification + PR

- [ ] **Step 1: Run the vocabulary/format drift test in tau-runtime-tokio**

The format-version-sensitive tests live in tau-runtime-tokio (`ir_smoke.rs`) and tau-ir (`trigger_hash_preservation.rs`, `lower_*`). Run:

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-tokio
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir
```
Expected: PASS. These assert `IrFormatVersion::CURRENT` symbolically (no hardcoded `v1.2.0` outside `module.rs`), so the bump propagates cleanly. If any test hardcodes `v1.2.0`, fix it to `v1.3.0`.

- [ ] **Step 2: Clippy on every touched crate**

```
for c in tau-pkg tau-ir tau-ts-extract tau-ir-conformance tau-runtime-core tau-runtime-tokio; do
  timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p $c --all-targets -- -D warnings || break
done
```
Expected: clean (no warnings).

- [ ] **Step 3: fmt check**

```
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt --check
```
Expected: clean. (`cargo fmt` is workspace-wide; if it reports a diff, run `cargo fmt` to fix, then re-check.)

- [ ] **Step 4: Grep for any remaining `v1.2.0` that should have bumped**

```
git grep -n "v1.2.0" -- crates/tau-ir crates/tau-runtime-core crates/tau-runtime-tokio
```
Expected: no hits in source/tests asserting the *current* IR version (historical mentions in unrelated bundle-format tests like `reproduce.rs` referencing `v1.0.0`/`v1.1.0` are fine — they test the bundle's own `schema_version`, not the IR `ir_format`). Investigate any hit.

- [ ] **Step 5: Push the branch and open the PR**

```bash
git push -u origin <branch>
gh pr create --base main --title "feat: agent output_schema (additive IR v1.3.0)" --body "<summary>"
```

PR body should note: additive field across tau-pkg/tau-ir/tau-ts-extract/conformance; IR format `v1.2.0`→`v1.3.0` (MINOR); `Agent` drops `Eq`; existing fixtures byte-stable; judge cross-check is a separate downstream task; ADR-0049.

- [ ] **Step 6: Enroll auto-merge + keep up to date**

```bash
gh pr merge <PR#> --squash --delete-branch --auto
```
Then `gh pr update-branch <PR#>` whenever the PR is BEHIND. CI is the gate.

---

## Self-Review

**Spec coverage:**
- SCOPE 1 (tau-pkg: UncheckedAgent + AgentEntry + new() + validate_agent) → Task 1 ✓
- SCOPE 2 (tau-ir: Agent node + format version v1.3.0 + lowering parse.rs) → Task 2 ✓
- SCOPE 3 (tau-ts-extract: IrAgent + extraction + TOML emission + byte-equal conformance) → Task 3 ✓
- SCOPE 4 (conformance fixture + tests) → Task 4 ✓
- WORKFLOW: branch+PR (Task 5), TDD (every task is failing-test-first), CARGO RULES (every cargo command templated), drift test (Task 5 Step 1), ADR (Task 4 Step 5) ✓

**Type consistency:** field name `output_schema` (Rust/TOML) ↔ `outputSchema` (TS camelCase) used consistently. `Option<serde_json::Value>` at every Rust layer. `Agent` field added after `produces` in all literals.

**Known ripple captured:** Task 2 Step 5 enumerates all 9 external + 2 in-file + 1 lowering `Agent { … }` literals. Dropping `Eq` verified safe.

**Risk note:** the only non-mechanical piece is the TS `expr_to_json` / `json_to_toml_inline` round-trip (Task 3). The conformance test (Task 3 Step 1) is the gate — it compares canonical IR bytes, so any encoding mismatch fails loudly with both strings printed.
