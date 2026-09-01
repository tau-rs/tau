# ADR-0022 — tau-workflow: linear pipeline runner

> **SUPERSEDED (2026-09-01).** Superseded twice: first functionally by
> the workflow IR + interpreter (ADR-0037, ADR-0058/0059 — structured
> control flow in the sealed artifact replaced the JSONL step runner),
> then architecturally by the three-surface split and multi-pipeline IR
> (ADR-0071, ADR-0073 — flow is authored in `pipelines/`, never as
> `workflows/*.toml`). The `tau-workflow` crate, its CLI verbs, and its
> docs are **deleted in Phase 0** of the 2026-09-01 redesign (epic E-0).
> Numbering note: this ADR shares number 0022 with
> [`0022-sandbox-darwin.md`](0022-sandbox-darwin.md) — see
> [`README.md`](README.md).

**Status:** Superseded (see banner). Originally accepted 2026-05-12.
**Branch / PR:** `feat/tau-workflow` (PR pending).
**Spec:** `docs/superpowers/specs/2026-05-12-tau-workflow-design.md`.

## Context

ROADMAP §10 calls for a "deterministic step-by-step pipeline" runner. Until now, tau composes multi-step behavior only inside an agent's tool-loop, which mixes LLM decisions with deterministic glue. A first-class workflow primitive lets users compose multiple agents (and direct tool calls) into a single artifact that is sandboxed, persisted, and resumable.

## Decision

Ship a new `tau-workflow` crate that defines the workflow data model, runs steps via `tau_runtime::Runtime`, and persists each step's output as append-only JSONL. `tau-cli` exposes `tau workflow {list, run, log, resume}`. Step kinds for v1 are `agent.run` and `tool.call`; templates are `${input}` and `${steps.<id>.output}` (string substitution only).

Persistence mirrors the REPL-session pattern (`.tau/sessions/*.jsonl`), giving us free `tau workflow log <run-id>` history and inspectable post-mortem state.

## Alternatives considered

1. **Bake it into `tau-runtime`.** Smaller diff but mixes orchestration with the plugin-host kernel. Rejected.
2. **Bake it into `tau-cli`.** Smallest diff but locks the runner to the CLI surface; a future serve-mode (Tier 4 §15) couldn't reuse it.
3. **DAG runner for v1.** Adds parallel branches and fan-in/out. More flexible but bigger design surface (~3× the LOC); we don't yet know whether users will need DAG capabilities. Earmarked as sub-project "workflow-DAG".
4. **`agent.run` only for v1.** Tool calls could compose via dummy "tool-runner" agents. Adds an LLM round-trip per tool call. Rejected in favor of `tool.call`, which adds `Runtime::invoke_tool` (small reusable API).

## Consequences

- New `Runtime::invoke_tool` API in `tau-runtime`. Pure addition; no behavior change for existing callers.
- Workflows are a first-class artifact at the project level (`workflows/*.toml`), peer to `tau.toml` agent definitions.
- Sandboxing is inherited: every step runs through the same sandboxed plugin host as `tau run`, with the workflow's referenced agents providing the capability grants.
- The JSONL log format is committed-to: any future schema change requires an additive migration with a once-per-process warn (same model as the lockfile v3→v4 transition).

## v1 limitations (to revisit)

- `tau-cli::cmd::workflow::build_runtime_for_workflow` builds a single Runtime from the first referenced agent's plugin config. Workflows whose agents use different LLM backend plugins will work but share one backend instance; revisit if users hit this.
- No DAG; no conditionals; no per-step capability overrides; no scheduled workflows; no public library API beyond `tau-cli`.

## Out of scope (deferred follow-ups)

- DAG / parallel branches / fan-out / fan-in.
- Conditionals, loops, variable assignment beyond `${steps.<id>.output}`.
- Per-step capability overrides.
- Cron-style scheduled workflows.
- `tau workflow runs gc` cleanup command.
