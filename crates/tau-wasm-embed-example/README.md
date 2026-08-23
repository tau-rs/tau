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
