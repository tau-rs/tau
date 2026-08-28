# Embed a tau wasm component (Variant A)

Run a workflow built with `tau build --target wasm` inside your own
product. Your binary never links tau's interpreter or workflow engine —
the workflow arrives as a `.wasm` file, so you can update it without
recompiling the product (as long as the component and the embedding host
come from compatible tau versions — see "Version compatibility" below).
The capability grants you pass are enforced at the wasm boundary: fs/net
you didn't grant is physically unreachable from the workflow. Today the
supported way to obtain a real, non-empty `caps` value is deserializing it
from a package manifest via `tau-pkg` (the same manifest `tau build`
reads) — the curated `Capability` constructors are behind tau-domain's
test-only `test-fixtures` feature, not part of the embedding surface. The
worked example below deliberately grants none (`&[]`): no fs/net reaches
the workflow.

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

Then one call runs the workflow — but **`Ok` from this call means only
that the guest's `run` returned its `ok` arm, not that the workflow
succeeded.** Once baked IR decodes, `run` always reaches `ok`; a mid-run
failure (an LLM error, for example) arrives as a `RunEvent::FatalError`
event on your `on_event` sink, with this call still returning `Ok`. `Err`
here covers only load/instantiate/trap failures and the guest's own
load-time error arm. To know whether the workflow actually completed,
track whether you observed a terminal `RunEvent::RunCompleted`:

```rust,ignore
use tau_wasm_host::embed::run_component_with_ports;

let bytes = std::fs::read("workflow.wasm")?;
let sandbox_root = std::path::Path::new("."); // safe only because `caps` is empty below
match run_component_with_ports(&bytes, "your prompt", Box::new(my_ports), &caps, sandbox_root) {
    Ok(_sentinel) if my_ports_saw_run_completed() => { /* the workflow actually finished */ }
    Ok(_sentinel) => { /* run ended without RunCompleted — check FatalError events */ }
    Err(e) => { /* load/instantiate/trap failure */ }
}
```

`crates/tau-wasm-embed-example/src/main.rs` shows the real pattern: its
`ProductPorts::on_event` sets a shared `completed: Arc<AtomicBool>` when it
sees `RunEvent::RunCompleted`, and `main` checks that flag after the call
returns `Ok` before declaring success.

Ports are owned by the run (`Box`); keep an `Arc` handle inside your impl
for anything you need afterwards (event counts, transcripts).

`on_event` receives raw JSON so your host stays dependency-light; add
`tau-runtime-core` (feature `std`) and `serde_json::from_str::<RunEvent>`
if you want typed events — the reference example does.

## Worked example

`crates/tau-wasm-embed-example` is a complete ~160-line product host:
echo LLM, wall clock, xorshift entropy, stdout event sink. It links
`tau-runtime-core` (for typed `RunEvent` deserialization) but not tau's
interpreter or workflow engine — that stays entirely in the `.wasm` file.

```text
tau build --target wasm -o workflow.wasm     # in your governed project
cargo run -p tau-wasm-embed-example -- workflow.wasm "hello"
```

It prints each `RunEvent` as a JSON line, live, then, only if it observed a
terminal `RunCompleted` event, `run completed: <n> events` (otherwise it
prints a warning and exits non-zero — see the `completed` flag pattern
above). The load-and-run acceptance test is
`crates/tau-cli/tests/embed_wasm_e2e.rs`.

## Version compatibility

The component and the embedding host must come from compatible tau
versions. The `complete` port round-trips `tau_ports::llm::CompletionRequest`
and `CompletionResponse` as JSON across the wasm boundary, and neither type
has serde defaults — they are tau's own serde types, not a stable wire
schema. Run an older component against a newer host (or vice versa) after a
field is added to either type, and every `complete` call fails. This
doesn't come back as an `Err` from `run_component_with_ports`: it arrives
as a `RunEvent::FatalError` event carrying a "malformed CompletionRequest"
(or response) message, same as any other mid-run failure.

## Determinism note

The deterministic conformance entrypoints (`run_component`,
`run_component_with_caps` at the crate root) run the same machinery with
canned ports — cassette LLM, fixed-step clock, seeded PRNG. They are for
conformance testing, not embedding.
