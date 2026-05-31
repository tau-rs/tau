# ADR-0024 — Multi-agent orchestration primitives

> **Updated 2026-05-31 (β.1.4):** the kernel split into `tau-runtime-core`
> (no_std + alloc) + `tau-runtime-tokio` (host shell). Orchestration
> primitives described below now live in
> `tau_runtime_core::orchestration`; the tokio mpsc `TraceSubscriber`
> impl lives in `tau_runtime_tokio::orchestration::trace_mpsc`.

**Status:** Accepted 2026-05-12.
**Branch / PR:** `feat/multi-agent-orchestration` (PR pending).
**Spec:** `docs/superpowers/specs/2026-05-12-multi-agent-orchestration-design.md`.

## Context

ROADMAP §9 — "Multi-agent orchestration (G10's deferred half)." Until now, tau composes multi-step behavior only via the linear `tau-workflow` runner. A first-class runtime-level primitive set lets agents spawn, coordinate with, and observe other agents — without the runtime imposing any particular orchestration pattern.

## Decision

Implement the v1 primitive set in `tau-runtime::orchestration` (new submodule). Six entities (Identity, Capability, Agent, Task, TraceEvent, Run), three verb classes (think/call/complete; virtual tools; host-emitted), three channels (sync return, shared state, trace), six invariants (capability subset, lock exclusivity, LLM-context immutability, trace monotonicity, run termination, budget enforcement). Coordination via shared TaskList with hierarchical task ids + locks (owner + lease + heartbeat). No bus, no inbox, no push-into-LLM. CLI output is npm/cargo-style line-feed.

## Alternatives considered

1. **Separate `tau-orchestration` crate.** Rejected — most operations are kernel-adjacent (capability checks, plugin dispatch), and serve-mode can import tau-runtime directly.
2. **Message bus / inbox stacks.** Rejected — tree topology, not many-to-many; LLM coherence breaks under unsolicited push.
3. **Background / monitor tools (claude-code-style).** Rejected for v1 — different primitive class (Channel D + BackgroundTool entity); tracked as deferred sub-project in ROADMAP.
4. **Plan DAG (CrewAI-style task dependencies).** Rejected for v1 — linear hierarchy is enough; deferred sub-project.

## Consequences

- `Capability::TaskList { mode }` + `Capability::Plan { mode }` variants added to tau-domain. Pure addition; no behavior change for existing callers.
- New `Runtime::spawn_root_agent` entry point. Single-agent `Runtime::run` is preserved.
- The JSONL log at `<scope>/.tau/runs/<run_id>.jsonl` is committed-to; future schema changes require additive migration.
- Sandboxing is preserved: every spawned child runs under its own sandbox plan; child grant must be a subset of parent grant.
- Three of the five spec patterns compose end-to-end on v1: linear (single agent + task list), worker pool (host pre-populates the task list, worker agents claim+complete), and plan-revise (orchestrator manages the task list and inspects results). The supervisor/critic and hierarchical patterns require agent.<kind>.spawn recursive dispatch and ship in the v1.1 follow-up.

## v1 implementation limits

- `agent.<kind>.spawn` is validated (parent.Agent::Spawn check + capability subset law via `check_capability_subset`) but recursive `Runtime::run` invocation from inside the streaming tool-dispatch loop is deferred. v1 returns a `ToolResult { is_error: true, ... }` with text explaining the limitation. Follow-up tracked in ROADMAP.
- Live trace rendering during a run is not yet wired into the CLI — the CLI prints a summary from the `RunSnapshot` after the run completes. The JSONL log at `<scope>/.tau/runs/<run-id>.jsonl` is the source of truth for replay/inspection. Follow-up tracked in ROADMAP.
- Pattern integration tests for the 5 spec patterns are skeletons that depend on multi-turn `MockLlmBackend` fixture wiring; they're added as the implementer is able.

## Out of scope (deferred to follow-ups, all tracked in ROADMAP)

- Background tools / monitors (claude-code Monitor pattern; new channel + entity).
- Inter-agent message bus / inbox stacks.
- Pull-status tool (`agent.<kind>_status()`).
- Output schemas / typed tool returns.
- Plan DAG with task dependencies.
- Cross-run memory.
- Group chat / mediator agent.
- Workflow-DAG (extension of tau-workflow v1).

## References

- Anthropic claude-code: `TodoWrite`, `Task` (synchronous subagent spawn).
- LangGraph: typed shared `State`, checkpoints, subgraphs.
- CrewAI: agents + tasks + memory tiers + hierarchical process.
- AutoGen / Magentic-One: orchestrator + specialists with a shared Ledger.
- OpenAI Swarm: handoffs + context_variables.
