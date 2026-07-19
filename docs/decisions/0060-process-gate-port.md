# ADR-0060: `DynProcessGate` port — decouple transports from the host runtime

**Status:** Accepted
**Date:** 2026-07-19
**Deciders:** tau core

## Context

`tau-mcp-tokio` (the MCP stdio/HTTP transport crate) needs to gate the
subprocess it spawns for a stdio MCP server: it calls `wrap_spawn(plan, cmd)`
on a capability gate the host supplies. To name that gate abstractly it stored
`Arc<dyn DynProcessCapabilityGate>` — an object-safe wrapper trait that lived in
`tau-runtime-tokio`.

Depending on `tau-runtime-tokio` for one trait + one default gate dragged the
**entire host runtime** into the MCP transport's dependency graph:
`tau-pkg`, `tau-plugin-protocol`, and all four `tau-sandbox-*` crates. A
transport that only needs to call `wrap_spawn` should not compile the plugin
host, the package manager, or the sandbox adapters.

The underlying port already exists: `tau_ports::ProcessCapabilityGate` (an
`async fn`-in-trait, so not dyn-compatible). The only thing missing in
`tau-ports` was the **dyn-compatible** form and a **default no-op gate**.

## Decision

Move the object-safe process-gate surface into `tau-ports`, behind the existing
`process` feature:

- **`tau_ports::DynProcessGate`** — object-safe wrapper of
  `ProcessCapabilityGate` with boxed-future `wrap_spawn` / `apply_post_spawn`.
  A blanket `impl<T: ProcessCapabilityGate + 'static> DynProcessGate for T`
  means every sandbox adapter and the passthrough gate satisfy it for free.
  It deliberately does **not** extend `tau-runtime-core`'s `DynCapabilityGate`
  (the universal-methods wrapper): transports only ever call `wrap_spawn`, and
  keeping the port free of that super-trait keeps `tau-ports` off
  `tau-runtime-core`.
- **`tau_ports::PassthroughGate`** — the no-isolation default gate (moved from
  `tau-runtime-tokio::process_gate::passthrough::PassthroughSandbox`).

`tau-runtime-tokio` keeps `DynProcessCapabilityGate` and `PassthroughSandbox` as
**back-compat re-export aliases** of the moved items, so the resolver's
`SandboxAdapter::Passthrough` variant, the adapter registry, and the CLI wiring
are unchanged. The host still injects the concrete gate at
`setup_mcp_runtime` time; nothing about the injection direction changes.

`tau-mcp-tokio` now depends on `tau-ports` (with `process`) **only**. A CI guard
(`mcp-tokio-decoupled`) asserts its manifest never names `tau-runtime-tokio`.

## Consequences

- `cargo tree -p tau-mcp-tokio` no longer contains `tau-runtime-tokio`,
  `tau-pkg`, `tau-plugin-protocol`, or any `tau-sandbox-*` crate. The MCP
  transport builds against the ports layer alone.
- **Semver:** the change to `tau-ports` (the one semver-gated crate) is purely
  **additive** — a new public trait, a new public struct, and two new
  re-exports, all behind the pre-existing `process` feature. No existing item
  changes or is removed, so the `ports-semver` lane (cargo-semver-checks vs
  `origin/main`) stays green; it is a minor-compatible addition.
- Decoupling surfaced one previously-implicit dependency: `tau-mcp-tokio`
  formats a `PathBuf` in a `thiserror` error, which needs `thiserror`'s `std`
  feature. It was unified in transitively via `tau-runtime-tokio`; it is now
  requested explicitly.
- The dyn wrapper is now defined once in the port layer rather than per host
  shell; a future `tau-runtime-embassy` reuses the same `DynProcessGate`.
