# Embed tau in a Rust product

Variant B embedding (EPIC 7.1): your product links tau as a **no_std
library**. tau compiles the workflow to canonical IR at build time; at run
time your product supplies the ports (LLM, tools, clock, entropy) and drives
the interpreter. Nothing else of tau ships in your binary — no CLI, no
server, no wasm runtime.

The curated API surface is one module:
[`tau_runtime_core::embed`](https://github.com/tau-rs/tau/blob/main/crates/tau-runtime-core/src/embed.rs).
The worked example is
[`crates/tau-embed-example`](https://github.com/tau-rs/tau/tree/main/crates/tau-embed-example).

## Generate the artifacts

From a governed project (a `tau.toml` with an `[allow]` constitution — see
[Build embedding artifacts](build-embedding-artifacts.md)):

```bash
tau build --target rust-lib ./my-flow --tau-dep-path /path/to/tau-checkout
tau embed --host rust       ./my-flow --tau-dep-path /path/to/tau-checkout -o ./my-flow/my-flow-rust-lib
```

The first emits the **library** crate (baked `TAU_IR` bytes + `run_ir` /
`run_ir_streaming` re-exports); the second emits a **runnable host
scaffold** under `embed-rust/` beside it. `cargo run` inside `embed-rust/`
works immediately, offline: the scaffold ships an echo LLM backend, a
reject-all tool dispatcher, and std clock/entropy — each marked `REPLACE`
for your product's real ports.

`--tau-dep-path` makes the generated `Cargo.toml` reference the tau crates
by filesystem path (a tau checkout root). It is required today: the tau
crates are not yet published to crates.io, so the default version
dependency cannot resolve outside this repository.

## The entry-point contract

`run_ir(module, entry, dispatcher, messages)` always takes an explicit
entry `AgentId`. Which agent that is, is settled by one rule:

> A module is **directly runnable** iff it contains exactly one agent —
> that agent is the entry point. `IrModule::entry_agent()` returns it, and
> errors (listing the candidates) on empty or multi-agent modules, which
> must pass an explicit `AgentId` instead.

The wasm guest enforces the same sole-agent rule at load, so both
embedding variants share one contract.

## The ports you implement

| Port | Mandatory | Where | Notes |
|---|---|---|---|
| `Clock` | yes | `ToolDispatcher::clock()` | ms since Unix epoch; run panics without it |
| `RandomSource` | yes | `ToolDispatcher::random()` | session-id/ULID entropy; run panics without it |
| LLM backend | yes | `ToolDispatcher::llm_backend_for()` | implement `LlmBackend`; build responses with `CompletionResponse::new` |
| Tool execution | per-workflow | `ToolDispatcher::invoke()` | reject-all is valid when the workflow declares no tools |

Everything above is importable from `tau_runtime_core::embed` — the module
doc walks the four steps (decode, entry, ports, drive).

## Drive it

The heart of `tau-embed-example` (abridged):

```rust
use tau_runtime_core::embed::{from_canonical_bytes, run_ir_streaming};

let module = Arc::new(from_canonical_bytes(TAU_IR)?);
let entry = module.entry_agent()?.clone();
let stream = run_ir_streaming(module, &entry, Arc::new(ProductDispatcher::new()), Vec::new()).await?;
futures::pin_mut!(stream);
while let Some(event) = stream.next().await {
    println!("{}", serde_json::to_string(&event)?);
}
```

```bash
cargo run -p tau-embed-example
```

```text
{"type":"run_started"}
{"type":"inference_call_started"}
[…]
{"type":"run_completed","outcome":{…}}
```

The terminal `RunCompleted` event fires exactly once. Prefer plain
`run_ir` over `run_ir_streaming` if you only want the final `RunOutcome`.

## Governance

The build-time gate is the enforcement point: `tau build --target
rust-lib` refuses ungoverned projects (no `[allow]`), over-reaching
capability requests, and control-flow the embedded interpreter cannot
execute. `run_ir` itself trusts the IR bytes it is given — provenance of
the baked IR is your product's responsibility (build from source you
control, or verify bundles with `tau verify` upstream).

## Variant A (wasm guest)

Embedding the workflow as a sandboxed wasm component in your product's
runtime is EPIC 7.2 — see the
[embedding artifacts how-to](build-embedding-artifacts.md) for the
`wasm-guest` target and C host stub it will complete.
