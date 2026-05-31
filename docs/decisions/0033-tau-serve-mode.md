# ADR-0033: Tau serve mode v1 — JSON-RPC 2.0 over NDJSON-framed stdio

> **Updated 2026-05-31 (β.1.4):** runtime references below are split
> between `tau-runtime-core` (no_std + alloc kernel) and
> `tau-runtime-tokio` (host shell, where serve-mode lives).

**Status:** Accepted
**Date:** 2026-05-17
**Deciders:** titouanlebocq

## Context

Phase 1 priority §15. Constitution G6 / QG12 commit to two public API surfaces: the `tau-runtime` Rust crate and a serve-mode IPC protocol. Serve mode has been deferred since Phase 0 — the empty `tau-app` crate was reserved for it. Phase 1 cannot close without it: Phase 3+ SDKs (npm/pip/etc.) wrap this protocol, IDE integrations have no embedding path otherwise, and high-throughput consumers can't amortize tau's plugin cold-start cost.

Per QG18, this ADR is required because serve mode is one of the two public API surfaces; SemVer applies.

See the spec at `docs/superpowers/specs/2026-05-17-tau-serve-mode-design.md` for full motivation and design.

## Decision

Ship v1 of serve mode as JSON-RPC 2.0 over NDJSON-framed stdio.

**Method surface (5 methods + 1 notification):**

- `meta.handshake` — first call, establishes protocol_version. Required before any non-meta method.
- `meta.ping` — liveness check; works pre-handshake.
- `runtime.run` — batch agent run; returns RunOutcome.
- `runtime.run_streaming` — streaming agent run; emits `runtime.event` notifications correlated by request id, then a final response.
- `runtime.cancel` — cancel an in-flight request by id (cooperative cancellation).
- `runtime.event` — server-initiated notification carrying a `RunEvent` payload (TextDelta, ToolCallStarted, ToolCallCompleted, TurnCompleted, RunCompleted, FatalError).

**Wire format:** newline-delimited JSON (NDJSON) over the child's stdin/stdout. Each JSON value occupies exactly one line.

**Lifecycle:** one `tau-runtime::Runtime` per process. Built at startup from `--project <path>` (defaults to cwd). All RPC calls share the runtime via `Arc<Runtime>`. Graceful shutdown on SIGTERM / SIGINT / stdin EOF / parent death (`PR_SET_PDEATHSIG` on Linux). Configurable max_concurrent cap (default 8); `--idle-timeout` initiates graceful shutdown on no activity.

**Reuses `RunEvent` shape canonicalized by ADR-0011.** Each `RunEvent` becomes one `runtime.event` notification with the variant name as `kind` and the variant payload as `data`.

**Tau-namespaced JSON-RPC error codes in -32000..-32099:** HANDSHAKE_MISMATCH (-32000), CANCELLED (-32001), HANDSHAKE_REQUIRED (-32002), ALREADY_HANDSHAKEN (-32003), SERVER_BUSY (-32004), PROJECT_ERROR (-32005), RUNTIME_ERROR (-32006), CAPABILITY_DENIED (-32007), TOOL_ERROR (-32008), LLM_ERROR (-32009), UNKNOWN_AGENT (-32010). -32011..-32099 reserved for future ADR amendments.

## Consequences

### Positive

- Phase 1 closes once this lands. Phase 2 (tau as a compiled language for agentic workflows) becomes the active phase.
- `tau-app` crate exits stub status; gains the `serve` module (~2k LOC across 13 submodules).
- Serve mode is a versioned public surface. Future additive method namespaces (`session.*`, `pkg.*`, `skill.*`, `workflow.*`) land via ADR amendments.
- External SDKs and IDE integrations now have a stable embedding path.

### Negative / new surface

- **New public method on `tau-runtime::RuntimeBuilder`**: `build_allow_empty()` — same semantics as `build()` but skips the `NoLlmBackend` check. Required because serve mode must boot even when the project has no agents (handshake reports `agents: []`; calls to `runtime.run` then return `-32010 UNKNOWN_AGENT`). This is a strict additive change to tau-runtime's public API (existing `build()` behavior is unchanged).
- Plugin host integration in serve mode uses default `PluginHostOptions` (no recorder, synthetic TraceContext, no sandbox enforcement). Real sandbox honoring is deferred. Production deployments that need sandboxed serve-mode plugin hosts should track a follow-up.
- Per-tool config selectors not implemented — all tools receive `{}` config in v1. Future enhancement.

### CI gate

- No new CI job required. The existing `test-stable` job runs
  `cargo nextest run --profile ci --workspace --all-targets`, which covers
  `tau-app` (Layer 2 in-process tests) and `tau-cli` (Layer 3 e2e serve
  tests) because both crates are workspace members. `CARGO_BIN_EXE_tau` is
  populated automatically by nextest's binary build phase before the
  integration tests run.

## Alternatives considered

| Alternative | Why rejected |
|---|---|
| JSON-RPC 2.0 + LSP-style framing (Content-Length headers) | More complex parser; harder to debug with shell pipes; no current IDE-extension consumer to justify it. Additive opt-in possible later via `--transport lsp`. |
| MessagePack-RPC | Diverges from Constitution wording ("JSON-RPC over stdio"); requires Constitution amendment. External SDK clients in every language would need msgpack libs. Loses the operational property that anyone can debug the protocol with `cat`/`jq`. |
| HTTP transport | Tau is not a hosted service (NG3) and explicitly does no auth (NG9). Subprocess-over-stdio is the lightest possible IPC. HTTP would add attack surface the design rejects. |
| Per-call project switching | RuntimeBuilder is expensive (tau.toml parse, dep resolution, plugin spawn, capability shape build). Building per call defeats cold-start-amortization. |
| Full CLI parity in v1 | 15-method surface locked forever. Forces premature decisions about session/skill/pkg method shapes. v1 ships kernel; later ADRs add increments. |

## Follow-ups (deferred)

- Session/persistence methods (`session.*`). Phase 1 §11 already exposes these via CLI; serve-mode coverage waits for concrete demand.
- Package management methods (`pkg.*`). External orchestrators can shell out to `tau install` / `tau resolve` as setup steps before `tau serve`.
- Skill / workflow methods. Same reasoning.
- LSP-style framing transport via `--transport lsp`.
- Sandbox enforcement (currently bypassed via `sandbox_plan = None` in build_runtime).
- Per-tool config selectors (all tools currently receive `{}`).

## References

- Constitution G6, QG12, QG18.
- Spec: `docs/superpowers/specs/2026-05-17-tau-serve-mode-design.md`.
- Plan: `docs/superpowers/plans/2026-05-17-tau-serve-mode.md`.
- ADR-0011 (RunEvent canonical shape).
- ADR-0032 (CapabilityOverride relocation; sibling refactor).
