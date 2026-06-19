# ADR-0049: Per-agent / per-judge model resolution

**Status:** Accepted
**Date:** 2026-06-19
**Deciders:** tau core

## Context

`Agent.model` and a deliverable's `judge_model` were parsed, validated, and
traced — but **ignored at runtime**. On the IR-interpreter path (`tau run
--bundle`, `tau dev`) every agent ran on a single ambient backend:

- `prepare_agent_run` called the arg-less `dispatcher.llm_backend()`, took
  `backend.name()`, and baked it into a synthesized `AgentDefinition`. The
  agent's declared backend was never consulted.
- `ir::Agent.model` was never read.
- `stream.rs` built `CompletionRequest::new(agent_def.llm_backend.as_str())`,
  so the request's `model` field received the **backend package name**
  (`"anthropic"`), not a vendor model id (`"claude-haiku-4-5"`).

So "per-agent model" did not exist on this path — neither the model nor the
backend selection. The builtin deliverable judge inherited the same
limitation: `judge_model` was a documented v1 no-op (ADR-0044, Decision 6).

tau's guiding principle is that any check that *can* run at build time *must*
run at build time ("tau is a compiler"). Model selection is exactly such a
check: the alias → concrete pair mapping is fully knowable offline.

## Decision

A declared `[models]` table maps an author alias to a concrete
`{ backend, model }` pair. Lowering resolves every `agent.model` /
`judge_model` alias into a `ModelRef { backend, model_id }` baked into the IR.
The dispatcher becomes multi-backend; the synthesized `AgentDefinition`
carries the resolved vendor model id so the `CompletionRequest` gets a real
model string. The builtin judge becomes the implicit `JudgeRef::Default`,
defaulting to its producer agent's model.

```toml
[models]
  haiku = { backend = "anthropic", model = "claude-haiku-4-5" }
  opus  = { backend = "anthropic", model = "claude-opus-4-8"  }

[agents.writer]
  model = "haiku"            # no llm_backend; backend derived from [models]

[deliverables.report]
  must_satisfy = "ships a passing test suite"
  judge_model  = "opus"      # optional; resolves through the same [models] table
```

The seven sub-decisions:

- **D1 — Mapping mechanism: an explicit `[models]` table.** Chosen over
  (B) backends self-advertising model lists and (C) opaque agent-config
  passthrough because it is the only option fully checkable at build time.
- **D2 — Resolution at lowering; the IR carries the concrete pair.** The
  alias is resolved during lowering; the compiled IR carries the resolved
  `{ backend, model_id }`, never the alias. The bundle is self-describing and
  reproducible; the runtime performs zero alias logic.
- **D3 — Schema 1: the alias is the single knob.** `[models]` is the single
  source of truth for both backend and model. The per-agent `llm_backend`
  field is **removed** (hard cutover — in this repo it was fixtures-only).
  There is no "alias or literal" duality: a literal escape hatch, if ever
  needed, is just another `[models]` entry.
- **D4 — Multi-backend dispatcher; `AgentDefinition` gains `model`.** The
  `ToolDispatcher` trait method `llm_backend(&self) -> Arc<dyn DynLlmBackend>`
  is replaced by `llm_backend_for(&self, backend: &str) -> Result<Arc<dyn
  DynLlmBackend>, RuntimeError>`. `prepare_agent_run` reads `agent.model_ref`,
  resolves the backend by name, and bakes `model_id` into the synthesized
  `AgentDefinition`; `stream.rs` reads `agent_def.model`. The host
  `ForwardingDispatcher` holds the whole name-keyed registry and selects by
  name; single-backend test/conformance dispatchers return their one backend.
  The dispatcher stays policy-free: name → backend object; all policy happened
  at lowering.
- **D5 — The judge rides the agent machinery; `JudgeRef::Default`.** The
  deliverable judge is already an `Agent` run through the same loop. The
  variant `JudgeRef::Builtin` is renamed `JudgeRef::Default { model_ref }`
  (the word "builtin" leaves the user surface). When a deliverable uses the
  canonical judge and omits `judge_model`, the judge resolves to its
  **producer agent's** `model_ref` (the producer is already resolved at build
  time). An explicit `judge_model` overrides it. A user `[agents.*]` named as
  judge clones that agent's own `model_ref` with zero judge-specific work.
- **D6 — Agents must declare `model`.** Under Schema 1 there is no per-agent
  backend fallback and no sensible local default, so an agent that omits
  `model` is a build error (`MissingAgentModel`). A reserved-`default`-alias
  convention can be added later, additively.
- **D7 — Build-time validation, three stages with a guarantee gradient:**
  - **Stage 1 — `tau build` (offline, `validate_models` in tau-pkg).** Each
    `[models]` entry must have a non-empty `backend`+`model`
    (`MalformedModelEntry`) and a backend that is a declared package
    (`ModelBackendNotDeclared`); every agent must declare a `model`
    (`MissingAgentModel`) that resolves (`UnknownModelAlias`); a deliverable's
    `judge_model`, when present, must resolve. On pass, `{ backend, model_id }`
    is baked into `ir::Agent.model_ref` and `JudgeRef::Default`.
  - **Stage 2 — `tau check` (probes plugins).** For each backend referenced by
    `[models]`, the installed package must expose LLM completion (declare
    `provides = "llm_backend"`), else the `tau.models.backend_not_llm_capable`
    finding fires. This is a diagnostic finding, not an escape hatch.
  - **Stage 3 — runtime.** Resolve the backend by `model_ref.backend`; set
    `request.model = model_ref.model_id`; call `backend.complete(request)`.

## Consequences

- Per-agent and per-judge model selection is **real** on the IR-interpreter
  path. This **closes the `judge_model` runtime no-op** honest limit recorded
  in ADR-0044 (Decision 6).
- **IR format version: MAJOR bump v1.2.0 → v2.0.0.** Changing `ir::Agent`'s
  `model: String` field to `model_ref: ModelRef` and renaming the `JudgeRef`
  variant are breaking shape changes per the semver rules in
  `crates/tau-ir/src/module.rs`. A drift test in `tau-ir` and the
  `tau-runtime-tokio` IR-format mirror assert v2.0.0.
- New error variants on `ProjectConfigError`: `MalformedModelEntry`,
  `ModelBackendNotDeclared`, `UnknownModelAlias`, `MissingAgentModel`
  (`UnknownJudgeAgent` already existed).
- TypeScript authoring parity: `tau-ts-extract` gains a `models({...})`
  factory and a per-agent `model` alias; it infers the top-level `packages`
  set from the `[models]` backends (a bare key that never enters the IR, so
  TOML↔TS byte-equality is preserved). A multi-alias TOML↔TS conformance
  fixture proves it.
- Conformance: a `14_models_multi` fixture exercises a `[models]` table with
  two aliases on one backend, two agents on different models, one deliverable
  whose judge inherits its producer and one with an explicit `judge_model`
  override.
- **Honest limit (trusted vendor string).** The vendor model id itself (e.g.
  `"claude-haiku-4-5"`) is **trusted, never validated offline** — a provider's
  catalog cannot be checked at build time. A correct-shape-but-wrong vendor
  string surfaces as a runtime provider error (Stage 3), not a build refusal.

## Alternatives considered

- **(B) Backends self-advertise model lists.** Rejected: a backend's model
  catalog is only knowable by probing the live provider, so it cannot gate
  `tau build` offline — it would push model validation to runtime, violating
  the build-time-enforcement principle.
- **(C) Opaque per-agent config passthrough** (model string in
  `[agents.<id>.config]`). Rejected: the build has no way to distinguish a
  model selector from arbitrary plugin config, so neither the backend nor the
  model can be resolved or validated at lowering.
- **Keep `llm_backend` alongside `model`** (alias-or-literal duality).
  Rejected: two knobs for one selection invites drift (an agent could name a
  backend that disagrees with its model's backend) and doubles the validation
  surface; a single alias into `[models]` is the one source of truth.
- **A project-level default judge model.** Rejected in favour of inheriting
  the producer agent's model (Q4): the producer is already build-time
  resolved and is the most relevant local default, with no new global config.
- **Reserved `default` agent-model alias** (so agents may omit `model`).
  Deferred, not rejected: it is purely additive and can land later without a
  format change. For now, omitting `model` is a build error (D6).
