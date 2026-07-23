# EPIC 5.3 — Authoring-SDK codegen from the IR JSON schema

**Date:** 2026-07-23
**Status:** Approved (brainstorming) → ready for plan
**Roadmap:** `docs/superpowers/plans/vision-roadmap.md` EPIC 5.3
**Branch:** `feat/epic-5-3-sdk-codegen`

## Goal

Generate typed authoring SDKs for TypeScript and Python so the *same agent*
expressed in tau-native TOML, TS, and Python all lower to **byte-identical
canonical IR**. This mirrors the guarantee tau already ships for TOML↔TS
(`tau-ts-extract`, PR #303) and extends it to Python and to *generated*
(rather than hand-recognized) SDK surfaces.

Acceptance (roadmap 5.3, verbatim): *"same agent in TOML/TS/Python → identical IR."*

This work is **off the critical compile-proof path** (pure DX/polyglot). It
introduces a **new crate only** and MUST NOT alter `tau-ir`, `tau-pkg`,
`tau-ir-lower`, or the published IR JSON schema.

## The central design decision (and why)

The published IR JSON schema (`schemas/ir/tau-ir.v2.5.0.schema.json`,
generated from `tau_ir::IrModule` via `schemars`, frozen in EPIC 2.2 / PR #430)
describes the **post-lowering** `IrModule` — *not* the authoring surface
(`ProjectConfig` / `tau.toml`). Lowering does real work between them:

- resolves model aliases → `{backend, model}` pairs,
- synthesizes the per-tool `capability_table`,
- content-addresses (hashes) native-tool / prompt assets,
- drops authoring-only fields (e.g. `packages`),
- orders all maps as `BTreeMap` and encodes via serde field-declaration order.

Three architectures were considered:

- **A — SDK emits IR directly.** Rejected. Requires re-implementing
  `tau-ir-lower` in *both* TS and Python and hand-matching serde
  declaration-order bytes (there is no language-neutral sorted-key canonical
  form yet — that is scheduled for `ir_format` 3.0.0). It duplicates the
  compile-proof core in two dynamic languages, defeating tau's "Rust core is
  the single source of truth" invariant.

- **B — SDK emits the authoring surface; Rust lowers.** The SDK produces a
  `ProjectConfig`-equivalent; the *same* `lower_project` + `to_canonical_bytes`
  runs on all three sources. Byte-equality is then structural, not
  coincidental: the instant each path reaches `ProjectConfig` the three values
  are equal, and exactly one Rust encoder runs afterward. This is precisely how
  `tau-ts-extract` already achieves TOML↔TS equality ("no duplicated logic").

- **C — Chosen. B's flow, plus schema-driven codegen for the overlapping
  vocabulary.** The frozen IR schema drives the **shared leaf/vocabulary
  types** (which are identical in authoring and IR because lowering does not
  reshape them); the small **authoring composition** (which factory has which
  fields) is an owned declarative table pinned by the byte-equal test.

**Chosen: C.** It is the only option that preserves the single-source-of-truth
invariant, satisfies the byte-equal acceptance without re-implementing the
compiler twice, *and* genuinely consumes the frozen schema as the source of the
shared vocabulary.

## Architecture

```
schemas/ir/tau-ir.v2.5.0.schema.json          authoring::SURFACE  (owned table in crate)
        │  serde_json::from_slice                     │  8 factories × their fields;
        ▼                                             │  each field references a leaf type
  schema::SchemaModel                                 │
  (mirrors the SHARED leaf/vocab $defs:      ◄────────┘
   Capability, backend/model enums,
   prompt/tool value shapes, output-schema)
        └──────────────┬───────────────┘
                       ▼
              emit_ts.rs   emit_python.rs
                       ▼
        sdk/ts/**        sdk/python/**   (typed packages, checked in; regenerated on demand)
```

### The two inputs

1. **Frozen IR schema → leaf/vocabulary types.** `schema.rs` parses the
   committed `tau-ir.v2.5.0.schema.json` and lifts the `$defs` that are shared
   verbatim between authoring and IR — `Capability`, backend/model enums,
   prompt shapes, tool-capability shapes, output-schema types — into a small
   `SchemaModel`. When the schema evolves, these SDK types update by
   re-running codegen, not by hand.

2. **Authoring composition → owned table.** `authoring.rs` declares the
   authoring factories (`agent`, `tool`, `mcp`, `models`, `goals`,
   `deliverables`, `pipeline`) and, per factory, its TOML
   target (`[[agents]]`, `[models]`, …) and its fields (sdk name → toml key →
   leaf type → required). (`contextManager` is recognized by `tau-ts-extract`
   but rejected as deferred, so the SDK does not emit it.) This cannot come
   from the IR schema (the IR is
   post-lowering); it is kept honest by the byte-equal + drift tests, not by
   trust. Field/factory names MUST match what `tau-ts-extract` already
   recognizes (`crates/tau-ts-extract/src/factory.rs`,
   `crates/tau-ts-extract/src/lower.rs`).

### Intermediate model (type sketch)

```rust
struct SchemaModel { leaves: BTreeMap<String, LeafType> }
enum LeafType { Enum { variants: Vec<String> }, Struct { fields: Vec<Field> }, Alias(TypeRef) }
struct Field  { name: String, ty: TypeRef, required: bool }
enum  TypeRef { Str, Bool, Int, Named(String), Array(Box<TypeRef>), Map(Box<TypeRef>) }

struct Factory   { name: &'static str, toml_target: TomlTarget, fields: &'static [AuthField] }
struct AuthField { sdk_name: &'static str, toml_key: &'static str, ty: TypeRef, required: bool }
enum   TomlTarget { Table(&'static str), ArrayOfTables(&'static str) }
```

### Public API of the crate

```rust
/// Generate both SDK packages into `out_dir` from the frozen IR schema.
pub fn generate(schema_path: &Path, out_dir: &Path) -> Result<(), CodegenError>;
```

`CodegenError` is a `thiserror` enum at the crate boundary; internals use
`anyhow`. Crate declares `#![forbid(unsafe_code)]`.

## Generated package shapes

### TypeScript — `sdk/ts/` (npm `@tau/sdk`)

Factories match `tau-ts-extract`'s recognized surface verbatim, so the
extractor ingests SDK-authored `project.ts` unchanged. The import source must
be exactly `"tau"` (the extractor hard-rejects any other source); `package.json`
name is `@tau/sdk`, resolved to the bare module `"tau"` via tsconfig path
aliasing in fixtures. The extractor only reads the AST — it never runs
`npm install`.

```ts
// sdk/ts/src/factories.ts   (GENERATED)
export interface ModelRef   { backend: string; model: string }
export interface ToolConfig { native: string; description?: string;
                              capabilities?: Record<string, boolean> }
export interface AgentConfig{ display_name: string; package: string; model: string;
                              prompt?: { system?: string }; tools?: string[] }
export const models = (m: Record<string, ModelRef>) => m;
export const tool   = (c: ToolConfig)  => c;
export const agent  = (c: AgentConfig) => c;
```

### Python — `sdk/python/` (PyPI `tau-sdk`, import `tau_sdk`)

Typed dataclass builders **plus a deterministic `tau.toml` renderer**. The
renderer walks authored objects in the fixed factory/field order the emitter
baked in and prints TOML to stdout; this is the artifact the live byte-equal
test captures.

```python
# sdk/python/tau_sdk/factories.py   (GENERATED)
@dataclass
class ModelRef:   backend: str; model: str
@dataclass
class ToolConfig: native: str; description: "str|None" = None
                  capabilities: "dict[str,bool]|None" = None
@dataclass
class AgentConfig: display_name: str; package: str; model: str
                   prompt: "dict|None" = None; tools: "list[str]|None" = None
def agent(**kw) -> AgentConfig: ...
def render_project(*, models=None, tools=(), agents=()) -> str: ...   # → tau.toml text
def print_toml(**kw) -> None:  print(render_project(**kw))
```

The renderer's TOML need not be byte-identical to a hand-written `tau.toml`:
TOML is order/whitespace-insensitive into `ProjectConfig`, and
`canonical_cosmetics_insensitive` already proves cosmetic TOML differences
vanish at the IR. It only needs to parse to the same `ProjectConfig`.

## The byte-equal proof (acceptance test — written first, TDD)

One fixture agent authored three ways; all three lowered by the single Rust
`lower_project`; assert three-way byte-equal canonical IR. Live per decision Q2
— Python is executed at test time.

```rust
// crates/tau-sdk-codegen/tests/byte_equal.rs
let target = TargetTriple::host();
let caches = test_caches();

let toml_cfg = ProjectConfig::parse_str(&read("fixtures/basic_agent/tau.toml"))?;
let ts_cfg   = tau_ts_extract::extract_project(&read("fixtures/basic_agent/project.ts"), path)?;
let py_toml  = run_python("fixtures/basic_agent/project.py")?;   // shells to python3, captures stdout
let py_cfg   = ProjectConfig::parse_str(&py_toml)?;

let bytes = |c| to_canonical_bytes(&lower_project(c, &target, &caches)?.module);
assert_eq!(bytes(&toml_cfg), bytes(&ts_cfg));   // TOML == TS
assert_eq!(bytes(&toml_cfg), bytes(&py_cfg));   // TOML == Python  ← EPIC 5.3 accept
```

- TOML and TS run **in-process** (no external toolchain: `tau-ts-extract` uses
  swc in Rust).
- Python is executed via `python3`. When `python3` is absent (some CI lanes,
  offline dev), the test is skipped with a clear message rather than failing —
  the crate still builds and its other tests still run everywhere. CI lanes that
  run this crate's tests MUST have `python3` available for the acceptance
  assertion to execute; this is documented in the plan.

### End-to-end trace (the `basic_agent` fixture)

All three sources describe: a `haiku` model alias
(`anthropic`/`claude-haiku-4-5`), one native `ReadTemp` tool needing
`sensor.read`, and a system prompt. Each path reaches an *identical*
`ProjectConfig`:

- **TOML**: `ProjectConfig::parse_str` directly.
- **TS**: `tau-ts-extract` recognizes the generated factories, re-serializes to
  its internal TOML, calls `ProjectConfig::parse_str`.
- **Python**: `render_project` prints `tau.toml`; the test captures stdout and
  calls `ProjectConfig::parse_str`.

From the identical `ProjectConfig`, exactly one Rust pass runs — resolving the
alias, synthesizing the capability table, hashing the tool asset, dropping
`packages`, ordering maps, and encoding — so the bytes are equal by
construction.

## Drift guard (separate from the acceptance test)

A pure-Rust test proves the checked-in `sdk/ts` + `sdk/python` equal a fresh
`generate()` (mirrors `crates/tau-ir/tests/schema_export.rs`): generate into a
temp dir, compare against the committed packages. Regenerate + commit with an
`UPDATE_SDK=1`-style flag when the schema or authoring surface changes. This
keeps the generated packages honest without coupling the acceptance test to
regeneration.

## Crate & repo layout

```
crates/tau-sdk-codegen/
  Cargo.toml                     # [lints] workspace = true; forbid(unsafe_code)
  src/
    lib.rs                       # pub fn generate(...)
    error.rs                     # thiserror CodegenError (boundary)
    schema.rs                    # IR schema JSON → SchemaModel (leaf types)
    authoring.rs                 # owned authoring-surface table (factories × fields)
    emit_ts.rs                   # SchemaModel + SURFACE → sdk/ts files
    emit_python.rs               # SchemaModel + SURFACE → sdk/python files
  tests/
    byte_equal.rs                # THE acceptance test (TDD-first)
    drift.rs                     # generated packages == fresh generate()
    fixtures/basic_agent/{tau.toml, project.ts, project.py}
sdk/
  ts/         package.json (@tau/sdk) + src/*.ts     (emitted, checked in)
  python/     pyproject.toml (tau-sdk) + tau_sdk/*.py (emitted, checked in)
```

- New crate `tau-sdk-codegen` added to root `[workspace.members]` and
  `[workspace.dependencies]` following existing conventions (all shared fields
  `.workspace = true`, deps `{ workspace = true }`, trailing `[lints] workspace = true`).
- `sdk/` is **top-level**, sibling to `schemas/` and `wit/`; the packages are
  publishable npm/PyPI artifacts and are deliberately *not* Cargo members.
- Dependencies: `serde_json` (schema parse), `thiserror` + `anyhow`; dev-deps
  `tau-pkg` (`ProjectConfig`), `tau-ts-extract`, `tau-ir`, `tau-ir-lower`,
  `tau-ports` — matching how `tau-ts-extract`'s own tests wire the lowering
  pipeline.

## Scope guard (YAGNI)

**In scope:** the codegen crate; emitted `@tau/sdk` (TS) and `tau-sdk` (Python)
package *source* incl. `package.json` / `pyproject.toml`; the live byte-equal
acceptance test; the drift guard; the `basic_agent` fixture (extendable to more
factories).

**Out of scope:** actual npm/PyPI *publishing* (release plumbing); the typed
React hook / Angular service and Web-Worker `RunEvent` streaming (roadmap 5.4);
covering every last authoring factory in the first fixture (start with the ones
the byte-equal proof needs; the table + emitters make adding more mechanical).

## Testing

- `tests/byte_equal.rs` — the acceptance test, written first (TDD).
- `tests/drift.rs` — generated packages equal fresh `generate()`.
- Unit tests in `schema.rs` / `emit_*` for leaf-type extraction and emitter
  output snippets.
- Run per CARGO rules:
  `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e53 cargo nextest run -p tau-sdk-codegen`
  (doctests, if any, via `cargo test --doc`).

## Risks / open points

- **`python3` availability on CI lanes.** The acceptance assertion for Python
  only executes where `python3` is present; the plan documents which lane runs
  this crate and confirms `python3`. Elsewhere the test self-skips.
- **Authoring-surface drift vs `tau-ts-extract`.** The owned table must track
  the extractor's recognized factories/fields. The byte-equal test is the
  guard: a mismatch fails to lower or diverges in bytes. If the surfaces grow,
  a future refactor could make `tau-ts-extract` expose its factory table as
  data — out of scope here.
- **Leaf-type selection from the schema.** The first pass mirrors only the leaf
  types the `basic_agent` fixture exercises; broadening is mechanical and
  guarded by the drift test.
