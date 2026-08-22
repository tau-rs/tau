# EPIC 4.5 — Runtime gate for dynamic regions (design)

**Date:** 2026-08-22 · **Issue:** #402 · **Closes EPIC 4** ·
**Predecessor:** EPIC 4.4 (#607, `2026-08-21-epic-4-4-dynamic-regions-design.md`)

## Summary

4.4 made dynamic regions authorable and build-time-verified but not executable: the
interpreter meets `StepRun::Dynamic` with `RuntimeError::DynamicRegionRequiresRuntimeGate`.
4.5 makes regions run, with the runtime gate the story is named for:

- **Spawn-as-tool (Claude Code `Task`-tool shape).** A region runs its owning
  *coordinator* agent; each spawnable kind is offered to the coordinator's LLM as an
  ordinary registry tool `agent.<kind>.spawn`. Spawning = a tool call; the gate lives in
  that tool's `invoke()`.
- **Store-backed kinds.** `[agent.kinds.*]` is the project's agent store (tau's analog of
  Claude Code's `.claude/agents/*` registry). Kinds gain the fields that make them
  runnable (`description`, `prompt`, `model`, `tools`). A region's `spawns` list becomes
  optional: omitted ⇒ the whole store is offered; present ⇒ restricted to that subset.
- **The gate:** membership by construction, pooled `max_spawns` counter, defensive
  `max_concurrency` guard, child grant = `meet(envelope, kind.capabilities)` enforced by
  the existing `AttenuatedDispatcher`.
- **Soft-deny:** a denied spawn is an `is_error` tool result the coordinator LLM must
  confront, never a run abort — paired with mandatory structured trace events so a
  bounded-out run is auditable without reading prose.

## Constraints honored (settled prior art)

- **No general in-guest or dispatch-site capability gate.** The 3.4↔4.5 collision is
  resolved in 3.4's favor (#557 compile-time skip, #583 reverted runtime flag, #582
  pinned-inert IR dispatch gate). The pinned test
  `ir_declared_caps_do_not_gate_root_dispatch` (tests/ir_dispatch_gate_inert.rs) is
  untouched: 4.5's clamps apply only to dynamic-region children, never root dispatch.
- **L1 spawn-cap deferral (#609) lands here.** Build time defers spawn-cap subset
  soundness to the runtime; the meet-attenuation in `SpawnTool.invoke()` is the layer
  that makes that deferral sound, including against hand-crafted IR.
- **Subflow attenuation (#482 / D5-C) is reused, not duplicated.** `AttenuatedDispatcher`
  gates children; nesting composes to the meet of ancestor frames by construction.
- **wasm:** dynamic regions remain build-time-rejected for wasm targets (4.4's
  `feature_fit`); the guest interpreter never sees a region. This is the DoD's
  "explicit documented divergence" — no guest code changes.

## Authoring surface (`tau.toml`)

```toml
[models]
fast = { backend = "anthropic", model = "claude-haiku-4-5" }

# ── the store: every spawnable kind in the project ─────────────────────
[agent.kinds.researcher]
description  = "Deep-dives one topic against crates.io."   # LLM-visible tool description
prompt       = "You research one topic using crates.io. Report findings."
model        = "fast"                  # [models] alias, resolved at lowering like agents
tools        = ["crates_api"]          # optional, default [] — each must exist in [tools.*]
capabilities = { "net.http" = { hosts = ["crates.io"] } }

[agent.kinds.critic]
description  = "Adversarially reviews one claim. No network."
prompt       = "Refute the claim you are given. Be concrete."
model        = "fast"
capabilities = {}

# ── a region offering the whole store ──────────────────────────────────
[[pipeline.steps]]
id    = "fanout"
input = "${input}"

[pipeline.steps.fanout.dynamic]
agent           = "coordinator"        # REQUIRED (was optional in 4.4): who runs the region
ceiling         = { "net.http" = { hosts = ["crates.io"] } }
max_spawns      = 8
max_concurrency = 4
# `spawns` omitted → whole store offered; `spawns = ["critic"]` → restricted subset
```

Field rules:

- A kind **offered to any region** must declare `prompt` + `model`
  (`dynamic_kind_not_runnable` otherwise). Kinds used only by `agent.spawn`
  allow-lists (L3) stay capabilities-only. `description` defaults to `""`;
  `tools` defaults to `[]`.
- Region `agent` is **required** (`dynamic_region_requires_agent`) and must name an
  `[agents.*]` entry (typecheck). It remains the L4a bound (region ⊆ owner) exactly as
  in 4.4 — 4.4's "omitted ⇒ root-owned" form is retired.
- `spawns` omitted ⇒ offered set = whole store, and L4b runs over that expansion
  **loudly**: any store kind ⊄ ceiling fails the build (`spawn_exceeds_region`). No
  silent filtering — the author either widens the ceiling or enumerates `spawns`.
- Kinds without prompt/model in a store-default project fail the region that offers
  them; enumerating `spawns` is the escape.
- The resolved offered set must be non-empty (`dynamic_region_no_spawnable_kinds`):
  a region with nothing to spawn — empty store with `spawns` omitted, or
  `spawns = []` — is an authoring error, not a degenerate runtime no-op.

## Data model (IR)

`ir_format` **v2.6 → v2.7** (canonical encoder + JSON-schema regen — the nightly tier-2
regen gate applies).

```rust
// tau-ir/src/pipeline.rs
pub struct DynamicSpawn {
    pub kind: String,
    pub capabilities: CapabilityRequirements,
    pub description: String,          // NEW — LLM-visible spawn-tool description
    pub prompt: PromptSource,         // NEW — child system prompt (inline or asset)
    pub model_ref: ModelRef,          // NEW — baked at lowering ([allow.models] applies)
    pub tool_refs: Vec<ToolId>,       // NEW — must exist in workflow.tools (typecheck)
}

StepRun::Dynamic {
    owner: AgentId,                   // NEW — the coordinator; must exist (typecheck)
    envelope: CapabilityRequirements,
    spawns: Vec<DynamicSpawn>,        // the OFFERED set, fully resolved at lowering
    max_spawns: u64,
    max_concurrency: u64,
}
```

The IR stays self-contained (4.4's stated intent): lowering resolves the store default
into a materialized `spawns` vector, so the runtime never consults `tau.toml` and
check-time/run-time cannot drift.

## Lowering / typecheck / governance deltas

| Rule | Layer | Fires when |
|---|---|---|
| `dynamic_region_requires_agent` | parse/validate (tau-pkg) | region has no `agent` |
| `dynamic_kind_not_runnable` | lowering (tau-ir-lower) | offered kind lacks `prompt` or `model` |
| owner-exists | typecheck | `owner` not in `workflow.agents` |
| kind-tools-exist | typecheck | a kind's `tool_refs` entry not in `workflow.tools` |
| `spawn_exceeds_region` (L4b, existing) | governance | now evaluated over the expanded offered set |
| model-alias resolution | lowering | same path as `[agents.*].model` — `[allow.models]` governance identical |

L4a unchanged in logic; 4.4 fixtures updated (kinds gain prompts, regions gain owners).

## Runtime design (`tau-runtime-core`)

### Region execution (interpreter/pipeline.rs)

The `StepRun::Dynamic` early-dispatch arm replaces its named error with:

1. Render the step's `input` template (the coordinator's initial message — no JSON
   contract; it's ordinary text).
2. Build the coordinator's agent run exactly as `StepRun::Agent` does, **plus** one
   `SpawnTool` registered per entry in `spawns` (tool name `agent.<kind>.spawn`,
   LLM-visible description = the kind's `description`, schema `{message: string}`).
3. Run the coordinator; its final text is the step's output in the `OutputStore`.

### `SpawnTool` (new, sibling of `DispatcherTool`)

```rust
struct SpawnTool<D> {
    spawn: DynamicSpawn,                 // the kind template
    envelope: CapabilityRequirements,    // region ceiling
    counters: Arc<RegionCounters>,       // spawned + in_flight atomics, shared per region
    region_step: PipelineStepId,         // provenance
    module: Arc<IrModule>,
    dispatcher: Arc<D>,                  // the coordinator's dispatcher (composes frames)
}
```

`invoke()` order:

1. **Membership** — by construction: only offered kinds are registered. (An invented
   kind name gets the kernel's normal unknown-tool error.)
2. **Bounds** — `spawned` counter: admit iff `spawned < max_spawns`, then increment.
   Deny ⇒ soft error (below).
3. **Concurrency guard** — `in_flight < max_concurrency` else soft-deny
   (`ConcurrencyExceeded`). Unreachable under today's sequential per-turn tool
   dispatch; exists so the declared invariant survives future parallel dispatch.
   Counter mismatch that indicates corruption ⇒ `RuntimeError::Internal` (hard).
4. **Attenuation** — `grant = tau_domain::meet(envelope, spawn.capabilities)`, computed
   at runtime (not lowering) so hand-crafted IR violating kind ⊆ envelope is still
   clamped.
5. **Run child** — synthesize `Agent { id: "<region-step>:<kind>#<n>", prompt,
   model_ref, tool_refs, budget: default, .. }` and `run_agent` it behind
   `AttenuatedDispatcher { grant, frame: "agent.<kind>.spawn" }`, wrapping the
   coordinator's dispatcher (ancestor frames compose). Child's final text ⇒ this
   call's `tool_result`. Child run failure ⇒ `is_error` tool result (coordinator
   recovers), not a run abort.

Denial payloads are typed (`DynamicSpawnDenial { region_step, kind, reason }`,
`reason ∈ { BoundsExhausted{spawned,max}, ConcurrencyExceeded{in_flight,max} }`;
attenuation reuses `CapabilityDenial` with `narrowing_frame` = spawn tool name) and
rendered into the error text — tests assert fields, not prose. `thiserror` at the crate
boundary; `#![forbid(unsafe_code)]` unchanged.

### Kernel-intercept non-collision (pinned by test)

The legacy orchestration intercept for `agent.<kind>.spawn` names
(`stream.rs`, `is_virtual`/`is_agent_spawn`) fires only when
`options.orchestration_state` is `Some` — set solely by `spawn_root_agent` launches. The
IR interpreter path builds `RunOptions::default()` (`orchestration_state: None`), so
registry `SpawnTool`s dispatch normally. A regression test pins this: an IR run whose
registry contains `agent.x.spawn` reaches the registry tool, not the intercept.

## Observability

Two existing channels; no new `RunEvent` variant (a dedicated variant would need a
tool→kernel side-channel carrying no information the channels below lack; revisit only
if review demands it).

- **RunEvent stream (conformance channel):** spawns and denials surface as the ordinary
  `ToolCallStarted` / `ToolCallCompleted` pairs — name identifies the kind, `Err` carries
  the rendered typed denial.
- **Trace events (TUI channel):**
  - `runtime.dynamic.spawned { region_step, kind, child_id, spawned, max_spawns }`
  - `runtime.dynamic.spawn_denied { region_step, kind, reason, spawned, max_spawns }`
    (drop-row family, as #618's capability drops)
  - `runtime.dynamic.attenuation_denied { … , frame }` — mirrors
    `runtime.subflow.attenuation_denied`

Per-child tokens ride the existing per-agent token surface (#538) keyed by child ids.

## Testing strategy (TDD order — denial first)

1. **First red test:** `SpawnTool` denies the `max_spawns+1`-th spawn with typed
   `BoundsExhausted`, as an `is_error` tool result; counter state asserted.
2. Unit: meet-attenuation clamps a hand-crafted over-reaching kind (child tool call
   soft-denied via `AttenuatedDispatcher`; `narrowing_frame` = spawn tool name).
3. Unit: concurrency guard denies when `in_flight` saturated (constructed directly).
4. Interpreter: positive path — scripted `MockLlmBackend` coordinator spawns twice
   within envelope; region completes; output = coordinator's final text; child ids
   deterministic.
5. Interpreter: `DynamicRegionRequiresRuntimeGate` error is retired; its old test
   (tests/pipeline_control_flow.rs:768) replaced by the executing-region tests.
6. Kernel-intercept non-collision pin (above).
7. Lowering: `dynamic_kind_not_runnable`, `dynamic_region_requires_agent`,
   owner-exists, kind-tools-exist, store-default expansion + loud L4b, model-alias
   resolution; IR v2.7 round-trip + canonical bytes; schema regen.
8. Trace: the three named events fire with correct fields.
9. Doctests (`cargo test --doc`) for touched crates.

Gate per CARGO RULES: `timeout 300 env CARGO_INCREMENTAL=0
CARGO_TARGET_DIR=target/agent-e45 cargo nextest run -p <crate>` for each touched crate
(tau-ir, tau-pkg, tau-ir-lower, tau-runtime-core, tau-cli), plus doctests.

## Conformance fixture + docs

- **One `tau run`-level fixture** (pattern of `cmd_build_dynamic_region.rs`, run
  profile): scripted coordinator spawns twice and over-asks once; asserts the
  `ToolCallCompleted` sequence (two successes with child outputs, one typed denial) and
  the step output.
- **Docs:** rewrite the "Runtime execution" stub in
  `docs/explanation/dynamic-regions.md` — the store, spawn-as-tool, gate order,
  soft-deny + trace auditability, and the wasm divergence statement ("regions are
  native-only; wasm builds reject at `tau build`"). Update the lattice section's
  runtime half, roadmap 4.5 checkbox, and `SUMMARY.md` needs no change (page exists).

## Out of scope (explicit)

- Data-driven spawn lists (Option A) — layerable later; nothing here precludes it.
- Parallel per-turn tool dispatch (would activate the concurrency guard for real).
- Budget/context/output_schema on kinds; per-request grant narrowing in spawn args.
- Any change to `skill.spawn` (build-time L3 for it is #613's lane).
- TS/Python authoring (tau-sdk-codegen) support for regions — 7.1's lane; divergence
  noted there when relevant.
- Any in-guest wasm gate or `RunEvent` variant addition.
