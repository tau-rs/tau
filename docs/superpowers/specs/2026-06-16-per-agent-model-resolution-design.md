# Per-agent / per-judge model resolution (multi-backend) — design

Date: 2026-06-16
Status: proposed
Track: "deliverables & goals" follow-up, Track 1 (the cross-cutting one)

## Problem

`Agent.model` and a deliverable's `judge_model` are parsed, validated, stored,
and traced — but **ignored at runtime**. In the IR interpreter path
(`tau run --bundle`, `tau dev`) every agent runs on a single ambient backend:

- `prepare_agent_run` (`crates/tau-runtime-core/src/interpreter/agent_loop.rs:394`)
  calls the arg-less `dispatcher.llm_backend()`, takes `backend.name()`, and bakes
  that into a freshly-synthesized `AgentDefinition`. The agent's declared backend
  is never consulted.
- `ir::Agent.model` is never read (verified: no `.model` use in `agent_loop.rs`).
- `stream.rs:292` builds `CompletionRequest::new(agent_def.llm_backend.as_str())`,
  i.e. the request's `model` field receives the **backend package name**
  (`"anthropic"`), not a vendor model id (`"claude-haiku-4-5"`).

So "per-agent model" does not exist in this path — neither the model nor the
backend selection. The builtin deliverable judge inherits the same limitation
(`judge_model` is a documented v1 no-op).

This design makes per-agent and per-judge model selection real, resolved at
build time, and refused at build time when unresolvable.

## Scope

In scope:
- A declared `[models]` table mapping an author-chosen alias to a concrete
  `{ backend, model }` pair.
- Build-time resolution (at lowering) of every `agent.model` / `judge_model`
  alias into a concrete pair baked into the IR.
- A multi-backend dispatcher trait that selects a backend by name.
- Build-time refusal of unknown aliases, undeclared backends, malformed entries,
  and agents missing a model.
- TypeScript authoring parity (`tau-ts-extract`) and conformance coverage.

Explicitly out of scope (deferred, additive later, non-breaking):
- A project-level / reserved `default` model alias for agents that omit `model`
  (Q6 chose "require it" instead).
- Validating the vendor model string against a provider's catalog (cannot be
  done offline).
- A separate project-level default *judge* model (the judge inherits its
  producer's model; see Q4).

## Decisions

### D1 — Mapping mechanism: explicit `[models]` table (Q1)

A declared table in `tau.toml` maps alias → concrete pair. Chosen over
(B) backends self-advertising model lists and (C) opaque agent-config
passthrough because it is the only option fully checkable at build time —
consistent with tau's "any check that can run at build time must" principle.

### D2 — Resolution happens at lowering; the IR carries the concrete pair (Q1/Q7)

The alias is resolved during lowering. The compiled IR carries the resolved
`{ backend, model_id }`, never the alias. The bundle is therefore fully
self-describing and reproducible; the runtime performs zero alias logic.

```
tau.toml: model = "haiku"
   └─lower─▶ ir::Agent.model_ref = { backend: "anthropic", model_id: "claude-haiku-4-5" }
       └─runtime─▶ backend = resolve_llm_backend("anthropic")   // existing name lookup
                   request.model = "claude-haiku-4-5"
                   backend.complete(request)
```

### D3 — Schema 1: the alias is the single knob; `llm_backend` removed from agents (Q2)

`[models]` is the single source of truth for both backend and model. The
per-agent `llm_backend` field is **removed** (hard cutover — in this repo it is
fixtures-only). Agents and judges resolve through the identical path.

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

No "alias or literal" duality: `model` is always an alias key. A literal escape
hatch, if ever needed, is just another `[models]` entry.

### D4 — Dispatcher trait becomes multi-backend; `AgentDefinition` gains `model` (Q3)

```rust
// crates/tau-runtime-core/src/interpreter/tool_dispatch.rs
// BEFORE
fn llm_backend(&self) -> Arc<dyn DynLlmBackend>;
// AFTER (replaces; no arg-less convenience retained)
fn llm_backend_for(&self, backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError>;
```

- `prepare_agent_run` reads `agent.model_ref`, calls `llm_backend_for(backend)`,
  and bakes `model_id` into the synthesized `AgentDefinition`.
- `AgentDefinition` (tau-domain) gains `model: String`; `stream.rs:292` reads
  `agent_def.model` instead of `agent_def.llm_backend.as_str()`.
- `ForwardingDispatcher` (`crates/tau-cli/src/cmd/ir_dispatcher.rs:123`) stops
  doing `.values().next()` and holds the full name-keyed registry already built
  by `collect_llm_backends_by_name` (`builder.rs:689`), selecting by name.
- Single-backend test dispatchers implement `llm_backend_for` by returning their
  one backend (asserting the requested name matches).

The dispatcher stays policy-free: it maps name → backend object. All policy
(alias → pair) happened at lowering.

### D5 — The judge rides the agent machinery; `JudgeRef::Default` (Q4 + nomenclature)

The deliverable judge is already an `Agent` run through the same loop
(`check.rs:218`). Therefore:

- `JudgeRef::Agent(id)` clones the module agent, which carries its own
  `model_ref` — resolves via D4 with zero judge-specific work.
- The canonical judge is **implicit**: a deliverable names a `judge` agent only
  when it wants a *custom* one. The enum variant `JudgeRef::Builtin` is renamed
  `JudgeRef::Default { model_ref }`. The word "builtin" disappears from the user
  surface.
- **Default model (Q4):** when a deliverable uses the canonical judge and omits
  `judge_model`, the judge resolves to the **producer agent's** `model_ref`. The
  producer is already resolved at build time (`deliverable.producer`). An
  explicit `judge_model` overrides it. No ambient-backend magic, no new global
  config.

### D6 — Agents must declare `model` (Q6)

Under Schema 1 there is no per-agent backend fallback and no sensible local
source for a default (unlike the judge, which has its producer). An agent that
omits `model` is a build error: `MissingAgentModel { agent }`. The deferred
reserved-`default`-alias convention (out of scope) can be added later without
breaking anything.

### D7 — Build-time validation split (Q5)

Three stages with a guarantee gradient:

```
STAGE 1 — tau build (OFFLINE, validate_models() in tau-pkg)
  for each [models] entry:
    ├─ has backend + model?           no ▶ MalformedModelEntry { alias }
    └─ backend ∈ declared packages?   no ▶ ModelBackendNotDeclared { alias, backend }
  for each agent.model / judge_model alias:
    └─ alias ∈ [models]?              no ▶ UnknownModelAlias { referrer, alias }
  agent missing model?                    ▶ MissingAgentModel { agent }
  judge = "<agent-id>":
    └─ agent-id ∈ [agents]?           no ▶ UnknownJudgeAgent  (already exists)
  ON PASS: bake { backend, model_id } into ir::Agent.model_ref and JudgeRef::Default

TRUSTED — never checked (cannot, offline)
  the vendor string "claude-haiku-4-5" itself
  → a bad string surfaces as a runtime provider error (documented honest limit)

STAGE 2 — tau check (PROBES plugins)
  for each backend referenced by [models]:
    └─ does the package expose LLM completion?  no ▶ check finding BackendNotLlmCapable

STAGE 3 — runtime
  resolve_llm_backend(model_ref.backend); request.model = model_ref.model_id;
  backend.complete(request)  — only a correct-shape-but-wrong-vendor-string fails here
```

New error variants on `ProjectConfigError`: `MalformedModelEntry`,
`ModelBackendNotDeclared`, `UnknownModelAlias`, `MissingAgentModel`.
`UnknownJudgeAgent` already exists. `BackendNotLlmCapable` is a `tau check`
finding, not a build error.

## IR shape changes

- `ir::Agent.model: String` → `ir::Agent.model_ref: ModelRef { backend: String, model_id: String }`
  (new `ModelRef` type in tau-ir).
- `JudgeRef::Builtin { model: Option<String> }` → `JudgeRef::Default { model_ref: ModelRef }`
  (alias resolved at lowering; the Q4 producer-inheritance default is applied
  during lowering, so the IR always carries a concrete pair).
- `tau-pkg`: drop `AgentEntry.llm_backend`; add the `[models]` table type and
  `validate_models`; resolve aliases in lowering (`crates/tau-ir/src/lower/parse.rs`).

### IR format version

This is a **breaking** shape change (changed `Agent.model` field, renamed
`JudgeRef` variant) per the rules in `crates/tau-ir/src/module.rs:17`. Bump
`IrFormatVersion::CURRENT` **v1.2.0 → v2.0.0** (MAJOR).

## Cross-track coordination (Track 2)

Track 2 (agent `output_schema`) also edits `ir::Agent`, `lower/parse.rs`, and the
conformance fixtures, and does a MINOR bump (v1.2.0 → v1.3.0). The two tracks are
logically independent but physically overlap in ~3 files. Coordination:

- Whichever lands second rebases and re-resolves the trivial conflicts in
  `node.rs` / `parse.rs` / fixtures and re-bumps the version (Track 1's MAJOR
  supersedes Track 2's MINOR → final is v2.0.0 if Track 1 lands last, or Track 2
  re-bases onto v2.0.0 and stays additive if Track 1 lands first).
- No logic conflict: `output_schema` and `model_ref` are orthogonal fields.

## Touch-points (by crate)

- **tau-pkg**: `[models]` table parse + `ModelEntry`/`ModelTable` types; remove
  `AgentEntry.llm_backend`; `validate_models`; new error variants; `MissingAgentModel`.
- **tau-ir**: `ModelRef` type; `Agent.model` → `model_ref`; `JudgeRef::Default`;
  format version v2.0.0; lowering resolution in `lower/parse.rs` (alias lookup +
  Q4 producer-inheritance default).
- **tau-runtime-core**: `ToolDispatcher::llm_backend_for`; rewire
  `prepare_agent_run`; judge synthesis in `check.rs`; `stream.rs:292` uses
  `agent_def.model`.
- **tau-domain**: `AgentDefinition.model` field.
- **tau-cli**: `ForwardingDispatcher` multi-backend selection; `tau check`
  `BackendNotLlmCapable` finding.
- **tau-ts-extract**: parity — extract the `[models]` table + per-agent `model`
  alias; TOML↔TS byte-equal conformance.
- **tau-ir-conformance**: fixture(s) with a `[models]` table, a multi-model
  workflow, and a deliverable judge; expected-report snapshot updates.
- **ADR**: record D1–D7 and the MAJOR IR bump (follow ADR `0006` semver rules).

## Testing

- tau-pkg unit tests per new error variant (each Stage-1 refusal).
- Lowering tests: alias → `ModelRef`; judge default → producer model_ref.
- tau-runtime-core: `llm_backend_for` selects the right backend; `stream.rs`
  sends the real model id; multi-backend dispatch test.
- Conformance: TOML↔IR byte-equal round trip with `[models]`; TOML↔TS parity.
- Migration: update all fixtures that currently use `llm_backend`.
