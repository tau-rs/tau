# wasm-guest tool-arg validation parity — design

**Date:** 2026-06-22
**Status:** Approved (brainstorm) — closes the EPIC 0 deferred follow-up
**Relates to:** EPIC 0 (`2026-06-22-epic-0-destd-run-loop-design.md`), the conformance
"no surprises" conviction in `docs/explanation/tau-philosophy.md`.

## Problem

After EPIC 0, `tau-runtime-core`'s `tool-validation` feature is `no_std`. But
`tau-wasm-guest` still depends on `tau-runtime-core` with only
`features = ["wasm-interpreter"]` — **not** `tool-validation`. So in production the
wasm guest skips per-call tool-argument validation while the host (`tau dev`, CLI)
enforces it. The conformance gate demands host and guest produce identical traces; a
guest that silently skips a `BadArgs` self-correction the host emits is a cross-target
divergence. The guest's Cargo comment justifying the opt-out
(`tool-validation→jsonschema→std`) is now **stale** — that std pull was removed in PR-0b.

## Decision (Option A)

Enable `tool-validation` on the guest's `tau-runtime-core` dependency. The guest drives
`run_ir_streaming → run_agent_streaming → RuntimeBuilder::build()`, and `build()` compiles
validators from each baked-IR tool's `input_schema` under `#[cfg(feature = "tool-validation")]`
— so enabling the feature is sufficient; **no guest code change**. The rejection behaviour on
this exact interpreter path is already proven `no_std` by EPIC 0's `no_std_validation` test
(PR-0c). This change makes the guest exercise it.

```toml
# crates/tau-wasm-guest/Cargo.toml — BEFORE
tau-runtime-core = { path = "../tau-runtime-core", default-features = false, features = ["wasm-interpreter"] }
# AFTER
tau-runtime-core = { path = "../tau-runtime-core", default-features = false, features = ["wasm-interpreter", "tool-validation"] }
```
Plus: delete/replace the stale comment block above that dep (the one claiming
`tool-validation` drags in `jsonschema`/std).

**Not** in scope (deferred to the wasm-execution conformance lane, β.6 / "PR-G"): a
dedicated in-wasm test that scripts the guest's baked LLM stub to emit bad args and asserts
the rejection *inside* wasmtime. That cross-target trace-parity proof belongs to the
conformance gate, not this follow-up.

## Verification

The risk is behavioural: enabling validation could now **reject** args in an existing guest
scenario whose stubbed LLM emits schema-violating args. So:

1. Guest links: `cargo build -p tau-wasm-guest --target wasm32-wasip2 --release` clean (the
   β.7.5 link gate — proves no std leak with the feature on).
2. Existing guest execution tests still pass with validation on:
   `cargo nextest run -p tau-wasm-host --run-ignored` for `fan_monitor_simple` + `roundtrip`
   (they build the guest with the new feature and run it in wasmtime). If either fails because
   its scenario's args don't satisfy a tool schema, that's a real finding — fix the
   fixture/schema, do not disable validation.
3. `cargo tree -p tau-wasm-guest --target wasm32-wasip2 -i jsonschema` → absent (the feature is
   no_std; no std crate sneaks in).

## DoD

Guest builds for wasm32-wasip2 with `tool-validation` on; existing guest execution tests green;
no std/jsonschema pulled into the guest; the stale comment is corrected. The guest now validates
tool args identically to the host (rejection path proven by the EPIC 0 no_std test on the shared
interpreter path).
