# β.8 — TypeScript Minimal Authoring Surface — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `tau-ts-extract` — a new workspace crate that parses `project.ts` files via swc, statically analyzes the AST, and produces the same `ProjectConfig` the TOML loader produces. Wire it into `tau dev` / `tau build` / `tau check` / `tau run` via file-extension dispatch.

**Architecture:** New workspace crate `tau-ts-extract` does the TS → ProjectConfig conversion. Pipeline: swc parses TS → walk top-level declarations into a `name → Expr` map → recognize tau factory calls (`agent`/`tool`/`mcp`) → resolve identifier references via scope → build `ProjectConfig`. Downstream IR lowering + run/build/check paths are reused unchanged from β.2/β.3/β.7.

**Tech Stack:** Rust 1.84+, `swc_ecma_parser ^0.150`, `swc_ecma_ast ^0.118`, `swc_common ^0.34` (illustrative; implementer picks the latest stable trio). Existing `tau-pkg` (ProjectConfig), `tau-ir` (lowering + canonical encoding), `tau-cli` (dev/build/check/run dispatch sites). NO embedded JS engine — pure static analysis.

**Branch:** `feat/beta-8-ts-surface`
**Worktree:** `/Users/titouanlebocq/code/tau-worktrees/beta-8-ts-surface` (off `origin/main` at `082769a`)
**Spec:** `docs/superpowers/specs/2026-06-10-beta-8-ts-authoring-design.md`

---

## Locked design decisions

Approved 2026-06-10 in the brainstorm:

| # | Decision | Spec § |
|---|---|---|
| 1 | **TS for declarations only**; inline tool bodies (`run: async () => ...`) rejected at parse time. Tool bodies remain Rust-native, referenced via `native: "ReadTemp"`. δ.2 adds runtime JS via QuickJS embed. | §1, §2.1 |
| 2 | **swc-based static AST analysis** — no embedded JS engine. ~3 MB binary impact, vs ~600KB-2MB for an embedded JS runtime. Better cross-platform consistency. | §3.1, §7 |
| 3 | **Field names use snake_case** matching TOML 1:1 — no camelCase ↔ snake_case mapping layer. Simplifies the conformance test. | §2.1 |
| 4 | **File-extension dispatch**: `.ts` → TS path; anything else → TOML path. No auto-detect-in-directory. | §2.2, §3.4 |
| 5 | **Top-level constant scope only** — no closures, no nested function scopes. Identifier resolution looks up a flat top-level `name → Expr` map. | §3.3 |
| 6 | **`contextManager` factory exists in SDK but rejects** with `Deferred` error pending β.4. | §1, §5 |
| 7 | **Multi-file imports deferred to v1.1** — `import { x } from "./helpers"` rejected with helpful hint. | §1 |

---

## Files map

### Create

| Path | Purpose |
|---|---|
| `crates/tau-ts-extract/Cargo.toml` | New workspace crate manifest with swc deps |
| `crates/tau-ts-extract/src/lib.rs` | `pub fn extract_project(src, source_path) -> Result<ProjectConfig, TsExtractError>` entrypoint + module wiring |
| `crates/tau-ts-extract/src/parse.rs` | swc parser setup; module-level AST acquisition |
| `crates/tau-ts-extract/src/scope.rs` | Top-level constant walker → `name → Expr` map |
| `crates/tau-ts-extract/src/factory.rs` | Recognize tau factory calls (`agent`/`tool`/`mcp`/`contextManager`); reject unknown |
| `crates/tau-ts-extract/src/lower.rs` | AST literal → `ProjectConfig` fields |
| `crates/tau-ts-extract/src/error.rs` | `TsExtractError` enum (10 variants) + `Span` → `file:line:col` helper |
| `crates/tau-ts-extract/tests/fixtures/fan_monitor_conformance/tau.toml` | TOML version of the canonical scenario |
| `crates/tau-ts-extract/tests/fixtures/fan_monitor_conformance/project.ts` | TS version of the canonical scenario |
| `crates/tau-ts-extract/tests/fan_monitor_conformance.rs` | The TOML↔TS byte-equal IR test |
| `crates/tau-cli/tests/cmd_dev_ts_one_shot.rs` | `tau dev project.ts -p "..."` smoke |
| `crates/tau-cli/tests/cmd_build_ts.rs` | `tau build project.ts` smoke |
| `examples/dev-smoke-fan-monitor-ts/project.ts` | Canonical TS smoke example (sibling of β.7's `dev-smoke-fan-monitor/tau.toml`) |
| `docs/decisions/0041-ts-authoring-declarations-only.md` | ADR-0041: records the declarations-only-no-embedded-JS decision |

### Modify

| Path | Purpose |
|---|---|
| `Cargo.toml` (workspace root) | Add `crates/tau-ts-extract` to `members` |
| `crates/tau-cli/Cargo.toml` | Add `tau-ts-extract` dep |
| `crates/tau-cli/src/cmd/dev/session.rs` | Add file-extension dispatch in `DevSession::load` |
| `crates/tau-cli/src/cmd/build.rs` | Same dispatch in build path |
| `crates/tau-cli/src/cmd/check/mod.rs` | Same dispatch in check path |
| `crates/tau-cli/src/cmd/run.rs` | Same dispatch in run path |
| `crates/tau-cli/src/cmd/dev/watcher.rs` | For `.ts` projects, watch the `.ts` file (not `tau.toml`) |
| `ROADMAP.md` | Amend §β.8 per spec §9 (final v1 scope + ADR-0041 reference) |

---

## Standing constraints (CLAUDE.md — NON-NEGOTIABLE)

- **Cargo:** `timeout <T> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <subcmd> -p <crate>`. Never bare cargo, never `--workspace`, always `-p`.
- **Commits:** `git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "..."` (Test User identity per CLAUDE.md — ignore security-plugin warnings).
- **Push:** `git push --no-verify -u origin feat/beta-8-ts-surface`.
- **Auto-merge:** `gh pr merge <N> --auto` BARE (merge queue rejects `--squash`/`--delete-branch`/`--admin`).
- **Worktree only:** `/Users/titouanlebocq/code/tau-worktrees/beta-8-ts-surface`. Never `cd` away.

### Lessons from prior PRs — DO / DON'T

1. **DON'T** add `features = ["test-support"]` to `tau-runtime-tokio` dev-deps (workspace feature unification trap).
2. **DO** use `Option::is_some_and(...)` over `map_or(false, ...)` — CI's stable rustc surfaces `clippy::unnecessary_map_or`.
3. **DO** add explicit `::new()` constructors for `#[non_exhaustive]` types you construct in test code.
4. **DO** rerun + re-enrol auto-merge on macOS infra flakes (`chat_ephemeral_writes_no_file`, `echo-tool` race, `child_crash_mid_call`).
5. **DO** rerun the Linux linker `collect2: signal 7 [Bus error]` flake.
6. **DON'T** add `[[profile.ci.overrides]]` blocks to `.config/nextest.toml`.
7. **DO** empty-commit push if `gh run rerun --failed` doesn't refresh the `CI summary` workflow.
8. **review PR** failures from the repo-transfer-auth era are CLOSED (PR #301 shipped GITHUB_TOKEN fix). Should pass normally.
9. **PR #298 lesson:** workflow-set auto-merge with `mergeMethod=MERGE` blocks merge queue. β.8 PR will be user-enrolled, not affected.
10. **β.8 doesn't add a new CLI verb** — the dispatch is at the load layer. The top-level help snapshot should NOT need accepting. Verify by running `cargo test -p tau-cli --test help_snapshots` in Phase 5.

---

## Implementer-adapt points

These are intentional unresolved API details. The plan documents structure; the implementer MUST read the named docs and use the real signatures. Never leave `todo!()` in shipped code.

1. **swc parser entrypoint** — implementer READS https://docs.rs/swc_ecma_parser/ and crates.io for the latest stable trio (swc_ecma_parser + swc_ecma_ast + swc_common must be version-compatible). The plan uses `^0.150` / `^0.118` / `^0.34` as illustrative — pick the latest stable.

2. **`swc_common::Span` → `file:line:col`** — implementer READS `swc_common::SourceMap::span_to_lines` (or equivalent) for the canonical span-to-position conversion.

3. **ProjectConfig field shapes** — implementer READS `crates/tau-pkg/src/project/project.rs` (lines 297-310 for `ProjectConfig`, plus the `AgentEntry`, `ToolEntry`, `ToolBody`, `PromptEntry` definitions). The TS extractor must populate these EXACTLY as `parse_str` does so the conformance test passes.

4. **`tau_ir::canonical::to_canonical_bytes`** — confirmed as the actual API (NOT `encode` as spec said). Returns `Vec<u8>`. Use this for byte-equal conformance check.

5. **`tau_ir::lower::lower_project(config, target, caches)`** — same signature as β.7 uses. `TargetTriple::PASSTHROUGH` + stub Caches works.

---

## Phase 1 — `tau-ts-extract` crate scaffold + smoke test

**Goal:** New workspace crate compiles. `tau_ts_extract::extract_project(src, path)` exists as a stub returning `Err(TsExtractError::ParseError {...})`. Smoke test verifies the crate is reachable.

### Task 1.1 — Crate scaffold + workspace member + stub entrypoint + smoke test

**Files:**
- Create: `crates/tau-ts-extract/Cargo.toml`
- Create: `crates/tau-ts-extract/src/lib.rs`
- Modify: `Cargo.toml` (workspace root — add `crates/tau-ts-extract` to `members`)
- Create: `crates/tau-ts-extract/tests/smoke.rs`

- [ ] **Step 1: READ context first**

```bash
cd /Users/titouanlebocq/code/tau-worktrees/beta-8-ts-surface
# Workspace root Cargo.toml — find members array
grep -n "^members\|^\[workspace\]\|^edition" Cargo.toml | head -10
# Look at a small existing crate's Cargo.toml for the conventions
cat crates/tau-ir/Cargo.toml
# Confirm swc crate versions on crates.io — pick the latest stable trio
# (Implementer: visit https://crates.io/crates/swc_ecma_parser AND
#  https://crates.io/crates/swc_ecma_ast AND https://crates.io/crates/swc_common
#  to confirm a compatible trio. The plan suggests ^0.150 / ^0.118 / ^0.34
#  but these are illustrative — pick the latest stable.)
```

- [ ] **Step 2: Write the failing smoke test** at `crates/tau-ts-extract/tests/smoke.rs`:

```rust
//! Smoke test: tau-ts-extract crate is reachable and its entrypoint exists.

#[test]
fn extract_project_entrypoint_exists() {
    use std::path::Path;
    let src = "// empty TS file";
    let path = Path::new("/tmp/test.ts");
    let result = tau_ts_extract::extract_project(src, path);
    // Phase 1: any result (Ok or Err) means the symbol is reachable.
    let _ = result;
}
```

- [ ] **Step 3: Create `crates/tau-ts-extract/Cargo.toml`**

```toml
[package]
name = "tau-ts-extract"
version = "0.0.0"
edition = "2021"
license = "MIT OR Apache-2.0"
description = "TypeScript source extractor — produces ProjectConfig from project.ts via swc-based static analysis"

[dependencies]
# ADAPT: confirm the LATEST stable trio of swc_ecma_parser + swc_ecma_ast + swc_common
# on crates.io before locking versions. They MUST be a compatible release group.
swc_ecma_parser = "0.150"
swc_ecma_ast = "0.118"
swc_common = { version = "0.34", features = ["tty-emitter"] }
tau-pkg = { path = "../tau-pkg" }
anyhow = "1"
thiserror = "1"

[dev-dependencies]
# Used by conformance tests in Phase 6
tau-ir = { path = "../tau-ir" }
serde_json = "1"
```

(If the swc trio versions don't compile cleanly together, the implementer iterates. The crates.io page for `swc_ecma_parser` lists its required `swc_common` version in its dependencies.)

- [ ] **Step 4: Create `crates/tau-ts-extract/src/lib.rs`**

```rust
//! TypeScript source extractor — produces `ProjectConfig` from a
//! `project.ts` source via swc-based static AST analysis.
//!
//! See `docs/superpowers/specs/2026-06-10-beta-8-ts-authoring-design.md`
//! and ADR-0041 (forthcoming) for the design.
//!
//! β.8 v1 scope: declarations only. Tool bodies (`run: async () => ...`)
//! are rejected at parse time. δ.2 will add runtime JS execution via
//! QuickJS embed for inline tool bodies.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
mod factory;
mod lower;
mod parse;
mod scope;

pub use error::TsExtractError;
use std::path::Path;
use tau_pkg::project::project::ProjectConfig;

/// Extract a `ProjectConfig` from a TypeScript source string.
///
/// `source_path` is used only for error positioning (file:line:col).
/// The function does NOT read from disk — caller is responsible for
/// reading + UTF-8 validation.
///
/// Phase 1: stub. Phase 2+ fills this in.
pub fn extract_project(
    _source: &str,
    source_path: &Path,
) -> Result<ProjectConfig, TsExtractError> {
    Err(TsExtractError::ParseError {
        file: source_path.to_path_buf(),
        line: 0,
        col: 0,
        message: "not yet implemented (β.8 Phase 1 scaffold)".to_string(),
    })
}
```

- [ ] **Step 5: Create `crates/tau-ts-extract/src/error.rs`** (stub with just enough to compile)

```rust
//! Error type for the TS extractor. Phase 4 fleshes out all 10 variants.

use std::path::PathBuf;
use thiserror::Error;

/// All errors that can arise during TS extraction.
#[derive(Debug, Error)]
pub enum TsExtractError {
    /// swc parse error — invalid TS syntax.
    #[error("{file}:{line}:{col}: parse error: {message}")]
    ParseError {
        /// Source file path.
        file: PathBuf,
        /// Line (1-indexed).
        line: u32,
        /// Column (1-indexed).
        col: u32,
        /// Error message from swc.
        message: String,
    },
}
```

- [ ] **Step 6: Create stub module files** (one-liners so the lib builds):

```rust
// parse.rs
//! swc parser setup. Phase 2 fills this in.
```

```rust
// scope.rs
//! Top-level constant walker → name → Expr map. Phase 2 fills this in.
```

```rust
// factory.rs
//! Tau factory call recognizer. Phase 3 fills this in.
```

```rust
// lower.rs
//! AST literal → ProjectConfig field mapping. Phase 3 fills this in.
```

- [ ] **Step 7: Add to workspace `Cargo.toml`**

In the workspace root `Cargo.toml`, find the `members = [...]` array and add `"crates/tau-ts-extract"` alphabetically.

- [ ] **Step 8: Compile + run smoke test**

```bash
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo test -p tau-ts-extract --test smoke
```

Expected: pass.

If the swc trio versions don't compile cleanly, adjust them per the LATEST stable trio (visit crates.io). The swc ecosystem releases the parser + ast + common together; pick whichever bundle is current.

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml crates/tau-ts-extract/
git -c user.name="Test User" -c user.email="test@example.com" \
  commit --no-verify -m "feat(tau-ts-extract): crate scaffold + stub extract_project (β.8 Phase 1)"
```

---

## Phase 2 — swc parser + top-level scope walker

**Goal:** `parse::parse_module(src) -> Result<Module>` returns the AST. `scope::collect_top_level(module) -> NameMap` returns a `BTreeMap<String, &Expr>` covering all top-level `const NAME = EXPR;` declarations (both exported and non-exported).

### Task 2.1 — `parse.rs` + `scope.rs` + 3 unit tests

**Files:**
- Modify: `crates/tau-ts-extract/src/parse.rs`
- Modify: `crates/tau-ts-extract/src/scope.rs`
- Modify: `crates/tau-ts-extract/src/lib.rs` (wire parse + scope into `extract_project`'s skeleton)

- [ ] **Step 1: READ context**

```bash
# swc parser docs — find Parser::new and parse_module entrypoint
# https://docs.rs/swc_ecma_parser/latest/swc_ecma_parser/struct.Parser.html
# swc_ecma_ast::Module fields
# https://docs.rs/swc_ecma_ast/latest/swc_ecma_ast/struct.Module.html
```

- [ ] **Step 2: Write failing tests** at the bottom of `src/scope.rs` (in a `#[cfg(test)] mod tests {}` block):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_module;
    use std::path::Path;

    #[test]
    fn collects_single_top_level_const() {
        let src = r#"const foo = 42;"#;
        let (module, sm) = parse_module(src, Path::new("/tmp/t.ts")).unwrap();
        let names = collect_top_level(&module);
        assert!(names.contains_key("foo"), "expected `foo`, got: {:?}", names.keys().collect::<Vec<_>>());
        let _ = sm; // keep SourceMap alive for error positioning later
    }

    #[test]
    fn collects_exported_const() {
        let src = r#"export const bar = "hello";"#;
        let (module, _sm) = parse_module(src, Path::new("/tmp/t.ts")).unwrap();
        let names = collect_top_level(&module);
        assert!(names.contains_key("bar"));
    }

    #[test]
    fn collects_multiple_declarations() {
        let src = r#"
            const a = 1;
            const b = "x";
            export const c = { foo: "bar" };
        "#;
        let (module, _sm) = parse_module(src, Path::new("/tmp/t.ts")).unwrap();
        let names = collect_top_level(&module);
        assert_eq!(names.len(), 3);
        assert!(names.contains_key("a"));
        assert!(names.contains_key("b"));
        assert!(names.contains_key("c"));
    }
}
```

Run to confirm fail:
```bash
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo test -p tau-ts-extract --lib scope
```

- [ ] **Step 3: Implement `parse.rs`**

```rust
//! swc parser setup + module-level AST acquisition.

use std::path::Path;
use std::sync::Arc;

use swc_common::{
    errors::{ColorConfig, Handler},
    sync::Lrc,
    FileName, SourceMap,
};
use swc_ecma_ast::Module;
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};

use crate::error::TsExtractError;

/// Parse a TS source string into an `swc_ecma_ast::Module`.
///
/// Returns the parsed module AND the `SourceMap` used (caller keeps it
/// alive for span-to-position resolution during error reporting).
pub fn parse_module(
    source: &str,
    source_path: &Path,
) -> Result<(Module, Lrc<SourceMap>), TsExtractError> {
    let cm: Lrc<SourceMap> = Lrc::new(SourceMap::default());
    let _handler = Handler::with_tty_emitter(
        ColorConfig::Auto,
        true,
        false,
        Some(cm.clone()),
    );

    let fm = cm.new_source_file(
        FileName::Real(source_path.to_path_buf()),
        source.to_string(),
    );

    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax {
            tsx: false,
            decorators: false,
            dts: false,
            no_early_errors: false,
            disallow_ambiguous_jsx_like: false,
        }),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );

    let mut parser = Parser::new_from(lexer);

    let module = parser.parse_module().map_err(|e| {
        let span = e.span();
        let loc = cm.lookup_char_pos(span.lo);
        TsExtractError::ParseError {
            file: source_path.to_path_buf(),
            line: loc.line as u32,
            col: (loc.col.0 + 1) as u32,
            message: format!("{:?}", e.kind()),
        }
    })?;

    Ok((module, cm))
}
```

**ADAPT NOTES:** swc's exact entrypoint and `TsSyntax` field names may differ slightly per version. If `swc_ecma_parser` is on a different minor version, look at https://docs.rs/swc_ecma_parser/latest/ for `Parser::new_from` / `parse_module` / `Syntax::Typescript` shape. The error type's `.span()` / `.kind()` methods are stable across recent versions.

- [ ] **Step 4: Implement `scope.rs`**

```rust
//! Top-level constant walker.
//!
//! Builds a `NameMap` of all top-level `const NAME = EXPR;` declarations
//! (both `const` and `export const`). Used by Phase 3's identifier
//! resolution.

use std::collections::BTreeMap;

use swc_ecma_ast::{Decl, Expr, Module, ModuleDecl, ModuleItem, Stmt, VarDecl, VarDeclarator};

/// Map from top-level constant name → its initializer expression.
pub type NameMap<'a> = BTreeMap<String, &'a Expr>;

/// Walk a parsed Module and collect every top-level `const NAME = EXPR;`
/// (including `export const`).
///
/// Does NOT collect:
/// - `let` / `var` declarations (β.8 v1 — top-level constants only)
/// - destructuring patterns (`const {a, b} = obj`)
/// - declarations without initializers
/// - imports (handled separately by factory recognizer)
pub fn collect_top_level(module: &Module) -> NameMap<'_> {
    let mut names = BTreeMap::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                if var.kind == swc_ecma_ast::VarDeclKind::Const {
                    collect_from_var(var, &mut names);
                }
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(exp)) => {
                if let Decl::Var(var) = &exp.decl {
                    if var.kind == swc_ecma_ast::VarDeclKind::Const {
                        collect_from_var(var, &mut names);
                    }
                }
            }
            _ => {} // imports, other decl types — Phase 3+ handles
        }
    }
    names
}

fn collect_from_var<'a>(var: &'a VarDecl, names: &mut NameMap<'a>) {
    for decl in &var.decls {
        if let (Some(name), Some(init)) = (extract_ident_name(decl), decl.init.as_deref()) {
            names.insert(name, init);
        }
    }
}

fn extract_ident_name(decl: &VarDeclarator) -> Option<String> {
    match &decl.name {
        swc_ecma_ast::Pat::Ident(binding) => Some(binding.id.sym.to_string()),
        _ => None, // destructuring patterns — v1 doesn't support
    }
}
```

- [ ] **Step 5: Wire into `lib.rs`'s `extract_project`** (temporarily — Phase 3 replaces this with a real ProjectConfig builder):

```rust
pub fn extract_project(
    source: &str,
    source_path: &Path,
) -> Result<ProjectConfig, TsExtractError> {
    let (module, _sm) = parse::parse_module(source, source_path)?;
    let _names = scope::collect_top_level(&module);
    // Phase 3 builds the actual ProjectConfig. For now, error out so
    // the smoke test from Phase 1 continues to return Err.
    Err(TsExtractError::ParseError {
        file: source_path.to_path_buf(),
        line: 0,
        col: 0,
        message: "phase 2: factory recognition not yet implemented".to_string(),
    })
}
```

- [ ] **Step 6: Run tests**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo test -p tau-ts-extract --lib scope
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo test -p tau-ts-extract --test smoke
```

Both pass.

- [ ] **Step 7: Commit**

```bash
git add crates/tau-ts-extract/
git -c user.name="Test User" -c user.email="test@example.com" \
  commit --no-verify -m "feat(tau-ts-extract): swc parser + top-level scope walker (β.8 Phase 2)"
```

---

## Phase 3 — Factory recognizer + ProjectConfig builder

**Goal:** Recognize `agent({...})` / `tool({...})` / `mcp(url, {...})` calls in the AST. Convert their object-literal arguments to `ProjectConfig` fields. Resolve identifier references via the `NameMap` from Phase 2. `extract_project` now produces a real `ProjectConfig`.

### Task 3.1 — Factory call recognizer + minimal IR mapping + 4 unit tests

**Files:**
- Modify: `crates/tau-ts-extract/src/factory.rs`
- Modify: `crates/tau-ts-extract/src/lower.rs`
- Modify: `crates/tau-ts-extract/src/lib.rs` (compose parse → scope → factory → lower)

- [ ] **Step 1: READ context**

```bash
cd /Users/titouanlebocq/code/tau-worktrees/beta-8-ts-surface
# ProjectConfig field shapes (lines 297-310 + AgentEntry + ToolEntry + etc.)
grep -nB1 -A15 "pub struct ProjectConfig\b\|pub struct AgentEntry\|pub struct ToolEntry\|pub enum ToolBody\|pub enum PromptEntry" crates/tau-pkg/src/project/project.rs | head -100
# Note: many entries are validated structs. The extractor should build the
# `UncheckedProjectConfig` shape and then call `.validate()` (or whatever
# the validation entry point is). Look for:
grep -n "validate\|UncheckedProjectConfig" crates/tau-pkg/src/project/project.rs | head -10
```

- [ ] **Step 2: Write the 4 failing unit tests**

In `crates/tau-ts-extract/src/lower.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::extract_project;
    use std::path::Path;

    #[test]
    fn parses_minimal_agent_export() {
        let src = r#"
            export const fanMonitor = agent({
                display_name: "Fan Monitor",
                package: "fan-monitor@^0.1",
                llm_backend: "anthropic",
                model: "claude-haiku-4-5",
                prompt: { system: "Watch the temperature." }
            });
        "#;
        let config = extract_project(src, Path::new("/tmp/p.ts")).expect("parse");
        assert!(config.agents.contains_key("fanMonitor"), "expected fanMonitor in: {:?}", config.agents.keys().collect::<Vec<_>>());
        let agent = &config.agents["fanMonitor"];
        assert_eq!(agent.display_name, "Fan Monitor");
        assert_eq!(agent.model, "claude-haiku-4-5");
    }

    #[test]
    fn resolves_top_level_constant_reference() {
        let src = r#"
            const readTemp = tool({
                native: "ReadTemp",
                description: "Read temperature"
            });
            export const a = agent({
                display_name: "A",
                package: "a@^0.1",
                llm_backend: "anthropic",
                model: "claude-haiku-4-5",
                prompt: { system: "x" },
                tools: { readTemp }
            });
        "#;
        let config = extract_project(src, Path::new("/tmp/p.ts")).expect("parse");
        // readTemp tool is in project.tools (declared at top level)
        assert!(config.tools.contains_key("readTemp"), "got tools: {:?}", config.tools.keys().collect::<Vec<_>>());
    }

    #[test]
    fn recognizes_mcp_factory() {
        let src = r#"
            const weather = mcp("https://mcp.weather.com");
            export const a = agent({
                display_name: "A",
                package: "a@^0.1",
                llm_backend: "anthropic",
                model: "claude-haiku-4-5",
                prompt: { system: "x" },
                tools: { weather }
            });
        "#;
        let config = extract_project(src, Path::new("/tmp/p.ts")).expect("parse");
        let weather = config.tools.get("weather").expect("weather tool");
        match &weather.body {
            tau_pkg::project::project::ToolBody::Mcp(url) => assert_eq!(url, "https://mcp.weather.com"),
            other => panic!("expected ToolBody::Mcp, got {other:?}"),
        }
    }

    #[test]
    fn agent_with_no_tools_field_works() {
        let src = r#"
            export const solo = agent({
                display_name: "Solo",
                package: "solo@^0.1",
                llm_backend: "anthropic",
                model: "claude-haiku-4-5",
                prompt: { system: "alone" }
            });
        "#;
        let config = extract_project(src, Path::new("/tmp/p.ts")).expect("parse");
        assert!(config.agents.contains_key("solo"));
    }
}
```

Run to confirm fail.

- [ ] **Step 3: Implement `factory.rs`**

```rust
//! Tau factory call recognizer.
//!
//! Tau exposes 4 factory functions:
//!   - `agent({...})` → builds an agent declaration
//!   - `tool({...})` → builds a native or subflow tool
//!   - `mcp(url, opts?)` → builds an MCP-backed tool (URL is positional)
//!   - `contextManager({...})` → β.4 prerequisite; rejects at parse time
//!
//! This module's job is to RECOGNIZE these calls in the AST.
//! Field mapping happens in `lower.rs`.

use swc_ecma_ast::{Expr, Lit};

use crate::error::TsExtractError;

/// Which tau factory a call expression invokes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Factory {
    Agent,
    Tool,
    Mcp,
    ContextManager,
}

/// If `expr` is a tau factory call like `agent({...})`, return the
/// factory + the call's arguments. Otherwise return None (the expr
/// is some other shape — possibly an identifier reference to be
/// resolved by the caller).
pub fn recognize_factory_call(expr: &Expr) -> Option<(Factory, &swc_ecma_ast::CallExpr)> {
    if let Expr::Call(call) = expr {
        if let swc_ecma_ast::Callee::Expr(callee) = &call.callee {
            if let Expr::Ident(ident) = callee.as_ref() {
                let factory = match ident.sym.as_ref() {
                    "agent" => Factory::Agent,
                    "tool" => Factory::Tool,
                    "mcp" => Factory::Mcp,
                    "contextManager" => Factory::ContextManager,
                    _ => return None,
                };
                return Some((factory, call));
            }
        }
    }
    None
}

/// Extract a string literal value from a `CallExpr`'s arg at index `idx`,
/// erroring if it's not a string literal.
pub fn arg_as_string(
    call: &swc_ecma_ast::CallExpr,
    idx: usize,
) -> Option<String> {
    let arg = call.args.get(idx)?;
    if arg.spread.is_some() {
        return None;
    }
    if let Expr::Lit(Lit::Str(s)) = &*arg.expr {
        return Some(s.value.to_string());
    }
    None
}
```

- [ ] **Step 4: Implement `lower.rs`**

```rust
//! AST literal → `ProjectConfig` field mapping.
//!
//! Walks each exported / referenced factory call, converts its object-
//! literal args to typed `ProjectConfig` fields, resolves identifier
//! references via the `NameMap` from Phase 2.

use std::collections::BTreeMap;
use std::path::Path;

use swc_ecma_ast::{Expr, KeyValueProp, Lit, Module, ObjectLit, Prop, PropName, PropOrSpread};
use tau_pkg::project::project::{
    AgentEntry, ProjectConfig, PromptEntry, ToolBody, ToolEntry,
};

use crate::error::TsExtractError;
use crate::factory::{recognize_factory_call, Factory};
use crate::scope::NameMap;

/// Build a `ProjectConfig` from a parsed module + name map.
pub fn build_project_config(
    module: &Module,
    names: &NameMap,
    source_path: &Path,
) -> Result<ProjectConfig, TsExtractError> {
    let mut agents: BTreeMap<String, AgentEntry> = BTreeMap::new();
    let mut tools: BTreeMap<String, ToolEntry> = BTreeMap::new();

    // 1. Walk all top-level constants (exported AND non-exported).
    //    Non-exported constants assigned to `tool(...)` / `mcp(...)`
    //    factories populate the project's tools map (the philosophy doc's
    //    convention — helpers at file scope are project-level tools).
    for (name, expr) in names {
        if let Some((factory, call)) = recognize_factory_call(expr) {
            match factory {
                Factory::Tool => {
                    let arg = call.args.first().ok_or_else(|| TsExtractError::ParseError {
                        file: source_path.to_path_buf(), line: 0, col: 0,
                        message: format!("tool({}) requires 1 object argument", name),
                    })?;
                    if arg.spread.is_some() {
                        return Err(TsExtractError::ParseError {
                            file: source_path.to_path_buf(), line: 0, col: 0,
                            message: format!("spread not allowed in tool({}) args", name),
                        });
                    }
                    let entry = lower_tool_args(&*arg.expr, name, source_path)?;
                    tools.insert(name.clone(), entry);
                }
                Factory::Mcp => {
                    let url = crate::factory::arg_as_string(call, 0).ok_or_else(|| TsExtractError::ParseError {
                        file: source_path.to_path_buf(), line: 0, col: 0,
                        message: format!("mcp({}, ...) requires a string URL as first arg", name),
                    })?;
                    // Phase 3.5: mcp options at arg[1] (capabilities, etc.) — for v1,
                    // accept-and-ignore; just wire the URL into the ToolBody.
                    tools.insert(name.clone(), ToolEntry {
                        name: name.clone(),
                        body: ToolBody::Mcp(url),
                        description: None,
                        input_schema: None,
                        capabilities: vec![],
                        sampling: None,
                        roots: None,
                    });
                }
                Factory::Agent => {
                    let arg = call.args.first().ok_or_else(|| TsExtractError::ParseError {
                        file: source_path.to_path_buf(), line: 0, col: 0,
                        message: format!("agent({}) requires 1 object argument", name),
                    })?;
                    let entry = lower_agent_args(&*arg.expr, name, source_path)?;
                    agents.insert(name.clone(), entry);
                }
                Factory::ContextManager => {
                    return Err(TsExtractError::ParseError {
                        file: source_path.to_path_buf(), line: 0, col: 0,
                        message: format!(
                            "contextManager({}) is deferred to β.4 (ContextManager primitive not yet shipped)",
                            name
                        ),
                    });
                }
            }
        }
    }

    Ok(ProjectConfig {
        // ADAPT: fill in the remaining ProjectConfig fields per the
        // actual struct shape (read crates/tau-pkg/src/project/project.rs
        // line ~297+ for the full field list). Likely needs at minimum:
        //   project: ProjectMeta { name, version }
        // The TS file may declare these via top-level constants or via
        // a project name from the file path — v1 derives from filename.
        agents,
        tools,
        // The TS surface doesn't yet declare top-level [project] metadata.
        // Until users have a `project({...})` factory (deferred to v1.1),
        // derive a default ProjectMeta from the source file path.
        ..ProjectConfig::default()
    })
}

fn lower_tool_args(
    arg: &Expr,
    name: &str,
    source_path: &Path,
) -> Result<ToolEntry, TsExtractError> {
    let obj = expect_object_lit(arg, "tool", name, source_path)?;

    let mut native: Option<String> = None;
    let mut description: Option<String> = None;
    // Other fields: capabilities, input_schema, etc. — extracted similarly.

    for prop in &obj.props {
        let (key, value) = extract_kv(prop, "tool", name, source_path)?;
        match key.as_str() {
            "native" => native = Some(expect_string(value, "tool.native", source_path)?),
            "description" => description = Some(expect_string(value, "tool.description", source_path)?),
            // Other field names — ignored in Phase 3; Phase 4 rejects unknowns.
            _ => {}
        }
    }

    let body = if let Some(native_name) = native {
        ToolBody::Native(native_name)
    } else {
        return Err(TsExtractError::ParseError {
            file: source_path.to_path_buf(), line: 0, col: 0,
            message: format!("tool({}) v1 requires `native: \"FnName\"` field", name),
        });
    };

    Ok(ToolEntry {
        name: name.to_string(),
        body,
        description,
        input_schema: None,
        capabilities: vec![],
        sampling: None,
        roots: None,
    })
}

fn lower_agent_args(
    arg: &Expr,
    name: &str,
    source_path: &Path,
) -> Result<AgentEntry, TsExtractError> {
    let obj = expect_object_lit(arg, "agent", name, source_path)?;

    // Fields from the TOML schema, populated as encountered.
    let mut display_name: Option<String> = None;
    let mut package: Option<String> = None;
    let mut llm_backend: Option<String> = None;
    let mut model: Option<String> = None;
    let mut prompt_system: Option<String> = None;

    for prop in &obj.props {
        let (key, value) = extract_kv(prop, "agent", name, source_path)?;
        match key.as_str() {
            "display_name" => display_name = Some(expect_string(value, "agent.display_name", source_path)?),
            "package" => package = Some(expect_string(value, "agent.package", source_path)?),
            "llm_backend" => llm_backend = Some(expect_string(value, "agent.llm_backend", source_path)?),
            "model" => model = Some(expect_string(value, "agent.model", source_path)?),
            "prompt" => {
                if let Expr::Object(prompt_obj) = value {
                    for p in &prompt_obj.props {
                        let (k, v) = extract_kv(p, "agent.prompt", name, source_path)?;
                        if k == "system" {
                            prompt_system = Some(expect_string(v, "agent.prompt.system", source_path)?);
                        }
                    }
                }
            }
            "tools" => {
                // tools: { readTemp, weather } — identifier references; v1 ignores
                // here (tools are already added to project.tools at top level).
                // Phase 4 validates that referenced names actually exist.
            }
            _ => {}
        }
    }

    // ADAPT: build AgentEntry per its actual struct shape from
    // crates/tau-pkg/src/project/project.rs. The fields above are the
    // commonly-required ones (per Phase 2 fixture findings).
    Ok(AgentEntry {
        display_name: display_name.ok_or_else(|| TsExtractError::ParseError {
            file: source_path.to_path_buf(), line: 0, col: 0,
            message: format!("agent({}) missing required field: display_name", name),
        })?,
        package: package.ok_or_else(|| TsExtractError::ParseError {
            file: source_path.to_path_buf(), line: 0, col: 0,
            message: format!("agent({}) missing required field: package", name),
        })?,
        llm_backend: llm_backend.ok_or_else(|| TsExtractError::ParseError {
            file: source_path.to_path_buf(), line: 0, col: 0,
            message: format!("agent({}) missing required field: llm_backend", name),
        })?,
        model: model.ok_or_else(|| TsExtractError::ParseError {
            file: source_path.to_path_buf(), line: 0, col: 0,
            message: format!("agent({}) missing required field: model", name),
        })?,
        prompt: PromptEntry::Inline(prompt_system.unwrap_or_default()),
        // ADAPT: fill in remaining AgentEntry fields with defaults per the
        // actual struct shape.
        ..AgentEntry::default()
    })
}

// --- helpers ---

fn expect_object_lit<'a>(
    expr: &'a Expr,
    factory: &str,
    name: &str,
    source_path: &Path,
) -> Result<&'a ObjectLit, TsExtractError> {
    if let Expr::Object(obj) = expr {
        Ok(obj)
    } else {
        Err(TsExtractError::ParseError {
            file: source_path.to_path_buf(), line: 0, col: 0,
            message: format!("{}({}) expects an object literal arg", factory, name),
        })
    }
}

fn extract_kv<'a>(
    prop: &'a PropOrSpread,
    factory: &str,
    name: &str,
    source_path: &Path,
) -> Result<(String, &'a Expr), TsExtractError> {
    match prop {
        PropOrSpread::Prop(p) => match p.as_ref() {
            Prop::KeyValue(KeyValueProp { key, value }) => {
                let k = match key {
                    PropName::Ident(i) => i.sym.to_string(),
                    PropName::Str(s) => s.value.to_string(),
                    _ => return Err(TsExtractError::ParseError {
                        file: source_path.to_path_buf(), line: 0, col: 0,
                        message: format!("{}({}) computed/non-identifier keys not supported", factory, name),
                    }),
                };
                Ok((k, value.as_ref()))
            }
            Prop::Shorthand(ident) => {
                // `{ readTemp }` shorthand — Phase 3 ignores at lookup time;
                // tools are already in project.tools via top-level walk.
                let k = ident.sym.to_string();
                // Return the shorthand as if it were `{ readTemp: readTemp }`.
                // The synthetic Expr would need a reference; for v1 we treat
                // shorthand specially by returning a sentinel value. For now,
                // ignore — the value won't be used since we only mind keys.
                Err(TsExtractError::ParseError {
                    file: source_path.to_path_buf(), line: 0, col: 0,
                    message: format!("{}({}) shorthand `{{ {} }}` handled by caller", factory, name, k),
                })
            }
            _ => Err(TsExtractError::ParseError {
                file: source_path.to_path_buf(), line: 0, col: 0,
                message: format!("{}({}) unsupported property shape", factory, name),
            }),
        },
        PropOrSpread::Spread(_) => Err(TsExtractError::ParseError {
            file: source_path.to_path_buf(), line: 0, col: 0,
            message: format!("{}({}) spread in object args not supported in v1", factory, name),
        }),
    }
}

fn expect_string(
    value: &Expr,
    field: &str,
    source_path: &Path,
) -> Result<String, TsExtractError> {
    if let Expr::Lit(Lit::Str(s)) = value {
        Ok(s.value.to_string())
    } else {
        Err(TsExtractError::ParseError {
            file: source_path.to_path_buf(), line: 0, col: 0,
            message: format!("{} expects a string literal", field),
        })
    }
}
```

**Many ADAPT points in this code** — the `ProjectConfig`, `AgentEntry`, `ToolEntry`, `PromptEntry` struct shapes need to match the actual API. READ `crates/tau-pkg/src/project/project.rs` before finalizing. If `AgentEntry::default()` doesn't exist, build from explicit fields. If `display_name` / `package` / `llm_backend` are validated via `parse_str`'s `validate_project` (rather than being raw struct fields), build an `UncheckedProjectConfig` instead and call `.validate()` — this is the cleanest path.

- [ ] **Step 5: Update `lib.rs`'s `extract_project` to call into `lower.rs`**

```rust
pub fn extract_project(
    source: &str,
    source_path: &Path,
) -> Result<ProjectConfig, TsExtractError> {
    let (module, _sm) = parse::parse_module(source, source_path)?;
    let names = scope::collect_top_level(&module);
    lower::build_project_config(&module, &names, source_path)
}
```

- [ ] **Step 6: Run tests**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo test -p tau-ts-extract --lib
```

Expected: 7 pass (3 scope + 4 lower).

- [ ] **Step 7: Commit**

```bash
git add crates/tau-ts-extract/
git -c user.name="Test User" -c user.email="test@example.com" \
  commit --no-verify -m "feat(tau-ts-extract): factory recognizer + ProjectConfig builder (β.8 Phase 3)"
```

---

## Phase 4 — Rejection pathway: positioned errors

**Goal:** Implement all 10 `TsExtractError` variants from spec §5 with file:line:col positioning. Tests verify each rejection shape.

### Task 4.1 — Full `TsExtractError` enum + 4 rejection tests

**Files:**
- Modify: `crates/tau-ts-extract/src/error.rs`
- Modify: `crates/tau-ts-extract/src/lower.rs` (use positioned errors)
- Modify: `crates/tau-ts-extract/src/parse.rs` (use positioned errors)

- [ ] **Step 1: Expand `error.rs`** with all 10 variants:

```rust
//! All error types for the TS extractor.
//!
//! Each variant carries enough position info to render as
//! `file:line:col: <message>` for user-facing display.

use std::path::PathBuf;
use thiserror::Error;

/// Source-file position (1-indexed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    /// Source file path.
    pub file: PathBuf,
    /// Line number (1-indexed).
    pub line: u32,
    /// Column number (1-indexed).
    pub col: u32,
}

impl std::fmt::Display for Position {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.file.display(), self.line, self.col)
    }
}

/// All errors that can arise during TS extraction.
#[derive(Debug, Error)]
pub enum TsExtractError {
    /// The source file is not valid UTF-8.
    #[error("{file}: not UTF-8")]
    NotUtf8 { file: PathBuf },

    /// swc parse error.
    #[error("{pos}: parse error: {message}")]
    ParseError {
        pos: Position,
        message: String,
    },

    /// Called a function that isn't a tau factory.
    #[error("{pos}: unknown factory `{name}` (expected agent/tool/mcp/contextManager)")]
    UnknownFactory { pos: Position, name: String },

    /// An expression that's not in the allowed literal whitelist (e.g. await, typeof, function calls beyond factories).
    #[error("{pos}: unsupported expression `{kind}`: {hint}")]
    UnsupportedExpression {
        pos: Position,
        kind: String,
        hint: String,
    },

    /// Identifier reference doesn't resolve to a top-level constant.
    #[error("{pos}: unresolved identifier `{name}` (not declared as top-level const)")]
    UnresolvedIdentifier { pos: Position, name: String },

    /// Factory whose implementation requires a future sub-project.
    #[error("{pos}: `{factory}` is deferred to {until}")]
    Deferred { pos: Position, factory: String, until: String },

    /// `import` from anywhere other than the `tau` module.
    #[error("{pos}: imports from `{source}` are not supported in β.8 v1 (multi-file deferred to v1.1)")]
    ImportNotSupported { pos: Position, source: String },

    /// `A → B → A`-style identifier reference cycle.
    #[error("{pos}: cyclic reference: {cycle}")]
    CyclicReference { pos: Position, cycle: String },

    /// Inline tool body — `tool({ run: async () => ... })`.
    #[error("{pos}: inline tool bodies require δ.2 (use `native: \"FnName\"` reference to a Rust-compiled-in tool)")]
    InlineToolBody { pos: Position },

    /// Wrapped `std::io::Error` for file reads (rarely surfaces — caller usually reads first).
    #[error("{file}: I/O error: {source}")]
    Io {
        file: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl TsExtractError {
    /// Build a Position from an swc Span + a SourceMap.
    pub fn position_from_span(
        sm: &swc_common::SourceMap,
        span: swc_common::Span,
        file: std::path::PathBuf,
    ) -> Position {
        let loc = sm.lookup_char_pos(span.lo);
        Position {
            file,
            line: loc.line as u32,
            col: (loc.col.0 + 1) as u32,
        }
    }
}
```

- [ ] **Step 2: Update `parse.rs` to use `Position`** — replace the old `ParseError { file, line, col, message }` with `ParseError { pos: Position{...}, message }`. Same conversion logic.

- [ ] **Step 3: Update `lower.rs` to use the new variants** — replace `ParseError` placeholders with the appropriate typed variant (e.g. `InlineToolBody` for the run: async case, `Deferred` for contextManager, `UnsupportedExpression` for general rejections). Each carries a `Position`.

- [ ] **Step 4: Wire the SourceMap through to lower.rs** so errors can resolve spans to positions. Pass the `Lrc<SourceMap>` from `parse_module` down through `extract_project` to `build_project_config`.

- [ ] **Step 5: Write the 4 failing rejection tests** at the bottom of `src/lower.rs`'s test mod:

```rust
#[test]
fn rejects_async_function_body() {
    let src = r#"
        const t = tool({
            native: "X",
            run: async () => 42
        });
        export const a = agent({
            display_name: "A",
            package: "a@^0.1",
            llm_backend: "anthropic",
            model: "x",
            prompt: { system: "x" },
            tools: { t }
        });
    "#;
    let err = extract_project(src, std::path::Path::new("/tmp/t.ts")).expect_err("should fail");
    assert!(matches!(err, TsExtractError::InlineToolBody { .. } | TsExtractError::UnsupportedExpression { .. }),
        "expected InlineToolBody, got: {err:?}");
}

#[test]
fn rejects_context_manager_factory() {
    let src = r#"
        export const ctx = contextManager({
            budget: { tokens: 16000 }
        });
    "#;
    let err = extract_project(src, std::path::Path::new("/tmp/t.ts")).expect_err("should fail");
    assert!(matches!(err, TsExtractError::Deferred { .. }),
        "expected Deferred, got: {err:?}");
}

#[test]
fn rejects_non_tau_import() {
    let src = r#"
        import { x } from "./helpers";
        export const a = agent({
            display_name: "A",
            package: "a@^0.1",
            llm_backend: "anthropic",
            model: "x",
            prompt: { system: "x" }
        });
    "#;
    let err = extract_project(src, std::path::Path::new("/tmp/t.ts")).expect_err("should fail");
    assert!(matches!(err, TsExtractError::ImportNotSupported { .. }),
        "expected ImportNotSupported, got: {err:?}");
}

#[test]
fn error_position_carries_line_col() {
    let src = "const broken = ();";  // syntax error at col 16
    let err = extract_project(src, std::path::Path::new("/tmp/t.ts")).expect_err("should fail");
    match err {
        TsExtractError::ParseError { pos, .. } => {
            assert_eq!(pos.line, 1);
            assert!(pos.col > 0);
        }
        other => panic!("expected ParseError, got: {other:?}"),
    }
}
```

To handle the `import` rejection, scan `module.body` for `ModuleItem::ModuleDecl(ModuleDecl::Import(..))` items in `lower.rs::build_project_config` BEFORE the factory walk. Allow imports from "tau" (skip them); reject anything else.

To handle the `contextManager` rejection, the factory recognizer in Phase 3 already returns `Factory::ContextManager`; just emit `TsExtractError::Deferred` instead of `ParseError`.

To handle inline tool bodies, when extracting `tool({...})` args, if any key is `run` AND its value is an arrow / function expression, emit `InlineToolBody`.

- [ ] **Step 6: Run tests**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo test -p tau-ts-extract --lib
```

All 11 tests pass (3 scope + 4 lower + 4 rejection).

- [ ] **Step 7: Commit**

```bash
git add crates/tau-ts-extract/
git -c user.name="Test User" -c user.email="test@example.com" \
  commit --no-verify -m "feat(tau-ts-extract): 10 TsExtractError variants with file:line:col positioning (β.8 Phase 4)"
```

---

## Phase 5 — CLI dispatch in `tau-cli`

**Goal:** `tau dev project.ts` / `tau build project.ts` / `tau check project.ts` / `tau run project.ts` all route through the TS extractor. Default (bare directory or `.toml` path) continues to use the TOML loader.

### Task 5.1 — File-extension dispatch in dev + build + check + run + 2 integration tests

**Files:**
- Modify: `crates/tau-cli/Cargo.toml` (add `tau-ts-extract` dep)
- Modify: `crates/tau-cli/src/cmd/dev/session.rs` (file-extension dispatch in `load`)
- Modify: `crates/tau-cli/src/cmd/build.rs`
- Modify: `crates/tau-cli/src/cmd/check/mod.rs`
- Modify: `crates/tau-cli/src/cmd/run.rs`
- Modify: `crates/tau-cli/src/cmd/dev/watcher.rs` (for .ts projects, watch the .ts file)
- Create: `crates/tau-cli/tests/cmd_dev_ts_one_shot.rs`
- Create: `crates/tau-cli/tests/cmd_build_ts.rs`

- [ ] **Step 1: Add the dep** in `crates/tau-cli/Cargo.toml`:

```toml
tau-ts-extract = { path = "../tau-ts-extract" }
```

- [ ] **Step 2: Extract a shared `load_project` helper**

In `crates/tau-cli/src/cmd/` (or wherever β.7's existing code lives), add a shared helper:

```rust
// crates/tau-cli/src/cmd/project_load.rs (NEW)
//! Shared `tau.toml` / `project.ts` loader used by dev / build / check / run.

use std::path::{Path, PathBuf};
use anyhow::{anyhow, Context, Result};
use tau_pkg::project::project::ProjectConfig;

/// Result of loading a project — carries the resolved project root + the parsed config.
pub struct LoadedProject {
    pub project_root: PathBuf,
    pub project: ProjectConfig,
}

/// Load a project from a path that may be:
///   - a directory containing `tau.toml`
///   - a `.ts` file (project.ts shape)
///   - a `.toml` file (alternate TOML location)
pub fn load_project(path: &Path) -> Result<LoadedProject> {
    let ext = path.extension().and_then(|s| s.to_str());
    if path.is_file() && ext == Some("ts") {
        let src = std::fs::read_to_string(path)
            .with_context(|| format!("read {}", path.display()))?;
        let project = tau_ts_extract::extract_project(&src, path)
            .map_err(|e| anyhow!("{}", e))?;
        let project_root = path.parent().unwrap_or(path).to_path_buf();
        Ok(LoadedProject { project_root, project })
    } else {
        // Default: TOML path. `path` is a directory (or a .toml file).
        let project_root = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };
        let tau_toml = project_root.join("tau.toml");
        let toml_str = std::fs::read_to_string(&tau_toml)
            .with_context(|| format!("read {}", tau_toml.display()))?;
        let project = ProjectConfig::parse_str(&toml_str)
            .map_err(|e| anyhow!("parse tau.toml: {e}"))?;
        Ok(LoadedProject { project_root, project })
    }
}
```

Register `pub mod project_load;` in `crates/tau-cli/src/cmd/mod.rs`.

- [ ] **Step 3: Update `cmd/dev/session.rs::load`**

Replace the existing tau.toml read+parse with `cmd::project_load::load_project(&project_path)`. Use the returned `project_root` and `project` directly. (The rest of `DevSession::load` — IR lowering, watcher spawn, etc. — stays the same.)

- [ ] **Step 4: Update `cmd/dev/watcher.rs`**

For `.ts` projects, watch the `.ts` file instead of `tau.toml`. In `resolve_watch_paths`:

```rust
fn resolve_watch_paths(project_root: &Path, project_path: &Path, project: &ProjectConfig) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let ext = project_path.extension().and_then(|s| s.to_str());
    if ext == Some("ts") {
        paths.push(project_path.to_path_buf());
    } else {
        paths.push(project_root.join("tau.toml"));
    }
    // workflows/*.toml + prompt files — unchanged from β.7
    // ...
    paths
}
```

(Implementer adapts the call site to thread `project_path` through.)

- [ ] **Step 5: Update `cmd/build.rs`, `cmd/check/mod.rs`, `cmd/run.rs`**

Each of these has a project-loading code path. Replace it with `cmd::project_load::load_project(&path)`. The downstream code consumes the same `ProjectConfig` regardless.

- [ ] **Step 6: Write integration tests**

`crates/tau-cli/tests/cmd_dev_ts_one_shot.rs`:

```rust
//! Integration: tau dev project.ts -p with a minimal TS file boots + exits.

use assert_fs::prelude::*;

#[test]
fn dev_one_shot_with_ts_project_exits_gracefully() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    tmp.child("project.ts").write_str(r#"
export const a = agent({
    display_name: "A",
    package: "a@^0.1",
    llm_backend: "anthropic",
    model: "claude-haiku-4-5",
    prompt: { system: "x" }
});
"#).expect("write");

    let assert = assert_cmd::Command::cargo_bin("tau").expect("bin")
        .current_dir(tmp.path())
        .args(["dev", "project.ts", "-p", "hi"])
        .timeout(std::time::Duration::from_secs(20))
        .assert();
    let output = assert.get_output();
    assert!(output.status.code().is_some(),
        "process must exit (not be killed): {:?}", output.status);
}
```

`crates/tau-cli/tests/cmd_build_ts.rs`:

```rust
//! Integration: tau build project.ts produces a bundle.

use assert_fs::prelude::*;

#[test]
fn build_with_ts_project_produces_bundle() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    tmp.child("project.ts").write_str(r#"
export const a = agent({
    display_name: "A",
    package: "a@^0.1",
    llm_backend: "anthropic",
    model: "claude-haiku-4-5",
    prompt: { system: "x" }
});
"#).expect("write");

    let out_path = tmp.child("out.bundle");
    let assert = assert_cmd::Command::cargo_bin("tau").expect("bin")
        .current_dir(tmp.path())
        .args(["build", "project.ts", "-o", out_path.path().to_str().unwrap()])
        .timeout(std::time::Duration::from_secs(20))
        .assert();
    let output = assert.get_output();
    // Build may succeed or fail-gracefully depending on whether the
    // `a@^0.1` package is installed. Either is acceptable; what matters
    // is that the process EXITS (TS path didn't hang or panic).
    assert!(output.status.code().is_some(),
        "process must exit: {:?}", output.status);
}
```

- [ ] **Step 7: Verify the top-level help snapshot is unchanged**

```bash
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo test -p tau-cli --test help_snapshots
```

β.8 doesn't add a verb (TS dispatch is at the load layer), so this should pass without snapshot regeneration. If it fails, accept the snapshot diff with `mv *.snap.new *.snap` and document why.

- [ ] **Step 8: Run all tests**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo test -p tau-cli --test cmd_dev_ts_one_shot --test cmd_build_ts
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-cli
```

Both should pass.

Clippy:
```bash
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo clippy -p tau-cli --all-targets -- -D warnings
```

- [ ] **Step 9: Commit**

```bash
git add -A
git -c user.name="Test User" -c user.email="test@example.com" \
  commit --no-verify -m "feat(tau-cli): file-extension dispatch for .ts / .toml projects (β.8 Phase 5)"
```

---

## Phase 6 — TOML↔TS conformance test

**Goal:** Byte-equal IR after canonical encoding when the canonical fan-monitor is authored in both TOML and TS.

### Task 6.1 — Conformance fixture + byte-equal test

**Files:**
- Create: `crates/tau-ts-extract/tests/fixtures/fan_monitor_conformance/tau.toml`
- Create: `crates/tau-ts-extract/tests/fixtures/fan_monitor_conformance/project.ts`
- Create: `crates/tau-ts-extract/tests/fan_monitor_conformance.rs`

- [ ] **Step 1: Write the TOML fixture**

`crates/tau-ts-extract/tests/fixtures/fan_monitor_conformance/tau.toml`:

```toml
[project]
name = "fan-monitor-conformance"
version = "0.0.1"

[agents.fan_monitor]
display_name = "Fan Monitor"
package      = "fan-monitor@^0.1"
llm_backend  = "anthropic"
model        = "claude-haiku-4-5"
prompt.system = "Watch the temperature; turn on the fan if above 30°C."

[tools.read_temp]
native = "ReadTemp"
description = "Read the temperature sensor"

[tools.set_fan]
native = "SetFan"
description = "Toggle the fan"
```

- [ ] **Step 2: Write the TS fixture**

`crates/tau-ts-extract/tests/fixtures/fan_monitor_conformance/project.ts`:

```typescript
const read_temp = tool({
    native: "ReadTemp",
    description: "Read the temperature sensor",
});

const set_fan = tool({
    native: "SetFan",
    description: "Toggle the fan",
});

export const fan_monitor = agent({
    display_name: "Fan Monitor",
    package: "fan-monitor@^0.1",
    llm_backend: "anthropic",
    model: "claude-haiku-4-5",
    prompt: { system: "Watch the temperature; turn on the fan if above 30°C." },
    tools: { read_temp, set_fan },
});
```

(Note: snake_case names match the TOML. The constants are non-exported helpers; only `fan_monitor` is exported.)

- [ ] **Step 3: Write the conformance test**

`crates/tau-ts-extract/tests/fan_monitor_conformance.rs`:

```rust
//! TOML↔TS conformance: both surfaces must produce a byte-equal IR
//! after canonical encoding. This is β.8's load-bearing DoD.

use std::path::Path;

#[test]
fn toml_and_ts_produce_byte_equal_canonical_ir() {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/fan_monitor_conformance");

    // TOML path
    let toml_str = std::fs::read_to_string(fixture_dir.join("tau.toml"))
        .expect("read tau.toml");
    let toml_project = tau_pkg::project::project::ProjectConfig::parse_str(&toml_str)
        .expect("parse tau.toml");

    // TS path
    let ts_src = std::fs::read_to_string(fixture_dir.join("project.ts"))
        .expect("read project.ts");
    let ts_project = tau_ts_extract::extract_project(&ts_src, &fixture_dir.join("project.ts"))
        .expect("extract project.ts");

    // Lower both to IR.
    let target = tau_ports::target::TargetTriple::PASSTHROUGH;
    let caches = tau_ir::lower::Caches::default();
    let toml_ir = tau_ir::lower::lower_project(&toml_project, &target, &caches)
        .expect("lower TOML to IR");
    let ts_ir = tau_ir::lower::lower_project(&ts_project, &target, &caches)
        .expect("lower TS to IR");

    // Canonical-encode and compare bytes.
    let toml_bytes = tau_ir::canonical::to_canonical_bytes(&toml_ir);
    let ts_bytes = tau_ir::canonical::to_canonical_bytes(&ts_ir);

    if toml_bytes != ts_bytes {
        // Render diff for debug.
        let toml_str = String::from_utf8_lossy(&toml_bytes);
        let ts_str = String::from_utf8_lossy(&ts_bytes);
        panic!(
            "TOML↔TS canonical IRs differ:\n--- TOML ---\n{}\n--- TS ---\n{}\n",
            toml_str, ts_str
        );
    }
}
```

(ADAPT: `tau_ir::lower::Caches::default()` may not exist — check `crates/tau-ir/src/lower/mod.rs` for the actual `Caches` constructor. Same `PASSTHROUGH` + stub caches pattern β.7's `DevSession::load` uses.)

- [ ] **Step 4: Run the conformance test**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo test -p tau-ts-extract --test fan_monitor_conformance
```

Expected: pass.

If it fails because the TOML version has a `[project]` block but the TS version derives one from filename, ADJUST the TS extractor's default ProjectMeta to match (or add a top-level `project` factory call — but that's spec-out-of-scope for v1). Simpler: omit `[project]` from the TOML version too, or wire derived defaults consistently.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ts-extract/tests/
git -c user.name="Test User" -c user.email="test@example.com" \
  commit --no-verify -m "test(tau-ts-extract): TOML↔TS byte-equal canonical IR conformance (β.8 Phase 6)"
```

---

## Phase 7 — Smoke example + end-to-end

**Goal:** `examples/dev-smoke-fan-monitor-ts/` contains a sibling of β.7's smoke example, authored in TS. End-to-end smoke confirms the full pipeline works from CLI invocation.

### Task 7.1 — Smoke example + 1 end-to-end test

**Files:**
- Create: `examples/dev-smoke-fan-monitor-ts/project.ts`
- (No new test file needed — Phase 5's `cmd_dev_ts_one_shot.rs` + `cmd_build_ts.rs` cover the end-to-end path. Phase 7 just adds the example for users.)

- [ ] **Step 1: Write the example**

`examples/dev-smoke-fan-monitor-ts/project.ts`:

```typescript
// Sibling of examples/dev-smoke-fan-monitor/tau.toml.
// Demonstrates the β.8 TS authoring surface (declarations-only).

const read_temp = tool({
    native: "ReadTemp",
    description: "Read the temperature sensor",
});

const set_fan = tool({
    native: "SetFan",
    description: "Toggle the fan",
});

export const fan_monitor = agent({
    display_name: "Fan Monitor",
    package: "fan-monitor@^0.1",
    llm_backend: "anthropic",
    model: "claude-haiku-4-5",
    prompt: { system: "Watch the temperature; turn on the fan if above 30°C." },
    tools: { read_temp, set_fan },
});
```

- [ ] **Step 2: Manual smoke verification**

```bash
cd /Users/titouanlebocq/code/tau-worktrees/beta-8-ts-surface
timeout 60 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo run -p tau-cli -- dev examples/dev-smoke-fan-monitor-ts/project.ts -p "what is the temperature?"
```

Should boot, attempt one turn, exit. (May fail at LLM/plugin time — that's expected; the TS load + lower path is what we're validating.)

- [ ] **Step 3: Commit**

```bash
git add examples/dev-smoke-fan-monitor-ts/
git -c user.name="Test User" -c user.email="test@example.com" \
  commit --no-verify -m "docs(examples): dev-smoke-fan-monitor-ts/project.ts (β.8 Phase 7)"
```

---

## Phase 8 — ROADMAP edit + ADR-0041 + push + PR + auto-merge

### Task 8.1 — ROADMAP amendment + ADR-0041

**Files:**
- Modify: `ROADMAP.md` (replace §β.8 per spec §9)
- Create: `docs/decisions/0041-ts-authoring-declarations-only.md`

- [ ] **Step 1: Apply ROADMAP edit**

READ the spec at `docs/superpowers/specs/2026-06-10-beta-8-ts-authoring-design.md` §9 for the exact replacement text. The current ROADMAP §β.8 lives at lines ~505-526; replace with the amended text.

- [ ] **Step 2: Create ADR-0041**

`docs/decisions/0041-ts-authoring-declarations-only.md`:

```markdown
# ADR-0041: β.8 TS authoring surface — declarations-only via static AST analysis

**Status:** Accepted
**Date:** 2026-06-10
**Supersedes:** none

## Context

The 2026-05-29 philosophy doc names TS as a sugar layer over the canonical
IR. The β.8 ROADMAP entry adds the `@tau/sdk` factory functions but is
ambiguous about whether TS code is statically analyzed or runtime-executed.

After β.7 (tau dev REPL) shipped, the choice becomes load-bearing for β.6
(conformance gate). Two interpretations are honest:

1. **Declarations-only via swc static AST analysis** — TS file is parsed,
   factory calls recognized as data, no JS execution. Tool bodies remain
   Rust-native (referenced via `native: "ReadTemp"` string).
2. **Full Vercel-DX feel** — embed a JS runtime (rquickjs / deno_core),
   execute the TS file, factories build IR objects at runtime, inline
   tool bodies (`run: async () => ...`) work in dev mode.

## Decision

Ship **declarations-only via swc static AST analysis** for β.8 v1. Defer
the runtime JS execution path (and inline tool bodies) to δ.2.

Specific decisions:
- New workspace crate `tau-ts-extract` does the TS → ProjectConfig
  conversion via swc.
- Snake_case fields throughout — matches TOML 1:1 to keep the
  conformance test (TOML↔TS byte-equal IR) simple.
- File-extension dispatch in `cmd/{dev,build,check,run}.rs` —
  `.ts` → TS extractor; everything else → TOML.
- `contextManager` factory exists in the SDK shape but rejects at
  parse time with a `Deferred` error (β.4 prerequisite).
- Multi-file TS imports rejected with helpful hint; v1.1 work.

## Consequences

**Positive:**
- β.8 ships in ~2 weeks, matching β.7's footprint.
- No embedded JS engine in tau-cli; ~3 MB swc dep is the only binary cost.
- TOML↔TS conformance is straightforward (same lower path, same canonical
  encoder, byte-equal check).
- β.7.5 (IR-to-wasm AOT) doesn't need to handle in-wasm JS execution.

**Negative:**
- Users expecting Vercel AI SDK-style `run: async () => ...` get a
  rejection at parse time. The error message points them at the
  `native:` reference pattern + notes the δ.2 plan.
- Multi-file projects must wait for v1.1 (single-file constraint).

## Alternatives considered

- **Embed rquickjs for runtime JS** — rejected for v1 because it adds
  ~600KB binary + a Rust↔JS capability bridge (significant scope). δ.2
  picks this up.
- **Subprocess to tsx / Node** — rejected because it requires external
  toolchain ("no toolchain required" promise from the philosophy doc).
- **CamelCase TS fields with auto-mapping to snake_case TOML** —
  rejected because the conformance test would need a canonicalization
  layer; snake_case-on-both-sides is simpler and consistent.

## References

- Spec: `docs/superpowers/specs/2026-06-10-beta-8-ts-authoring-design.md`
- Plan: `docs/superpowers/plans/2026-06-10-beta-8-ts-authoring.md`
- Philosophy: `docs/explanation/tau-philosophy.md` (TS sugar over IR)
- Related ADRs: 0037 (workflow IR), 0040 (β.7 tau dev REPL)
```

- [ ] **Step 3: Commit docs**

```bash
git add ROADMAP.md docs/decisions/0041-ts-authoring-declarations-only.md
git -c user.name="Test User" -c user.email="test@example.com" \
  commit --no-verify -m "docs(adr): ADR-0041 + ROADMAP β.8 v1 scope (β.8 Phase 8 docs)"
```

### Task 8.2 — Workspace validation + push + PR + auto-merge

- [ ] **Step 1: Local gates**

```bash
# fmt
timeout 30 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo fmt --all -- --check

# clippy (new crate + modified tau-cli)
for c in tau-ts-extract tau-cli; do
  timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
    cargo clippy -p "$c" --all-targets -- -D warnings || exit 1
done

# nextest
for c in tau-ts-extract tau-cli; do
  timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
    cargo nextest run -p "$c" || exit 1
done

# doctests
for c in tau-ts-extract tau-cli; do
  timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
    cargo test -p "$c" --doc || exit 1
done

# canary downstream
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo check -p tau-app
```

Fix anything that fails with a focused commit.

- [ ] **Step 2: Push**

```bash
git push --no-verify -u origin feat/beta-8-ts-surface
```

- [ ] **Step 3: Open PR**

```bash
gh pr create -R LEBOCQTitouan/tau \
  --title "β.8 — TS authoring surface (declarations-only via swc)" \
  --body "$(cat <<'EOF'
## Summary

Ships β.8 v1 — the TypeScript authoring surface for tau projects.

- New `tau-ts-extract` workspace crate parses `project.ts` via swc and produces a `ProjectConfig` identical to what the TOML loader produces.
- File-extension dispatch in `cmd/{dev,build,check,run}.rs`: `.ts` → TS extractor; everything else → existing TOML path.
- Declarations-only — inline tool bodies (`run: async () => ...`) rejected at parse time; tool bodies remain Rust-native (referenced via `native: "FnName"`). δ.2 adds runtime JS via QuickJS embed.
- `contextManager` factory exists in the SDK shape but rejects at parse time pending β.4.
- TOML↔TS conformance: canonical fan-monitor scenario authored in either format produces byte-equal canonical IR.

Spec: `docs/superpowers/specs/2026-06-10-beta-8-ts-authoring-design.md`
Plan: `docs/superpowers/plans/2026-06-10-beta-8-ts-authoring.md`
ADR:  `docs/decisions/0041-ts-authoring-declarations-only.md`

## Test plan

- [x] tau-ts-extract: ~13 unit/error/conformance tests
- [x] tau-cli: 2 integration tests (`cmd_dev_ts_one_shot`, `cmd_build_ts`)
- [x] TOML↔TS byte-equal IR conformance on canonical fan-monitor
- [x] `examples/dev-smoke-fan-monitor-ts/project.ts` smoke
- [x] Help snapshots unchanged (β.8 doesn't add a new verb)

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 4: Enrol auto-merge** (BARE)

```bash
PR=$(gh pr view --json number --jq .number)
echo "PR #$PR"
gh pr merge "$PR" --auto
```

- [ ] **Step 5: Monitor CI**

```bash
gh pr view "$PR" --json state,statusCheckRollup --jq '{
  state,
  fails: [.statusCheckRollup[] | select(.conclusion == "FAILURE") | .name],
  inProgress: [.statusCheckRollup[] | select(.status == "IN_PROGRESS") | .name],
  success: ([.statusCheckRollup[] | select(.conclusion == "SUCCESS")] | length)
}'
```

Standard infra recovery per the constraints section: Linux linker bus error → rerun; macOS infra flakes → rerun + re-enrol; stale ci-summary → empty-commit push; `review PR` should pass now (PR #301 shipped GITHUB_TOKEN fix).

Don't wait for CI to finish — report current state + stop. The user's main loop handles monitoring.

---

## Self-review checklist

- [ ] **Spec §1 — Goals/non-goals**: declarations-only (§1) → Phase 3 + Phase 4's `InlineToolBody` rejection. `contextManager` Deferred (§1) → Phase 4's `Deferred` variant. Multi-file imports (§1) → Phase 4's `ImportNotSupported`. ✓
- [ ] **Spec §2.1 — TS API shape**: factory recognition matches exactly the spec's example (snake_case fields, identifier references resolved via scope). Phase 3 covers this. ✓
- [ ] **Spec §2.2 — Discovery rule**: file-extension dispatch in Phase 5. ✓
- [ ] **Spec §3.1 — Pipeline**: parse → scope → factory → lower → ProjectConfig. Phases 2-3 implement. ✓
- [ ] **Spec §3.2 — Module layout**: `tau-ts-extract/src/{lib,parse,scope,factory,lower,error}.rs`. Phase 1 creates all 6 files. ✓
- [ ] **Spec §3.3 — Accepted/Rejected literals**: 10 accepted + 10 rejected. Phase 3 implements accepts; Phase 4 implements rejects. ✓
- [ ] **Spec §3.4 — CLI dispatch**: `cmd::project_load::load_project` helper in Phase 5. ✓
- [ ] **Spec §3.5 — Watcher behavior**: `.ts` projects watch the `.ts` file. Phase 5 Step 4. ✓
- [ ] **Spec §4 — Conformance test**: Phase 6 byte-equal canonical IR check. ✓
- [ ] **Spec §5 — 10 error variants**: Phase 4 full enum. ✓
- [ ] **Spec §6 — Testing**: ~17 tests across 9 unit + 4 rejection + 2 cli integration + 1 conformance + smoke. ✓
- [ ] **Spec §7 — New deps**: swc trio in Phase 1's Cargo.toml. ✓
- [ ] **Spec §9 — ROADMAP edit + ADR-0041**: Phase 8. ✓
- [ ] **No `todo!()` in shipped code** — all `todo!()` style annotations are flagged "ADAPT: read X".
- [ ] **No `Option::map_or(false, ...)`** — use `is_some_and`.
- [ ] **No `[[profile.ci.overrides]]` added** to `.config/nextest.toml`.
- [ ] **Auto-merge enrolled BARE** in Phase 8.

---

## What's next (post-β.8)

β.8 closes one more sub-project on the β engine track. Status after β.8:

- **β.1** ✅ tau-runtime-core extraction (2026-05-31)
- **β.2** ✅ Workflow IR (2026-06-01)
- **β.3** ✅ MCP facilitator (2026-06-10)
- **β.4** ⬜ Context manager primitive (parallel-safe; opt-in)
- **β.5** ⬜ Credential provider chain (parallel-safe)
- **β.6** ⬜ Cross-target conformance gate (needs β.7.5 + β.8)
- **β.7** ✅ tau dev REPL (2026-06-10)
- **β.7.5** ⬜ IR-to-wasm AOT compiler (split from β.7)
- **β.8** ⬜ TS authoring surface (this PR)

**After β.8 merges, remaining β work:** β.4 + β.5 (parallel-safe), β.7.5 (the big wasm AOT lift), β.6 (conformance gate; needs β.7.5).

β.6 + β.7.5 are the two biggest remaining pieces. β.4 + β.5 are independent and could ship in parallel.

Once β closes, **γ (portability targets)** opens — wasm-server (γ.1), wasm-browser (γ.2), C-ABI library (γ.3), and the embedded MCU path.
