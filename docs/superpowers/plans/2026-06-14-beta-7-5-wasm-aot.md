# β.7.5 — IR-to-wasm AOT compiler Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `tau build wasm <project>` — an ahead-of-time compiler that lowers a tau project's workflow IR + `tau-runtime-core` + its native tools into a single runnable WASI 0.2 wasm component that reproduces `tau dev`'s observable side effects in wasmtime.

**Architecture:** Approach B (fully-linked guest). A new `no_std` guest crate (`tau-wasm-guest`) links `tau-runtime-core` + `tau-ir` + `tau-native-tools` + `tau-mcp` (facilitator + cassette) + the project's IR (baked at build time via `include_bytes!`), drives `run_ir` with a single-threaded `block_on`, records a `ConformanceReport`, and exports `run(prompt)`. A std host crate (`tau-wasm-host`) embeds wasmtime, satisfies 3 WIT imports (`LlmBackend`/`Clock`/`RandomSource`), and runs the component. `WasmMode` joins `DevMode`/`BundleMode` in `tau-ir-conformance` and `assert_conform` proves dev↔wasm parity. Spec: `docs/superpowers/specs/2026-06-14-beta-7-5-wasm-aot-design.md`.

**Tech Stack:** Rust `wasm32-wasip2` (stable Tier-2, ≥1.82), `wit-bindgen` (`no_std`), `wasmtime` (component model, `bindgen!` + `Linker`), `pollster`/custom single-threaded executor, `dlmalloc` global allocator.

---

## Plan structure & honesty boundary

This sub-project is **toolchain-discovery-heavy** (foreign-target compilation, novel `wit-bindgen`/`wasmtime`-component integration, first compile of `tau-mcp` to wasm). Writing confident bite-sized TDD steps for PR-3..6 *before* PR-2 verifies the toolchain behaves would be speculation.

Therefore:
- **Phase 1 (PR-1)** is written at **full bite-sized TDD granularity** — pure in-repo Rust following existing patterns; execute as-is.
- **Phases 2–6 (PR-2..6)** are **detailed structured task outlines** — files, interfaces, test intent, exact commands, known gotchas — each ending with an `EXPAND` marker. At the start of each phase (after the prior phase verified its toolchain assumptions), expand that phase's tasks to full bite-sized steps using the verified reality. subagent-driven-development naturally does READ-context-first per task, so this fits the execution model.

Do NOT batch-implement Phases 2–6 from the outlines alone. Expand each phase first.

---

## Files map

### Create
- `crates/tau-wasm-guest/Cargo.toml` — `no_std` component crate.
- `crates/tau-wasm-guest/src/lib.rs` — `run` export, `block_on(run_ir)`, recording dispatcher, baked-IR wiring, allocator/panic glue.
- `crates/tau-wasm-host/Cargo.toml` — std wasmtime embedder.
- `crates/tau-wasm-host/src/lib.rs` — `bindgen!` host, `Linker`, determinism `Config`, `run_component(wasm_bytes, prompt, imports) -> ConformanceReport`.
- `crates/tau-native-tools/Cargo.toml` — `no_std` shared deterministic tools.
- `crates/tau-native-tools/src/lib.rs` — `read_temp`, `set_fan` + a registry-shaped accessor.
- `wit/tau-run.wit` — the `tau:run` world (host imports + `run` export).
- `crates/tau-cli/src/cmd/build_wasm.rs` — `tau build wasm` driver (lower → bake → cargo → emit).
- `docs/decisions/0046-wasm-aot-artifact.md`
- `docs/decisions/0047-in-wasm-mcp-facilitator.md`

### Modify
- `crates/tau-ports/src/target/registry.rs` — add `any-wasi-strict` Available entry + `fs_rw_net` shapes fn.
- `crates/tau-cli/src/cli.rs` — `tau build wasm` subcommand surface.
- `crates/tau-cli/src/lib.rs` — dispatch the new subcommand.
- `crates/tau-cli/src/cmd/mod.rs` — register `build_wasm`.
- `crates/tau-ir-conformance/src/lib.rs` + new `src/wasm_mode.rs` — `WasmMode`.
- `crates/tau-ir-conformance/tests/conformance.rs` — wasm conformance test(s).
- `Cargo.toml` (workspace) — add 3 members.
- `ROADMAP.md` — mark β.7.5 progress.
- `docs/decisions/0040-tau-dev-repl.md` — fix the stale "ADR-0041 forthcoming" reference (β.7.5's ADRs are 0046/0047).
- `docs/SUMMARY.md` — only if ADRs/pages are book-published (verify; specs/plans are not).
- `.github/workflows/*` — `conformance (wasm)` lane (Phase 6).

---

## Standing constraints (CLAUDE.md — NON-NEGOTIABLE)

1. **CARGO RULES.** Every cargo invocation: `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo <cmd> -p <crate>`. Subagents use `target/agent-<role>`; main agent `target/main`. Never bare `cargo`, never `--workspace`, never omit `-p`.
2. **Wasm builds need the target installed:** `rustup target add wasm32-wasip2` (once per machine + in CI). The guest build is a *foreign-target* cargo invocation — give it its **own** `CARGO_TARGET_DIR` (e.g. `target/agent-wasm-guest`) so it never contends with host-target builds on the lock (Rule 5).
3. **Git identity:** commit with `git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com"`. Docs/YAML-only commits may use `--no-verify`; Rust changes go through CI as the gate (deep-gate is opt-in).
4. **Branch:** `feat/beta-7-5-wasm-aot`. PR per phase to `main`. `gh pr merge <N> --squash --delete-branch --auto`; `gh pr update-branch <N>` when BEHIND.
5. **Conventional commits**, imperative, scoped (`feat(wasm)`, `feat(cli)`, `docs(adr)`).

---

## Implementer-adapt points (verify, don't assume)

These are the discovery points. Resolve each by reading/trying, not guessing:

1. **`tau build wasm` clap shape.** Recommended: convert `Build(BuildArgs)` into a subcommand group with `bundle` (current behavior, the default when no sub given) + `wasm(BuildWasmArgs)`, OR add `tau build wasm` as a distinct nested subcommand while leaving bare `tau build` = bundle. Constraint: `tau build <project>` (bundle) MUST keep parsing and passing its existing tests. Pin both with parse tests in Phase 1.
2. **`wit-bindgen` macro + generated bindings shape** (Phase 2) — the exact `generate!` invocation, the generated import trait names, and whether `no_std` mode needs `generate!({ ... runtime_path, ... })` tweaks. Verify against the installed `wit-bindgen` version; do not hardcode symbol names from this plan.
3. **`wasmtime::component::bindgen!` host API** (Phase 3) — `HasSelf<_>`, `add_to_linker`, `instantiate_async` vs sync, `ResourceTable`. Verify against the installed wasmtime version.
4. **`tau-mcp` → `wasm32-wasip2` cleanliness** (Phase 5) — smoke-compile `tau-mcp` for the wasm target before wiring it into the guest; fix any std-leaning corners there first.
5. **`LlmBackend`/`CompletionRequest`/`CompletionResponse` serde at the WIT boundary** — confirm `tau_ports::llm` types serialize under `no_std` serde; the guest sends/receives JSON strings.
6. **`tau-native-tools` registration surface** — match how fixture `01_agent_native_tool` wires `read_temp`/`set_fan` today (canned `{"ok":true}` via `RecordingDispatcher`); the new lib must slot into both the dev dispatcher and the guest. Decide `ToolDispatcher` vs `DeterministicRegistry` shape by reading fixture 01's `dev_mode.rs` wiring.
7. **PR-6 test speed** — bake-per-fixture (true AOT, slow cargo build per test) vs prebuilt-guest-loads-IR-at-runtime (fast). The DoD `tau build wasm` path bakes regardless; the *conformance test harness* may take the fast route. Decide when writing Phase 6.

---

## Phase 1 — `any-wasi-strict` triple + `tau build wasm` CLI skeleton + ADR-0046/0040 (PR-1)

**Outcome:** A mergeable PR that graduates the wasi triple to Available, makes `tau build wasm <project>` parse and emit a clear "not yet implemented" error, fixes the ADR-0040 reference, and lands the ADR-0046 skeleton. No wasm compilation yet. Unblocks naming for all later phases.

### Task 1.1 — Add the `any-wasi-strict` registry entry + `fs_rw_net` shapes

**Files:**
- Modify: `crates/tau-ports/src/target/registry.rs`
- Test: same file's `#[cfg(test)] mod tests`

- [ ] **Step 1: READ context first.** Read `crates/tau-ports/src/target/registry.rs` (the `REGISTRY` static, the `fs_rw_exec_net`/`all_shapes` helper fns, and the existing `#[cfg(test)] mod tests`), `crates/tau-ports/src/target/platform.rs` (confirm `Platform::Any` + `as_str()=="any"`), `crates/tau-ports/src/target/adapter_family.rs` (confirm `AdapterFamily::Wasi` + `"wasi"`), and `crates/tau-domain/src/package/capability.rs:250-320` (`CapabilityShape` variants + `CapabilityShapeSet::{new,insert,contains}`).

- [ ] **Step 2: Write the failing test.** Add to `registry.rs` tests:

```rust
#[test]
fn any_wasi_strict_is_available_with_fs_rw_net_shapes() {
    let t: TargetTriple = "any-wasi-strict".parse().unwrap();
    let e = lookup(&t).expect("any-wasi-strict must be registered");
    assert!(matches!(e.status, TripleStatus::Available));
    let shapes = (e.shapes_fn)();
    assert!(shapes.contains(&CapabilityShape::FilesystemRead));
    assert!(shapes.contains(&CapabilityShape::FilesystemWrite));
    assert!(shapes.contains(&CapabilityShape::NetworkHttp));
    // wasm cannot subprocess or spawn OS agents:
    assert!(!shapes.contains(&CapabilityShape::ProcessExec));
    assert!(!shapes.contains(&CapabilityShape::AgentSpawn));
}
```

- [ ] **Step 3: Run it — expect FAIL.**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ports any_wasi_strict_is_available -- --nocapture`
Expected: FAIL — `lookup` returns `None` (panic on `.expect`).

- [ ] **Step 4: Implement.** Add the shapes fn (next to `fs_rw_exec_net`) and the registry entry (before the Windows Reserved entry):

```rust
fn fs_rw_net() -> CapabilityShapeSet {
    let mut s = CapabilityShapeSet::new();
    s.insert(CapabilityShape::FilesystemRead);
    s.insert(CapabilityShape::FilesystemWrite);
    s.insert(CapabilityShape::NetworkHttp);
    s
}
```

```rust
    TargetTripleEntry {
        triple: TargetTriple {
            platform: Platform::Any,
            adapter_family: AdapterFamily::Wasi,
            tier: CapabilityTier::Strict,
        },
        shapes_fn: fs_rw_net,
        status: TripleStatus::Available,
    },
```

- [ ] **Step 5: Run — expect PASS.** Same command as Step 3 → PASS. Also run the full crate suite: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ports`. Expect the `list_available_excludes_reserved` test still green (the new entry is Available — fine).

- [ ] **Step 6: Commit.**

```bash
git add crates/tau-ports/src/target/registry.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(target): graduate any-wasi-strict triple to Available"
```

### Task 1.2 — `tau target list` / `tau check --target` surface the new triple

**Files:**
- Modify: (none if `tau target list` enumerates `list_available()` dynamically — verify)
- Test: `crates/tau-cli/tests/` target/check snapshot or assertion test

- [ ] **Step 1: READ.** `git grep -n "list_available\|list_all" crates/tau-cli/src` to find where `tau target list` and `tau check --target` enumerate triples. Confirm they iterate the registry (so the new entry appears automatically) vs. a hardcoded list.

- [ ] **Step 2: Write/adjust test.** If there's a snapshot test for `tau target list`, add `any-wasi-strict` to the expected output. If assertion-style, add:

```rust
#[test]
fn target_list_includes_any_wasi_strict() {
    // invoke the list command surface and assert the triple string is present
    // (mirror the existing target-list test in this file)
}
```

- [ ] **Step 3: Run — expect FAIL** (snapshot mismatch or missing string).
Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli target_list`

- [ ] **Step 4: Implement.** If dynamic, just `cargo insta accept` / update the snapshot literal. If hardcoded, add the entry.

- [ ] **Step 5: Run — expect PASS.**

- [ ] **Step 6: Commit.**

```bash
git add crates/tau-cli
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "test(cli): any-wasi-strict appears in target list"
```

### Task 1.3 — `tau build wasm <project>` parses; bundle path unchanged

**Files:**
- Modify: `crates/tau-cli/src/cli.rs`, `crates/tau-cli/src/lib.rs`, `crates/tau-cli/src/cmd/mod.rs`
- Create: `crates/tau-cli/src/cmd/build_wasm.rs`
- Test: `crates/tau-cli/src/cli.rs` `#[cfg(test)]` parse tests

- [ ] **Step 1: READ.** `crates/tau-cli/src/cli.rs:186-233` (`Build`/`BuildArgs`), the `#[cfg(test)]` parse tests near line 990, `crates/tau-cli/src/lib.rs:201` (build dispatch), `crates/tau-cli/src/cmd/mod.rs` (module list), and `crates/tau-cli/src/cmd/mcp/mod.rs` (the `#[command(subcommand)]` sibling pattern). Decide the clap shape per Implementer-adapt point #1 (recommended: `tau build wasm` nested subcommand, bare `tau build` stays bundle).

- [ ] **Step 2: Write failing parse tests** (in `cli.rs` tests):

```rust
#[test]
fn build_wasm_subcommand_parses_with_project() {
    let cli = Cli::try_parse_from(["tau", "build", "wasm", "examples/fan-monitor"]).unwrap();
    // assert the parsed variant is the wasm build with project = examples/fan-monitor
}

#[test]
fn bare_build_still_parses_as_bundle() {
    let cli = Cli::try_parse_from(["tau", "build", "examples/fan-monitor"]).unwrap();
    // assert it's the existing bundle BuildArgs path
}
```

- [ ] **Step 3: Run — expect FAIL** (`wasm` unknown subcommand).
Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli build_wasm_subcommand bare_build_still`

- [ ] **Step 4: Implement.** Add `BuildWasmArgs { project: Option<PathBuf>, output: Option<PathBuf> (-o) }` and wire the subcommand per the chosen shape. Create `cmd/build_wasm.rs`:

```rust
//! `tau build wasm <project>` — IR-to-wasm AOT compiler (β.7.5).
//! Phase 1: surface only. Lowering/baking/cargo wiring lands in Phase 4.
use crate::cli::BuildWasmArgs;
use crate::output::Output;

pub async fn run(_args: &BuildWasmArgs, output: &mut Output) -> anyhow::Result<()> {
    // Mirror the error-render style used elsewhere in tau-cli.
    anyhow::bail!("`tau build wasm` is not yet implemented (β.7.5 Phase 4)")
}
```

Register in `cmd/mod.rs` (`pub mod build_wasm;`) and dispatch in `lib.rs`.

- [ ] **Step 5: Run — expect PASS** on both parse tests.

- [ ] **Step 6: Commit.**

```bash
git add crates/tau-cli
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(cli): tau build wasm subcommand skeleton (not-yet-implemented)"
```

### Task 1.4 — ADR-0046 skeleton + fix ADR-0040 reference + ROADMAP

**Files:**
- Create: `docs/decisions/0046-wasm-aot-artifact.md`
- Modify: `docs/decisions/0040-tau-dev-repl.md`, `ROADMAP.md`

- [ ] **Step 1: Write ADR-0046** (status Proposed) capturing the spec's §14 ADR-0046 content: Approach B, `wasm32-wasip2` + `wit-bindgen`, the `tau:run` world, the 3-host-import boundary, guest `block_on` + p2/p3 hedge, IR baking + cargo hand-off, determinism `Config`, observable = `ConformanceReport` with the `RunEvent`-deferred-to-β.6 note. Reference the spec path. (ADR-0047 lands in Phase 5 when the in-wasm MCP facilitator is actually built.)

- [ ] **Step 2: Fix ADR-0040.** In `docs/decisions/0040-tau-dev-repl.md`, change the stale "β.7.5 (separate, ADR-0041 forthcoming)" to "β.7.5 (separate, ADR-0046 + ADR-0047)".

- [ ] **Step 3: ROADMAP.** Under §β.7.5, add an implementation-status line: "In progress (2026-06-14): spec + plan landed; ADR-0046/0047."

- [ ] **Step 4: Build the book** (ADRs are book-published — verify in `docs/SUMMARY.md`, add entries if needed):

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build` → only `[INFO]` lines; then `rm -rf docs/book`.

- [ ] **Step 5: Commit.**

```bash
git add docs ROADMAP.md
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "docs(adr): ADR-0046 wasm AOT skeleton; fix ADR-0040 ref; ROADMAP β.7.5"
```

### Task 1.5 — Push, PR, validate

- [ ] **Step 1: Workspace validation** (the crates touched):

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ports -p tau-cli`
Run: `timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-ports -p tau-cli -- --check`
Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-ports -p tau-cli`

- [ ] **Step 2: Push + PR.**

```bash
git push -u origin feat/beta-7-5-wasm-aot
gh pr create --base main --title "feat(β.7.5): wasi triple + tau build wasm skeleton (PR-1)" \
  --body "Phase 1 of β.7.5. Graduates any-wasi-strict; tau build wasm parses (NYI); ADR-0046 skeleton. Spec: docs/superpowers/specs/2026-06-14-beta-7-5-wasm-aot-design.md"
gh pr merge <N> --squash --delete-branch --auto
```

---

## Phase 2 — WIT world + `tau-wasm-guest` scaffold builds to `wasm32-wasip2` (PR-2)

**Outcome:** A `no_std` guest crate that compiles to a WASI 0.2 component with stub host imports and a hardcoded trivial IR, exports `run(prompt)`, and a CI step that builds it in the exact release profile (catching the `_rdl_*` LTO bug early). **This is the #1 toolchain de-risk.**

**Key tasks (EXPAND to bite-sized before executing):**
1. `wit/tau-run.wit` — author the world from spec §5 (`host` interface: `complete`/`now-millis`/`next-u64`; `runner` world: import `host`, export `run`).
2. `crates/tau-native-tools` scaffold (empty deterministic lib, builds for both host + `wasm32-wasip2`).
3. `crates/tau-wasm-guest/Cargo.toml` — `[lib] crate-type=["cdylib"]`; deps `tau-runtime-core`/`tau-ir`/`wit-bindgen` (no_std); `dlmalloc` global allocator; `[profile.release] panic="abort"`.
4. `src/lib.rs` — `#![no_std]` + `extern crate alloc`; `wit-bindgen::generate!` (adapt #2); `#[global_allocator]`, `#[panic_handler]`, `cabi_realloc` (or rely on the wasip2 stdlib weak symbol — verify); `run` export returns a hardcoded `Ok("{}")` first, then `block_on` over a trivial `run_ir` of a hardcoded IR.
5. Single-threaded executor seam (`pollster` or ~30-line custom waker) isolated in `src/executor.rs` (p3-upgrade boundary).
6. **Verification commands:** `rustup target add wasm32-wasip2`; `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-wasm-guest cargo build -p tau-wasm-guest --target wasm32-wasip2 --release`; assert a `.wasm` is produced and is a component (`wasm-tools component wit target/.../tau_wasm_guest.wasm` lists the `tau:run` world).
7. CI: add a build-only step for the guest in the exact release profile.

**Risks surfaced here:** `_rdl_*` LTO symbol bug (risk #2) — test `lto=true` early; `std` creeping via deps (use raw `wit-bindgen`, not the `wasi` crate).

**EXPAND** this phase using the verified `wit-bindgen` version's macro shape before writing steps. Commit per task; PR-2 to main.

---

## Phase 3 — `tau-wasm-host` wasmtime round-trip (PR-3)

**Outcome:** A std host crate that loads the Phase-2 guest, satisfies the 3 WIT imports with deterministic stubs, applies the determinism `Config`, calls `run`, and gets a value back. Proves host↔guest.

**Key tasks (EXPAND before executing):**
1. `crates/tau-wasm-host/Cargo.toml` — `wasmtime` (component feature), `tau-ports`, `serde_json`.
2. `src/lib.rs` — `wasmtime::component::bindgen!` over `wit/tau-run.wit` (adapt #3); a `Host` store-data struct implementing the generated `host` import trait: `complete` (returns a caller-supplied canned `CompletionResponse` JSON), `now_millis` (deterministic counter), `next_u64` (deterministic seeded PRNG).
3. `Config`: `cranelift_nan_canonicalization(true)`, `wasm_relaxed_simd(false)` (or deterministic), memory limiter.
4. `run_component(wasm_bytes: &[u8], prompt: &str, llm_responses: Vec<String>) -> Result<String>` — instantiate, call `run`, return the JSON string.
5. **Test:** load the Phase-2 guest (built as a fixture or via `include_bytes!` of a CI-built artifact), call `run_component`, assert it returns the hardcoded `Ok` value. Mark `#[ignore]` if it depends on a CI-built guest artifact not present in the dev tree; document the build step.
6. Determinism unit test: two `run_component` calls produce byte-identical output.

**EXPAND** using the verified wasmtime version's `bindgen!`/`Linker`/`instantiate(_async)` API. PR-3 to main.

---

## Phase 4 — IR baking: `tau build wasm` emits the real `.wasm` (PR-4)

**Outcome:** `tau build wasm <project>` lowers the project IR, bakes the canonical bytes into the guest, shells cargo for `wasm32-wasip2`, and emits a `.wasm` whose baked IR drives `run_ir`.

**Key tasks (EXPAND before executing):**
1. `cmd/build_wasm.rs` — replace the NYI stub: load+validate project (reuse `tau build` front-end), `lower_project(config, "any-wasi-strict".parse()?, caches)`, `to_canonical_bytes`, write to a build-scratch file.
2. Guest baked-IR wiring: guest reads the bytes via `include_bytes!(env!("TAU_IR_BYTES_PATH"))` or a generated `ir_bytes.rs`; decode with `tau_ir::from_canonical_bytes` at guest startup; pick entry agent (first BTreeMap key — match `bundle_mode.rs` convention).
3. Shell the guest build with CARGO hygiene + dedicated `CARGO_TARGET_DIR` (adapt: pass the IR path via env to the cargo invocation). Capability-fit refusal (ProcessExec/AgentSpawn) surfaces as a build error (Rust-like build-time enforcement — spec §9 step 2).
4. Emit artifact to `-o`/default; print its sha256 (reuse `tau-cli` `sha256_hex`).
5. **Tests:** `tau build wasm` on a minimal fixture produces a `.wasm`; a project requiring `process.exec` is **refused at build** with a capability-fit error; reproducibility (same project → identical IR bytes baked, mirroring the C3 contract).

**EXPAND** once the guest's baked-IR mechanism from Phase 2 is concrete. PR-4 to main.

---

## Phase 5 — Fan-monitor in-guest + ADR-0047 (PR-5)

**Outcome:** The full simplified fan-monitor runs inside the guest: shared `tau-native-tools` (used by dev too), cassette `LlmBackend` host import wired, in-guest MCP facilitator + cassette replay, context manager. ADR-0047 lands.

**Key tasks (EXPAND before executing):**
1. **Smoke-compile `tau-mcp` for `wasm32-wasip2`** (adapt #4) — fix any std-leaning corners FIRST: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-wasm-mcp cargo build -p tau-mcp --target wasm32-wasip2 --no-default-features`.
2. `tau-native-tools`: implement `read_temp`→`{"celsius":32}`, `set_fan(args)`→records `{"on":bool}` (deterministic). Expose a registry accessor usable by both the guest dispatcher and the dev path. Refactor fixture `01`/`07`'s dev wiring to use these real impls (adapt #6) so dev and wasm run identical tool code.
3. Wire the cassette `LlmBackend` as the `complete` host import in `tau-wasm-host` (replay `mock_llm.jsonl`-shaped responses).
4. In-guest MCP: link `tau-mcp` facilitator + cassette replayer into the guest; the weather server replays from a baked cassette (zero host import).
5. Context manager already compiles in (β.4) — confirm it runs in-guest via fixture 13's config.
6. **ADR-0047** — in-wasm MCP facilitator: facilitator in-guest, cassette in-guest, real transport host-import reserved for γ.1, the reserved `tau:mcp` WIT slot (spec §14).
7. **Test:** drive the guest with the simplified fan-monitor IR + cassette; assert `set_fan` was called with `{"on":true}` (the scenario's expected effect).

**EXPAND** after Phase 4. PR-5 to main.

---

## Phase 6 — `WasmMode` conformance + CI lane + docs (PR-6)

**Outcome:** `WasmMode` joins `DevMode`/`BundleMode`; the simplified scenario is gated dev↔wasm; fixtures 07 + 13 pass under wasm; CI lane added; ADRs finalized; mdBook pages.

**Key tasks (EXPAND before executing):**
1. `crates/tau-ir-conformance/src/wasm_mode.rs` — `WasmMode` impl of `ExecutionMode`: build (or load prebuilt — adapt #7) the guest with the fixture IR, run via `tau-wasm-host`, return a `ConformanceReport`. The guest must record tool calls/messages into the same `ConformanceReport` shape (port the `RecordingDispatcher` logic into the guest, or have the guest return raw records the host folds into a report).
2. `tests/conformance.rs` — add `assert_conform(dev, wasm)` for the simplified fan-monitor fixture (the DoD gate), plus `07_mcp_weather_cassette` and `13_context_pipeline`.
3. Determinism guard: a byte-equal parity fixture comparing dev vs wasm reports (risk #3).
4. CI: `conformance (wasm)` lane (needs `wasm32-wasip2` + wasmtime). Per ADR-0039 tiers, Tier-1 runs the simplified gate; broader fixture sweep may be nightly/label if build time bites — `log()`/document any coverage dropped from Tier-1.
5. Finalize ADR-0046/0047 status → Accepted; mark β.7.5 done in ROADMAP; 1–2 mdBook pages (how-to: `tau build wasm`; explanation: the two-profile wasm path). Add to `docs/SUMMARY.md`; `mdbook build` clean.
6. **DoD verification:** `tau build wasm <simplified-fan-monitor>` produces a `.wasm`; running it in wasmtime yields a `ConformanceReport` equal to `tau dev`'s; every existing test green.

**EXPAND** after Phase 5. PR-6 to main — closes β.7.5.

---

## Self-review (against spec)

- **Spec coverage:** D1 Approach B → guest links tools (Ph2/5). D2 ConformanceReport observable → Ph6 `WasmMode` + `assert_conform`. D3 in-guest MCP + cassette → Ph5 + ADR-0047. D4 3 imports → Ph2 WIT + Ph3 host. D5 build pipeline → Ph4. D6 triple → Ph1. D7 toolchain → Ph2. Determinism §7 → Ph3 Config + Ph6 guard. Async §8 → Ph2 executor seam. ADRs → Ph1 (0046 skel), Ph5 (0047), Ph6 (finalize). DoD §11 → Ph6. ✓ all sections mapped.
- **Placeholder scan:** Phase 1 is fully concrete. Phases 2–6 are intentionally outline-level with `EXPAND` markers (honesty boundary, §"Plan structure") — NOT placeholder vagueness; each names files, interfaces, commands, and the verify-then-adapt points. The `<N>` PR-number and `<role>` cargo-dir tokens are fill-at-execution, standard for this repo.
- **Type consistency:** `ConformanceReport`, `assert_conform`, `ExecutionMode`, `lower_project`, `to_canonical_bytes`, `from_canonical_bytes`, `CapabilityShapeSet`, `any-wasi-strict` used consistently with the spec + verified code.
