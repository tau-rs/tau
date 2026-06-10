# β.8 — TypeScript minimal authoring surface — design

**Status:** Approved 2026-06-10.

**Date:** 2026-06-10.

**Builds on:** β.2 (workflow IR), β.7 (tau dev REPL, `1105f7c`), β.3 (MCP facilitator).

**Preserves:** TOML manifest authoring stays first-class. `tau.toml`-based projects load and run unchanged. β.7's REPL behavior is identical regardless of project format.

**Adds:** a TS authoring path. `tau dev project.ts` and `tau build project.ts` parse a TS source file via swc, statically analyze the AST, and emit the same `ProjectConfig` (and through the existing β.2 path, the same `IrModule`) that the TOML loader produces.

**Out of scope (deferred to δ.2):** inline TS tool bodies via QuickJS embed, npm publishing pipeline, TS type generation from skill schemas, browser-side runtime, multi-file TS imports.

---

## 1. Goals & non-goals

### Goals (load-bearing)

1. `tau dev project.ts -p "hi"` works end-to-end on the canonical fan-monitor scenario.
2. `tau build project.ts` produces the same bundle a `tau.toml` version would, modulo the source-file content hash.
3. **TOML↔TS round-trip equivalence:** the canonical fan-monitor scenario authored in either format produces an identical `IrModule`. The conformance test guards this byte-equally (after canonical encoding).
4. β.7's REPL surface unchanged — `:reload` works on `.ts` files identically to `.toml`.
5. No new JS engine embedded in the binary. swc is Rust-native, ~3 MB compiled in.

### Non-goals (β.8 v1)

- **Inline TS tool bodies.** `tool({ run: async () => readSensor() })` is rejected at parse time. Tool bodies stay Rust-native; TS factories reference them by name (`native: "ReadTemp"`).
- **`contextManager` factory implementation.** Stub (rejects with "β.4 prerequisite"); the factory shape exists in the SDK but emits no IR until β.4 ships ContextManager primitive.
- **Multi-file TS imports.** `import { ... } from "./helpers"` rejected; deferred to v1.1.
- **JS scope beyond top-level constants.** Closures, nested functions, dynamic scope — all rejected.
- **npm publishing.** The `@tau/sdk` shape exists as a Rust crate; npm pipeline is δ.2.
- **`tau dev` watching multiple TS files for hot-reload.** β.7's watcher watches one file path; β.8 keeps that contract.
- **TS as parallel runtime.** Per philosophy doc: TS is sugar over the canonical IR. β.8 v1 has no runtime JS execution.

---

## 2. User-facing surface

### 2.1 The TS API (`@tau/sdk` shape)

Mirrors the philosophy doc's example. Snake_case fields (matching TOML directly) — no name-mapping layer:

```ts
// project.ts

import { agent, tool, mcp } from "tau";

const readTemp = tool({
  native: "ReadTemp",                            // Rust-compiled-in tool reference
  description: "Read the temperature sensor",
  capabilities: { hardware: ["i2c:0x48"] },
});

const weather = mcp("https://mcp.weather.com", {
  capabilities: { network: ["api.weather.com"] },
});

export const fanMonitor = agent({
  display_name: "Fan Monitor",
  package: "fan-monitor@^0.1",
  llm_backend: "anthropic",
  model: "claude-haiku-4-5",
  prompt: { system: "Watch the temperature; turn on the fan if above 30°C." },
  tools: { readTemp, weather },
});
```

Rules:
- **Exported top-level constants define the project.** Multiple `export const`s = multiple agents.
- **Non-exported top-level constants are helpers** — referenced via identifier from exported declarations' factory call args.
- **Factory functions:** `agent(...)`, `tool(...)`, `mcp(...)`. Each accepts an object literal matching the TOML schema 1:1.
- **`contextManager(...)` factory exists** but rejects with `"β.4 prerequisite — context manager not yet implemented (see ROADMAP §β.4)"` at parse time.

### 2.2 CLI surface (no new verbs)

`tau dev`, `tau build`, `tau check`, `tau run` — all unchanged. The dispatch decision happens at project-load time:

| Path | Behavior |
|---|---|
| `tau dev project.ts` | Loads via TS extractor; rest of dev REPL identical |
| `tau dev .` (directory) | Looks for `tau.toml`; errors if not found. Does NOT auto-detect `project.ts`. |
| `tau dev project.ts -p "hi"` | One-shot path through TS extractor |
| `tau build project.ts` | Build path through TS extractor; emits same bundle shape |
| `tau check project.ts` | Validation through TS extractor |

**Discovery rule:** file-extension explicit. `tau dev <path>` where path ends in `.ts` → TS path. Otherwise TOML path. No auto-detect-in-directory magic.

---

## 3. Architecture

### 3.1 Pipeline

```
project.ts (utf-8 source)
     │
     ▼
 swc_ecma_parser → swc_ecma_ast::Module
     │
     ▼
 tau_ts_extract::extract_project()
     │   1. Collect top-level constant decls into name → expr map
     │   2. For each export, recursively resolve the export's factory call:
     │      • Factory function whitelist: agent / tool / mcp / contextManager
     │      • Args = object literal (or identifier → resolved literal)
     │      • Reject anything not in the literal whitelist
     │   3. Build ProjectConfig from the resolved tree
     │
     ▼
 ProjectConfig (existing struct from tau_pkg)
     │
     ▼
 tau_ir::lower::lower_project (existing β.2 path — unchanged)
     │
     ▼
 IrModule (identical to TOML-derived IR)
     │
     ▼
 run_ir / build / check (unchanged)
```

The TS extractor's only job is to produce a `ProjectConfig`. Everything downstream is the existing β.2/β.3/β.7 stack.

### 3.2 New crate: `tau-ts-extract`

```
crates/tau-ts-extract/
├── Cargo.toml          # deps: swc_ecma_parser ^0.150, swc_ecma_ast ^0.118, swc_common ^0.34, tau-pkg, anyhow
├── src/
│   ├── lib.rs          # pub fn extract_project(src: &str, source_path: &Path) -> Result<ProjectConfig, TsExtractError>
│   ├── parse.rs        # swc parser setup; module-level AST acquisition
│   ├── scope.rs        # top-level Decl walker → name → factory-call map
│   ├── factory.rs      # recognize tau factory calls; reject unknown
│   ├── lower.rs        # AST literal → ProjectConfig fields
│   └── error.rs        # TsExtractError carries swc Span → file:line:col
└── tests/
    └── ts_fixtures/    # snapshot fixtures: canonical TS files + expected ProjectConfig
```

### 3.3 What the extractor accepts in factory args

| Allowed | Rejected at parse time |
|---|---|
| Object literals (`{ key: value, ... }`) | Async functions / `await` |
| Array literals (`[1, 2, 3]`) | Function expressions / arrow functions |
| String literals (`"hello"`) | Template literals with interpolation (`` `model-${pick()}` ``) |
| Number literals (`42`, `3.14`) | Function calls (except whitelisted tau factories) |
| Boolean literals (`true`, `false`) | `await` / `Promise` / `async` |
| `null` | Dynamic imports |
| Identifier references resolved to a top-level constant | Imports beyond `from "tau"` (v1) |
| Spread of constant objects (`{ ...defaults, model: "X" }`) | Conditional expressions (`a ? b : c`) — v1 |
| Template literals WITHOUT interpolation (`` `hello` ``) | `typeof`, `instanceof`, member expressions on dynamic values |
| `as const` assertions | Object methods (`{ run() {...} }`) |
| Array of identifier references (resolved) | Computed property keys (`{ [key]: val }`) |

When a rejected node is encountered, the extractor emits a `TsExtractError::UnsupportedExpression { span, kind }` with file:line:col + a one-sentence remediation hint.

### 3.4 Discovery + dispatch

In `cmd::dev::session.rs::load`, the file-extension dispatch:

```rust
pub async fn load(project_path: PathBuf, agent_override: Option<String>) -> Result<Self> {
    let (project_root, project) = match project_path.extension().and_then(|s| s.to_str()) {
        Some("ts") => {
            // TS path
            let src = std::fs::read_to_string(&project_path).context("read project.ts")?;
            let project = tau_ts_extract::extract_project(&src, &project_path)
                .with_context(|| format!("extract from {}", project_path.display()))?;
            let project_root = project_path.parent().unwrap_or(&project_path).to_path_buf();
            (project_root, project)
        }
        _ => {
            // TOML path (default — bare directory or .toml file)
            let project_root = if project_path.is_dir() { project_path.clone() } else {
                project_path.parent().unwrap_or(&project_path).to_path_buf()
            };
            let tau_toml = project_root.join("tau.toml");
            let toml_str = std::fs::read_to_string(&tau_toml).context("read tau.toml")?;
            let project = ProjectConfig::parse_str(&toml_str)?;
            (project_root, project)
        }
    };
    // ... rest of load unchanged
}
```

Same dispatch in `cmd::build.rs::run` + `cmd::check::run` + `cmd::run::run`.

### 3.5 Watcher behavior

β.7's notify-based watcher watches `tau.toml` + `workflows/*.toml`. For `.ts` projects: watch the `.ts` file + `workflows/*.toml`. No additional config files watched (multi-file TS imports are v1.1).

The `pending_reload` mechanic + `:reload` semantics are unchanged. Reload re-runs the extractor on the new TS source; on parse error, keeps old config (matches β.7's malformed-reload behavior).

---

## 4. The TS↔TOML conformance test

The DoD: "the canonical β.6 scenario can be authored in either TOML or TS and produces an identical IR."

Conformance fixture:

```
crates/tau-ts-extract/tests/fixtures/fan_monitor_conformance/
├── tau.toml             # TOML version of the canonical fan-monitor
├── project.ts           # TS version (semantically equivalent)
└── expected_ir.json     # the canonical IR (canonical-encoded) — both should produce this
```

Test:

```rust
#[test]
fn toml_and_ts_produce_identical_ir() {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fan_monitor_conformance");

    // TOML path
    let toml_str = std::fs::read_to_string(fixture_dir.join("tau.toml")).unwrap();
    let toml_project = tau_pkg::project::project::ProjectConfig::parse_str(&toml_str).unwrap();
    let toml_ir = tau_ir::lower::lower_project(&toml_project, &target, &caches).unwrap();

    // TS path
    let ts_src = std::fs::read_to_string(fixture_dir.join("project.ts")).unwrap();
    let ts_project = tau_ts_extract::extract_project(&ts_src, &fixture_dir.join("project.ts")).unwrap();
    let ts_ir = tau_ir::lower::lower_project(&ts_project, &target, &caches).unwrap();

    // Compare via canonical encoding (β.2 §C3).
    let toml_canonical = tau_ir::canonical::encode(&toml_ir).unwrap();
    let ts_canonical = tau_ir::canonical::encode(&ts_ir).unwrap();
    assert_eq!(toml_canonical, ts_canonical, "TOML↔TS IRs must be byte-equal");
}
```

This test is the load-bearing β.8 deliverable. If it passes, the surface is correct.

---

## 5. Error handling

| Condition | Behavior |
|---|---|
| `project.ts` doesn't exist | Standard I/O error from `std::fs::read_to_string` with path |
| `project.ts` is not UTF-8 | `TsExtractError::NotUtf8 { path }` |
| swc parse error (syntax error) | `TsExtractError::ParseError { span: {file, line, col}, message }` |
| Unknown factory function called (e.g. `unknown({...})`) | `TsExtractError::UnknownFactory { span, name }` |
| Unsupported expression in factory args (e.g. `await fetch(...)`) | `TsExtractError::UnsupportedExpression { span, kind, hint }` |
| Identifier reference to undefined name | `TsExtractError::UnresolvedIdentifier { span, name }` |
| `contextManager(...)` call | `TsExtractError::Deferred { span, factory: "contextManager", until: "β.4" }` |
| Multi-file `import from "./helpers"` | `TsExtractError::ImportNotSupported { span, source }` with hint "v1.1 will add multi-file support" |
| Cycle in identifier references (rare; A references B references A) | `TsExtractError::CyclicReference { span, cycle: Vec<Name> }` |
| `tool({ run: async () => ... })` (inline tool body) | `TsExtractError::InlineToolBody { span, hint: "β.8 v1 supports only `native: \"FnName\"` references; inline TS bodies are δ.2" }` |

All errors carry swc spans and emit `file:line:col` formatted via swc's source map. β.7's `:reload` error path renders them.

---

## 6. Testing

### 6.1 Unit tests (`tau-ts-extract` lib)

| Test | What it verifies |
|---|---|
| `parses_minimal_agent_export` | `export const x = agent({display_name: "X", ...})` → `ProjectConfig` with one agent |
| `resolves_top_level_constant_reference` | `const t = tool({...}); export const a = agent({tools: {t}})` → tools field includes `t`'s decl |
| `recognizes_three_factories` | agent/tool/mcp parsed correctly; unknown rejected with `UnknownFactory` |
| `rejects_async_function_body` | `tool({run: async () => ...})` → `InlineToolBody` error |
| `rejects_interpolated_template` | `` model: `claude-${x}` `` → `UnsupportedExpression` |
| `rejects_imports_beyond_tau` | `import { x } from "./helpers"` → `ImportNotSupported` |
| `error_positions_carry_line_col` | Verify each error type's span resolves to file:line:col |
| `defers_context_manager` | `contextManager({...})` → `Deferred` error |
| `spread_of_constant_resolves` | `agent({...defaults, model: "X"})` where defaults is a top-level const |

### 6.2 Integration tests (in `crates/tau-cli/tests/`)

| Test | What it verifies |
|---|---|
| `cmd_dev_ts_one_shot.rs` | `tau dev project.ts -p "hi"` boots + exits gracefully |
| `cmd_build_ts.rs` | `tau build project.ts -o /tmp/out.bundle` produces a valid bundle |

### 6.3 The conformance test

`tau-ts-extract/tests/fan_monitor_conformance.rs` — the load-bearing TS↔TOML round-trip described in §4.

### 6.4 Total

~17 tests (9 unit + 2 cli integration + 1 conformance + 5 error-shape tests). Matches β.7's footprint.

---

## 7. Dependencies (new)

| Crate | Version | Why |
|---|---|---|
| `swc_ecma_parser` | `^0.150` | TS source → AST |
| `swc_ecma_ast` | `^0.118` | AST node types |
| `swc_common` | `^0.34` | source maps, spans, error positions |

(Versions are illustrative; implementer pins to the latest stable trio. The swc workspace versions trios together — `swc_common` must be compatible with the parser version.)

Size impact on `tau-cli` binary: ~3 MB (acceptable; bypasses the need for a JS engine).

No new system dependencies — pure Rust. Works cross-platform identically.

---

## 8. Sub-project sizing

| Phase | Scope | Tests |
|---|---|---|
| 1 | `tau-ts-extract` crate scaffold + Cargo deps + workspace member | 1 smoke |
| 2 | swc parser setup + module-level constant walker → name map | 3 unit |
| 3 | Factory recognizer (agent/tool/mcp) + object-literal → ProjectConfig field mapping | 4 unit |
| 4 | Rejection pathway: positioned errors for unsupported expressions | 4 error-shape tests |
| 5 | CLI dispatch in cmd/{dev,build,check,run}.rs based on file extension | 2 integration |
| 6 | The TOML↔TS conformance test on canonical fan-monitor | 1 conformance |
| 7 | `examples/dev-smoke-fan-monitor-ts/project.ts` + end-to-end smoke | 2 integration |
| 8 | ROADMAP edit (β.8 v1 = declarations; note δ.2 adds runtime JS) + ADR-0041 + push + PR + auto-merge | — |

**~11 tasks, ~17 tests, ~2 weeks.** Comparable to β.7's footprint.

---

## 9. ROADMAP edit (shipped as part of this PR)

### Current ROADMAP §β.8 (lines 505–526)

```
### β.8 — TypeScript minimal authoring surface

The philosophy argues for Vercel-DX-like authoring, and deferring TS
entirely to δ.2 would contradict that. β.8 lands the **minimal** TS
surface needed for an authoring-quality experience; δ.2 polishes it
into a publishable SDK.

- Builds on: β.2 (the IR) + β.7 (tau dev).
- Preserves: TOML manifest authoring stays first-class.
- Adds: @tau/sdk package shape — agent({...}), tool({...}), mcp({...}),
  contextManager({...}) factory functions that produce IR-emitting JS
  objects. tau dev project.ts reads the TS file via a thin loader
  (esbuild-in-process) and emits the IR. One way to write a project
  (TOML or TS, your choice), one IR underneath.
- Supersedes: nothing.
- DoD: the canonical β.6 scenario can be authored in either TOML or
  TS and produces an identical IR (verified by the conformance gate).

Out of scope for β.8 (held for δ.2): npm publishing pipeline, TS type
generation from skill schemas, browser-side runtime, full editor
plugin polish.
```

### Amended

```
### β.8 — TypeScript minimal authoring surface

The philosophy argues for Vercel-DX-like authoring, and deferring TS
entirely to δ.2 would contradict that. β.8 lands the **minimal** TS
surface needed for an authoring-quality experience; δ.2 polishes it
into a publishable SDK.

- Builds on: β.2 (the IR) + β.7 (tau dev).
- Preserves: TOML manifest authoring stays first-class. β.7's REPL
  behavior is identical regardless of project format.
- Adds: @tau/sdk package shape — agent({...}), tool({...}), mcp({...})
  factory functions accepting object literals matching the TOML schema
  1:1 (snake_case fields, no name-mapping layer). tau dev project.ts
  parses via swc + statically analyzes the AST + emits the same
  ProjectConfig the TOML loader produces. contextManager({...}) factory
  EXISTS but rejects at parse time pending β.4. One way to write a
  project (TOML or TS, your choice), one IR underneath.
- Supersedes: nothing.
- DoD: the canonical β.6 scenario authored in either TOML or TS
  produces a byte-equal IR after canonical encoding (verified by the
  TOML↔TS conformance test).
- Design: docs/superpowers/specs/2026-06-10-beta-8-ts-authoring-design.md
- ADR: 0041 (records the declarations-only-no-embedded-JS decision)

Out of scope for β.8 v1:
- Inline TS tool bodies (run: async () => ...) — δ.2 adds runtime JS
  execution via QuickJS embed
- Multi-file TS imports (from "./helpers") — v1.1
- npm publishing pipeline, TS type generation from skill schemas,
  browser-side runtime, full editor plugin polish — δ.2
- contextManager factory implementation — β.4 prerequisite
```

---

## 10. Risks

| Risk | Mitigation |
|---|---|
| swc API drift (versioned ecosystem) | Pin `swc_ecma_parser` + `swc_ecma_ast` + `swc_common` as a versioned trio; upgrade in coordinated PRs |
| Error messages from static analysis are unhelpful | All errors carry swc spans → file:line:col; Phase 4 explicitly tests error positioning |
| TS users expect `run: async () => ...` to work | Reject explicitly with `InlineToolBody` error including the remediation hint ("use `native:` reference + Rust-compiled-in tool") |
| Conformance test is brittle if TS↔TOML lower to subtly different IRs (e.g. field ordering) | The β.2 canonical encoding handles this — same lowering function for both surfaces; compare canonical bytes, not raw struct equality |
| swc binary size impact on tau-cli | ~3 MB compiled. Acceptable vs ~600KB-2MB for an embedded JS engine; better cross-platform consistency |
| The δ.2 work (runtime JS) may render β.8 v1's static analysis obsolete | No — the static path stays the canonical path. δ.2 ADDS runtime JS for inline tool bodies but doesn't replace the declaration-extraction layer. The two layers compose. |
| Users author multi-file TS projects and hit the v1.1 deferral | Document clearly in error message. The Vercel AI SDK pattern is one-file-per-agent anyway; v1's single-file constraint matches |

---

## 11. Open questions (deferred to plan / v1.1, not blocking spec approval)

- Should `tau check project.ts` validate that all referenced native tools EXIST in the compiled tau binary? (Yes per "Rust-like build-time enforcement" principle — but the implementation is a sweep across `NativeRegistry`, deferred to plan time.)
- Should the TS extractor emit a `// @generated` warning if a user has a `tau.toml` AND a `project.ts` in the same directory? (Probably yes — flag potential confusion. Plan-time.)
- Does β.8 ship with a `project.ts` template for `tau init`? (Yes; δ.4 reference templates extend this but β.8 should ship at least one.)

---

## 12. Lineage

This spec descends from:
- 2026-05-29 philosophy pivot — "TS sugar over IR, not parallel runtime"
- ROADMAP §β.8 (now amended per §9 above)
- β.7's `tau dev` REPL (PR #302) — provides the dev path; β.8 adds the TS loader inside it
- β.2's IR + canonical encoding — TOML↔TS equivalence depends on canonical encoding
