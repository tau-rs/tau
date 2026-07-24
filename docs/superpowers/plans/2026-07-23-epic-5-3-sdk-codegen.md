# EPIC 5.3 — Authoring-SDK Codegen Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A new `tau-sdk-codegen` crate that emits typed TypeScript (`@tau/sdk`) and Python (`tau-sdk`) authoring front-ends from the frozen IR JSON schema, such that the same agent authored in TOML, TS, and Python lowers to byte-identical canonical IR.

**Architecture:** The SDKs are *authoring front-ends*, not IR emitters — they produce a `ProjectConfig`-equivalent that the single Rust `lower_project` + `to_canonical_bytes` pipeline turns into IR, so byte-equality is structural (mirrors `tau-ts-extract`). Codegen combines two inputs: the frozen IR schema (for shared leaf/vocabulary types) and a small owned "authoring surface" table (for which factory has which fields). The byte-equal acceptance test lowers one fixture agent three ways (TOML + TS in-process, Python via live `python3`) and asserts equal bytes.

**Tech Stack:** Rust (new crate), `serde_json` (schema parse), `thiserror` + `anyhow`; generated TS (npm `@tau/sdk`) and Python (`tau_sdk`, stdlib `dataclasses` only, no third-party deps); `python3` invoked at test time.

## Global Constraints

- **New crate only.** MUST NOT modify `tau-ir`, `tau-pkg`, `tau-ir-lower`, `tau-ts-extract`, or the published IR schema. All consumption of those is read-only / dev-dependency.
- **Branch:** `feat/epic-5-3-sdk-codegen`. PR to `main`. Never push to `main`.
- **Every cargo command:** `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e53 cargo nextest run -p tau-sdk-codegen` (tests); `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e53 cargo check -p tau-sdk-codegen` (check). Doctests via `cargo test -p tau-sdk-codegen --doc`. Never bare `cargo`; always `-p tau-sdk-codegen`.
- **Crate root declares** `#![forbid(unsafe_code)]`.
- **thiserror at the crate boundary; anyhow internally.**
- **Lowering harness values (copy verbatim):** target = `tau_ports::target::TargetTriple::PASSTHROUGH`; parse via `tau_pkg::project::ProjectConfig::parse_str(&str)`; TS via `tau_ts_extract::extract_project(&src, &path)`; encode via `tau_ir::canonical::to_canonical_bytes(&module)`; lower via `tau_ir_lower::lower_project(&cfg, &target, &caches).unwrap().module`.
- **Cargo.toml shape:** all shared fields `.workspace = true`, deps `{ workspace = true }`, trailing `[lints] workspace = true` (match `crates/tau-ts-extract/Cargo.toml`).
- **Generated packages live at top-level `sdk/ts/` and `sdk/python/`** — NOT under `crates/`, NOT workspace members.
- **Python: stdlib only** (`dataclasses`, `typing`) — no pip installs, so `python3 project.py` runs on a bare interpreter.
- **`python3`-dependent tests self-skip** (print + early-return) when `python3` is absent, so the crate builds and its other tests pass everywhere.

---

### Task 1: Crate scaffold + python3 harness proof

Stand up the crate and prove the two mechanical risks early: (a) the crate compiles and is wired into the workspace, (b) a Rust test can shell out to `python3`, capture stdout as TOML, parse it via `ProjectConfig::parse_str`, lower it, and get canonical bytes equal to the same agent's TOML. This de-risks the whole approach before any codegen exists.

**Files:**
- Create: `crates/tau-sdk-codegen/Cargo.toml`
- Create: `crates/tau-sdk-codegen/src/lib.rs`
- Create: `crates/tau-sdk-codegen/src/error.rs`
- Create: `crates/tau-sdk-codegen/tests/common.rs` (shared test harness helpers)
- Create: `crates/tau-sdk-codegen/tests/harness_proof.rs`
- Create: `crates/tau-sdk-codegen/tests/fixtures/harness/tau.toml`
- Create: `crates/tau-sdk-codegen/tests/fixtures/harness/emit_toml.py`
- Modify: `Cargo.toml` (root) — add member + workspace dep

**Interfaces:**
- Produces: `tau_sdk_codegen::error::CodegenError` (thiserror enum, starts with one variant `Io(#[from] std::io::Error)`).
- Produces (test-only, in `tests/common.rs`): `pub fn lower_toml_bytes(toml: &str) -> Vec<u8>`, `pub fn run_python_toml(script: &std::path::Path) -> Option<String>` (returns `None` when `python3` is unavailable), `pub fn python3_available() -> bool`.

- [ ] **Step 1: Create the crate Cargo.toml**

```toml
[package]
name = "tau-sdk-codegen"
description = "Codegen for tau authoring SDKs (TypeScript @tau/sdk + Python tau-sdk) from the frozen IR JSON schema"
version.workspace      = true
edition.workspace      = true
rust-version.workspace = true
license.workspace      = true
repository.workspace   = true
authors.workspace      = true

[dependencies]
serde_json = { workspace = true, features = ["std"] }
thiserror  = { workspace = true }
anyhow     = { workspace = true }

[dev-dependencies]
tau-pkg        = { workspace = true }
tau-ts-extract = { workspace = true }
tau-ir         = { workspace = true }
tau-ir-lower   = { workspace = true }
tau-ports      = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 2: Create src/error.rs**

```rust
//! Error type at the crate boundary.

use thiserror::Error;

/// Errors surfaced by SDK codegen.
#[derive(Debug, Error)]
pub enum CodegenError {
    /// Reading the schema or writing an output file failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The IR schema JSON was malformed or missing an expected structure.
    #[error("schema error: {0}")]
    Schema(String),
}
```

- [ ] **Step 3: Create src/lib.rs**

```rust
//! Codegen for tau authoring SDKs from the frozen IR JSON schema.
//!
//! The generated SDKs are *authoring front-ends*: they produce the same
//! `ProjectConfig` the TOML surface parses to, so all three surfaces lower
//! to byte-identical canonical IR via the single Rust lowering pass.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod error;

pub use error::CodegenError;
```

- [ ] **Step 4: Register the crate in the root workspace**

In `/Users/titouanlebocq/conductor/workspaces/tau/dakar/Cargo.toml`, add `"crates/tau-sdk-codegen",` to `[workspace.members]` (alongside the other `crates/*` entries), and add to `[workspace.dependencies]`:

```toml
tau-sdk-codegen     = { path = "crates/tau-sdk-codegen", version = "0.0.0" }
```

- [ ] **Step 5: Create the harness fixture — tau.toml**

`crates/tau-sdk-codegen/tests/fixtures/harness/tau.toml`:

```toml
[models.haiku]
backend = "anthropic"
model = "claude-haiku-4-5"

[[agents]]
display_name = "Fast"
package = "research@^0.1"
model = "haiku"
```

- [ ] **Step 6: Create the harness fixture — emit_toml.py**

`crates/tau-sdk-codegen/tests/fixtures/harness/emit_toml.py` (stdlib only; prints TOML that parses to the same ProjectConfig — key/table order and whitespace need not match):

```python
import sys

sys.stdout.write(
    '[models.haiku]\n'
    'backend = "anthropic"\n'
    'model = "claude-haiku-4-5"\n'
    '\n'
    '[[agents]]\n'
    'display_name = "Fast"\n'
    'package = "research@^0.1"\n'
    'model = "haiku"\n'
)
```

- [ ] **Step 7: Create tests/common.rs (shared harness)**

```rust
//! Shared test harness: lower a ProjectConfig to canonical IR bytes and run
//! a Python authoring script through `python3`.
#![allow(dead_code)] // used by multiple integration test files

use std::path::Path;
use std::process::Command;

/// The native-tool content-hash cache used by every fixture: a deterministic
/// hash seeded by the first byte of the symbolic name (matches the pattern in
/// tau-ts-extract's conformance tests).
fn caches() -> tau_ir_lower::Caches<'static> {
    tau_ir_lower::Caches {
        native_tool: &|fn_name: &str| {
            let seed = fn_name.as_bytes().first().copied().unwrap_or(1);
            Some([seed; 32])
        },
        mcp_contract: &|_| None,
        skill: &|_| None,
        prompt_file: &|_| Ok(Vec::new()),
    }
}

/// Lower a parsed ProjectConfig to canonical IR bytes.
pub fn lower_config_bytes(cfg: &tau_pkg::project::ProjectConfig) -> Vec<u8> {
    let target = tau_ports::target::TargetTriple::PASSTHROUGH;
    let module = tau_ir_lower::lower_project(cfg, &target, &caches())
        .expect("lowering must succeed")
        .module;
    tau_ir::canonical::to_canonical_bytes(&module)
}

/// Parse TOML text and lower it to canonical IR bytes.
pub fn lower_toml_bytes(toml: &str) -> Vec<u8> {
    let cfg = tau_pkg::project::ProjectConfig::parse_str(toml).expect("parse tau.toml");
    lower_config_bytes(&cfg)
}

/// True if `python3` is on PATH.
pub fn python3_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run a Python authoring script; return its stdout as a String, or `None` if
/// `python3` is unavailable. `pythonpath` is prepended to PYTHONPATH so the
/// script can `import tau_sdk`. Panics if python3 is present but the script
/// exits non-zero.
pub fn run_python_toml(script: &Path, pythonpath: Option<&Path>) -> Option<String> {
    if !python3_available() {
        return None;
    }
    let mut cmd = Command::new("python3");
    cmd.arg(script);
    if let Some(pp) = pythonpath {
        cmd.env("PYTHONPATH", pp);
    }
    let out = cmd.output().expect("spawn python3");
    assert!(
        out.status.success(),
        "python3 {} failed:\n{}",
        script.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    Some(String::from_utf8(out.stdout).expect("python stdout is utf8"))
}
```

- [ ] **Step 8: Write the failing harness test**

`crates/tau-sdk-codegen/tests/harness_proof.rs`:

```rust
mod common;

use std::path::Path;

/// Proves the python3 → TOML → ProjectConfig → IR path yields the same canonical
/// bytes as the hand-written tau.toml. This is the mechanical spine of the 5.3
/// acceptance test, isolated so it fails loudly if the toolchain wiring breaks.
#[test]
fn python_emitted_toml_lowers_equal_to_native_toml() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/harness");

    let toml = std::fs::read_to_string(dir.join("tau.toml")).unwrap();
    let toml_bytes = common::lower_toml_bytes(&toml);

    match common::run_python_toml(&dir.join("emit_toml.py"), None) {
        None => {
            eprintln!("SKIP: python3 not available; skipping python-path assertion");
        }
        Some(py_toml) => {
            let py_bytes = common::lower_toml_bytes(&py_toml);
            assert_eq!(toml_bytes, py_bytes, "python-emitted TOML must lower equal");
        }
    }
}
```

- [ ] **Step 9: Run the test to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e53 cargo nextest run -p tau-sdk-codegen`
Expected: PASS (`python_emitted_toml_lowers_equal_to_native_toml`). If the dev machine lacks `python3`, expect PASS with the `SKIP:` line on stderr.

- [ ] **Step 10: Commit**

```bash
git add crates/tau-sdk-codegen Cargo.toml
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(sdk-codegen): scaffold crate + python3 lowering harness"
```

---

### Task 2: Schema model — consume the frozen IR schema

Load the committed IR schema and expose the leaf-type lookups the emitters use. This is the "genuinely consume the frozen schema" half of Option C: the emitters assert every leaf type they mirror is a real `$def`, and pull enum variants from the schema rather than hardcoding them.

**Files:**
- Create: `crates/tau-sdk-codegen/src/schema.rs`
- Modify: `crates/tau-sdk-codegen/src/lib.rs` (add `pub mod schema;`)
- Test: inline `#[cfg(test)]` in `src/schema.rs`

**Interfaces:**
- Consumes: `CodegenError` from Task 1.
- Produces:
  - `pub const SCHEMA_PATH: &str = "schemas/ir/tau-ir.v2.5.0.schema.json";`
  - `pub struct SchemaModel { /* private */ }`
  - `pub fn SchemaModel::load(repo_root: &Path) -> Result<SchemaModel, CodegenError>`
  - `pub fn SchemaModel::has_def(&self, name: &str) -> bool`
  - `pub fn SchemaModel::enum_variants(&self, def: &str) -> Option<Vec<String>>` (reads `$defs.<def>.enum` of JSON strings)
  - `pub fn SchemaModel::schema_id(&self) -> Option<&str>` (the top-level `$id`)

- [ ] **Step 1: Write the failing test**

Add to `src/schema.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn repo_root() -> std::path::PathBuf {
        // crates/tau-sdk-codegen -> repo root is two levels up.
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf()
    }

    #[test]
    fn loads_frozen_schema_and_finds_known_defs() {
        let model = SchemaModel::load(&repo_root()).expect("load schema");
        // IrModule is the schema root; Capability is a shared leaf type the
        // authoring surface reuses verbatim.
        assert!(model.has_def("Capability"), "Capability must be a $def");
        assert!(model.schema_id().unwrap().contains("tau-ir"));
    }

    #[test]
    fn missing_def_is_reported() {
        let model = SchemaModel::load(&repo_root()).expect("load schema");
        assert!(!model.has_def("NotARealType"));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e53 cargo nextest run -p tau-sdk-codegen schema`
Expected: FAIL to compile (`SchemaModel` undefined).

- [ ] **Step 3: Implement src/schema.rs**

```rust
//! Parse the frozen IR JSON schema into a lookup model.
//!
//! Only the pieces the SDK emitters need are lifted out: the set of `$defs`
//! (so emitters can assert a mirrored leaf type is real) and per-`$def` enum
//! variants (so vocabulary enums are sourced from the schema, not hardcoded).

use std::path::Path;

use crate::error::CodegenError;

/// Repo-relative path to the frozen schema this codegen consumes.
pub const SCHEMA_PATH: &str = "schemas/ir/tau-ir.v2.5.0.schema.json";

/// A parsed view of the frozen IR schema.
pub struct SchemaModel {
    root: serde_json::Value,
}

impl SchemaModel {
    /// Load and parse `schemas/ir/tau-ir.v2.5.0.schema.json` under `repo_root`.
    pub fn load(repo_root: &Path) -> Result<SchemaModel, CodegenError> {
        let path = repo_root.join(SCHEMA_PATH);
        let bytes = std::fs::read(&path)?;
        let root: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| CodegenError::Schema(format!("parse {}: {e}", path.display())))?;
        Ok(SchemaModel { root })
    }

    fn defs(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.root.get("$defs").and_then(|d| d.as_object())
    }

    /// True if `name` is a `$def` in the schema.
    pub fn has_def(&self, name: &str) -> bool {
        self.defs().map(|d| d.contains_key(name)).unwrap_or(false)
    }

    /// String enum variants declared on `$defs.<def>.enum`, if any.
    pub fn enum_variants(&self, def: &str) -> Option<Vec<String>> {
        let arr = self.defs()?.get(def)?.get("enum")?.as_array()?;
        Some(
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
        )
    }

    /// The schema's top-level `$id`.
    pub fn schema_id(&self) -> Option<&str> {
        self.root.get("$id").and_then(|v| v.as_str())
    }
}
```

- [ ] **Step 4: Wire the module in lib.rs**

Add `pub mod schema;` to `src/lib.rs` after `pub mod error;`.

- [ ] **Step 5: Run to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e53 cargo nextest run -p tau-sdk-codegen schema`
Expected: PASS (both schema tests). If `has_def("Capability")` fails, open `schemas/ir/tau-ir.v2.5.0.schema.json`, confirm the exact `$defs` key name, and adjust the test's asserted name to a real leaf def (do NOT change the schema).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-sdk-codegen/src
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(sdk-codegen): parse frozen IR schema into leaf lookups"
```

---

### Task 3: Authoring surface table

Declare the owned authoring-surface descriptor: the factories and their fields, matching what `tau-ts-extract` recognizes. This is the composition the IR schema cannot describe. Kept honest by the byte-equal test in Task 6.

**Files:**
- Create: `crates/tau-sdk-codegen/src/authoring.rs`
- Modify: `crates/tau-sdk-codegen/src/lib.rs` (add `pub mod authoring;`)
- Test: inline `#[cfg(test)]` in `src/authoring.rs`

**Interfaces:**
- Produces:
  - `pub enum FieldTy { Str, Bool, ModelMap, ToolList }`
  - `pub struct AuthField { pub sdk_name: &'static str, pub toml_key: &'static str, pub ty: FieldTy, pub required: bool }`
  - `pub enum TomlTarget { Table(&'static str), KeyedTable(&'static str) }` — `Table` is a single `[name]` table (e.g. `[project]`); `KeyedTable` is a map of named subtables `[name.<key>]` (e.g. `[models.<alias>]`, `[agents.<id>]`).
  - `pub struct Factory { pub name: &'static str, pub target: TomlTarget, pub fields: &'static [AuthField] }`
  - `pub const SURFACE: &[Factory]` — for the first fixture: `models`, `agent`.

**IMPORTANT (schema fact, verified in Task 1):** the real `ProjectConfig` represents both `models` and `agents` as *keyed tables* — `[models.<alias>]` and `[agents.<id>]`, NOT `[[agents]]` array-of-tables. The agent's id is the table key. In TS authoring the id comes from the exported `const` name (`export const fast` → `agents.fast`); in Python authoring the id is the dict key the author supplies (`agents={"fast": fast}`).
- Consumed by: `emit_ts` (Task 5), `emit_python` (Task 4).

- [ ] **Step 1: Write the failing test**

Add to `src/authoring.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_declares_agent_and_models() {
        let names: Vec<_> = SURFACE.iter().map(|f| f.name).collect();
        assert!(names.contains(&"agent"));
        assert!(names.contains(&"models"));
    }

    #[test]
    fn agent_has_required_display_name() {
        let agent = SURFACE.iter().find(|f| f.name == "agent").unwrap();
        let dn = agent.fields.iter().find(|f| f.sdk_name == "display_name").unwrap();
        assert!(dn.required);
        assert_eq!(dn.toml_key, "display_name");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e53 cargo nextest run -p tau-sdk-codegen authoring`
Expected: FAIL to compile (`SURFACE` undefined).

- [ ] **Step 3: Implement src/authoring.rs**

```rust
//! The authoring-surface descriptor: which factory has which fields, and how
//! each field maps into `tau.toml`. This is the composition the (post-lowering)
//! IR schema cannot describe; it mirrors what `tau-ts-extract` recognizes and
//! is pinned by the byte-equal conformance test.

/// The value shape of an authoring field (drives TS/Python type emission and
/// the Python TOML renderer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldTy {
    /// A string scalar.
    Str,
    /// A boolean scalar.
    Bool,
    /// The `[models]` alias table: name -> { backend, model }.
    ModelMap,
    /// A list of tool symbolic names.
    ToolList,
}

/// One authoring field on a factory.
#[derive(Debug, Clone, Copy)]
pub struct AuthField {
    /// Field name in the SDK surface (TS/Python).
    pub sdk_name: &'static str,
    /// TOML key it lowers to.
    pub toml_key: &'static str,
    /// Value shape.
    pub ty: FieldTy,
    /// Whether the field is required.
    pub required: bool,
}

/// Where a factory writes in `tau.toml`.
#[derive(Debug, Clone, Copy)]
pub enum TomlTarget {
    /// A single `[name]` table (e.g. `[project]`).
    Table(&'static str),
    /// A map of named subtables `[name.<key>]` (e.g. `[models.<alias>]`,
    /// `[agents.<id>]`). The key is the model alias / agent id.
    KeyedTable(&'static str),
}

/// One authoring factory.
#[derive(Debug, Clone, Copy)]
pub struct Factory {
    /// Factory function name (`agent`, `models`, ...).
    pub name: &'static str,
    /// TOML target.
    pub target: TomlTarget,
    /// Fields, in emission order.
    pub fields: &'static [AuthField],
}

const AGENT_FIELDS: &[AuthField] = &[
    AuthField { sdk_name: "display_name", toml_key: "display_name", ty: FieldTy::Str, required: true },
    AuthField { sdk_name: "package",      toml_key: "package",      ty: FieldTy::Str, required: true },
    AuthField { sdk_name: "model",        toml_key: "model",        ty: FieldTy::Str, required: true },
];

const MODELS_FIELDS: &[AuthField] = &[
    AuthField { sdk_name: "models", toml_key: "models", ty: FieldTy::ModelMap, required: true },
];

/// The authoring surface covered by the first conformance fixture.
pub const SURFACE: &[Factory] = &[
    Factory { name: "models", target: TomlTarget::KeyedTable("models"), fields: MODELS_FIELDS },
    Factory { name: "agent",  target: TomlTarget::KeyedTable("agents"), fields: AGENT_FIELDS },
];
```

- [ ] **Step 4: Wire the module in lib.rs**

Add `pub mod authoring;` to `src/lib.rs`.

- [ ] **Step 5: Run to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e53 cargo nextest run -p tau-sdk-codegen authoring`
Expected: PASS (both authoring tests).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-sdk-codegen/src
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(sdk-codegen): declare authoring-surface table"
```

---

### Task 4: Python emitter + generate() → sdk/python

Emit the Python `tau_sdk` package: typed dataclass builders plus a deterministic `tau.toml` renderer. Add the crate's public `generate()` entry and a thin bin so `cargo run` (re)writes `sdk/`. Prove the generated package renders TOML that lowers equal to the fixture's `tau.toml`.

**Files:**
- Create: `crates/tau-sdk-codegen/src/emit_python.rs`
- Create: `crates/tau-sdk-codegen/src/emit.rs` (shared `generate()` orchestration)
- Create: `crates/tau-sdk-codegen/src/bin/gen.rs`
- Modify: `crates/tau-sdk-codegen/src/lib.rs`
- Test: `crates/tau-sdk-codegen/tests/python_render.rs`
- Create: `crates/tau-sdk-codegen/tests/fixtures/basic_agent/tau.toml`
- Create: `crates/tau-sdk-codegen/tests/fixtures/basic_agent/project.py`
- Generated (committed after Step 7): `sdk/python/pyproject.toml`, `sdk/python/tau_sdk/__init__.py`, `sdk/python/tau_sdk/factories.py`

**Interfaces:**
- Consumes: `SchemaModel` (Task 2), `SURFACE`/`Factory`/`FieldTy` (Task 3), `common::{lower_toml_bytes, run_python_toml}` (Task 1).
- Produces:
  - `pub fn generate(repo_root: &Path) -> Result<(), CodegenError>` (writes `sdk/ts` + `sdk/python`; in this task, `sdk/python` only — `sdk/ts` added in Task 5).
  - `pub fn emit_python::render_package(schema: &SchemaModel) -> BTreeMap<PathBuf, String>` (relative path -> file contents).

- [ ] **Step 1: Create the basic_agent fixture — tau.toml**

`crates/tau-sdk-codegen/tests/fixtures/basic_agent/tau.toml` (real `ProjectConfig` shape — `[project]` + top-level `packages` + `[agents.<id>]` keyed table, verified in Task 1 against the proven `models_conformance` fixture):

```toml
packages = ["anthropic"]

[project]
name = "basic-agent"

[models.haiku]
backend = "anthropic"
model = "claude-haiku-4-5"

[agents.fast]
display_name = "Fast"
package = "research@^0.1"
model = "haiku"
```

- [ ] **Step 2: Create the basic_agent fixture — project.py**

`crates/tau-sdk-codegen/tests/fixtures/basic_agent/project.py` (authored against the generated `tau_sdk`; imported via PYTHONPATH pointing at `sdk/python`). Note: `agents` is a dict keyed by agent id (the id becomes `[agents.<id>]`), matching how TS derives the id from the exported const name:

```python
from tau_sdk import agent, models, model, print_toml

m = models(haiku=model(backend="anthropic", model="claude-haiku-4-5"))
fast = agent(display_name="Fast", package="research@^0.1", model="haiku")

print_toml(project="basic-agent", models=m, agents={"fast": fast})
```

- [ ] **Step 3: Write the failing test**

`crates/tau-sdk-codegen/tests/python_render.rs`:

```rust
mod common;

use std::path::Path;

/// The generated Python SDK, driving the basic_agent fixture, must render TOML
/// that lowers to the same canonical IR as the fixture's tau.toml.
#[test]
fn generated_python_sdk_lowers_equal_to_toml() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest.parent().unwrap().parent().unwrap();

    // Regenerate the SDK into the repo tree so the test drives current output.
    tau_sdk_codegen::generate(repo_root).expect("generate SDK");

    let fixture = manifest.join("tests/fixtures/basic_agent");
    let toml = std::fs::read_to_string(fixture.join("tau.toml")).unwrap();
    let toml_bytes = common::lower_toml_bytes(&toml);

    let sdk_python = repo_root.join("sdk/python");
    match common::run_python_toml(&fixture.join("project.py"), Some(&sdk_python)) {
        None => eprintln!("SKIP: python3 not available"),
        Some(py_toml) => {
            let py_bytes = common::lower_toml_bytes(&py_toml);
            assert_eq!(toml_bytes, py_bytes, "python SDK output must lower equal");
        }
    }
}
```

- [ ] **Step 4: Run to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e53 cargo nextest run -p tau-sdk-codegen generated_python`
Expected: FAIL to compile (`tau_sdk_codegen::generate` undefined).

- [ ] **Step 5: Implement src/emit_python.rs**

```rust
//! Emit the Python `tau_sdk` authoring package.
//!
//! Output is stdlib-only (dataclasses/typing): typed builders plus a
//! deterministic `render_project`/`print_toml` that walks authored objects in
//! `SURFACE` order and prints `tau.toml`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::schema::SchemaModel;

/// Render the Python package as relative-path -> file-contents.
pub fn render_package(_schema: &SchemaModel) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();

    out.insert(
        PathBuf::from("pyproject.toml"),
        r#"[project]
name = "tau-sdk"
version = "0.0.0"
description = "Typed Python authoring SDK for tau agents"
requires-python = ">=3.9"

[tool.setuptools.packages.find]
where = ["."]
"#
        .to_string(),
    );

    out.insert(
        PathBuf::from("tau_sdk/__init__.py"),
        "from .factories import agent, tool, models, model, render_project, print_toml\n\n\
         __all__ = [\"agent\", \"tool\", \"models\", \"model\", \"render_project\", \"print_toml\"]\n"
            .to_string(),
    );

    out.insert(PathBuf::from("tau_sdk/factories.py"), FACTORIES_PY.to_string());
    out
}

/// The hand-tuned factories module. Fields mirror `authoring::SURFACE`; the
/// TOML renderer emits tables in SURFACE order. Kept as a literal because the
/// authoring surface is small and the byte-equal test pins its correctness.
const FACTORIES_PY: &str = r#"# GENERATED by tau-sdk-codegen. Do not edit by hand.
"""Typed authoring builders for tau agents (stdlib only)."""
from dataclasses import dataclass
from typing import Optional


@dataclass
class Model:
    backend: str
    model: str


@dataclass
class ToolConfig:
    native: str
    description: Optional[str] = None


@dataclass
class AgentConfig:
    display_name: str
    package: str
    model: str


def model(*, backend: str, model: str) -> Model:
    return Model(backend=backend, model=model)


def models(**aliases: Model) -> dict:
    return dict(aliases)


def tool(*, native: str, description: Optional[str] = None) -> ToolConfig:
    return ToolConfig(native=native, description=description)


def agent(*, display_name: str, package: str, model: str) -> AgentConfig:
    return AgentConfig(display_name=display_name, package=package, model=model)


def _toml_str(value: str) -> str:
    return '"' + value.replace('\\', '\\\\').replace('"', '\\"') + '"'


def render_project(project: str = "project", models: Optional[dict] = None, agents: Optional[dict] = None) -> str:
    models = models or {}
    agents = agents or {}
    # `packages` must declare every model backend (ProjectConfig validation);
    # it is authoring-only and dropped during lowering, so its exact contents
    # do not affect the canonical IR — only that validation passes.
    backends = sorted({m.backend for m in models.values()})
    lines = []
    lines.append("packages = [" + ", ".join(_toml_str(b) for b in backends) + "]")
    lines.append("")
    lines.append("[project]")
    lines.append("name = " + _toml_str(project))
    lines.append("")
    for alias, m in models.items():
        lines.append("[models." + alias + "]")
        lines.append("backend = " + _toml_str(m.backend))
        lines.append("model = " + _toml_str(m.model))
        lines.append("")
    for aid, a in agents.items():
        lines.append("[agents." + aid + "]")
        lines.append("display_name = " + _toml_str(a.display_name))
        lines.append("package = " + _toml_str(a.package))
        lines.append("model = " + _toml_str(a.model))
        lines.append("")
    return "\n".join(lines).rstrip("\n") + "\n"


def print_toml(project: str = "project", models: Optional[dict] = None, agents: Optional[dict] = None) -> None:
    import sys
    sys.stdout.write(render_project(project=project, models=models, agents=agents))
"#;
```

- [ ] **Step 6: Implement src/emit.rs (generate orchestration)**

```rust
//! Orchestrates writing the generated SDK packages under the repo root.

use std::path::Path;

use crate::emit_python;
use crate::error::CodegenError;
use crate::schema::SchemaModel;

/// Generate all SDK packages under `repo_root` (writes `sdk/python`; `sdk/ts`
/// is added by the TS emitter task).
pub fn generate(repo_root: &Path) -> Result<(), CodegenError> {
    let schema = SchemaModel::load(repo_root)?;

    let py = emit_python::render_package(&schema);
    write_tree(&repo_root.join("sdk/python"), py)?;

    Ok(())
}

fn write_tree(
    base: &Path,
    files: std::collections::BTreeMap<std::path::PathBuf, String>,
) -> Result<(), CodegenError> {
    for (rel, contents) in files {
        let path = base.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, contents)?;
    }
    Ok(())
}
```

- [ ] **Step 7: Wire lib.rs + bin**

In `src/lib.rs` add:

```rust
pub mod authoring;
pub mod emit;
pub mod emit_python;
pub mod schema;

pub use emit::generate;
```

Create `crates/tau-sdk-codegen/src/bin/gen.rs`:

```rust
//! Regenerate the SDK packages: `cargo run -p tau-sdk-codegen --bin gen`.
use std::path::Path;

fn main() -> anyhow::Result<()> {
    // repo root = two levels up from this crate's manifest dir at dev time;
    // callers may pass an explicit root as argv[1].
    let root = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .to_path_buf()
        });
    tau_sdk_codegen::generate(&root)?;
    eprintln!("generated SDK under {}/sdk", root.display());
    Ok(())
}
```

- [ ] **Step 8: Run to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e53 cargo nextest run -p tau-sdk-codegen generated_python`
Expected: PASS (`generated_python_sdk_lowers_equal_to_toml`) — the test calls `generate()`, writing `sdk/python`, then runs `project.py` through it. SKIP line if no `python3`.

- [ ] **Step 9: Commit (including the generated sdk/python)**

```bash
git add crates/tau-sdk-codegen sdk/python
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(sdk-codegen): Python emitter + generate() → sdk/python"
```

---

### Task 5: TypeScript emitter → sdk/ts

Emit the `@tau/sdk` TS package whose factories match `tau-ts-extract`'s recognized surface, and wire it into `generate()`. Prove the fixture `project.ts` (authored against those factories) extracts and lowers equal to the fixture `tau.toml`.

**Files:**
- Create: `crates/tau-sdk-codegen/src/emit_ts.rs`
- Modify: `crates/tau-sdk-codegen/src/emit.rs` (also write `sdk/ts`)
- Modify: `crates/tau-sdk-codegen/src/lib.rs` (add `pub mod emit_ts;`)
- Test: `crates/tau-sdk-codegen/tests/ts_conformance.rs`
- Create: `crates/tau-sdk-codegen/tests/fixtures/basic_agent/project.ts`
- Generated (committed): `sdk/ts/package.json`, `sdk/ts/src/factories.ts`, `sdk/ts/tsconfig.json`

**Interfaces:**
- Consumes: `SchemaModel`, `SURFACE`, `common::lower_toml_bytes`, `tau_ts_extract::extract_project`.
- Produces: `pub fn emit_ts::render_package(schema: &SchemaModel) -> BTreeMap<PathBuf, String>`.

- [ ] **Step 1: Create the fixture — project.ts**

`crates/tau-sdk-codegen/tests/fixtures/basic_agent/project.ts`:

```ts
import { agent, models } from "tau";

export const m = models({ haiku: { backend: "anthropic", model: "claude-haiku-4-5" } });
export const fast = agent({ display_name: "Fast", package: "research@^0.1", model: "haiku" });
```

Note: the TS fixture has no `[project]` name or `packages` even though the `tau.toml` fixture does. That is correct — `tau-ts-extract` synthesizes both internally (packages inferred from `[models]` backends), and both are authoring-only fields dropped during lowering, so they never reach the canonical IR. The agent id `fast` comes from the exported `const` name, matching the TOML `[agents.fast]` key. This is exactly the proven `crates/tau-ts-extract/tests/fixtures/models_conformance/` pattern.

- [ ] **Step 2: Write the failing test**

`crates/tau-sdk-codegen/tests/ts_conformance.rs`:

```rust
mod common;

use std::path::Path;

/// The fixture project.ts, authored against the generated @tau/sdk factory
/// surface, must extract + lower to the same canonical IR as the tau.toml.
#[test]
fn ts_fixture_lowers_equal_to_toml() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/basic_agent");

    let toml = std::fs::read_to_string(fixture.join("tau.toml")).unwrap();
    let toml_bytes = common::lower_toml_bytes(&toml);

    let ts_src = std::fs::read_to_string(fixture.join("project.ts")).unwrap();
    let ts_cfg = tau_ts_extract::extract_project(&ts_src, &fixture.join("project.ts"))
        .expect("extract project.ts");
    let ts_bytes = common::lower_config_bytes(&ts_cfg);

    assert_eq!(toml_bytes, ts_bytes, "TS fixture must lower equal to TOML");
}

/// The generated @tau/sdk source must declare each factory the fixture uses.
#[test]
fn generated_ts_declares_fixture_factories() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    tau_sdk_codegen::generate(repo_root).expect("generate");
    let src = std::fs::read_to_string(repo_root.join("sdk/ts/src/factories.ts")).unwrap();
    assert!(src.contains("export const agent"));
    assert!(src.contains("export const models"));
}
```

Note: `common::lower_config_bytes` is already defined in `tests/common.rs` (Task 1, Step 7).

- [ ] **Step 3: Run to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e53 cargo nextest run -p tau-sdk-codegen ts_`
Expected: `ts_fixture_lowers_equal_to_toml` PASSES immediately (tau-ts-extract already recognizes these factories); `generated_ts_declares_fixture_factories` FAILS (no `sdk/ts/src/factories.ts` yet).

- [ ] **Step 4: Implement src/emit_ts.rs**

```rust
//! Emit the `@tau/sdk` TypeScript authoring package. Factory names + fields
//! match `tau-ts-extract`'s recognized surface so authored `project.ts`
//! extracts unchanged. Import source must be exactly `"tau"`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::schema::SchemaModel;

/// Render the TS package as relative-path -> file-contents.
pub fn render_package(_schema: &SchemaModel) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();

    out.insert(
        PathBuf::from("package.json"),
        r#"{
  "name": "@tau/sdk",
  "version": "0.0.0",
  "description": "Typed TypeScript authoring SDK for tau agents",
  "types": "src/factories.ts",
  "main": "src/factories.ts"
}
"#
        .to_string(),
    );

    out.insert(
        PathBuf::from("tsconfig.json"),
        r#"{
  "compilerOptions": {
    "strict": true,
    "target": "ES2020",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "declaration": true
  }
}
"#
        .to_string(),
    );

    out.insert(PathBuf::from("src/factories.ts"), FACTORIES_TS.to_string());
    out
}

const FACTORIES_TS: &str = r#"// GENERATED by tau-sdk-codegen. Do not edit by hand.
export interface Model { backend: string; model: string }
export interface ToolConfig { native: string; description?: string }
export interface AgentConfig { display_name: string; package: string; model: string }

export const models = (m: Record<string, Model>): Record<string, Model> => m;
export const tool = (c: ToolConfig): ToolConfig => c;
export const agent = (c: AgentConfig): AgentConfig => c;
"#;
```

- [ ] **Step 5: Wire emit_ts into generate() and lib.rs**

In `src/emit.rs`, after the `sdk/python` write, add:

```rust
    let ts = crate::emit_ts::render_package(&schema);
    write_tree(&repo_root.join("sdk/ts"), ts)?;
```

In `src/lib.rs` add `pub mod emit_ts;`.

- [ ] **Step 6: Run to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e53 cargo nextest run -p tau-sdk-codegen ts_`
Expected: both TS tests PASS.

- [ ] **Step 7: Commit (including generated sdk/ts)**

```bash
git add crates/tau-sdk-codegen sdk/ts
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(sdk-codegen): TypeScript emitter → sdk/ts"
```

---

### Task 6: The 5.3 acceptance test — three-way byte-equal IR

Combine the three surfaces into the single acceptance test the roadmap names: TOML, TS, and Python authoring of the *same* `basic_agent` all lower to byte-identical canonical IR.

**Files:**
- Create: `crates/tau-sdk-codegen/tests/byte_equal.rs`

**Interfaces:**
- Consumes: `common::{lower_toml_bytes, lower_config_bytes, run_python_toml}`, `tau_sdk_codegen::generate`, `tau_ts_extract::extract_project`.

- [ ] **Step 1: Write the acceptance test**

`crates/tau-sdk-codegen/tests/byte_equal.rs`:

```rust
mod common;

use std::path::Path;

/// EPIC 5.3 acceptance: the same agent authored in TOML, TS, and Python lowers
/// to byte-identical canonical IR. TOML and TS run in-process; Python is
/// executed live via python3 (skipped, with the TOML==TS assertion still
/// enforced, when python3 is unavailable).
#[test]
fn toml_ts_python_lower_to_identical_ir() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest.parent().unwrap().parent().unwrap();
    tau_sdk_codegen::generate(repo_root).expect("generate SDK");

    let fixture = manifest.join("tests/fixtures/basic_agent");

    // TOML
    let toml = std::fs::read_to_string(fixture.join("tau.toml")).unwrap();
    let toml_bytes = common::lower_toml_bytes(&toml);

    // TS (in-process via swc)
    let ts_src = std::fs::read_to_string(fixture.join("project.ts")).unwrap();
    let ts_cfg = tau_ts_extract::extract_project(&ts_src, &fixture.join("project.ts"))
        .expect("extract project.ts");
    let ts_bytes = common::lower_config_bytes(&ts_cfg);

    assert_eq!(toml_bytes, ts_bytes, "TOML and TS must lower to identical IR");

    // Python (live)
    let sdk_python = repo_root.join("sdk/python");
    match common::run_python_toml(&fixture.join("project.py"), Some(&sdk_python)) {
        None => eprintln!("SKIP: python3 unavailable; TOML==TS still asserted"),
        Some(py_toml) => {
            let py_bytes = common::lower_toml_bytes(&py_toml);
            assert_eq!(toml_bytes, py_bytes, "TOML and Python must lower to identical IR");
        }
    }
}
```

- [ ] **Step 2: Run to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e53 cargo nextest run -p tau-sdk-codegen toml_ts_python`
Expected: PASS (three-way equal; or TOML==TS with SKIP line if no python3).

- [ ] **Step 3: Commit**

```bash
git add crates/tau-sdk-codegen/tests/byte_equal.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "test(sdk-codegen): EPIC 5.3 three-way byte-equal IR acceptance"
```

---

### Task 7: Drift guard — committed packages match fresh generate()

Prove the checked-in `sdk/ts` + `sdk/python` equal a fresh `generate()` into a temp dir, so the committed packages can never silently drift from the emitters. Mirrors `crates/tau-ir/tests/schema_export.rs`.

**Files:**
- Create: `crates/tau-sdk-codegen/tests/drift.rs`
- Modify: `crates/tau-sdk-codegen/src/emit.rs` (add a pure `render_all` returning the file map, so the drift test can compare without writing to the repo)

**Interfaces:**
- Produces: `pub fn emit::render_all(repo_root: &Path) -> Result<BTreeMap<PathBuf, String>, CodegenError>` — every generated file keyed by repo-relative path (e.g. `sdk/python/tau_sdk/factories.py`).
- `generate()` is refactored to call `render_all` then write.

- [ ] **Step 1: Refactor emit.rs to expose render_all**

Replace the body of `src/emit.rs` with:

```rust
//! Orchestrates rendering + writing the generated SDK packages.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::emit_python;
use crate::emit_ts;
use crate::error::CodegenError;
use crate::schema::SchemaModel;

/// Render every generated file, keyed by repo-relative path.
pub fn render_all(repo_root: &Path) -> Result<BTreeMap<PathBuf, String>, CodegenError> {
    let schema = SchemaModel::load(repo_root)?;
    let mut all = BTreeMap::new();
    for (rel, contents) in emit_python::render_package(&schema) {
        all.insert(PathBuf::from("sdk/python").join(rel), contents);
    }
    for (rel, contents) in emit_ts::render_package(&schema) {
        all.insert(PathBuf::from("sdk/ts").join(rel), contents);
    }
    Ok(all)
}

/// Generate all SDK packages under `repo_root`.
pub fn generate(repo_root: &Path) -> Result<(), CodegenError> {
    for (rel, contents) in render_all(repo_root)? {
        let path = repo_root.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, contents)?;
    }
    Ok(())
}
```

- [ ] **Step 2: Write the failing test**

`crates/tau-sdk-codegen/tests/drift.rs`:

```rust
use std::path::Path;

/// The checked-in sdk/ packages must equal a fresh render. If this fails, run
/// `cargo run -p tau-sdk-codegen --bin gen` and commit the result.
#[test]
fn committed_sdk_matches_fresh_render() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let rendered = tau_sdk_codegen::emit::render_all(repo_root).expect("render");

    let mut drifted = Vec::new();
    for (rel, expected) in &rendered {
        let path = repo_root.join(rel);
        let actual = std::fs::read_to_string(&path).unwrap_or_default();
        if &actual != expected {
            drifted.push(rel.display().to_string());
        }
    }
    assert!(
        drifted.is_empty(),
        "committed SDK drifted from generator; run `cargo run -p tau-sdk-codegen --bin gen` and commit:\n{}",
        drifted.join("\n")
    );
}
```

- [ ] **Step 3: Run to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e53 cargo nextest run -p tau-sdk-codegen drift`
Expected: PASS (committed files were written by the same emitters in Tasks 4–5). If it fails, run `env CARGO_TARGET_DIR=target/agent-e53 cargo run -p tau-sdk-codegen --bin gen` and re-commit `sdk/`.

- [ ] **Step 4: Commit**

```bash
git add crates/tau-sdk-codegen sdk
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "test(sdk-codegen): drift guard — committed sdk equals fresh render"
```

---

### Task 8: Full crate gate, docs, roadmap checkbox, PR

Run the full crate test suite + clippy, add a short README to the crate and each package, tick the roadmap, and open the PR.

**Files:**
- Create: `crates/tau-sdk-codegen/README.md`
- Create: `sdk/README.md`
- Modify: `docs/superpowers/plans/vision-roadmap.md` (mark 5.3)

- [ ] **Step 1: Run the full crate test suite**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e53 cargo nextest run -p tau-sdk-codegen`
Expected: all tests PASS (byte_equal, python_render, ts_conformance, drift, harness_proof, unit tests).

- [ ] **Step 2: Run clippy + fmt**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e53 cargo clippy -p tau-sdk-codegen --all-targets -- -D warnings`
Run: `timeout 30 env CARGO_TARGET_DIR=target/agent-e53 cargo fmt -p tau-sdk-codegen -- --check`
Expected: no warnings, formatting clean. Fix any issues and re-run.

- [ ] **Step 3: Write the crate README**

`crates/tau-sdk-codegen/README.md`:

```markdown
# tau-sdk-codegen

Generates tau's typed authoring SDKs from the frozen IR JSON schema:

- `sdk/ts/`     — npm `@tau/sdk` (TypeScript)
- `sdk/python/` — PyPI `tau-sdk` (import `tau_sdk`)

The SDKs are *authoring front-ends*, not IR emitters: they produce the same
`ProjectConfig` the TOML surface parses to, so TOML / TS / Python all lower to
byte-identical canonical IR via the single Rust lowering pass (`tau-ir-lower`).

## Regenerate

    cargo run -p tau-sdk-codegen --bin gen

Then commit `sdk/`. The `drift` test fails if the committed packages diverge
from the emitters.

## Acceptance

`tests/byte_equal.rs` lowers one agent authored three ways and asserts
byte-equal canonical IR (Python via live `python3`; skipped when absent).
```

- [ ] **Step 4: Write the sdk/ README**

`sdk/README.md`:

```markdown
# tau authoring SDKs (generated)

These packages are generated by `tau-sdk-codegen` from the frozen IR JSON
schema. Do not edit by hand — run `cargo run -p tau-sdk-codegen --bin gen`.

- `ts/`     — `@tau/sdk`   (`import { agent, tool, models } from "tau"`)
- `python/` — `tau-sdk`    (`from tau_sdk import agent, tool, models`)

Publishing to npm/PyPI is out of scope for EPIC 5.3.
```

- [ ] **Step 5: Tick the roadmap**

In `docs/superpowers/plans/vision-roadmap.md`, mark the 5.3 line as delivered (match the file's existing done-marking convention — e.g. append ` ✅` or check the box; look at how 5.1/5.2 or other completed stories are marked and follow suit).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-sdk-codegen/README.md sdk/README.md docs/superpowers/plans/vision-roadmap.md
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "docs(sdk-codegen): READMEs + roadmap 5.3 checkbox"
```

- [ ] **Step 7: Push and open the PR**

```bash
git push -u origin feat/epic-5-3-sdk-codegen
gh pr create --base main --title "feat(sdk-codegen): EPIC 5.3 — authoring-SDK codegen (TS + Python), byte-equal IR" \
  --body "$(cat <<'EOF'
Implements EPIC 5.3. New `tau-sdk-codegen` crate emits typed TS (`@tau/sdk`) and
Python (`tau-sdk`) authoring front-ends from the frozen IR JSON schema.

The SDKs defer lowering to the Rust core (mirrors `tau-ts-extract`), so the same
agent in TOML / TS / Python lowers to byte-identical canonical IR. Acceptance
test (`tests/byte_equal.rs`) proves the three-way equality (Python via live
`python3`, self-skipping when absent).

New crate only — no changes to `tau-ir`, `tau-pkg`, `tau-ir-lower`, or the schema.
Publishing to npm/PyPI is out of scope (5.4 owns the typed consumers).

Design: `docs/superpowers/specs/2026-07-23-epic-5-3-sdk-codegen-design.md`
Plan:   `docs/superpowers/plans/2026-07-23-epic-5-3-sdk-codegen.md`

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 8: Enrol auto-merge**

```bash
gh pr merge --squash --delete-branch --auto
```

Then poll CI; `gh pr update-branch <PR#>` whenever the branch is BEHIND.

---

## Self-Review

**Spec coverage:**
- Codegen crate from IR schema → Tasks 1–5, 7 ✓
- Ship TS + Python → Tasks 4 (Python), 5 (TS) ✓
- Byte-equal TOML/TS/Python acceptance (live python3) → Task 6 ✓ (TDD spine seeded in Task 1)
- Schema consumed (leaf/vocab) → Task 2 ✓
- Authoring surface as owned table → Task 3 ✓
- Drift guard mirroring schema_export.rs → Task 7 ✓
- Top-level `sdk/ts`+`sdk/python`, `@tau/sdk`/`tau-sdk`, not workspace members → Tasks 4/5 ✓
- New-crate-only, no core edits → Global Constraints; only root Cargo.toml + roadmap/docs touched ✓
- Publishing out of scope → README/PR body state it ✓
- forbid(unsafe_code), thiserror boundary/anyhow internal → Task 1 ✓
- CARGO rules (target dir agent-e53, -p, timeouts, nextest) → every run step ✓

**Placeholder scan:** No TBD/TODO; every code step shows complete content; the only deferred choice is the roadmap done-marker convention (Task 8 Step 5), which instructs matching the file's existing style — acceptable since it's a one-token cosmetic match.

**Type consistency:** `render_package` (per-emitter, returns `BTreeMap<PathBuf,String>`) vs `render_all` (repo-root-relative, Task 7) are distinct and consistently used; `generate()` signature stable across Tasks 4/7; `common::{lower_toml_bytes,lower_config_bytes,run_python_toml,python3_available}` defined once (Task 1) and reused; Python factory `models(**aliases)`/`model(...)` and TS `models`/`agent` names match the fixtures.
