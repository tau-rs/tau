# Fixture 05 — `deterministic_step` (DEFERRED)

This fixture is intentionally empty. The conformance test harness skips
this directory via the `DEFERRED_FIXTURES` constant in
`tests/conformance.rs`.

## Why it is deferred

The IR contains `Deterministic` nodes (parsed from `[steps.<name>]`)
but the interpreter loop in
`crates/tau-runtime-core/src/interpreter/agent_loop.rs` never
dispatches them — there is no execution path that resolves a tool-like
reference to a `Deterministic` node and calls
`interpreter::deterministic::DeterministicRegistry::run_step` against
it.

## What is needed to author this fixture

1. `crates/tau-runtime-core/src/interpreter/agent_loop.rs`: when a
   tool-like reference resolves to a `Deterministic` node, call
   `DeterministicRegistry::run_step` (or equivalent).
2. A `ToolDispatcher` extension method (or a separate trait) so the
   registry can be wired through the dispatcher without coupling
   `ToolDispatcher` to the deterministic surface.
