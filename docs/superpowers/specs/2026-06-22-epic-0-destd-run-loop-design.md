# EPIC 0 — de-std the run loop (no_std tool-arg validation) — design

**Date:** 2026-06-22
**Status:** Approved (brainstorm) — pending spec review
**Epic:** 0 — Core no_std readiness (BLOCKS EPICs 3, 4, 5, 7)
**Issues:** #378 (0.1), #379 (0.2), #380 (0.3), #381 (0.4), #382 (0.5)
**Relates to:** [ADR-0055](../../decisions/0055-tau-identity-two-contracts.md)
(the no_std ports API is part of the public stability surface),
[ADR-0051](../../decisions/0051-tau-ir-crate-split.md) (no_std boundary),
the canonical philosophy
[`tau-philosophy.md`](../../explanation/tau-philosophy.md).

---

## 1. Problem & current state

EPIC 0's goal: `run` / `stream` / `interpreter` in `tau-runtime-core` compile
**and run** `no_std`, so the run loop is portable to the wasm guest and to
embedded targets. This is the prerequisite that unblocks EPICs 3/4/5/7.

A read-only inventory of the run path (Story 0.1, completed) established:

- `tau-runtime-core`, `tau-ir`, `tau-domain` are already `#![no_std]` with
  `extern crate alloc`. `tau-runtime-core` already compiles `no_std` under
  `--no-default-features --features wasm-interpreter` (CI verifies via a
  `cargo check` lane + a `tau-wasm-guest` wasm32-wasip2 link gate + a
  forbidden-`std::`-imports grep).
- **The entire std footprint of the *default* run loop is one feature:**
  `tool-validation`, defined as
  `tool-validation = ["wasm-interpreter", "dep:jsonschema", "tau-domain/std"]`.
  - `jsonschema` (0.46) is std-only and is the per-call tool-argument
    validator.
  - `tau-domain/std` (`chrono/std`, `chrono/clock`, `uuid/std`) is pulled
    transitively by the same feature; the run-loop code itself already uses
    the no_std-safe `ids::message_id` / `ids::agent_instance_id` helpers and
    `Message::new_with`, so the std constructors it activates are not actually
    needed on the run path.
- **Story 0.4 (serde_json alloc-only) is already done:** the workspace alias
  is `serde_json = { version = "1", default-features = false, features =
  ["alloc"] }`.
- **Story 0.5 (no_std CI lane) partially exists:** today it *checks* and
  *links* no_std; it does not *run* the loop no_std.
- All direct `std::` usages in the run-path source are inside `#[cfg(test)]`
  or behind the unrelated `host-fs` / `with-std-adapters` features.

So EPIC 0 reduces to one substantive change (Story 0.2), from which 0.3 and
most of 0.5 follow mechanically.

### Per-call validation today

In `stream.rs::run_streaming_inner` (gated `#[cfg(feature = "tool-validation")]`):
the LLM emits tool-call arguments → `validate_tool_args(&input, &name,
validator)` checks them against the tool's `input_schema` via the compiled
`jsonschema` validator → on failure a structured `ToolError::BadArgs { reason }`
is written into the conversation and the LLM self-corrects. Validators are
compiled once at `RuntimeBuilder::build()` time
(`builder.rs::collect_tools_by_name` → `tool_args::ToolArgsValidator::compile`).

The conformance gate (β.6) demands the host (`tau dev`) and wasm
(`tau build`) paths produce **byte-identical** traces. Silently dropping
validation on no_std builds would therefore be observable drift, not a
no-op — it would change the conversation transcript.

---

## 2. Decision

Replace the std-only `jsonschema` per-call validator with a **no_std
validator over a fixed JSON-Schema subset**, enforced **fail-closed at build
time**. Both halves — *compile* (schema → rule set) and *validate* (args →
violations) — are no_std, because schema compilation runs at
`RuntimeBuilder::build()` time which executes **both** on the host (CLI
reference host) **and inside the wasm guest** (per the β.7.5 in-guest
`run_ir_streaming` path). The consequence: there is **no `std` left in tool
validation at all**, and the `tool-validation` feature stops implying `std`.

This is Option A from the brainstorm. Option B (bake a serializable
`ValidationProgram` into the bundle) was deferred because it changes the
bundle/IR contract, which is EPIC 2's responsibility; Option C (drop runtime
validation on no_std) was rejected because it breaks cross-target trace
parity.

### Feature & attribute changes

```toml
# crates/tau-runtime-core/Cargo.toml — BEFORE
tool-validation = ["wasm-interpreter", "dep:jsonschema", "tau-domain/std"]
# AFTER
tool-validation = ["wasm-interpreter"]
```

- Remove the `feature = "tool-validation"` arm from the
  `#[cfg(any(test, feature = "host-fs", feature = "tool-validation"))] extern
  crate std;` line in `lib.rs` (leaving `test` + `host-fs`).
- Remove `jsonschema` from `tau-runtime-core` deps and, if unused elsewhere in
  the workspace, from the root `Cargo.toml`.
- `tau-domain/std` is no longer activated by the run path. (`host-fs` and the
  CLI host may still activate it independently — out of scope here.)

---

## 3. The validator

New responsibility lives in `tau-runtime-core`'s `tool_args` module
(reworked; the public `validate_tool_args` signature is preserved, only
`CompiledSchema` changes from `Arc<jsonschema::Validator>` to the no_std type).

### Types & interfaces

```rust
// no_std + alloc throughout

/// A tool input schema compiled to an executable rule set. Replaces
/// `Arc<jsonschema::Validator>`.
pub struct CompiledSchema { /* alloc-backed node tree */ }

/// Compile a JSON-Schema `Value` into a CompiledSchema. Fail-closed: any
/// keyword outside the supported v1 subset is a hard error, NOT skipped.
/// Runs at build time on host and in-guest; must be no_std.
pub fn compile(schema: &serde_json::Value) -> Result<CompiledSchema, SchemaCompileError>;

/// Per-call validation. Returns the first violation (or all — see §3.3) as a
/// human-readable reason the LLM can self-correct from. no_std.
impl CompiledSchema {
    pub fn validate(&self, input: &serde_json::Value) -> Result<(), ArgsViolation>;
}

#[derive(Debug)]
pub enum SchemaCompileError {
    /// A JSON-Schema keyword outside the supported v1 subset.
    UnsupportedKeyword { keyword: String, pointer: String },
    /// Structurally malformed schema (e.g. `required` not an array of strings).
    Malformed { detail: String, pointer: String },
}

/// One or more reasons the args did not conform. `reason` is what flows into
/// `ToolError::BadArgs { reason }`.
#[derive(Debug)]
pub struct ArgsViolation { pub reason: String }
```

`validate_tool_args` keeps its current signature so `stream.rs` is unchanged
apart from the validator type:

```rust
pub fn validate_tool_args(
    input: &serde_json::Value,
    tool_name: &str,
    schema: &CompiledSchema,
) -> Result<(), ToolError>;   // Err(ToolError::BadArgs { reason })
```

`builder.rs::collect_tools_by_name` calls `compile()` and maps
`SchemaCompileError` into the existing `BuildError::ToolSchemaInvalid`
(extended with the `UnsupportedKeyword` case so `tau check` reports exactly
which keyword and where).

### 3.1 Supported v1 subset

| group | keywords |
|---|---|
| structural | `type`, `properties`, `required`, `items`, `enum`, `const`, `additionalProperties` (boolean form only) |
| combinators | `oneOf`, `anyOf`, `allOf`, `not` |
| numeric | `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`, `multipleOf` |
| string | `minLength`, `maxLength` |
| array | `minItems`, `maxItems`, `uniqueItems` |

`type` accepts the JSON Schema primitive names (`object`, `array`, `string`,
`number`, `integer`, `boolean`, `null`) and an array-of-types union.

The combinators are included because the audit (§4) found a real production
schema (`fs-write`) using `oneOf`, and combinators are no_std-trivial to
evaluate (run each subschema, count passes: `oneOf` = exactly one, `anyOf` =
at least one, `allOf` = all, `not` = zero) — they need no regex engine, which
is what makes `pattern`/`format` the only genuinely hard exclusions.

**Ignored annotation keywords (NOT errors):** `title`, `description`,
`default`, `$comment`, `examples`, `$schema`, `$id`. These are
non-validating; `compile()` skips them silently. (Necessary: `fs-write`'s
schema carries `title`/`description`/`default` on its sub-schemas, so
treating unknown keys as `UnsupportedKeyword` would wrongly reject it. Only
keys in the §3.2 list are hard errors; annotations are ignored.)

### 3.2 Explicitly unsupported in v1 (fail-closed)

`pattern`, `format`, `$ref`, `$defs`/`definitions`, `if`/`then`/`else`,
`additionalProperties` (schema form), `patternProperties`, `dependencies`.
Each produces `SchemaCompileError::UnsupportedKeyword` at compile/build time.

**Rationale for fail-closed:** an unsupported keyword must be a loud build
error, never silently skipped validation. `pattern`/`format` are excluded
because they require a regex engine and there is no clean no_std one (and the
audit confirmed no real tool schema uses them). The rest are YAGNI for
tool-argument schemas (no real schema uses them) and can graduate to a v2
subset if one ever does.

### 3.3 Violation reporting

`validate()` collects **all** top-level violations into one `reason` string
(matching `jsonschema`'s behavior of surfacing multiple errors), so the LLM
gets the same self-correction signal. Exact format is pinned by the
differential test (§5) against the current `jsonschema` output shape, not
invented.

---

## 4. Schema-subset audit (DONE — Story 0.1/0.2 gate, resolved)

A repo-wide audit of every tool `input_schema` (production plugins, test
fixtures, reference skill packages, conformance scenarios) was run before
committing to the subset. Method: grep all `*.rs`/`*.json`/`*.toml` under
`crates/` for the risky JSON-Schema keywords
(`pattern`/`format`/`allOf`/`anyOf`/`oneOf`/`not`/`$ref`/`patternProperties`/`if`/`then`/`else`/`dependencies`)
and characterise each hit as a real schema keyword vs a false positive.

**Findings:**

- **One real combinator use:** `crates/tau-plugins/fs-write/src/plugin.rs`
  (`fn schema()`) uses top-level `oneOf` to discriminate a "write" branch
  from an "edit" branch (each with `const` mode discriminant, nested
  `properties`/`required`, and `additionalProperties: false`). This is a
  well-motivated discriminated union, not a casual usage.
- **No real use of regex keywords.** Every `pattern` hit is a runtime *arg*
  to the builtin `matches` deterministic function
  (`builtin_registry.rs` tests, `interpreter/check.rs` —
  `json!({ "pattern": x })`), where regex matching is the function's job; none
  is a JSON-Schema `pattern` keyword in a tool `input_schema`. No `format` or
  `$ref` in any tool schema. `"dependencies"` hits
  (`install.rs`, `agent_loop.rs`) are unrelated data fields, not the schema
  keyword.

**Resolution:** include the combinators in the v1 subset (§3.1) — this covers
`fs-write` with no production-schema edits — and keep only the regex-dependent
keywords (and the YAGNI rest) excluded (§3.2). The §3.1-with-combinators
subset covers every tool schema in the repo today. The `pattern`/regex risk is
therefore **not realised**; no regex engine is needed for v1.

If a future tool schema introduces `pattern`/`format`, the fail-closed build
error names the keyword and the team chooses then: narrow the schema, add a
fixed-purpose no_std matcher, or a documented host-only parity exception. No
regex engine is pre-committed.

---

## 5. Testing & parity

TDD throughout (the repo's standard).

- **Unit tests per keyword:** accept + reject cases for every supported
  keyword, plus an `UnsupportedKeyword` test per excluded keyword.
- **Differential test (the key safety net):** while `jsonschema` is still in
  the tree, a host-only test feeds a matrix of (schema, args) pairs through
  **both** the old `jsonschema` path and the new `CompiledSchema` and asserts
  they agree on accept/reject for every pair. `jsonschema` is deleted only
  after this passes. This proves no behavioral regression before the swap.
- **Conformance parity:** PR-0c extends the β.6 gate with a
  tool-arg-rejection scenario, asserting the host and wasm runs produce the
  identical `ToolError::BadArgs` transcript.

---

## 6. PR sequencing

The schema-subset audit (PR-0a in the original plan) is **done** and recorded
in §4, so implementation is two PRs:

1. **PR-0b — the no_std validator (Stories 0.1 + 0.2 + 0.3 + 0.4).**
   `CompiledSchema` + `compile` (incl. combinators + ignored annotations per
   §3.1) + `validate`; swap `tool_args.rs` internals; differential test
   against `jsonschema`; delete `jsonschema`; feature refactor
   (`tool-validation = ["wasm-interpreter"]`, drop `tau-domain/std`, drop the
   std arm in `lib.rs`); wire `tau check` to surface `SchemaCompileError`.
   The bulk of the work. Body closes #378 (0.1), #379 (0.2), #380 (0.3),
   #381 (0.4) — the last already satisfied (§1).
2. **PR-0c — run-loop no_std CI lane (Story 0.5 + close-out).** A lane that
   *runs* `run_ir_streaming` no_std (not just checks/links) with
   `tool-validation` on, asserting tool-arg-rejection parity with the host
   path. Closes #382 and the epic; updates ROADMAP EPIC 0 status.

### Epic DoD

The agent loop compiles **and runs** no_std with tool validation intact;
`jsonschema` and the run-path `tau-domain/std` pull are gone; the new CI lane
is green; cross-target validation parity is proven by the conformance gate.

---

## 7. Out of scope

- Baking a serializable validation program into the bundle (Option B) — that
  is EPIC 2 ("lock the two contracts"), since it changes the bundle/IR
  contract.
- De-std-ing `host-fs` (`globset`) or the CLI host — those are intentionally
  host-only and not on the portable run path.
- Adding `pattern`/regex support — deferred to a v2 subset, only if a real
  schema demands it (§4).
