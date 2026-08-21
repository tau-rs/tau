# EPIC 4.4 — Dynamic regions + per-kind agent definitions (design)

**Status:** Accepted (brainstorm approved 2026-08-21)
**Roadmap:** `docs/superpowers/plans/vision-roadmap.md` EPIC 4, story 4.4
**Origin ADRs:** 0024 (multi-agent orchestration; per-kind agent defs), 0057 (root `[allow]`
governance; the `root ⊇ agent ⊇ dyn-region ⊇ spawn ⊇ tool` chain), 0058 (IR control flow)

## User-observable delta

> When this merges, a user can newly declare a **bounded dynamic region** whose spawns are
> **lattice-checked at build**.

This is a *build-time-verify* slice (like EPIC 1.4 "`tau check` fails if cap ⊄ `[allow]`" and
EPIC 6.1 "declare a durability intent and see how it resolves"): its delta is a build-time catch,
not a runtime execution. The **runtime gate** (membership + attenuation + bounds counters) is the
named follow-up **EPIC 4.5** — this slice emits it. Cutting runtime out is not a trailing-conformance
violation (slicing-policy rule 4): conformance for what 4.4 *delivers* — the envelope verify — is in
this slice's DoD. The construct's runtime is a separately-storied consumer (rule 2), exactly as
Suspend's runtime was split from #454 into 4.3.

## The lattice enforced

```
root  = [allow].ceiling
  ⊇ agent    [agents.X] effective caps (package ∩ overrides)   — L1/L2, already enforced
      ⊇ region   dynamic-region `ceiling` (envelope)           — NEW (L4a)
          ⊇ spawn   per-kind agent def [agent.kinds.K] caps    — NEW (L4b); also unblocks
              ⊇ tool  a tool the kind may reference            —   the 1.5-deferred agent⊇spawn (L3)
```

`governance.rs` today enforces `root ⊇ agent ⊇ tool` (L0–L2) and emits a **Note** at
`governance.rs:329`: *"agent ⊇ spawn is enforced at runtime; build-time enforcement is deferred
pending per-kind agent definitions (EPIC 4)."* 4.4 supplies those definitions and promotes that
Note to a hard **Error**, then adds the region-envelope links.

## Authoring surface (tau.toml)

### Per-kind agent definitions (ADR-0024) — dedicated `[agent.kinds.<name>]` table

```toml
[agent.kinds.researcher]
capabilities = [ { "net.http" = { hosts = ["api.crawler.test"] } } ]
```

- A **named spawnable kind** carrying its own capability set. This is the static kind→caps map
  whose absence deferred build-time `agent ⊇ spawn` in story 1.5 ("no static kind→agent map").
- `capabilities` uses the **same raw-cap grammar** as `[allow.tools.*].ceiling` and tool caps
  (kind-as-key inline tables), round-tripped through the single `tau-domain` capability
  deserializer (so typos get did-you-mean).
- Chosen over reusing `[agents.<id>]` (Option B) to avoid conflating pipeline-agent *instances*
  with spawnable *kinds* and to avoid silently making every declared agent spawnable-by-name.

### Bounded dynamic region — 5th pipeline-step form

```toml
[[pipeline.steps]]
id = "research-fanout"
[pipeline.steps.dynamic]
spawns          = ["researcher"]     # references [agent.kinds.*]
ceiling         = [ { "net.http" = { hosts = ["api.crawler.test"] } } ]  # region envelope
max_spawns      = 8                  # bounds (must be > 0)
max_concurrency = 4                  # bounds (must be > 0, <= max_spawns)
# agent = "orchestrator"             # OPTIONAL owner — inserts the agent ⊇ region middle link
```

The step is a **fifth mutually-exclusive form** in `UncheckedPipelineStep` (alongside leaf-`run`,
branch, parallel, loop), detected by the presence of the `[pipeline.steps.dynamic]` table.
`input` defaults to `"${input}"` like the other forms.

## Data model

### tau-domain (`#![no_std]` + alloc, `#![forbid(unsafe_code)]`)

`src/agent_kind.rs` (new):
```rust
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct AgentKind {
    pub name: String,
    pub capabilities: Vec<Capability>,
}
```
Re-exported from `lib.rs`. No new lattice primitive is needed — `capability_subset` (D3/#497)
already decides `child ⊆ parent` soundly for every `Capability` kind, including
`Agent(Spawn { allowed_kinds })`.

`src/ir_feature.rs`: add `IrFeature::Dynamic` to the enum **and** to `IrFeature::ALL` (the doc there
says a new variant forces this; native/host targets grant `ALL`, so `Dynamic` becomes executable on
native and, by omission from `any-wasi-strict`'s empty `supported_features`, auto-rejected on wasm).

### tau-ir (`pipeline.rs`)

```rust
pub enum StepRun {
    // …Agent | Tool | Deterministic | Check | Branch | Parallel | Loop | Suspend…
    Dynamic {
        /// Region capability envelope (ceiling). Every spawn ⊆ this.
        envelope: Vec<tau_domain::Capability>,
        /// Per-kind agent defs this region may spawn, resolved with their caps
        /// so the 4.5 runtime gate is self-contained.
        spawns: Vec<DynamicSpawn>,
        max_spawns: u64,
        max_concurrency: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DynamicSpawn {
    pub kind: String,
    pub capabilities: Vec<tau_domain::Capability>,
}
```

- `StepRun` is a plain (non-`#[non_exhaustive]`) enum: adding `Dynamic` **compile-forces** new arms
  in every exhaustive match — `features.rs::collect_step` (→ insert `IrFeature::Dynamic`),
  `typecheck.rs::{collect_all_ids, collect_suspend_ids, validate_step_run}`, and
  `parse.rs::lower_step`. This compiler force is the "honesty" mechanism (not a runtime iterating
  test).
- IR carries the resolved kind caps + envelope + bounds so EPIC 4.5's runtime gate reads them
  directly (no re-resolution against tau.toml).
- `module.rs`: bump `IrFormatVersion::CURRENT` `v2.5.0 → v2.6.0` (additive variant = MINOR, mirrors
  the `v2.4.0` note for 4.1's Branch/Parallel/Loop/Suspend). Update the two pin tests
  (`module.rs:148`, `:169`) and add a history line. Regenerate the schema snapshot
  (`tau-ir/tests/schema_export.rs`, `UPDATE_SCHEMA=1`).

### tau-pkg (`project.rs`, `allow.rs`)

- `PipelineRunRef::Dynamic { spawns: Vec<String>, ceiling: Vec<Capability>, max_spawns, max_concurrency, agent: Option<String> }`.
- `UncheckedPipelineStep` gains a `dynamic: Option<UncheckedDynamic>` field; form-count detection
  (`validate_pipeline_step`, project.rs:1972-1999) treats it as the 5th form; validation builds the
  `PipelineRunRef::Dynamic` (parses `ceiling` via the existing raw-cap bridge, enforces
  `max_spawns > 0`, `0 < max_concurrency <= max_spawns`, non-empty `spawns`).
- `ProjectConfig` gains `agent_kinds: BTreeMap<String, AgentKindEntry>` from a new
  `[agent.kinds.<name>]` table; `AgentKindEntry { name, capabilities: Vec<Capability> }`. Uses the
  same `bridge_caps` path as `allow.rs` for the raw-cap list.

### tau-ir-lower (`parse.rs::lower_step`)

New arm: `PipelineRunRef::Dynamic { .. }` → `StepRun::Dynamic { .. }`, resolving each spawn name to
its `[agent.kinds.*]` caps (a `DynamicSpawn` per kind). Unknown kind → `LowerError::UnknownAgentKind`
(new thiserror variant).

## Build-time enforcement (`tau-cli` `governance.rs`)

Extend the `lattice` function (governance.rs:199-336):

- **L3 (promoted from Note → Error).** For each `[agents.X]` whose effective caps include
  `Capability::Agent(Spawn { allowed_kinds })`: every kind in `allowed_kinds` **must** resolve to an
  `[agent.kinds.K]` definition (else `tau.governance.unknown_spawn_kind` Error), and
  `capability_subset(kind.caps, X.effective)` **must** hold (else `tau.governance.spawn_exceeds_agent`
  Error). Removes the `spawn_runtime_enforced` Note.
- **L4a (region ⊆ owner).** For each dynamic-region step: `capability_subset(region.ceiling, owner)`
  where `owner` = the named `agent`'s effective caps if `agent = "X"` is set, else
  `allow.ceiling`. Violation → `tau.governance.region_exceeds_ceiling`.
- **L4b (spawn ⊆ region).** For each kind in the region's `spawns`:
  `capability_subset(kind.caps, region.ceiling)`. Violation →
  `tau.governance.spawn_exceeds_region`. Unknown kind → `tau.governance.unknown_spawn_kind`.

All violations reuse the existing `CeilingViolation { kind, offender, reason }` → summary formatting
(`"{subject}: capability {kind} \"{offender}\" exceeds {bound} ({reason})"`) and
`Severity::Error`, matching the L0–L2 style. `tau check` exits non-zero (unchanged aggregator
behaviour).

## Runtime (interpreter) — named deferral to 4.5

`tau-runtime-core` interpreter: `StepRun::Dynamic` returns a named error
`DynamicRegionRequiresRuntimeGate` (message names EPIC 4.5), mirroring how Suspend aborted with a
named error before 4.3 landed its runtime. No in-guest wasm gate is added or touched — 3.4 (#557),
the reverted flag (#583), and the inert dispatch gate (#582) settled that; wasm parity here =
explicit feature-reject at lowering, nothing else.

## wasm parity

`any-wasi-strict` lists `supported_features: &[]`; `feature_fit::check` already refuses any used
feature not in that set, so a pipeline containing a dynamic region is refused at
`tau build --target wasm` with `LowerError::FeatureUnsupported { missing: [Dynamic], .. }`. No new
wasm code; extend `feature_fit` tests to cover `Dynamic`.

## Conformance fixtures (in CI) + docs

Build-level fixtures (this is a build-verify slice, so fixtures are `tau check`/`tau build`, not
`tau run`):

1. **Well-formed region builds.** `tau build --target dev` on a tau.toml with a bounded region +
   `[agent.kinds.*]` succeeds; emitted IR contains `StepRun::Dynamic`.
2. **Over-reaching spawn fails `tau check`** (the TDD anchor, written first). A kind whose
   `net.http hosts=["*"]` exceeds the region `ceiling` (or root `[allow]`) →
   `tau.governance.spawn_exceeds_region` Error, non-zero exit.
3. **wasm rejects Dynamic.** `tau build --target wasm` on fixture 1 fails with
   `FeatureUnsupported { missing: [Dynamic] }`.

Docs: one example page under `docs/`, added to `docs/SUMMARY.md` (mdBook silently drops unlisted
pages). Update the roadmap 4.4 checkbox and, if present, the escape-hatch/feature registry docs.

## Testing strategy (TDD order)

1. **First test:** over-reaching spawn → `tau check` Error (fixture 2), red before any impl.
2. tau-domain: `AgentKind` + `IrFeature::Dynamic` unit tests; existing lattice proptests unaffected.
3. tau-ir: `StepRun::Dynamic` round-trips; `features.rs` collects `IrFeature::Dynamic`; version pin.
4. tau-pkg: `[agent.kinds.*]` + dynamic-step parse/validate (bounds, form-count exclusivity,
   unknown-kind).
5. tau-ir-lower: `lower_step` Dynamic arm; unknown-kind lowering error; `feature_fit` wasm reject.
6. tau-cli governance: L3/L4a/L4b positive + negative; Note removed.
7. Interpreter: `Dynamic` → named `DynamicRegionRequiresRuntimeGate` error.

Boundaries: `thiserror` at crate boundaries (`LowerError`, `ProjectConfigError`), plain-data
`CeilingViolation` inside the no_std lattice (unchanged). `#![forbid(unsafe_code)]` throughout.

## Out of scope (explicit)

- Runtime gate: membership, attenuation of live spawns, bounds *counters* → **EPIC 4.5** (this
  slice's emitted follow-up).
- Any in-guest wasm capability gate (settled: #557/#583/#582).
- Data-driven spawn *count* execution — the interpreter does not execute Dynamic in 4.4.
