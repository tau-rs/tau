# EPIC 5.4 Foundation (5.2 slices) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the streaming + typed-embed foundation that EPIC 5.4's React/Angular consumers sit on: a frozen `RunEvent` JSON schema, a per-event wasm→host streaming transport, and a `tau embed --host js` generator that emits `@tau/embed-js`.

**Architecture:** Three layered slices. (1) `RunEvent` gains an optional `schema` feature deriving `schemars::JsonSchema`; a frozen schema file + freeze test make the Rust enum the single source of truth. (2) A new `emit-event` host import lets the guest stream events one at a time instead of buffering into one blob; the host buffers them. (3) A `tau embed --host js` subcommand emits the generic `@tau/embed-js` scaffold (jco config + hand-written `RunEvent.ts` + `normalize.ts` + Worker host), drift-tested like the 5.3 SDK.

**Tech Stack:** Rust (schemars v1, wit-bindgen guest, wasmtime host, clap), TypeScript (emitted templates), jco (dev-dep of the emitted package, not invoked by Rust).

## Global Constraints

- **CARGO RULES (repo CLAUDE.md):** every cargo command MUST be `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo <cmd> -p <crate>`. Main agent uses `target/main`; a subagent uses `target/agent-<role>`. Timeouts: test 300, build/check 180, clippy 240, fmt 30. Never bare `cargo`, never `--workspace`, always `-p`.
- **Design source of truth:** `docs/superpowers/specs/2026-08-08-epic-5-4-typed-consumers-design.md`. Decisions D1–D4 are binding.
- **`RunEvent` stays externally-tagged serde** (`{"TextDelta":{…}}`). Do NOT add `#[serde(tag=…)]` (D3 rejected the flip).
- **Do NOT modify the guest single-agent limit** (`crates/tau-wasm-guest/src/guest.rs:106`). It is a known dependency for multi-agent streaming, out of scope here.
- **`emit-event` is fire-and-forget:** WIT `emit-event: func(event-json: string)` returns nothing (no `result`).
- **ADR-0056 WIT freeze:** any change to `wit/tau-host.wit`'s `interface host` MUST be mirrored in `crates/tau-wasm-host/tests/wit_host_drift.rs` (`HOST_PORT_REGISTRY` + param-shape assertions) or that test fails by design.
- **schemars v1 API:** use `schemars::generate::SchemaSettings::draft2020_12().into_generator().into_root_schema_for::<T>()`; `$defs` (not `definitions`). Workspace dep: `schemars = { version = "1", default-features = false }`, pulled per-crate as `{ workspace = true, optional = true, features = ["derive"] }`.
- **Schema-freeze test convention (mirror `crates/tau-ir/tests/schema_export.rs`):** pretty-print via `serde_json::to_string_pretty`, append a trailing `\n`, string `assert_eq!` generated vs on-disk; `UPDATE_SCHEMA=1` env regenerates the file; test file gated `#![cfg(feature = "schema")]`.

---

## File Structure

**Task 1 — schema:**
- Modify: `crates/tau-runtime-core/Cargo.toml` (add optional `schemars` + `schema` feature)
- Modify: `crates/tau-runtime-core/src/stream.rs` (derive on `RunEvent`)
- Modify: `crates/tau-runtime-core/src/outcome.rs` (derive on `RunOutcome`)
- Modify: `crates/tau-ports/src/llm.rs`, `crates/tau-ports/src/tool.rs` (derive on `StopReason`, `TokenUsage`, `ToolResult`)
- Create: `schemas/run-event/run-event.v1.schema.json` (generated, committed)
- Create/Test: `crates/tau-runtime-core/tests/run_event_schema.rs`

**Task 2 — transport:**
- Modify: `wit/tau-host.wit` (add `emit-event`)
- Modify: `crates/tau-wasm-guest/src/executor.rs` (add `for_each_stream`)
- Modify: `crates/tau-wasm-guest/src/guest.rs` (stream per-event)
- Modify: `crates/tau-wasm-host/src/lib.rs` (`emit_event` impl + `HostState` buffer)
- Modify/Test: `crates/tau-wasm-host/tests/wit_host_drift.rs` (registry + param shapes)
- Test: `crates/tau-wasm-host/tests/emit_event_buffer.rs`

**Task 3 — embed generator:**
- Create: `crates/tau-sdk-codegen/src/embed_js.rs` (scaffold templates + render fn)
- Modify: `crates/tau-sdk-codegen/src/lib.rs` (module + re-export)
- Modify: `crates/tau-cli/Cargo.toml` (dep on `tau-sdk-codegen`)
- Modify: `crates/tau-cli/src/cli.rs` (`Embed` command + `EmbedArgs`)
- Modify: `crates/tau-cli/src/lib.rs` (dispatch arm)
- Modify: `crates/tau-cli/src/cmd/mod.rs` (`pub mod embed;`)
- Create: `crates/tau-cli/src/cmd/embed.rs` (handler)
- Create: `sdk/embed-js/**` (committed emitter output)
- Test: `crates/tau-sdk-codegen/tests/embed_js_drift.rs`, `crates/tau-sdk-codegen/tests/run_event_ts_coverage.rs`

---

## Task 1: `RunEvent` JSON schema + freeze test

**Files:**
- Modify: `crates/tau-runtime-core/Cargo.toml`, `crates/tau-runtime-core/src/stream.rs:128`, `crates/tau-runtime-core/src/outcome.rs:49`, `crates/tau-ports/src/llm.rs:341,359`, `crates/tau-ports/src/tool.rs:205`
- Create: `schemas/run-event/run-event.v1.schema.json`
- Test: `crates/tau-runtime-core/tests/run_event_schema.rs`

**Interfaces:**
- Consumes: nothing (first task).
- Produces: committed `schemas/run-event/run-event.v1.schema.json` (draft2020-12, root `$defs` for `RunEvent` variants as externally-tagged `oneOf` of single-key objects). Task 3's TS-coverage test reads this file. A `pub const RUN_EVENT_SCHEMA_VERSION: &str = "v1"` in `crates/tau-runtime-core/src/stream.rs`.

- [ ] **Step 1: Add derives to the leaf types in tau-ports**

In `crates/tau-ports/src/llm.rs`, add the schema-gated derive above `pub enum StopReason` (line 341) and `pub struct TokenUsage` (line 359):

```rust
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
```

Do the same above `pub struct ToolResult` in `crates/tau-ports/src/tool.rs:205`. (tau-ports already declares `schema = ["dep:schemars"]` and `schemars = { workspace = true, optional = true }`, so no Cargo change here.)

- [ ] **Step 2: Add the schema feature + derive in tau-runtime-core**

In `crates/tau-runtime-core/Cargo.toml`, add under `[dependencies]`:

```toml
schemars = { workspace = true, optional = true, features = ["derive"] }
```

and under `[features]`:

```toml
schema = ["dep:schemars", "tau-ports/schema", "tau-domain/schema"]
```

In `crates/tau-runtime-core/src/outcome.rs`, add above `pub enum RunOutcome` (line 49):

```rust
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
```

In `crates/tau-runtime-core/src/stream.rs`, add above `pub enum RunEvent` (line 128, keep the existing `#[non_exhaustive]` and derive line):

```rust
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
```

and add near the top-level items of the file:

```rust
/// Frozen schema version for `RunEvent` (see schemas/run-event/).
pub const RUN_EVENT_SCHEMA_VERSION: &str = "v1";
```

- [ ] **Step 3: Verify it compiles under the schema feature (follow the compiler for transitive leaves)**

Run:

```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo check -p tau-runtime-core --features schema
```

Expected: it may fail with `the trait bound '<T>: JsonSchema' is not satisfied` for a transitive field type of `ToolResult`/`RunOutcome` not yet covered. For EACH such type the compiler names, add the same `#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]` line above its definition (in whichever crate owns it; enable that crate's `schema` feature in the feature chain if needed). `serde_json::Value` (the `args` field) needs no derive — schemars supports it natively. Re-run until check passes.

- [ ] **Step 4: Write the failing freeze test**

Create `crates/tau-runtime-core/tests/run_event_schema.rs`:

```rust
//! Freeze test: the committed RunEvent JSON schema must equal fresh schemars
//! output. Regenerate with:
//!   UPDATE_SCHEMA=1 cargo test -p tau-runtime-core --features schema --test run_event_schema
#![cfg(feature = "schema")]

use std::path::PathBuf;

use tau_runtime_core::stream::{RunEvent, RUN_EVENT_SCHEMA_VERSION};

fn schema_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schemas/run-event/run-event.v1.schema.json")
}

fn generate() -> serde_json::Value {
    let settings = schemars::generate::SchemaSettings::draft2020_12();
    let generator = settings.into_generator();
    let schema = generator.into_root_schema_for::<RunEvent>();
    let mut v = serde_json::to_value(&schema).unwrap();
    let obj = v.as_object_mut().unwrap();
    obj.insert(
        "$id".into(),
        format!(
            "https://lebocqtitouan.github.io/tau/schemas/run-event/{}/run-event.schema.json",
            RUN_EVENT_SCHEMA_VERSION
        )
        .into(),
    );
    obj.insert(
        "title".into(),
        format!("tau RunEvent ({})", RUN_EVENT_SCHEMA_VERSION).into(),
    );
    v
}

fn pretty(v: &serde_json::Value) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap();
    s.push('\n');
    s
}

#[test]
fn run_event_schema_matches_checked_in_file() {
    let generated = pretty(&generate());
    if std::env::var("UPDATE_SCHEMA").is_ok() {
        std::fs::create_dir_all(schema_path().parent().unwrap()).unwrap();
        std::fs::write(schema_path(), &generated).unwrap();
        return;
    }
    let on_disk = std::fs::read_to_string(schema_path())
        .expect("schemas/run-event/run-event.v1.schema.json missing — run with UPDATE_SCHEMA=1");
    assert_eq!(
        generated, on_disk,
        "RunEvent schema drifted from serde types; regenerate with UPDATE_SCHEMA=1"
    );
}
```

If `RunEvent` is not re-exported at `tau_runtime_core::stream`, adjust the `use` path to wherever `RunEvent` is public (check `crates/tau-runtime-core/src/lib.rs` re-exports).

- [ ] **Step 5: Run it to verify it fails (file missing)**

Run:

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test -p tau-runtime-core --features schema --test run_event_schema
```

Expected: FAIL with the "missing — run with UPDATE_SCHEMA=1" panic.

- [ ] **Step 6: Generate + commit the frozen schema**

Run:

```bash
timeout 300 env UPDATE_SCHEMA=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test -p tau-runtime-core --features schema --test run_event_schema
```

Then re-run WITHOUT `UPDATE_SCHEMA` and confirm PASS:

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test -p tau-runtime-core --features schema --test run_event_schema
```

Expected: PASS. Inspect `schemas/run-event/run-event.v1.schema.json` — it should contain a `$defs`/`oneOf` with the ten `RunEvent` variants as externally-tagged single-key objects.

- [ ] **Step 7: Confirm no default-feature regression**

Run (guest/no_std consumers must still build without the schema feature):

```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo check -p tau-runtime-core --no-default-features
```

Expected: PASS (schemars is optional; `#[cfg_attr]` compiles out).

- [ ] **Step 8: Commit**

```bash
git add crates/tau-runtime-core/Cargo.toml crates/tau-runtime-core/src/stream.rs \
  crates/tau-runtime-core/src/outcome.rs crates/tau-ports/src/llm.rs \
  crates/tau-ports/src/tool.rs schemas/run-event/ \
  crates/tau-runtime-core/tests/run_event_schema.rs
git commit -m "feat(runtime-core): freeze RunEvent JSON schema behind schema feature"
```

---

## Task 2: `emit-event` streaming host import

**Files:**
- Modify: `wit/tau-host.wit:7-18`, `crates/tau-wasm-guest/src/executor.rs:39`, `crates/tau-wasm-guest/src/guest.rs:95-141`, `crates/tau-wasm-host/src/lib.rs:67-110`
- Modify/Test: `crates/tau-wasm-host/tests/wit_host_drift.rs:29-106`
- Test: `crates/tau-wasm-host/tests/emit_event_buffer.rs`

**Interfaces:**
- Consumes: nothing from Task 1 (independent; can run in parallel).
- Produces: WIT host import `emit-event: func(event-json: string)`; guest emits each `RunEvent` as a JSON string via `host::emit_event` during `run`; host `HostState` exposes a `pub emitted: Vec<String>` buffer populated during `call_run`.

- [ ] **Step 1: Add the host import to the frozen WIT**

In `wit/tau-host.wit`, inside `interface host { … }` (after `next-u64`, before the closing brace), add:

```wit
    /// Stream one RunEvent to the host as it is produced. `event-json` is a
    /// serialized tau_runtime_core::stream::RunEvent (externally-tagged serde).
    /// Fire-and-forget: no return, the guest does not observe host errors.
    emit-event: func(event-json: string);
```

- [ ] **Step 2: Update the WIT drift test to expect it (write the failing expectation)**

In `crates/tau-wasm-host/tests/wit_host_drift.rs`, add `("emit-event", …)` to `HOST_PORT_REGISTRY` (around lines 29-33) following the existing tuple shape, and add a param-shape assertion for `emit-event` (one `string` param, no return) in `host_function_param_shapes_are_frozen` (around lines 82-106), mirroring how `complete` is asserted. (Read the current test body first; match its exact tuple/assertion format.)

- [ ] **Step 3: Run the drift test to verify it now fails on the missing host impl**

Run:

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test -p tau-wasm-host --test wit_host_drift
```

Expected: FAIL — compile error (`emit_event` not implemented on `impl host::Host`) or an assertion mismatch, because the host side hasn't implemented `emit_event` yet.

- [ ] **Step 4: Implement `emit_event` on the host + buffer field**

In `crates/tau-wasm-host/src/lib.rs`, add a buffer to `HostState` (struct around lines 67-86):

```rust
    /// RunEvents streamed from the guest during `call_run`, in order.
    pub emitted: Vec<String>,
```

Initialize it in `HostState::new(...)` (`emitted: Vec::new()`). Then add to `impl host::Host for HostState` (lines 88-110):

```rust
    fn emit_event(&mut self, event_json: String) {
        self.emitted.push(event_json);
    }
```

No linker change is needed — `Runner::add_to_linker` wires all `impl host::Host` methods automatically.

- [ ] **Step 5: Add a per-event stream drain to the guest executor**

In `crates/tau-wasm-guest/src/executor.rs`, add beside `collect_stream` (line 39):

```rust
/// Drain a stream to completion, invoking `f` on each item as it arrives.
pub fn for_each_stream<S: Stream, F: FnMut(S::Item)>(stream: S, mut f: F) {
    let mut stream = pin!(stream);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    loop {
        match stream.as_mut().poll_next(&mut cx) {
            Poll::Ready(Some(item)) => f(item),
            Poll::Ready(None) => return,
            Poll::Pending => core::hint::spin_loop(),
        }
    }
}
```

- [ ] **Step 6: Change the guest `run` to stream per-event**

In `crates/tau-wasm-guest/src/guest.rs`, replace the final two lines of the `run` body (lines 138-139, the `collect_stream` + `serde_json::to_string(&events)`) with:

```rust
        crate::executor::for_each_stream(stream, |event| {
            if let Ok(json) = serde_json::to_string(&event) {
                crate::wit_host::emit_event(&json);
            }
        });
        Ok(String::new())
```

`run`'s `Ok` payload is now an empty sentinel; events flow via `emit-event` (design D2). `crate::wit_host` is the re-export module already defined in `guest.rs`.

- [ ] **Step 7: Write the host-side behavior test**

Create `crates/tau-wasm-host/tests/emit_event_buffer.rs`. Model it on the existing round-trip test in this crate (open `crates/tau-wasm-host/tests/` and reuse the same fixture-component + canned-completion setup; copy its harness verbatim rather than inventing one). The assertion:

```rust
// After run_component(...) with a fixture that produces >=1 turn,
// the HostState.emitted buffer must be non-empty and every entry must
// deserialize as a RunEvent, with the first == RunStarted and the last
// containing "RunCompleted".
assert!(!state.emitted.is_empty(), "no events streamed via emit-event");
let first: serde_json::Value = serde_json::from_str(&state.emitted[0]).unwrap();
assert_eq!(first, serde_json::json!("RunStarted"));
assert!(state.emitted.last().unwrap().contains("RunCompleted"));
```

If the existing round-trip test consumes the `run` return payload for its assertions, update it: the payload is now `""`, so move its event assertions onto `state.emitted`.

- [ ] **Step 8: Run the guest build + host tests**

Guest must still compile to wasm (no_std):

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo check -p tau-wasm-guest --target wasm32-wasip2
```

Then the host tests:

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test -p tau-wasm-host
```

Expected: `wit_host_drift` PASS, `emit_event_buffer` PASS. (If the host test builds a component, it may take 60–90s the first time; the 300s timeout covers it.)

- [ ] **Step 9: Commit**

```bash
git add wit/tau-host.wit crates/tau-wasm-guest/src/executor.rs \
  crates/tau-wasm-guest/src/guest.rs crates/tau-wasm-host/src/lib.rs \
  crates/tau-wasm-host/tests/
git commit -m "feat(wasm): stream RunEvents via emit-event host import"
```

---

## Task 3: `tau embed --host js` generator

**Files:**
- Create: `crates/tau-sdk-codegen/src/embed_js.rs`
- Modify: `crates/tau-sdk-codegen/src/lib.rs:17`
- Modify: `crates/tau-cli/Cargo.toml`, `crates/tau-cli/src/cli.rs:198`, `crates/tau-cli/src/lib.rs:207`, `crates/tau-cli/src/cmd/mod.rs`
- Create: `crates/tau-cli/src/cmd/embed.rs`, `sdk/embed-js/**`
- Test: `crates/tau-sdk-codegen/tests/embed_js_drift.rs`, `crates/tau-sdk-codegen/tests/run_event_ts_coverage.rs`

**Interfaces:**
- Consumes: `schemas/run-event/run-event.v1.schema.json` (Task 1) for the coverage test.
- Produces: `tau_sdk_codegen::embed_js::render_embed_js() -> BTreeMap<PathBuf, String>` (repo-relative paths under `sdk/embed-js/`, matching 5.3's `render_all` convention); a `tau embed --host js -o <dir>` subcommand that writes those files.

- [ ] **Step 1: Write the emitter with hand-authored templates**

Create `crates/tau-sdk-codegen/src/embed_js.rs`. Follow `emit_ts.rs`'s pattern (const `&str` templates + a `render_*` fn returning `BTreeMap<PathBuf, String>`). Emit these files under `sdk/embed-js/`:

- `package.json` — name `@tau/embed-js`, `type: "module"`, `jco` + `typescript` as `devDependencies`, a `build` script `jco transpile $npm_config_wasm --out-dir src/generated` (documented in README as taking `--wasm`).
- `src/RunEvent.ts` — the hand-written union (the illustrative type in the design doc §"Public surface"; `type` tags in kebab-case, `TokenUsage`/`RunOutcome`/`StopReason` aliases included). Header comment: `// Hand-written; guarded by run_event_ts_coverage test against schemas/run-event/.`
- `src/normalize.ts` — `export function normalize(raw: unknown): RunEvent` mapping serde externally-tagged `{"TextDelta":{delta}}` → `{type:"text-delta", delta}` for every variant (snake_case→camelCase fields; `Result` → `{ok}|{err}`).
- `src/index.ts` — `loadTau(wasm)` / `loadTauInWorker(wasm)` + `TauComponent` interface (signatures from design §"Public surface"); imports the jco output from `./generated` and supplies host imports incl. `emitEvent` pushing `normalize(JSON.parse(json))` into an async queue.
- `src/worker.ts` — the Web-Worker host.
- `README.md` — how to `npm install && npm run build --wasm=path/to/component.wasm`.

Keep `src/generated/` out of the emitter (it is jco's build output; add it to a `.gitignore` line emitted in `sdk/embed-js/.gitignore`).

Public fn:

```rust
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Render the @tau/embed-js scaffold as repo-relative-path -> contents.
pub fn render_embed_js() -> BTreeMap<PathBuf, String> { /* insert templates */ }
```

- [ ] **Step 2: Export the module**

In `crates/tau-sdk-codegen/src/lib.rs`, add `pub mod embed_js;` and (optionally) re-export `render_embed_js`. Confirm it compiles:

```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo check -p tau-sdk-codegen
```

- [ ] **Step 3: Write the failing drift test**

Create `crates/tau-sdk-codegen/tests/embed_js_drift.rs`, mirroring `tests/drift.rs` (repo_root = two parents up from `CARGO_MANIFEST_DIR`; compare each rendered entry against the committed file):

```rust
use std::path::Path;

#[test]
fn committed_embed_js_matches_fresh_render() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
    let rendered = tau_sdk_codegen::embed_js::render_embed_js();
    let mut drifted = Vec::new();
    for (rel, expected) in &rendered {
        let actual = std::fs::read_to_string(repo_root.join(rel)).unwrap_or_default();
        if &actual != expected { drifted.push(rel.display().to_string()); }
    }
    assert!(drifted.is_empty(), "committed sdk/embed-js drifted; regenerate:\n{}", drifted.join("\n"));
}
```

- [ ] **Step 4: Run it to verify it fails (files not committed yet)**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test -p tau-sdk-codegen --test embed_js_drift
```

Expected: FAIL — every `sdk/embed-js/*` path reported as drifted (missing).

- [ ] **Step 5: Write the TS-coverage test (hop 2)**

Create `crates/tau-sdk-codegen/tests/run_event_ts_coverage.rs`. Parse the frozen schema, extract the externally-tagged variant keys, and assert each appears (kebab-cased) as a `type:` literal in the committed `sdk/embed-js/src/RunEvent.ts`:

```rust
use std::path::Path;

fn variant_keys(schema: &serde_json::Value) -> Vec<String> {
    // RunEvent is $defs-rooted oneOf of single-key objects (unit variants are
    // const strings). Collect both forms from the root schema.
    let mut out = Vec::new();
    if let Some(one_of) = schema.get("oneOf").and_then(|v| v.as_array()) {
        for entry in one_of {
            if let Some(k) = entry.get("required").and_then(|r| r.as_array())
                .and_then(|a| a.first()).and_then(|s| s.as_str()) {
                out.push(k.to_string());               // struct variant
            } else if let Some(c) = entry.get("const").and_then(|s| s.as_str()) {
                out.push(c.to_string());               // unit variant
            }
        }
    }
    out
}

fn to_kebab(pascal: &str) -> String {
    let mut s = String::new();
    for (i, ch) in pascal.chars().enumerate() {
        if ch.is_uppercase() && i != 0 { s.push('-'); }
        s.extend(ch.to_lowercase());
    }
    s
}

#[test]
fn run_event_ts_covers_every_schema_variant() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
    let schema: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo_root.join("schemas/run-event/run-event.v1.schema.json")).unwrap()
    ).unwrap();
    let ts = std::fs::read_to_string(repo_root.join("sdk/embed-js/src/RunEvent.ts")).unwrap();
    let mut missing = Vec::new();
    for key in variant_keys(&schema) {
        let tag = to_kebab(&key);
        if !ts.contains(&format!("type: \"{tag}\"")) { missing.push(tag); }
    }
    assert!(missing.is_empty(), "RunEvent.ts missing variants: {missing:?}");
}
```

Inspect the generated `run-event.v1.schema.json` first to confirm the exact `oneOf`/`const`/`required` shape schemars v1 produces, and adjust `variant_keys` to match (it may nest variants under `$defs` — if so, read `$defs.RunEvent` instead of the root).

- [ ] **Step 6: Wire the CLI subcommand**

In `crates/tau-cli/Cargo.toml` `[dependencies]`, add:

```toml
tau-sdk-codegen = { workspace = true }
```

In `crates/tau-cli/src/cli.rs`, add a variant to `enum Command` (near line 198):

```rust
/// Emit host-embedding glue for a target language (Phase 2 §5.2).
Embed(EmbedArgs),
```

and define the args struct near the other `*Args` structs:

```rust
#[derive(clap::Args, Debug)]
pub struct EmbedArgs {
    /// Host language for the generated glue. Only `js` is supported today.
    #[arg(long, value_name = "HOST")]
    pub host: String,
    /// Output directory (default: ./sdk/embed-js).
    #[arg(long, short = 'o', value_name = "DIR")]
    pub output: Option<std::path::PathBuf>,
}
```

In `crates/tau-cli/src/cmd/mod.rs`, add `pub mod embed;` (alphabetical). In `crates/tau-cli/src/lib.rs`, add the dispatch arm (near line 207):

```rust
cli::Command::Embed(ref args) => cmd::embed::run(args, &mut output).await,
```

- [ ] **Step 7: Write the handler**

Create `crates/tau-cli/src/cmd/embed.rs`:

```rust
use crate::cli::EmbedArgs;
use crate::output::Output;
use anyhow::{bail, Result};

/// CLI entry point for `tau embed --host js`.
pub async fn run(args: &EmbedArgs, output: &mut Output) -> Result<()> {
    if args.host != "js" {
        bail!("unsupported --host '{}': only 'js' is supported", args.host);
    }
    let out_root = args.output.clone().unwrap_or_else(|| std::path::PathBuf::from("."));
    for (rel, contents) in tau_sdk_codegen::embed_js::render_embed_js() {
        // rendered paths are repo-relative under sdk/embed-js/; write beneath out_root.
        let path = out_root.join(&rel);
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, contents)?;
    }
    output.println("emitted @tau/embed-js");
    Ok(())
}
```

(Match `output` API to an existing handler — e.g. how `cmd::build_wasm::run` prints. Adjust `output.println` to the real method name.)

- [ ] **Step 8: Generate + commit the emitted package, then verify all tests green**

Emit the committed scaffold into the repo (from repo root):

```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo run -p tau-cli -- embed --host js -o .
```

Then run all three tests:

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test -p tau-sdk-codegen --test embed_js_drift --test run_event_ts_coverage
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo check -p tau-cli
```

Expected: both tests PASS, CLI checks. If `run_event_ts_coverage` fails, add the missing variant to `sdk/embed-js/src/RunEvent.ts`'s template in `embed_js.rs`, re-run the `embed` command, and re-test.

- [ ] **Step 9: Commit**

```bash
git add crates/tau-sdk-codegen/src/embed_js.rs crates/tau-sdk-codegen/src/lib.rs \
  crates/tau-sdk-codegen/tests/embed_js_drift.rs crates/tau-sdk-codegen/tests/run_event_ts_coverage.rs \
  crates/tau-cli/Cargo.toml crates/tau-cli/src/cli.rs crates/tau-cli/src/lib.rs \
  crates/tau-cli/src/cmd/mod.rs crates/tau-cli/src/cmd/embed.rs sdk/embed-js/
git commit -m "feat(cli): tau embed --host js emits @tau/embed-js scaffold"
```

---

## Final verification

- [ ] Run the workspace slices touched, clippy-clean (CI treats warnings as errors):

```bash
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo clippy -p tau-runtime-core --features schema --all-targets
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo clippy -p tau-wasm-host --all-targets
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo clippy -p tau-sdk-codegen --all-targets
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo clippy -p tau-cli --all-targets
```

- [ ] `cargo fmt --check` on the touched crates.
- [ ] Open the PR against `main`. This slice is Rust — run `lefthook run deep-gate` (or `scripts/agent-push.sh`) as a pre-flight before pushing if desired; CI is the gate.

## Out of scope (follow-up: EPIC 5.4 consumers plan)

`@tau/react` (`useTauRun`), `@tau/angular` (`TauRunService`), and `examples/streaming-demo`. Author that plan against the real emitted `@tau/embed-js` surface once this foundation lands. The guest single-agent limit remains a dependency for any multi-agent streaming.
