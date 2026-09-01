# E-2 — Flow Lane Implementation Plan (Phase 2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The choreography surface is real: `pipelines/*.ts` synthesizes through a sandboxed subprocess into `ProjectConfig`-shaped JSON, merges at the unchecked level, and lowers into the **v3 multi-pipeline IR** with working pipeline imports (`Compose`); `[steps]`/`[tools] native=` enter their deprecate-warn cycle; predicate algebra + structured template access land; `tau init --ts` is the golden path; the wasm feature registry stops denying the flagship target its shipped control flow.

**Architecture:** `cmd/project_load.rs` grows the synth dispatch hook: when `tau.toml` has `[synth]`, spawn the entry under `tau-sandbox-native` (reuse the `install_sandbox.rs` port pattern), read canonical JSON from stdout, deserialize as `UncheckedProjectConfig` overlay (strict, `synth_format`-gated per ADR-0065/0072), and merge exactly where `[dirs]` merges (`ProjectConfig::parse_str_at`) before the single `validate()`. `pipelines/` joins the `[dirs]` reserved-kinds scanning (`crates/tau-pkg/src/project/dirs/` — its reserved-kinds comment currently names `steps`; update it). `IrModule` moves to `pipelines: BTreeMap<PipelineId, Pipeline>` (v3.0.0, frozen v2 reader) through the 7-stage lowering (`crates/tau-ir-lower/src/lower/`); `SubflowKind::Compose` lowers mounted imports. The `@tau/sdk` L1 factories are generated from `schemas/project-manifest/` (E-1 Task 4).

**Tech Stack:** Rust; Node/tsx subprocess (runner overridable); `@tau/sdk` (new, thin, generated L1 + handle/predicate L2 per design §4); `cargo nextest`; schema freeze via `UPDATE_SCHEMA=1`.

**Design:** [`../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md`](../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md) §1, §3.4, §4, §12.
**ADRs:** [0072](../../decisions/0072-synth-contract.md) (contract) · [0073](../../decisions/0073-ir-v3-multi-pipeline.md) (IR v3) · [0071](../../decisions/0071-three-surface-split.md) (removals) · [0065](../../decisions/0065-unknown-input-policy.md) (strictness).
**Trees:** [`../implementation-trees/authoring-surfaces.md`](../implementation-trees/authoring-surfaces.md) · [`../implementation-trees/instruction-set.md`](../implementation-trees/instruction-set.md)

## Global Constraints

- Every cargo command: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p <crate> <filter>` (repo CARGO RULES; `timeout 180` + `cargo check`; never bare cargo; never workspace-wide).
- Commit with explicit identity: `git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "..."`.
- **IR versioning discipline (ADR-0073/0056):** schema files + `REACHABLE-TYPES.md` + conformance fixtures move together; the `UPDATE_SCHEMA=1` flow in `crates/tau-ir/tests/schema_export.rs`; v3.0.0 is the ONE major bump — everything else in this epic is carried inside it or is MINOR.
- **One validation path:** synth output NEVER bypasses `validate()`; collisions with TOML facts are hard errors (ADR-0069 discipline). No parallel validation, ever.
- Source-mapped synth errors (file:line) are in-scope acceptance (design §12), not polish.
- TOML-only projects must be untouched at every task boundary: no `[synth]` = no subprocess, no Node requirement, no new files.
- ISSUE RULES sweep before each task. **Task 10 note:** PR #687 (`feat/621-wasm-guest-flip`) already flips `any-wasi-strict` features — check its state first; if merged, Task 10 reduces to verification.

---

### Task 1: `[synth]` table in `tau.toml`

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs` (`UncheckedSynth { entry, format, runner }`, validated `SynthEntry`; field list is locked in ADR-0072 — anything more is a format bump)
- Test: inline `mod tests` (house convention)

**Steps:**
- [ ] **Step 1 (red):** Parse/validate tests: valid table round-trips; unknown key rejected (`deny_unknown_fields`); missing `format` rejected; `entry` path validated with the `[dirs]` path-hygiene rules (relative, contained).
- [ ] **Step 2:** Run red → implement → green (`timeout 300 ... cargo nextest run -p tau-pkg synth`).
- [ ] **Step 3:** Commit `feat(pkg): [synth] table (ADR-0072 field set)`.

### Task 2: The synth subprocess runner + sandbox

**Files:**
- Create: `crates/tau-pkg/src/project/synth.rs` (spawn, collect stdout/stderr, timeout, exit-code mapping)
- Modify: `crates/tau-pkg/src/install_sandbox.rs` neighborhood — reuse the sandbox port for the synth profile (no network, project-root read-only)
- Modify: `crates/tau-cli/src/cmd/project_load.rs` (the dispatch hook: `[synth]` present → run synth → overlay)
- Test: fixture synth programs (a `printf` shell fixture keeps tests Node-free; a real tsx fixture behind an ignored-unless-Node marker per the test-ignores inventory convention)

**Interfaces:**
- Produces: `run_synth(&SynthEntry, root) -> Result<UncheckedProjectConfig, SynthError>`; `SynthError::{Spawn, Timeout, NonZeroExit{code, stderr_tail}, InvalidJson{source_mapped}, FormatMismatch{declared, supported}}`.
- Consumes: Task 1.

**Steps:**
- [ ] **Step 1 (red):** Tests: fixture emitting valid JSON → parsed overlay; junk stdout → `InvalidJson`; nonzero exit → error carrying stderr tail; unknown field in JSON → strict rejection (ADR-0065 authored rule); network attempt in the sandboxed fixture → denied (behind the sandbox-capable test gate, mirroring install-sandbox tests).
- [ ] **Step 2:** Run red → implement → green on `tau-pkg` + `tau-cli`.
- [ ] **Step 3:** Commit `feat(pkg,cli): sandboxed synth subprocess emitting ProjectConfig JSON`.

### Task 3: Merge at the unchecked level + collision rules

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs` (`parse_str_at` merge point — synth overlay joins where `[dirs]` definitions join)
- Test: inline + a `tau check` integration fixture

**Steps:**
- [ ] **Step 1 (red):** Tests: synth-declared pipeline + TOML vocabulary → one validated config; synth emitting an agent (vocabulary — ADR-0071 violation) → hard error naming the surface rule; synth emitting `[allow]` → hard error (never emittable); id collision synth-vs-TOML → hard error (never override, never auto-suffix).
- [ ] **Step 2:** Run red → implement → green.
- [ ] **Step 3:** Commit `feat(pkg): synth overlay merges before the single validate(); surface violations are hard errors`.

### Task 4: `pipelines/` dir scanning

**Files:**
- Modify: `crates/tau-pkg/src/project/dirs/` (add the `pipelines` root kind; **update the reserved-kinds comment that names `steps`**), `project.rs` (`[dirs] pipelines = "pipelines"` or implicit-by-presence per ADR-0069 conventions)
- Test: dirs scan tests (id = file path, `/` separator, hygiene rules from the dirs plan)

**Steps:**
- [ ] **Step 1 (red):** Scan fixture: `pipelines/deploy.ts` + `pipelines/ops/rotate.ts` → pipeline ids `deploy`, `ops/rotate` (ADR-0070 grammar); junk/hidden files ignored; non-`.ts` rejected with a named error.
- [ ] **Step 2:** Wire scanning to feed the synth entry set (each file = one pipeline module compiled by the synth program; per-file error isolation per design §7).
- [ ] **Step 3:** Commit `feat(pkg): pipelines/ scanning — one file = one pipeline, id = path`.

### Task 5: IR v3 — `pipelines: BTreeMap<PipelineId, Pipeline>` (THE major bump)

**Files:**
- Modify: `crates/tau-ir/src/{module,pipeline}.rs` (the map; entry-pipeline accessor replacing `entry_agent()` call sites), `check.rs` (typecheck over every pipeline), `crates/tau-ir/tests/schema_export.rs` fixtures + `schemas/ir/` + `REACHABLE-TYPES.md`
- Create: frozen v2 reader (`crates/tau-ir/src/compat_v2.rs` or the existing load-gate module per ADR-0064/D8-B pattern — locate via `rg -n "ir_format" crates/tau-ir/src`)
- Modify: `crates/tau-ir-lower/src/lower/` (7 stages emit the map; single `[pipeline]` → one-entry map), `tau-runtime-core` interpreter entry (`interpreter/pipeline.rs`), wasm guest `run_pipeline` path (ADR-0068), `tau-ir-conformance` fixtures

**Steps:**
- [ ] **Step 1 (red):** Conformance: a v2 bundle fixture loads through the frozen reader byte-for-byte into the degenerate map; a two-pipeline project lowers, typechecks, and runs each pipeline by id; `tau run` with two pipelines and no `--pipeline` selector → named ambiguity error (mirrors today's exactly-one-agent rule).
- [ ] **Step 2:** Implement across `tau-ir` → `tau-ir-lower` → `tau-runtime-core` → guest, in that order, keeping each crate's suite green before the next (`timeout 300 ... cargo nextest run -p <crate>`), schema last via `UPDATE_SCHEMA=1`.
- [ ] **Step 3:** Commit `feat(ir)!: ir_format v3.0.0 — multi-pipeline modules (frozen v2 reader)` — CHANGELOG + contract-compatibility doc entry (ADR-0056).

### Task 6: Pipeline imports → `SubflowKind::Compose`

**Files:**
- Modify: `crates/tau-ir/src/subflow.rs` + `error.rs` (retire `UnsupportedComposeSubflow`), `crates/tau-ir-lower/src/lower/` (mount + namespace under call-site id, ADR-0070 lineage), cycle detection at synth/validate (`tau-pkg`)
- Test: conformance fixture (pipeline A composes B; nested ids namespaced; a cycle fixture errors at validate, never lowers)

**Steps:**
- [ ] **Step 1 (red):** Fixtures above; capability assertion: composed steps carry the SAME project `[allow]` bounds (no attenuation, no widening — ADR-0073 rule 4c).
- [ ] **Step 2:** Implement; green across `tau-ir`, `tau-ir-lower`, `tau-runtime-core`, conformance.
- [ ] **Step 3:** Commit `feat(ir,lower): pipeline imports mount as Compose subflows (acyclic, namespaced)`.

### Task 7: `@tau/sdk` L2 — handles, predicates, fragments, source maps

**Files:**
- Create: `sdk/tau-sdk/` (new package, emitted-or-authored per the L1-generated/L2-authored split; L1 factories generated from `schemas/project-manifest/` — extend `crates/tau-sdk-codegen` with a `sdk_ts.rs` emitter + drift test)
- Test: vitest suite (house convention from `sdk/embed-js`); Rust-side: synth-integration fixtures using the SDK

**Interfaces (audit-locked, design §4 — violations are synth errors):**
- Typed non-coercible handles (no `toString`, no truthiness; interpolation only via the tagged template); predicate vocabulary as handle methods + `.and/.or/.not` (never lambdas); loop callback returns a `Predicate` (plain boolean = synth error); explicit ids, collision = error (never auto-suffix); `defineFragment` `(scope, id, props)`; statement order ≠ execution order documented + unconsumed-step lint; synth errors carry file:line.

**Steps:**
- [ ] **Step 1 (red):** Vitest: handle coercion throws; boolean-returning loop callback → synth error with file:line; id collision → error naming both sites; fragment mounts under its scope id.
- [ ] **Step 2:** Implement L1 generation + L2; green; the north-star-v2 fixture authors in `pipelines/` and produces config passing Task 3's merge.
- [ ] **Step 3:** Commit `feat(sdk): @tau/sdk — generated L1 + handle/predicate L2 (design §4 contract)`.

### Task 8: Predicate algebra + structured template access (TOML twin)

**Files:**
- Modify: `crates/tau-ir/src/check.rs` / the goal-predicate model (`crates/tau-native-tools/src/goal_predicates.rs`, `rg -n "OnFail" crates/tau-ir`), template parsing (`${steps.x.output.field}` — JSON-pointer read only; transformations stay deterministic fns), `tau-ir-lower` typecheck (nested template validation), `tau-runtime-core` evaluation
- Test: conformance fixtures both surfaces (TS predicate ↔ TOML predicate → byte-equal IR where applicable — the epic DoD's "TOML twin")

**Steps:**
- [ ] **Step 1 (red):** Fixtures: numeric compare + and/or/not + two-locus predicates evaluate; structured access reads a field; a *transforming* expression is rejected at validate (pointer-read-only rule); scalar-coercion footgun case from design §3.4 (rendered arg keeps its JSON type) fixed and locked by test.
- [ ] **Step 2:** Implement; MINOR ir_format bump per ADR-0073 discipline (schema+fixtures together).
- [ ] **Step 3:** Commit `feat(ir): predicate algebra + JSON-pointer template access (minor bump)`.

### Task 9: `[steps]` / `[tools] native=` deprecate-warn cycle + `tau init --ts`

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs` (parse-time deprecation warning naming ADR-0071 + the migration; removal PR lands next release cycle per no-flag-day), `crates/tau-cli/src/cmd/init.rs` (`--ts` scaffolds `tau.toml` + `[synth]` + `pipelines/hello.ts` + `tau.gen.ts` regen note)
- Test: warning presence test; init golden-file test

**Steps:**
- [ ] **Step 1 (red):** Fixture with `[steps]` → build succeeds WITH the warning (once per load, not per step); `tau init --ts` scaffold builds end-to-end in a temp dir (the golden path — north-star-v2's skeleton).
- [ ] **Step 2:** Implement; green; commit `feat(cli): tau init --ts golden path; [steps]/native= deprecation warnings`.
- [ ] **Step 3:** File the removal follow-up (next cycle) in `vision-roadmap.md` E-2 notes — removal itself is NOT this task.

### Task 10: Wasm feature-registry repair

**Files:**
- Modify: `crates/tau-ports/src/target/registry.rs:136-139` — `any-wasi-strict` declares the control-flow features the guest actually links (`run_pipeline`, ADR-0068), plus the v3 features from Tasks 5–6; stale comment ("Wasm guests cannot execute control-flow steps") deleted
- Test: D8 feature-set honesty test (`rg -n "supported_features" crates/tau-ports/src/target` for the existing gate)

**Steps:**
- [ ] **Step 1:** Check PR #687 (`feat/621-wasm-guest-flip`) — if merged, verify its flip covers Branch/Parallel/Loop AND extend for v3/Compose only; if open, coordinate (ISSUE RULES) rather than duplicate.
- [ ] **Step 2 (red→green):** Conformance: the north-star fixture builds for `any-wasi-strict` and executes control flow in-guest; features absent from the guest still reject at load (honesty preserved).
- [ ] **Step 3:** Commit `fix(ports): any-wasi-strict declares its real feature set (design §3.4 repair)`.

### Task 11: CI double-synth + epic close-out

**Files:**
- Modify: `.github/workflows/` (the double-synth byte-identity job: run synth twice, `cmp` outputs; hermeticity gate per ADR-0072)

**Steps:**
- [ ] **Step 1:** Add the job on the north-star-v2 fixture; red if a fixture synth is nondeterministic (fix the fixture, not the check).
- [ ] **Step 2:** DoD: north-star-v2 authors + builds via `pipelines/`; TOML twin byte-equal where applicable (Task 8 fixtures); original north-star still green (legacy witness).
- [ ] **Step 3:** Update [authoring-surfaces](../implementation-trees/authoring-surfaces.md) + [instruction-set](../implementation-trees/instruction-set.md) trees + `vision-roadmap.md` E-2 stories.
