# embed-native — Variant B (no_std lib) embedding example

EPIC 7.1: a worked example of a product **embedding tau as a no_std library**
rather than shelling out to the `tau` CLI. The product links the generated
workflow lib directly into its own binary and implements the runtime ports
(`ToolDispatcher`, `LlmBackend`, `Clock`, `Random`) itself, in Rust, with no
process boundary.

## What Variant B is

tau ships two embedding shapes (see `docs/superpowers/plans/vision-roadmap.md`,
EPIC 7):

- **Variant A** (7.2) — the product embeds a **wasm guest** (tau's compiled
  workflow as a Wasm component) inside its own runtime.
- **Variant B** (7.1, this example) — the product links tau's **no_std Rust
  lib** (a `tau build --target rust-lib` artifact) as an ordinary Rust
  dependency, and supplies its own implementations of the runtime ports
  instead of using tau's built-in host adapters (no wasmtime, no CLI, no
  subprocess).

Variant B is the right shape when the product already controls its own
process/executable (e.g. firmware, a native app, a service binary) and wants
tau's IR interpreter and workflow semantics without carrying tau's CLI,
Wasm runtime, or std-heavy host adapters.

## Tree

```
examples/embed-native/
├── workflow/           source tau.toml — 1 agent (`assistant`), 1
│                       capability-free tool (`echo`), backend `anthropic`,
│                       max_turns = 4
├── workflow-lib/        generated 5.1 artifact (`tau build --target rust-lib`):
│                       #![no_std] crate exporting `TAU_IR: &[u8]`,
│                       `TAU_IR_HASH`, and `pub use tau_runtime_core::run_ir`
└── host/                the Variant-B product crate: implements the ports
                        and links workflow-lib + tau-runtime-core to run it
```

`workflow/` → `workflow-lib/` → `host/` is the full pipeline: author once,
generate the portable lib artifact, then link it from a product that
supplies its own ports.

`host/src/`:

- `llm.rs` — `ScriptedLlmBackend`, a deterministic FIFO-turn implementation
  of `tau_ports::LlmBackend` (no `test-fixtures` feature — this is a real,
  externally-implementable port, not a test double baked into tau).
- `ports.rs` — `SystemClock` (std-backed `Clock`) and `HostRandom` (a
  time-seeded xorshift `Random`).
- `dispatcher.rs` — `HostDispatcher`, implementing tau-runtime-core's
  `ToolDispatcher`: `invoke` runs the `echo` tool, `llm_backend_for` returns
  the scripted backend, `clock`/`random` return the real ports above.
- `main.rs` — decodes `TAU_IR` via `tau_ir::from_canonical_bytes`, calls
  `run_ir`, and prints the resulting `RunOutcome`.
- `tests/runs.rs` — asserts the embedding runs to completion in CI.

## Run and test

```bash
CARGO_TARGET_DIR=target/main cargo run -p embed-native-host
```

prints a `{:#?}`-formatted `Completed { .. }` outcome (abbreviated below — the
real derive(Debug) output nests the full `Message` struct and orders fields
as declared) with `total_turns: 2` and a `final_message` whose payload is
`Text { content: "done" }`, then exits 0 (one tool-call turn for `echo`, one
final text turn).

```bash
CARGO_TARGET_DIR=target/main cargo test -p embed-native-host
```

runs the crate's unit tests plus `tests/runs.rs`.

(Per this repo's cargo rules, always set `CARGO_TARGET_DIR` and scope with
`-p`; see the root `CLAUDE.md` for the full policy.)

## Regenerating `workflow-lib/`

`workflow-lib/` is generated output — **do not hand-edit `workflow-lib/src/lib.rs`**.
A drift test (`crates/tau-cli/tests/embed_native_lib_drift.rs`) byte-compares
the committed file against a fresh build and fails the build if they diverge.

To regenerate after changing `workflow/tau.toml`:

```bash
tau build --target rust-lib --allow-ungoverned \
  -o examples/embed-native/workflow-lib examples/embed-native/workflow
```

Note: the committed `workflow-lib/Cargo.toml` turns on tau-runtime-core's
`wasm-interpreter` feature (it gates `run_ir`), with `default-features =
false` to keep the crate `no_std`. The stock 5.1 template omits this
feature — it's added by hand when wiring the generated artifact into this
example's workspace.

## Limitations (documented, not bugs)

1. **This proves "links and runs," not a bare-metal no_std compile.**
   `host` (std + tokio) and `workflow-lib` (`default-features = false`)
   build together in one Cargo workspace. Cargo's feature unification means
   `host`'s std-enabled dependency graph turns std back on for
   `tau-runtime-core` for the whole build. So this example demonstrates that
   the no_std lib artifact links and runs correctly inside a product — it
   does **not** demonstrate a true target-isolated bare-metal build. A
   from-scratch, feature-isolated no_std build belongs to the gated MCU work
   in 7.3/7.4, out of scope for 7.1.

2. **`HostRandom` is non-cryptographic.** It's a dependency-free, time-seeded
   xorshift generator, chosen so the example has no extra crates and no
   platform entropy source to wire up. A real product implementing the
   `Random` port should wrap `getrandom` or an OS/hardware entropy source
   instead. The example's asserted outcome does not depend on entropy
   quality — `HostRandom` is exercised but never asserted on.

## Building a real product on this pattern

To turn this into a real integration, replace `ScriptedLlmBackend` in
`llm.rs` with a provider adapter (e.g. an Anthropic or OpenAI HTTP client)
that implements the same `tau_ports::LlmBackend` trait, and return it from
`HostDispatcher::llm_backend_for` in place of the scripted backend. Nothing
else in the pipeline (`workflow-lib`, `dispatcher.rs`'s tool routing, ports
wiring) needs to change.
