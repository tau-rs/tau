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
