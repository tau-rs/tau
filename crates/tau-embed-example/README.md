# tau-embed-example

EPIC 7.1 (Variant B) reference host: a "product" that embeds tau as a
library. It bakes the canonical IR of the governed workflow in
`project/tau.toml` (committed as `fixtures/trivial.ir.json`; a drift test in
tau-cli keeps the two byte-equal), implements the embedding ports — LLM
backend (offline echo), tool dispatch (reject-all; the workflow declares no
tools), `Clock`, and `RandomSource` — and drives `run_ir_streaming`,
printing every `RunEvent` as a JSON line.

Run it:

```sh
cargo run -p tau-embed-example
```

See `docs/how-to/embed-rust-native.md` for the full Variant B walkthrough
and `tau_runtime_core::embed` for the curated API surface.
