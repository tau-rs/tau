# E-1 — Rust Declarations Implementation Plan (Phase 1)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The muscle surface exists: a tool authored with `#[tau::tool]` (or `#[tau::deterministic]`) + `tau::export![]` flows through a unified registry into build (real content hashes — the name-hash hole closed), `tau check` (capabilities), the capability card, and `tau.gen.ts` typed bindings; `schemas/project-manifest/` is published + drift-tested; the legacy TS/Python authoring lane is deleted.

**Architecture:** A new proc-macro crate (`tau-macros`) parses attribute + signature into a const registration record (name, JSON schema derived from arg types, description from doc comment, declared capabilities); `tau::export![]` assembles the records into a **unified registry** (extending the `tau-native-tools` one-source-of-truth pattern, `crates/tau-native-tools/src/lib.rs`) that feeds BOTH dispatchers (native host + wasm guest) and the build pipeline. `cmd/build.rs`'s `sha256_name` sentinel (lines ~533, ~597 — `native_tool_hash`) is replaced by the registry's source-content hash, exactly as its doc-comment promises. A `tau.gen.ts` emitter (in `tau-sdk-codegen`, following the `embed_js.rs` emitter+drift-test pattern) exposes typed bindings stamped with the registry content hash. `schemas/project-manifest/` is generated from the `UncheckedProjectConfig` serde model like `schemas/ir/` (schemars freeze + `UPDATE_SCHEMA=1` flow).

**Tech Stack:** Rust proc-macro (`syn`/`quote`), `schemars`, existing `tau-sdk-codegen` emitter/drift-test conventions, `cargo nextest`.

**Design:** [`../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md`](../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md) §1 (Rust surface, `tau.gen.ts`), §3.4 (name-hash repair), §4 (L1 factories).
**ADRs:** [0071](../../decisions/0071-three-surface-split.md) (surface roles) · [0072](../../decisions/0072-synth-contract.md) (gen stamping, manifest schema) · [0041 banner](../../decisions/0041-ts-authoring-declarations-only.md) (legacy-lane deletion sanction).
**Tree:** [`../implementation-trees/authoring-surfaces.md`](../implementation-trees/authoring-surfaces.md)

## Global Constraints

- Every cargo command: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p <crate> <filter>` (repo CARGO RULES; `timeout 180` + `cargo check`; `timeout 240` clippy; never bare cargo; never workspace-wide).
- Commit with explicit identity: `git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "..."`.
- `tau-native-tools` is `no_std` + `forbid(unsafe_code)`; the unified registry must keep the guest path `no_std` (registration records are const data; only the emitter/build side may use std).
- No-flag-day (design constraint): the legacy lane (Task 8) is deleted only in the PR **after** its replacement demonstrably covers the north-star fixture; deprecate-warn for `[tools] native=` strings rides E-2's removal cycle, not this epic.
- Schema discipline: `schemas/project-manifest/` follows ADR-0065 (authored = strict) + the `schemas/ir/` freeze pattern; schema + fixtures move in the same PR.
- ISSUE RULES sweep before each task (`gh pr list --search "<topic> in:title" --state all`).

---

### Task 1: `tau-macros` crate — `#[tau::tool]` parses to a registration record

**Files:**
- Create: `crates/tau-macros/` (proc-macro crate; workspace member), `crates/tau-macros/src/lib.rs`, `crates/tau-macros/tests/expand.rs` (trybuild or macrotest-style)
- Create: registration record type in `crates/tau-native-tools/src/registry.rs` — `pub struct ToolRegistration { pub name: &'static str, pub description: &'static str, pub args_schema: &'static str /* canonical JSON */, pub capabilities: &'static [CapabilityDecl], pub body: fn(&Value) -> Result<Value, ToolBodyError>, pub source_hash: [u8; 32] }`

**Interfaces:**
- Produces: `#[tau::tool(capabilities(...))] fn read_temp(args: ReadTempArgs) -> ToolResult` → a `const` `ToolRegistration`; name defaults to fn name (id-grammar-checked per ADR-0070), description from `///` doc, schema derived from the arg struct (`schemars` at macro-expansion via derive on the struct + build-time canonicalization).
- Consumes: `tau-domain` capability types.

**Steps:**
- [ ] **Step 1 (red):** Expansion test: a fixture fn with doc comment + typed args expands to a record with the right name/description; a fn with an invalid name (`BadName`) fails to compile with the id-grammar error (compile-fail case).
- [ ] **Step 2:** `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-macros` — expect red/compile fail.
- [ ] **Step 3:** Implement the attribute macro (record synthesis only — no registry yet). Keep schema derivation as a two-step: macro emits the schemars derive + a const fn the build step canonicalizes.
- [ ] **Step 4 (green):** Re-run; `ARCHITECTURE.md` gains the crate row (architecture gate).
- [ ] **Step 5:** Commit `feat(macros): #[tau::tool] attribute — registration record synthesis`.

### Task 2: `#[tau::deterministic]` + `tau::export![]` — the unified registry

**Files:**
- Modify: `crates/tau-macros/src/lib.rs` (second attribute; the `export![]` macro assembling `pub static TAU_REGISTRY: &[ToolRegistration]`)
- Modify: `crates/tau-native-tools/src/registry.rs` (lookup: `pub fn find(name: &str) -> Option<&'static ToolRegistration>`; duplicate-name = compile error via const assertion)
- Test: `crates/tau-native-tools/tests/registry.rs`

**Interfaces:**
- Produces: one registry statically containing every exported tool + deterministic fn; `invoke(tool_id, args)` (the existing `lib.rs:25` dispatch) reimplemented over the registry so the dev/wasm one-source-of-truth property is preserved by construction.
- Consumes: Task 1 records.

**Steps:**
- [ ] **Step 1 (red):** Tests: `find("read_temp")` returns the record; the existing conformance bodies (`read_temp` → `32`, `set_fan` → `{ok:true}`) still answer through `invoke` (moved from the match to registry entries — byte-identical outputs, the PR-G `dev == wasm` gate depends on it); duplicate export is a compile-fail fixture.
- [ ] **Step 2:** Run — red. Implement. Run — green, **including** `timeout 300 ... cargo nextest run -p tau-ir-conformance` and the no_std guest build check (`timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-native-tools --no-default-features`).
- [ ] **Step 3:** Commit `feat(native-tools): unified static registry behind invoke()`.

### Task 3: Real content hashes — close the name-hash hole

**Files:**
- Modify: `crates/tau-cli/src/cmd/build.rs` (~533: the `Caches.native_tool` closure; ~597: `native_tool_hash`) — replace `sha256_name(name)` with the registry record's `source_hash`
- Modify: `crates/tau-macros/src/lib.rs` (macro captures a hash of the fn's token stream into `source_hash`)
- Test: `crates/tau-cli/tests/` build-hash test + a lowering test in `tau-ir-lower`

**Interfaces:**
- Produces: two different tool bodies with the same name → different IR content hashes; rebuilding an unchanged body → identical hash (deterministic tokens-hash).
- Fixes: the design-§3.4 defect "capability-order-sensitive hashes" is **out of scope here** (E-4 repairs); only the name-hash hole closes now.

**Steps:**
- [ ] **Step 1 (red):** Test: build the north-star fixture twice — hashes stable; patch a tool body in a scratch fixture — the tool's hash (and the module hash) change; an *unregistered* name in config still resolves to the named unknown-tool error path (never the old name-hash fallback).
- [ ] **Step 2:** Run red → implement → green. Verify bundle reproducibility job fixtures (`tau verify --bundle` path) still pass: `timeout 300 ... cargo nextest run -p tau-cli build`.
- [ ] **Step 3:** Commit `fix(build)!: native tool hashes are source-content hashes (closes the name-hash hole)`. `!` because bundle hashes shift once — CHANGELOG entry required.

### Task 4: `schemas/project-manifest/` — published + drift-tested

**Files:**
- Create: `schemas/project-manifest/project-manifest.v1.schema.json`
- Create: `crates/tau-pkg/tests/schema_export.rs` (mirror `crates/tau-ir/tests/schema_export.rs`'s `UPDATE_SCHEMA=1` flow)
- Modify: `crates/tau-pkg/src/project/project.rs` (add `schemars::JsonSchema` derives to the `Unchecked*` model behind a `schema-export` feature)

**Steps:**
- [ ] **Step 1 (red):** Drift test: committed schema == freshly generated schema; red until the file exists.
- [ ] **Step 2:** Generate via `UPDATE_SCHEMA=1 timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg schema_export`; commit schema + test together.
- [ ] **Step 3:** Round-trip fixture: a sample manifest JSON validates against the schema AND parses via `UncheckedProjectConfig` (both directions agree on one fixture set).
- [ ] **Step 4:** Commit `feat(pkg): publish schemas/project-manifest (drift-tested, ADR-0072)`.

### Task 5: `tau.gen.ts` emitter

**Files:**
- Create: `crates/tau-sdk-codegen/src/gen_ts.rs` (emitter; follow `embed_js.rs` conventions), drift test `gen_ts_drift`
- Modify: `crates/tau-cli/src/cmd/` — `tau dev`/`tau build` write `tau.gen.ts` into the project root when the project opts in (presence of `[synth]` or `pipelines/`; zero-cost for TOML-only projects per design §12)
- Test: emitter unit tests + a CLI test on the north-star fixture

**Interfaces:**
- Produces: `tau.gen.ts` exporting typed ids for agents, models, tools, deterministic fns, agent kinds; header carries `// tau:registry-hash <hex>` — the **stamp**.
- Consumes: `ProjectConfig` (vocabulary) + the Task-2 registry (muscle).

**Steps:**
- [ ] **Step 1 (red):** Emitter test: north-star fixture → snapshot `tau.gen.ts` (committed fixture); stamp equals the registry content hash.
- [ ] **Step 2:** Implement; green; commit `feat(sdk-codegen): tau.gen.ts emitter (typed bindings, hash-stamped)`.

### Task 6: Stale-gen = loud build error (the anti-Prisma rule)

**Files:**
- Modify: `crates/tau-cli/src/cmd/build.rs` + `project_load.rs` — when `tau.gen.ts` exists, compare its stamp against the current registry hash; mismatch = named error `StaleGeneratedBindings { expected, found }` with the regen command in the message
- Test: CLI test (stale stamp fixture)

**Steps:**
- [ ] **Step 1 (red):** Fixture with a doctored stamp → build fails with the named error; message names `tau dev` as the fix.
- [ ] **Step 2:** Implement; green; `tau dev` regenerates and clears it (integration test).
- [ ] **Step 3:** Commit `feat(build): stale tau.gen.ts is a loud error, never wrong autocomplete`.

### Task 7: Capability card + `tau check` read the registry

**Files:**
- Modify: the check path (`rg -n "native" crates/tau-cli/src/cmd/check*.rs` + `tau-ir-lower` capability collection) — declared caps of registered tools join the lattice check (tool caps ⊆ agent ⊆ `[allow]`, ADR-0057 lattice)
- Test: over-reaching fixture — a `#[tau::tool(capabilities(network("evil.example")))]` fn in a project whose `[allow]` lacks it → `tau check` fails with the existing lattice error naming the tool

**Steps:**
- [ ] **Step 1 (red):** Fixture + assertion on the named error.
- [ ] **Step 2:** Implement (wire registry caps where the string-named native tools currently contribute none); green; the card (`inspect` precursor output in `tau check --verbose` or current card surface) lists the tool with its declared caps.
- [ ] **Step 3:** Commit `feat(check): registry capabilities enter the lattice + card`.

### Task 8: Delete the legacy authoring lane

**Files:**
- Delete: `crates/tau-ts-extract/`, `sdk/ts/` (static factories), `sdk/python/` (TOML-emitting authoring SDK)
- Modify: `Cargo.toml` members, `crates/tau-cli/src/cmd/project_load.rs` (`.ts` file-extension dispatch → named error pointing at `[synth]`/E-2), `examples/dev-smoke-fan-monitor-ts/` (deleted or converted to the north-star `pipelines/` shape **only if** E-2's runner already landed; else deleted with the smoke job), `ARCHITECTURE.md`, `docs/SUMMARY.md` + live docs referencing the lane
- Keep: `sdk/embed-js`, `sdk/react`, `sdk/angular` (consumer surface — NOT the authoring lane)

**Steps:**
- [ ] **Step 1:** Gate: confirm Tasks 1–7 merged AND the north-star fixture builds without the lane. ISSUE-RULES sweep for parallel deletions.
- [ ] **Step 2 (red):** Tombstone test: loading `project.ts` yields the named error (not a parse attempt).
- [ ] **Step 3:** Delete; rewire; scoped checks on `tau-cli`, `xtask`; mdbook build for the docs sweep.
- [ ] **Step 4:** Commit `refactor(cli)!: delete the legacy TS/Python authoring lane (ADR-0041 banner; replacement shipped)`.

### Task 9: Epic close-out

**Steps:**
- [ ] **Step 1:** DoD check: author a new tool via `#[tau::tool]` in the north-star fixture end-to-end — gen + check + card all see it; hash changes when its body changes.
- [ ] **Step 2:** Update the [authoring-surfaces tree](../implementation-trees/authoring-surfaces.md) (statuses, PR numbers, discoveries) and `vision-roadmap.md` E-1 stories.
