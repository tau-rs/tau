# EPIC 7.1 — Variant B no_std embedding API + example (design)

Issue #413. First EPIC 7 story. Fills the `TODO(7.1)` stubs EPIC 5.2 (#611)
left in the embed-rust template, and makes the EPIC 5.1 rust-lib artifact
actually compilable + runnable by a host product.

## Problem

`tau build --target rust-lib` (EPIC 5.1) and `tau embed --host rust`
(EPIC 5.2) emit source scaffolds that are only string-checked, never
compiled. Verified defects:

1. **rust-lib cannot compile.** Generated `Cargo.toml` pins
   `tau-runtime-core = { version, default-features = false }`, but the
   re-exported `run_ir` is gated behind the non-default `wasm-interpreter`
   feature (`tau-runtime-core/Cargo.toml` `[features]`). The wasm guest —
   the only real embedder today — enables
   `features = ["wasm-interpreter", "tool-validation"]`.
2. **Deps cannot resolve.** `tau-runtime-core`/`tau-ir` are not on
   crates.io (workspace version `0.0.0`), so the generated version deps
   are unresolvable outside this repo.
3. **Entry point is arbitrary.** `embed_rust.rs:100` picks
   `agents.keys().next()`; there is no documented contract and no accessor.
4. **Port bodies are `todo!()`.** `ToolDispatcher::invoke` /
   `llm_backend_for` stubs; the inherited `clock()`/`random()` defaults
   return `None`, which panics at run ("host shell must supply a clock",
   `tool_dispatch.rs:96-110`).

## Approaches considered

- **A. New `EmbedEngine` wrapper type** (builder that owns decode + ports +
  run). Rejected: `run_ir(Arc<IrModule>, &AgentId, Arc<dyn ToolDispatcher>,
  Vec<Message>)` already *is* a minimal engine constructor; a wrapper adds a
  second API to keep in sync and hides the streaming variant.
- **B. Curate the existing surface** (chosen): a documented
  `tau_runtime_core::embed` prelude + a `tau-ir` entry-point helper + fix
  the templates so the generated crates compile and run. Matches the
  handoff's "prefer exposing/curating what tau-runtime-core already has".
- **C. Richer codegen** (bake a working dispatcher into rust-lib itself).
  Rejected: port impls are the product's code; baking them into the no_std
  lib couples it to std and over-generates.

## Design (approach B)

### 1. Entry-point contract (tau-ir)

New method on `IrModule`:

```rust
/// Variant B embedding entry-point contract: a module is directly
/// runnable iff it contains exactly one agent — that agent is the entry
/// point. Multi-agent modules must select explicitly via `run_ir`'s
/// `entry` parameter.
pub fn entry_agent(&self) -> Result<&AgentId, EntryAgentError>
```

`EntryAgentError` (thiserror, no_std): `NoAgents` | `Ambiguous { available:
Vec<AgentId> }` (message lists the ids). Precedent: the wasm guest already
enforces exactly-one-agent at load (`guest.rs:157`); it is refactored to use
the helper so the contract has one home. The embed-rust template replaces
`agents.keys().next()` with `module.entry_agent()?`.

### 1b. `CompletionResponse::new` (tau-ports)

`CompletionResponse` is `#[non_exhaustive]` and its only constructor
(`fixtures::make_completion_response`) is gated behind the std-only
`test-fixtures` feature — an external product cannot construct an LLM
response at all. Add a public inherent constructor
`CompletionResponse::new(text, tool_uses, stop_reason, usage)` (additive,
non-breaking; the fixtures helper delegates to it).

### 2. Curated embedding prelude (tau-runtime-core, public surface only)

`pub mod embed` (gated on `wasm-interpreter`, like `interpreter`/`stream`):
re-exports `run_ir`, `run_ir_streaming`, `ToolDispatcher`,
`ToolInvocationResult`, `RunEvent`, `RunOutcome`, `RuntimeError`,
`DynLlmBackend`, plus the port traits/types an embedder implements
(`tau_ports::{Clock, RandomSource, LlmBackend, CompletionRequest,
CompletionResponse, CompletionStream, LlmError}`, `tau_ir::{AgentId,
IrModule, from_canonical_bytes, EntryAgentError}`). The module doc is the
embedding API documentation (mirrored by the docs how-to). No interpreter
internals change (4.5 lane stays disjoint).

### 3. Template fixes (tau-sdk-codegen)

`RustLibInput`/`EmbedRustInput` gain a `tau_dep: TauDep<'a>` input:

```rust
pub enum TauDep<'a> {
    Version(&'a str),          // { version = "X", ... }  (post-publish default)
    Path(&'a str),             // { path = "<dir>/<crate>", ... }
}
```

- rust-lib `Cargo.toml`: `default-features = false, features =
  ["wasm-interpreter"]` (fixes defect 1); lib.rs re-exports `run_ir` and
  `run_ir_streaming`.
- embed-rust: drop `tokio` (replace with `futures` `block_on` +
  `StreamExt`) — lighter generated crate, cheaper e2e compile; fill the
  stubs so the scaffold **runs offline out of the box**:
  - `EchoBackend: LlmBackend` — canned single-turn completion (clearly
    marked "replace with your product's inference"); `stream` returns the
    same content as a single-chunk stream (or delegates to `complete` if
    `CompletionStream` construction requires it).
  - `Dispatcher::invoke` → structured `RuntimeError` "tool not wired:
    <id>" (trivial workflows have no tools; products fill this in).
  - `clock()`/`random()` overridden with std impls (`SystemTime` millis;
    xorshift64 seeded from the clock — scaffold-grade, documented).
  - entry via `module.entry_agent()?`; drive `run_ir_streaming`, print
    each `RunEvent` as JSON lines (DoD: "prints events").
- CLI: `--tau-dep-path <dir>` optional flag on `tau build --target
  rust-lib` and `tau embed --host rust` → `TauDep::Path`; default stays
  `Version(<workspace version>)`. Until tau publishes to crates.io, the
  flag (documented in the how-to + generated README) is how a real user
  gets a buildable artifact from a git checkout.

### 4. E2E compile-and-run test (TDD anchor, replaces string-only checking)

`crates/tau-cli/tests/embed_rust_e2e.rs`:

1. tempdir; `emit_rust_lib_to(trivial fixture, dir, TauDep::Path(workspace
   crates dir))`; `emit_host_to("rust", fixture, dir, TauDep::Path(..))` —
   layout is rust-lib at root + `embed-rust/` (path dep `..`), exactly what
   the CLI ships.
2. `cargo run` in `embed-rust/` (`CARGO_TARGET_DIR` = tempdir-local,
   `CARGO_INCREMENTAL=0`), assert exit 0 and stdout contains a
   `RunCompleted` event.

Known empirical risk: outside the workspace the root `[patch.crates-io]
async-stream` vendor patch does not apply; upstream `async-stream` 0.3 is
std-only, which is fine for a native host build — if it fails to resolve,
the emitted (or test-written) manifest gains the patch and the how-to
documents it. String-assertion tests in `embed_rust.rs` are updated to the
new template (no `todo!()` markers left).

### 5. Example host product

New workspace member `crates/tau-embed-example` (bin, publish = false),
deps: `tau-runtime-core` (`default-features = false`,
`["wasm-interpreter"]`), `tau-ir`, `tau-ports`, `futures`, `serde_json` —
a product that never touches the CLI:

- owns a governed workflow project (`project/tau.toml` with `[allow]`) and
  the pre-lowered canonical IR bytes committed as a fixture
  (`include_bytes!`);
- implements SystemClock / xorshift `RandomSource` / `EchoBackend` /
  dispatcher; resolves entry via `entry_agent()`; runs
  `run_ir_streaming`, prints events;
- `#[test]` drives the same path and asserts a `RunCompleted` outcome (CI
  coverage without running the bin);
- drift guard in tau-cli tests: lower the example's project and
  byte-compare against the committed IR fixture (same pattern as the SDK
  byte-equal tests).

### 6. Docs

`docs/how-to/embed-rust-native.md` (+ `SUMMARY.md`): Variant B how-to —
build the rust-lib artifact, generate the host scaffold, port table
(mandatory: Clock, RandomSource, LlmBackend, ToolDispatcher; optional:
tools/storage), the entry-point contract, the unpublished-crates
`--tau-dep-path` note, pointer to `tau-embed-example`. Verified with
`mdbook build`.

## Testing summary

- unit: template renders (updated assertions), `entry_agent()` (0/1/n
  agents), `TauDep` rendering.
- integration: embed_rust_e2e (compile + run generated artifact),
  example-crate run test, IR fixture drift guard.
- gates: nextest `-p tau-sdk-codegen`, `-p tau-cli` (new tests),
  `-p tau-embed-example`, `-p tau-ir`; no_std: `cargo check -p
  tau-runtime-core --no-default-features --features wasm-interpreter`
  and the same shape for `tau-ir`; mdbook build.

## Out of scope

7.2 (wasm-guest embedding / embed-c completion — follow-up), interpreter
internals (4.5 lane), publishing crates to crates.io, MCU paths (7.3/7.4).
