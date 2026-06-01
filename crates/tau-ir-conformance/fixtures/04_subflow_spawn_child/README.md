# Fixture 04 — `subflow_spawn_child` (DEFERRED)

This fixture is intentionally empty. The conformance test harness skips
this directory via the `DEFERRED_FIXTURES` constant in
`tests/conformance.rs`.

## Why it is deferred

The IR's parse stage (`crates/tau-ir/src/lower/parse.rs`, lines ~58–68)
treats `tools.<name>.subflow = "child"` as creating a `SubflowEdge` and
explicitly does NOT register a `Tool` node for it (see the `continue`
in the `ToolBody::Subflow(_)` arm). Typecheck step 1
(`UnknownToolRef`) then refuses any `agent.tool_refs` entry not in
`tools` — including a subflow name. The "agent declares a subflow tool
ref" shape this fixture wants is unbuildable today.

## What is needed to author this fixture

One of:

1. `crates/tau-ir/src/lower/parse.rs`: register subflow names as
   `Tool` nodes with a `ToolImpl::Subflow` variant the interpreter can
   dispatch; OR
2. `crates/tau-ir/src/lower/typecheck.rs`: extend the unknown-tool
   check to also accept subflow ids; AND
3. `crates/tau-runtime-core/src/interpreter/agent_loop.rs`: wire
   subflow dispatch through the `DispatcherTool` path.
