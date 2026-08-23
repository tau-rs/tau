# EPIC 7.2 — Variant A wasm embedding API + example — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a product load and run a built tau wasm component by supplying the four host functions (`EmbedPorts` trait seam in `tau-wasm-host`), proven by a runnable example product (closes #414).

**Architecture:** Cut an injection seam into `tau-wasm-host`: the four `tau:host/runner` WIT imports become an `EmbedPorts` trait; the existing deterministic conformance fakes become one private impl (`DeterministicPorts`) so `run_component`/`run_component_with_caps` stay byte-identical. New `run_component_with_ports` + `embed` prelude are the product-facing surface. A new example crate `tau-wasm-embed-example` loads a `.wasm` from argv and runs it live; a tau-cli e2e test builds the trivial fixture component and drives the example (the roadmap acceptance).

**Tech Stack:** Rust, wasmtime 47 (component model, `Store<T: 'static>`), tau-ports (serde feature), serde_json.

**Spec:** `docs/superpowers/specs/2026-08-22-epic-7-2-wasm-embed-design.md`

## Global Constraints

- Every cargo command: `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <cmd> -p <crate>` (subagents: `target/agent-e72-t<task#>`). Timeouts: test 300, build/check 180, clippy 240, fmt 30. Never bare cargo, never workspace-wide, check `pgrep -af cargo` first (CLAUDE.md CARGO RULES).
- Prefer `cargo nextest run` over `cargo test` (except `--doc`).
- Ignored wasm tests need `rustup target add wasm32-wasip2`; run with `--run-ignored all` and timeout 600 (they shell a release guest build).
- Workspace lints apply (warnings deny in CI); example crate is `#![forbid(unsafe_code)]`.
- Commits: conventional, scoped, and ALWAYS `git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "..."` (lefthook identity-corruption + hook-timeout gotchas). Append `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Conformance invariant: `run_component` / `run_component_with_caps` signatures AND behavior (including exact error strings, clock/PRNG sequences, event buffering) must not change. Existing tests are the guard — none of them may be edited except where a task explicitly says so.
- Test helpers duplicated across integration-test files (`build_guest_component`, `trivial_ir_bytes`, `build_trivial_component`) follow existing repo precedent (roundtrip.rs / build_wasm_e2e.rs already duplicate them); do NOT introduce a shared `tests/common` module.

---

### Task 1: `EmbedPorts` trait + `DeterministicPorts` (pure addition, no behavior change)

**Files:**
- Modify: `crates/tau-wasm-host/src/lib.rs` (add trait + impl + tests; do not touch `HostState` yet)

**Interfaces:**
- Consumes: `tau_ports::llm::{CompletionRequest, CompletionResponse}` (serde feature already on), existing constants `CLOCK_STEP_MILLIS`, `PRNG_SEED`.
- Produces (used by Tasks 2–4):
  - `pub trait EmbedPorts: Send` with methods `complete(&mut self, CompletionRequest) -> Result<CompletionResponse, String>`, `now_millis(&mut self) -> u64`, `next_u64(&mut self) -> u64`, `on_event(&mut self, &str)`.
  - `pub(crate) struct DeterministicPorts` with `pub(crate) fn new(responses: Vec<String>) -> (Self, Arc<Mutex<Vec<String>>>)`.

- [ ] **Step 1: Write the failing tests**

Append to the existing `mod tests` in `crates/tau-wasm-host/src/lib.rs`:

```rust
    /// DeterministicPorts must reproduce HostState's exact sequences —
    /// cross-checked against the legacy impl before Task 2 rewires it.
    #[test]
    fn deterministic_ports_matches_legacy_clock_and_prng() {
        let mut legacy = HostState::new(vec![], empty_wasi_ctx(), empty_egress());
        let (mut ports, _emitted) = DeterministicPorts::new(vec![]);
        for _ in 0..4 {
            assert_eq!(ports.now_millis(), legacy.now_millis());
            assert_eq!(ports.next_u64(), legacy.next_u64());
        }
    }

    #[test]
    fn deterministic_ports_pops_cassette_in_order_and_exhausts_with_legacy_error() {
        let first = canned_response();
        let (mut ports, _emitted) = DeterministicPorts::new(vec![first]);
        let req = CompletionRequest::new("m".to_string());
        let resp = ports.complete(req.clone()).expect("first canned response");
        assert_eq!(resp.text, "");
        assert_eq!(
            ports.complete(req).unwrap_err(),
            "tau-wasm-host: no canned completion response left"
        );
    }

    #[test]
    fn deterministic_ports_rejects_malformed_canned_response() {
        let (mut ports, _emitted) = DeterministicPorts::new(vec!["not json".to_string()]);
        let err = ports.complete(CompletionRequest::new("m".to_string())).unwrap_err();
        assert!(err.contains("invalid canned CompletionResponse"), "got: {err}");
    }

    #[test]
    fn deterministic_ports_buffers_events_via_shared_handle() {
        let (mut ports, emitted) = DeterministicPorts::new(vec![]);
        ports.on_event("{\"RunStarted\":null}");
        ports.on_event("{\"RunCompleted\":{}}");
        assert_eq!(
            emitted.lock().unwrap().as_slice(),
            ["{\"RunStarted\":null}", "{\"RunCompleted\":{}}"]
        );
    }
```

Add `use tau_ports::llm::CompletionRequest;` to the test module's imports (the module already has access to `super::*`).

- [ ] **Step 2: Run tests to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-wasm-host`
Expected: compile FAIL — `DeterministicPorts` not found.

- [ ] **Step 3: Implement trait + DeterministicPorts**

In `crates/tau-wasm-host/src/lib.rs`, after the `PRNG_SEED` const, add (adjusting the `use` list at the top: add `std::sync::{Arc, Mutex}` and `tau_ports::llm::CompletionRequest`; `CompletionResponse` is already imported):

```rust
/// The four host functions a product supplies to run a tau component —
/// the Rust face of the `tau:host/runner` WIT imports (EPIC 7.2).
///
/// Ports are owned by the run (`Box<dyn EmbedPorts>`; wasmtime's
/// `Store<T: 'static>` forbids borrows). Impls that need state back after
/// the run hold an `Arc` handle and keep a clone outside.
pub trait EmbedPorts: Send {
    /// Live inference. Typed: the host layer (de)serializes across the WIT
    /// boundary; the `Err` string crosses into the guest's `result` error
    /// arm. The WIT import is synchronous — an async product client blocks
    /// inside its impl.
    fn complete(&mut self, req: CompletionRequest) -> Result<CompletionResponse, String>;
    /// Wall clock in milliseconds.
    fn now_millis(&mut self) -> u64;
    /// Next value from the product's entropy source.
    fn next_u64(&mut self) -> u64;
    /// One serialized `RunEvent` per call, in order, live as the guest emits
    /// it. Raw JSON so this crate needs no tau-runtime-core dependency;
    /// products deserialize with their own dep if they want typed events.
    fn on_event(&mut self, event_json: &str);
}

/// The conformance ports: canned-cassette LLM, fixed-step clock, seeded
/// SplitMix64 — exactly the behavior `run_component` has always had, now as
/// one `EmbedPorts` impl.
pub(crate) struct DeterministicPorts {
    responses: VecDeque<String>,
    clock_millis: u64,
    prng_state: u64,
    emitted: Arc<Mutex<Vec<String>>>,
}

impl DeterministicPorts {
    /// Returns the ports plus the shared event-buffer handle the caller
    /// drains after the run.
    pub(crate) fn new(responses: Vec<String>) -> (Self, Arc<Mutex<Vec<String>>>) {
        let emitted = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                responses: responses.into(),
                clock_millis: 0,
                prng_state: PRNG_SEED,
                emitted: Arc::clone(&emitted),
            },
            emitted,
        )
    }
}

impl EmbedPorts for DeterministicPorts {
    fn complete(&mut self, _req: CompletionRequest) -> Result<CompletionResponse, String> {
        let raw = self
            .responses
            .pop_front()
            .ok_or_else(|| "tau-wasm-host: no canned completion response left".to_string())?;
        serde_json::from_str(&raw)
            .map_err(|e| format!("tau-wasm-host: invalid canned CompletionResponse: {e}"))
    }

    fn now_millis(&mut self) -> u64 {
        let now = self.clock_millis;
        self.clock_millis = self.clock_millis.wrapping_add(CLOCK_STEP_MILLIS);
        now
    }

    fn next_u64(&mut self) -> u64 {
        // SplitMix64 — identical to the legacy HostState sequence.
        self.prng_state = self.prng_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.prng_state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn on_event(&mut self, event_json: &str) {
        self.emitted
            .lock()
            .expect("event buffer lock")
            .push(event_json.to_string());
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-wasm-host`
Expected: PASS (all pre-existing + 4 new).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-wasm-host/src/lib.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(wasm-host): EmbedPorts trait + DeterministicPorts impl (#414)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: `run_component_with_ports` — rewire `HostState` through the trait; `embed` prelude

**Files:**
- Modify: `crates/tau-wasm-host/src/lib.rs` (HostState fields, `host::Host` impl, `run_component_with_caps`, new `run_component_with_ports`, unit tests)
- Create: `crates/tau-wasm-host/src/embed.rs`
- Create: `crates/tau-wasm-host/tests/embed_ports.rs`
- Modify: `crates/tau-wasm-host/Cargo.toml` (description only — it says "three tau:host/host imports" and omits embedding)

**Interfaces:**
- Consumes: Task 1's `EmbedPorts` + `DeterministicPorts`.
- Produces (used by Tasks 3–5):
  - `pub fn run_component_with_ports(wasm_bytes: &[u8], prompt: &str, ports: Box<dyn EmbedPorts>, caps: &[tau_domain::Capability], sandbox_root: &Path) -> Result<String, WasmHostError>`
  - `pub mod embed` re-exporting `run_component_with_ports`, `EmbedPorts`, `WasmHostError`, `tau_domain::Capability`, `tau_ports::llm::{CompletionRequest, CompletionResponse, StopReason, TokenUsage, ToolUse}`.

- [ ] **Step 1: Write the failing integration test**

Create `crates/tau-wasm-host/tests/embed_ports.rs`. Copy `build_guest_component` and `trivial_ir_bytes` VERBATIM from `crates/tau-wasm-host/tests/roundtrip.rs:30-133` (per Global Constraints, duplication is the repo's precedent). Keep the guest target dir `target/wasm-guest-fixture` unchanged so the release guest built by roundtrip.rs is reused instead of rebuilt. Then:

```rust
use std::path::Path;
use std::sync::{Arc, Mutex};

use tau_wasm_host::embed::{
    run_component_with_ports, CompletionRequest, CompletionResponse, EmbedPorts,
};

/// A live-style ports impl: fixed wall clock, counting entropy, canned LLM,
/// and a shared handle recording events as they arrive.
struct RecordingPorts {
    responses: Vec<String>,
    clock: u64,
    entropy: u64,
    events: Arc<Mutex<Vec<String>>>,
}

impl EmbedPorts for RecordingPorts {
    fn complete(&mut self, _req: CompletionRequest) -> Result<CompletionResponse, String> {
        if self.responses.is_empty() {
            return Err("recording ports: cassette exhausted".to_string());
        }
        serde_json::from_str(&self.responses.remove(0)).map_err(|e| e.to_string())
    }
    fn now_millis(&mut self) -> u64 {
        self.clock += 7;
        self.clock
    }
    fn next_u64(&mut self) -> u64 {
        self.entropy += 1;
        self.entropy
    }
    fn on_event(&mut self, event_json: &str) {
        self.events.lock().unwrap().push(event_json.to_string());
    }
}

fn end_turn_response() -> String {
    r#"{"text":"done","tool_uses":[],"stop_reason":"EndTurn","usage":null}"#.to_string()
}

#[test]
#[ignore = "builds the wasm32-wasip2 guest; run with --run-ignored"]
fn with_ports_streams_the_same_events_the_buffered_api_returns() {
    let component = build_guest_component(Some(&trivial_ir_bytes()));

    let events = Arc::new(Mutex::new(Vec::new()));
    let ports = RecordingPorts {
        responses: vec![end_turn_response()],
        clock: 1_724_000_000_000,
        entropy: 0,
        events: Arc::clone(&events),
    };
    let payload =
        run_component_with_ports(&component, "hi", Box::new(ports), &[], Path::new("."))
            .expect("live ports run the guest");

    let (buf_payload, buffered) =
        tau_wasm_host::run_component(&component, "hi", vec![end_turn_response()])
            .expect("buffered API still runs");

    assert_eq!(payload, buf_payload);
    let live = events.lock().unwrap();
    assert!(
        live.first().is_some_and(|e| e.contains("RunStarted")),
        "live stream starts with RunStarted: {live:?}"
    );
    assert!(
        live.last().is_some_and(|e| e.contains("RunCompleted")),
        "live stream ends with RunCompleted: {live:?}"
    );
    // Same component, same cassette → identical event *sequence shape*
    // (timestamps inside events may differ: live clock ≠ deterministic clock,
    // so compare count and variant markers, not bytes).
    assert_eq!(live.len(), buffered.len(), "live: {live:?}\nbuffered: {buffered:?}");
}

#[test]
#[ignore = "builds the wasm32-wasip2 guest; run with --run-ignored"]
fn with_ports_surfaces_complete_error_as_guest_error() {
    let component = build_guest_component(Some(&trivial_ir_bytes()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let ports = RecordingPorts {
        responses: vec![], // exhausted immediately
        clock: 0,
        entropy: 0,
        events: Arc::clone(&events),
    };
    let err = run_component_with_ports(&component, "hi", Box::new(ports), &[], Path::new("."))
        .unwrap_err();
    assert!(
        matches!(err, tau_wasm_host::WasmHostError::Guest(_)),
        "LLM failure crosses as the guest's error arm: {err:?}"
    );
}
```

- [ ] **Step 2: Verify it fails to compile**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-wasm-host --no-run`
Expected: FAIL — `tau_wasm_host::embed` / `run_component_with_ports` not found.

- [ ] **Step 3: Implement the rewire**

In `crates/tau-wasm-host/src/lib.rs`:

3a. `HostState` — replace the four deterministic fields with the ports box:

```rust
struct HostState {
    /// The four host functions, supplied by the caller (EPIC 7.2). The
    /// conformance entrypoints pass `DeterministicPorts`.
    ports: Box<dyn EmbedPorts>,
    table: ResourceTable,
    wasi: WasiCtx,
    http: WasiHttpCtx,
    egress: EgressPolicy,
}

impl HostState {
    fn new(ports: Box<dyn EmbedPorts>, wasi: WasiCtx, egress: EgressPolicy) -> Self {
        Self {
            ports,
            table: ResourceTable::new(),
            wasi,
            http: WasiHttpCtx::new(),
            egress,
        }
    }
}
```

(Keep the existing doc comments about the WASI fields; delete the field docs for the removed queue/clock/prng/emitted fields.)

3b. `host::Host for HostState` — delegate, doing the WIT JSON translation:

```rust
impl host::Host for HostState {
    fn complete(&mut self, request_json: String) -> Result<String, String> {
        let req: CompletionRequest = serde_json::from_str(&request_json)
            .map_err(|e| format!("tau-wasm-host: malformed CompletionRequest from guest: {e}"))?;
        let resp = self.ports.complete(req)?;
        serde_json::to_string(&resp)
            .map_err(|e| format!("tau-wasm-host: failed to serialize CompletionResponse: {e}"))
    }

    fn now_millis(&mut self) -> u64 {
        self.ports.now_millis()
    }

    fn next_u64(&mut self) -> u64 {
        self.ports.next_u64()
    }

    fn emit_event(&mut self, event_json: String) {
        self.ports.on_event(&event_json);
    }
}
```

3c. New public entry — extract the engine/linker/store plumbing currently in `run_component_with_caps` (lib.rs:278-320) into it:

```rust
/// Variant A embedding entry (EPIC 7.2): run a built tau component with
/// caller-supplied [`EmbedPorts`]. Events reach `ports.on_event` live, one
/// serialized `RunEvent` per call; the `Ok` value is the guest's payload
/// sentinel (empty today, reserved for forward-compat).
pub fn run_component_with_ports(
    wasm_bytes: &[u8],
    prompt: &str,
    ports: Box<dyn EmbedPorts>,
    caps: &[tau_domain::Capability],
    sandbox_root: &Path,
) -> Result<String, WasmHostError> {
    let cfg = resolve_wasi_config(caps);
    let wasi = wasi_ctx_from_config(&cfg, sandbox_root)?;
    let egress = EgressPolicy::from_config(&cfg);

    let config = determinism_config().map_err(|e| WasmHostError::Instantiate(e.into()))?;
    let engine = Engine::new(&config).map_err(|e| WasmHostError::Instantiate(e.into()))?;
    let component =
        Component::new(&engine, wasm_bytes).map_err(|e| WasmHostError::Load(e.into()))?;

    let mut linker: Linker<HostState> = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
        .map_err(|e| WasmHostError::Instantiate(e.into()))?;
    add_only_http_to_linker_sync(&mut linker).map_err(|e| WasmHostError::Instantiate(e.into()))?;
    Runner::add_to_linker::<_, HasSelf<HostState>>(&mut linker, |state| state)
        .map_err(|e| WasmHostError::Instantiate(e.into()))?;

    let mut store = Store::new(&engine, HostState::new(ports, wasi, egress));
    let runner = Runner::instantiate(&mut store, &component, &linker)
        .map_err(|e| WasmHostError::Instantiate(e.into()))?;

    match runner.call_run(&mut store, prompt) {
        Ok(Ok(payload)) => Ok(payload),
        Ok(Err(guest_err)) => Err(WasmHostError::Guest(guest_err)),
        Err(trap) => Err(WasmHostError::Trap(trap.into())),
    }
}
```

3d. `run_component_with_caps` becomes the deterministic wrapper (same signature, same up-front cassette validation, same return):

```rust
pub fn run_component_with_caps(
    wasm_bytes: &[u8],
    prompt: &str,
    llm_responses: Vec<String>,
    caps: &[tau_domain::Capability],
    sandbox_root: &Path,
) -> Result<(String, Vec<String>), WasmHostError> {
    // Fail fast on a malformed cassette before touching wasmtime.
    for resp in &llm_responses {
        serde_json::from_str::<CompletionResponse>(resp).map_err(WasmHostError::InvalidResponse)?;
    }
    let (ports, emitted) = DeterministicPorts::new(llm_responses);
    let payload = run_component_with_ports(wasm_bytes, prompt, Box::new(ports), caps, sandbox_root)?;
    let emitted = std::mem::take(&mut *emitted.lock().expect("event buffer lock"));
    Ok((payload, emitted))
}
```

Note the type annotation on `caps` matches the existing signature exactly; `run_component` (the no-caps wrapper) is untouched.

3e. Create `crates/tau-wasm-host/src/embed.rs` and add `pub mod embed;` in lib.rs next to `mod wasi;`:

```rust
//! Curated Variant A embedding surface (EPIC 7.2).
//!
//! A product embeds tau by (1) building a component (`tau build --target
//! wasm`), (2) implementing [`EmbedPorts`] — live inference, wall clock,
//! entropy, and a live [`on_event`](EmbedPorts::on_event) sink — and
//! (3) calling [`run_component_with_ports`]. Capabilities granted to the
//! component ([`Capability`]) are enforced at the wasm boundary: fs/net the
//! caps don't grant is physically unreachable from the workflow.
//!
//! Everything here is a re-export: this module pins *which* items form the
//! supported embedding API, mirroring `tau_runtime_core::embed` (Variant B,
//! EPIC 7.1). See `docs/how-to/embed-wasm-component.md` for the worked
//! example (`crates/tau-wasm-embed-example`). The deterministic conformance
//! entrypoints (`run_component`, `run_component_with_caps`) stay at the
//! crate root — they are not part of the embedding surface.

pub use crate::{run_component_with_ports, EmbedPorts, WasmHostError};
pub use tau_domain::Capability;
pub use tau_ports::llm::{
    CompletionRequest, CompletionResponse, StopReason, TokenUsage, ToolUse,
};
```

3f. Update the existing lib.rs unit tests that construct `HostState::new(vec![], empty_wasi_ctx(), empty_egress())` (around lib.rs:354): replace the first argument with `Box::new(DeterministicPorts::new(vec![]).0)` — assertions stay identical (they exercise the same sequences through delegation). The Task 1 cross-check test `deterministic_ports_matches_legacy_clock_and_prng` now compares delegation vs direct impl — update it to construct legacy via `HostState::new(Box::new(DeterministicPorts::new(vec![]).0), empty_wasi_ctx(), empty_egress())` and keep the sequence-equality assertions.

3g. `crates/tau-wasm-host/Cargo.toml` description — replace with:

```toml
description = "β.7.5 wasm host: a std wasmtime embedder for the tau-wasm-guest WASI 0.2 component. Products supply the four tau:host/runner imports via the EmbedPorts trait (EPIC 7.2); the deterministic conformance entrypoints run the same machinery with canned ports. Host-only (std + wasmtime); never built for wasm32."
```

- [ ] **Step 4: Run the fast suite, then the ignored wasm suite**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-wasm-host`
Expected: PASS (all unit tests incl. Task 1's, plus `emit_event_buffer`, `wit_host_drift` — proves the refactor preserved buffered behavior).

Run: `timeout 600 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-wasm-host --run-ignored all`
Expected: PASS — `roundtrip`, `wasi_fs/http` enforcement (unchanged conformance behavior) AND the two new `embed_ports` tests. Requires `wasm32-wasip2`; first run cold-builds the release guest (minutes).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-wasm-host
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(wasm-host): run_component_with_ports + embed prelude — products supply the four host imports (#414)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: Example product crate `tau-wasm-embed-example`

**Files:**
- Modify: `Cargo.toml` (workspace members — insert `"crates/tau-wasm-embed-example",` directly after the `"crates/tau-wasm-host",` line 43)
- Modify: `ARCHITECTURE.md` (code-map table — an xtask guard fails CI for workspace members missing from it; insert after the `tau-embed-example` row, line 115): `| `tau-wasm-embed-example` | EPIC 7.2 Variant A reference host: a product-shaped binary that loads a built tau wasm component and runs it via `tau-wasm-host`'s `EmbedPorts` | `crates/tau-wasm-embed-example/src/main.rs` |`
- Create: `crates/tau-wasm-embed-example/Cargo.toml`
- Create: `crates/tau-wasm-embed-example/src/main.rs`
- Create: `crates/tau-wasm-embed-example/README.md`

**Interfaces:**
- Consumes: Task 2's `tau_wasm_host::embed::{run_component_with_ports, EmbedPorts, CompletionRequest, CompletionResponse, StopReason}`, `tau_runtime_core::stream::RunEvent`, `tau_ports` `CompletionResponse::new` (via re-export).
- Produces: binary `tau-wasm-embed-example <component.wasm> [prompt]` — exit 0 + `run completed: <n> events` on stdout iff a `RunCompleted` event was seen (Task 4's e2e relies on this exact contract).

- [ ] **Step 1: Create the crate manifest and workspace entry**

`crates/tau-wasm-embed-example/Cargo.toml`:

```toml
[package]
name = "tau-wasm-embed-example"
description = "EPIC 7.2 example: a product that embeds a built tau wasm component — supplies the four EmbedPorts host functions and consumes the live RunEvent stream."
version.workspace      = true
edition.workspace      = true
rust-version.workspace = true
license.workspace      = true
repository.workspace   = true
authors.workspace      = true
publish = false

[dependencies]
tau-wasm-host    = { path = "../tau-wasm-host" }
tau-runtime-core = { workspace = true, features = ["std"] }
serde_json       = { workspace = true }

[lints]
workspace = true
```

Add the workspace member line in the root `Cargo.toml`.

- [ ] **Step 2: Write main.rs with a failing-first unit test for arg parsing**

`crates/tau-wasm-embed-example/src/main.rs` (complete file):

```rust
//! tau-wasm-embed-example — EPIC 7.2 Variant A reference host.
//!
//! A "product" that embeds tau as a *component*: it loads a workflow built
//! with `tau build --target wasm` from disk (workflow-as-data — the product
//! binary never changes when the workflow does), supplies the four host
//! ports via [`EmbedPorts`], and prints every `RunEvent` live as a JSON
//! line. Offline out of the box: the LLM port answers with a canned reply.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tau_runtime_core::stream::RunEvent;
use tau_wasm_host::embed::{
    run_component_with_ports, CompletionRequest, CompletionResponse, EmbedPorts, StopReason,
};

const USAGE: &str = "usage: tau-wasm-embed-example <component.wasm> [prompt]";

/// The product's port surface: echo LLM, real wall clock, clock-seeded
/// entropy, and a live event sink. A real product supplies its inference
/// client (credentials stay host-side) and OS entropy here.
struct ProductPorts {
    entropy: AtomicU64,
    events: Arc<AtomicUsize>,
    completed: Arc<AtomicBool>,
}

fn wall_clock_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl EmbedPorts for ProductPorts {
    fn complete(&mut self, _req: CompletionRequest) -> Result<CompletionResponse, String> {
        Ok(CompletionResponse::new(
            "tau-wasm-embed-example reply".to_string(),
            Vec::new(),
            StopReason::EndTurn,
            None,
        ))
    }

    fn now_millis(&mut self) -> u64 {
        wall_clock_millis()
    }

    fn next_u64(&mut self) -> u64 {
        // xorshift64* — NOT cryptographic; a real product supplies OS entropy.
        let mut x = self.entropy.load(Ordering::Relaxed);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.entropy.store(x, Ordering::Relaxed);
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn on_event(&mut self, event_json: &str) {
        // Prove the typed contract: every line is a deserializable RunEvent
        // (via the product's own tau-runtime-core dep — exactly how a real
        // product consumes the stream).
        match serde_json::from_str::<RunEvent>(event_json) {
            Ok(event) => {
                if matches!(event, RunEvent::RunCompleted { .. }) {
                    self.completed.store(true, Ordering::Relaxed);
                }
                self.events.fetch_add(1, Ordering::Relaxed);
                println!("{event_json}");
            }
            Err(err) => eprintln!("unparseable RunEvent ({err}): {event_json}"),
        }
    }
}

fn parse_args(args: Vec<String>) -> Result<(PathBuf, String), String> {
    let mut it = args.into_iter();
    let component = it.next().ok_or(USAGE)?;
    let prompt = it
        .next()
        .unwrap_or_else(|| "hello from the product".to_string());
    if it.next().is_some() {
        return Err(USAGE.to_string());
    }
    Ok((PathBuf::from(component), prompt))
}

fn main() -> ExitCode {
    let (component, prompt) = match parse_args(std::env::args().skip(1).collect()) {
        Ok(parsed) => parsed,
        Err(usage) => {
            eprintln!("{usage}");
            return ExitCode::from(2);
        }
    };
    let bytes = match std::fs::read(&component) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("failed to read {}: {err}", component.display());
            return ExitCode::FAILURE;
        }
    };

    let events = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicBool::new(false));
    let ports = ProductPorts {
        entropy: AtomicU64::new(wall_clock_millis() | 1),
        events: Arc::clone(&events),
        completed: Arc::clone(&completed),
    };

    // No capabilities granted: the workflow gets no fs/net, whatever it asks
    // for. A real product passes the caps its governance approved.
    match run_component_with_ports(&bytes, &prompt, Box::new(ports), &[], Path::new(".")) {
        Ok(_sentinel) => {
            let seen = events.load(Ordering::Relaxed);
            if completed.load(Ordering::Relaxed) {
                println!("run completed: {seen} events");
                ExitCode::SUCCESS
            } else {
                eprintln!("run ended without RunCompleted ({seen} events)");
                ExitCode::FAILURE
            }
        }
        Err(err) => {
            eprintln!("embedding failed: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_args;

    #[test]
    fn component_path_is_required() {
        assert!(parse_args(vec![]).is_err());
    }

    #[test]
    fn prompt_defaults_when_omitted() {
        let (path, prompt) = parse_args(vec!["wf.wasm".to_string()]).unwrap();
        assert_eq!(path.to_str(), Some("wf.wasm"));
        assert_eq!(prompt, "hello from the product");
    }

    #[test]
    fn extra_args_are_rejected() {
        let args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert!(parse_args(args).is_err());
    }
}
```

`crates/tau-wasm-embed-example/README.md`:

```markdown
# tau-wasm-embed-example

EPIC 7.2 (Variant A) reference host: a "product" that embeds a **built tau
wasm component** instead of linking tau as a library (that's Variant B,
`tau-embed-example`).

    # in any governed tau project:
    tau build --target wasm -o workflow.wasm

    cargo run -p tau-wasm-embed-example -- workflow.wasm "hello"

The binary implements the four `EmbedPorts` host functions (canned echo
LLM, wall clock, xorshift entropy, stdout event sink) and prints each
`RunEvent` as a JSON line, live, followed by `run completed: <n> events`.

See `docs/how-to/embed-wasm-component.md` for the walkthrough and
`crates/tau-cli/tests/embed_wasm_e2e.rs` for the load-and-run gate.
```

- [ ] **Step 3: Build + test + lint gates**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-wasm-embed-example`
Expected: clean build.
Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-wasm-embed-example`
Expected: 3 parse_args tests PASS.
Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-wasm-embed-example --all-targets`
Expected: no warnings.

- [ ] **Step 4: Smoke the failure paths by hand**

Run: `CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo run -q -p tau-wasm-embed-example 2>&1; echo "exit=$?"`
Expected: usage line, `exit=2`.
Run: `CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo run -q -p tau-wasm-embed-example -- /nonexistent.wasm 2>&1; echo "exit=$?"`
Expected: `failed to read /nonexistent.wasm: ...`, `exit=1`.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/tau-wasm-embed-example
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(embed): tau-wasm-embed-example — Variant A product host (#414)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: Acceptance e2e — build the fixture component, run the example

**Files:**
- Create: `crates/tau-cli/tests/embed_wasm_e2e.rs`

**Interfaces:**
- Consumes: `tau_cli::cmd::build_wasm::lower_to_wasm_ir` (same as `build_wasm_e2e.rs`), Task 3's binary contract (exit 0 + `run completed:` + `RunCompleted` on stdout).
- Produces: the roadmap acceptance gate for #414.

- [ ] **Step 1: Write the test**

Create `crates/tau-cli/tests/embed_wasm_e2e.rs`. Copy the `fixture` and `build_trivial_component` helpers VERBATIM from `crates/tau-cli/tests/build_wasm_e2e.rs:8-73` (duplication is the file-local precedent), changing only the dedicated target dir to `target/tau-embed-wasm-e2e-guest`. Then:

```rust
//! EPIC 7.2 DoD (#414): a product runtime loads a built tau wasm component
//! and runs it. Builds the trivial fixture component, then drives the
//! `tau-wasm-embed-example` binary against it — the roadmap acceptance
//! ("example loads + runs") as a test.

#[test]
#[ignore = "builds a wasm component; run with --run-ignored"]
fn example_product_loads_and_runs_component() {
    let component = build_trivial_component();
    let wasm_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(wasm_file.path(), &component).unwrap();

    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .to_path_buf();

    let run = Command::new(env!("CARGO"))
        .current_dir(&workspace_root)
        .args(["run", "--quiet", "-p", "tau-wasm-embed-example", "--"])
        .arg(wasm_file.path())
        .arg("hi")
        .env("CARGO_INCREMENTAL", "0")
        .env(
            "CARGO_TARGET_DIR",
            workspace_root.join("target/tau-embed-wasm-e2e"),
        )
        .output()
        .expect("cargo spawn");

    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        run.status.success(),
        "example product failed\n--- stdout\n{stdout}\n--- stderr\n{stderr}"
    );
    assert!(
        stdout.contains("RunCompleted"),
        "expected a terminal RunCompleted event line:\n{stdout}"
    );
    assert!(
        stdout.contains("run completed:"),
        "expected the product's summary line:\n{stdout}"
    );
    assert!(
        !stderr.contains("unparseable RunEvent"),
        "every emitted event must parse as a typed RunEvent:\n{stderr}"
    );
}
```

(`use std::path::PathBuf; use std::process::Command;` at the top, as in build_wasm_e2e.rs; `tempfile` is already a tau-cli dev-dependency.)

- [ ] **Step 2: Run the acceptance test and read the output**

Run: `timeout 600 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli -E 'binary(embed_wasm_e2e)' --run-ignored all`
Expected: PASS (Tasks 2–3 are already in place, so this test passes on first honest run). Because it never failed first, verify it bites: rerun once with the `stdout.contains("RunCompleted")` assertion inverted to `!stdout.contains(...)` and confirm that FAILS, then revert the inversion. Do not skip this.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-cli/tests/embed_wasm_e2e.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "test(cli): embed_wasm_e2e — example product loads + runs the component (closes #414 acceptance)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: Docs — how-to page + SUMMARY + book build

**Files:**
- Create: `docs/how-to/embed-wasm-component.md`
- Modify: `docs/SUMMARY.md` (insert after the `embed-rust-native.md` line 33)

- [ ] **Step 1: Write the how-to**

`docs/how-to/embed-wasm-component.md`:

```markdown
# Embed a tau wasm component (Variant A)

Run a workflow built with `tau build --target wasm` inside your own
product. Your binary never links tau's runtime — the workflow arrives as
a `.wasm` file, so you can update it without recompiling the product, and
the capability grants you pass are enforced at the wasm boundary: fs/net
you didn't grant is physically unreachable from the workflow.

Prefer this over [native linking](embed-rust-native.md) (Variant B) when
you need workflows-as-data, sandboxed third-party workflows, or a
non-Rust host later. Prefer Variant B for a pure-Rust product running
trusted workflows — it is lighter (no wasmtime).

## The contract

The component imports exactly four host functions; you implement them as
the `EmbedPorts` trait from `tau_wasm_host::embed`:

| Port | You supply |
|---|---|
| `complete(CompletionRequest) -> Result<CompletionResponse, String>` | your LLM client (credentials stay host-side; the WIT import is sync — block inside) |
| `now_millis() -> u64` | wall clock |
| `next_u64() -> u64` | OS entropy |
| `on_event(&str)` | live sink; one serialized `RunEvent` JSON per call, in order |

Then one call runs the workflow:

```rust,ignore
use tau_wasm_host::embed::run_component_with_ports;

let bytes = std::fs::read("workflow.wasm")?;
run_component_with_ports(&bytes, "your prompt", Box::new(my_ports), &caps, sandbox_root)?;
```

Ports are owned by the run (`Box`); keep an `Arc` handle inside your impl
for anything you need afterwards (event counts, transcripts).

`on_event` receives raw JSON so your host stays dependency-light; add
`tau-runtime-core` (feature `std`) and `serde_json::from_str::<RunEvent>`
if you want typed events — the reference example does.

## Worked example

`crates/tau-wasm-embed-example` is a complete ~180-line product host:
echo LLM, wall clock, xorshift entropy, stdout event sink.

```text
tau build --target wasm -o workflow.wasm     # in your governed project
cargo run -p tau-wasm-embed-example -- workflow.wasm "hello"
```

It prints each `RunEvent` as a JSON line, live, then
`run completed: <n> events`. The load-and-run acceptance test is
`crates/tau-cli/tests/embed_wasm_e2e.rs`.

## Determinism note

The deterministic conformance entrypoints (`run_component`,
`run_component_with_caps` at the crate root) run the same machinery with
canned ports — cassette LLM, fixed-step clock, seeded PRNG. They are for
conformance testing, not embedding.
```

- [ ] **Step 2: Add the SUMMARY entry**

In `docs/SUMMARY.md`, directly after `- [Embed tau in a Rust product](how-to/embed-rust-native.md)` (line 33), insert:

```markdown
- [Embed a tau wasm component](how-to/embed-wasm-component.md)
```

- [ ] **Step 3: Build the book (DOCS RULES gate)**

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build`
Expected: only `[INFO]` lines. Then: `rm -rf docs/book`.

- [ ] **Step 4: Commit**

```bash
git add docs/how-to/embed-wasm-component.md docs/SUMMARY.md
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "docs(how-to): embed a tau wasm component (Variant A) (#414)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: Full gates, PR, follow-up issues

**Files:** none new (verification + PR).

- [ ] **Step 1: Format + lint gates on every touched crate**

First apply formatting (clears the deferred fmt minors from Tasks 1-2 in one shot), then verify:

```bash
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-wasm-host -p tau-wasm-embed-example -p tau-cli
git diff --stat   # review what fmt touched; commit as chore(fmt) if non-empty
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-wasm-host -p tau-wasm-embed-example -p tau-cli --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-wasm-host --all-targets
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-wasm-embed-example --all-targets
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-cli --all-targets
```
Expected: all clean (CI treats warnings as errors).

- [ ] **Step 2: Test gates**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-wasm-host
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-wasm-embed-example
timeout 600 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-wasm-host --run-ignored all
timeout 600 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli -E 'binary(embed_wasm_e2e) or binary(build_wasm_e2e)' --run-ignored all
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-wasm-host
```
Expected: all PASS. Do not claim success without reading each summary line.

- [ ] **Step 3: Push and open the PR**

```bash
git push -u origin feat/epic-7-2-wasm-embed
gh pr create --base main --title "feat(wasm-host): EPIC 7.2 — Variant A wasm component embedding API + example" --body "$(cat <<'EOF'
Closes #414 (EPIC 7.2).

- `EmbedPorts` trait: the four `tau:host/runner` imports become a product-implementable seam; deterministic conformance fakes are now one impl (`DeterministicPorts`) — `run_component`/`run_component_with_caps` behavior byte-identical (existing tests unchanged).
- `run_component_with_ports` + `tau_wasm_host::embed` prelude (mirrors `tau_runtime_core::embed`, EPIC 7.1).
- `tau-wasm-embed-example`: product host loading a `.wasm` from argv, live typed `RunEvent` stream.
- Acceptance: `crates/tau-cli/tests/embed_wasm_e2e.rs` builds the trivial fixture component and drives the example (`--run-ignored`, wasm lane).
- Docs: `docs/how-to/embed-wasm-component.md`.

Spec: `docs/superpowers/specs/2026-08-22-epic-7-2-wasm-embed-design.md`; plan: `docs/superpowers/plans/2026-08-22-epic-7-2-wasm-embed.md`.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
gh pr merge --squash --auto
```

(NO `--delete-branch` — it conflicts with the merge queue. If main moves while CI runs: `gh pr update-branch <N>`; if auto-merge drops after churn, re-enroll bare `gh pr merge <N> --auto`.)

- [ ] **Step 4: File the follow-up issues**

```bash
gh issue create --title "On-shelf language packages for Variant A embedding (npm/pip wrappers around EmbedPorts)" --body "Follow-up to #414 / EPIC 5 lane: idiomatic JS/Python packages wrapping the tau:host/runner contract (EmbedPorts) so a product does npm/pip install instead of hand-wiring a wasm runtime. Needs a publishing story (crates not on crates.io, workspace v0.0.0). Raised during EPIC 7.2 brainstorm."
gh issue create --title "tau embed --host wasm: scaffold an EmbedPorts product host" --body "Follow-up to #414: add a wasm-host template to tau-sdk-codegen scaffolding an EmbedPorts impl + run_component_with_ports wiring, mirroring the embed-rust template (EPIC 5.2/7.1). Gated on the 7.2 API staying stable for one release."
```

- [ ] **Step 5: Babysit mergeability**

Watch CI; on `BEHIND`, `gh pr update-branch <N>`; GitHub auto-merges when green and up-to-date.
