# ADR-0054: in-wasm MCP facilitator

**Status:** Accepted
**Date:** 2026-06-20
**Deciders:** Titouan (architect), implementing session
**Supersedes:** none
**Renumbered from:** the β.7.5 design's "ADR-0050" (0050/0051/0052 were taken
by output-schema, the tau-ir crate split, and per-agent model resolution).

## Context

The β.7.5 wasm guest runs the workflow IR with no host imports beyond
inference, clock, and randomness (ADR-0046). The canonical β.6 fan-monitor
includes an MCP `weather` tool. To run that scenario in-guest the facilitator
must execute inside the wasm component — a host MCP import would re-introduce
a transport the determinism + parity story (ADR-0049) does not account for.

`tau-mcp` is `#![no_std]` and already contains the pure cassette
`Replayer` (`crates/tau-mcp/src/cassette/replayer.rs`); the std pieces
(`CassetteTransport`, `McpClient`) live behind `with-std-adapters` /
`tau-mcp-tokio`.

## Decision

1. The MCP facilitator runs **in-guest** on the no_std `tau-mcp` types. The
   conformance `weather` tool replays a **cassette baked into the component**
   via `tau_mcp::cassette::Replayer` — zero host import.
2. A no_std MCP client path over `Replayer` is built for the guest; the std
   `tau-mcp-tokio` `McpClient` stays the host/dev path. Both consume the same
   cassette bytes, so dev and wasm replay identically.
3. **Real (non-cassette) MCP transport from inside wasm is reserved for γ.1**
   via a future `tau:mcp` WIT import slot. β.7.5 ships cassette-only.

## Consequences

- The simplified fan-monitor (PR-F) needs no MCP and ships first; the full
  fan-monitor with in-guest `weather` + conformance fixture `07` lands in
  **PR-G** alongside `WasmMode`.
- Parity holds: the same cassette bytes drive both profiles.
- Risk: untested no_std corners of `tau-mcp` when linked into wasm — PR-G
  smoke-compiles `tau-mcp` for `wasm32-wasip2` before wiring the tool.

## References

- ADR-0046 — wasm AOT artifact + WIT world.
- ADR-0049 — single-channel typed conformance observable.
- `docs/superpowers/specs/2026-06-14-beta-7-5-wasm-aot-design.md` §10–§11.
