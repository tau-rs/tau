# EPIC 4.4 — Dynamic Regions + Per-Kind Agent Definitions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a user declare a bounded dynamic region in `tau.toml` whose spawnable per-kind agent definitions are capability-lattice-checked at build time (`tau check`).

**Architecture:** New `[agent.kinds.<name>]` tables give a static kind→capability-set map (the piece story 1.5 lacked). A 5th pipeline-step form `[pipeline.steps.dynamic]` lowers to `StepRun::Dynamic { envelope, spawns, max_spawns, max_concurrency }`. `tau check governance` promotes its deferred agent⊇spawn Note to a hard Error and adds region⊆owner and spawn⊆region links, all via the existing sound `capability_subset` primitive. Runtime execution is deferred to EPIC 4.5 (interpreter meets `Dynamic` with a named error, exactly as Suspend did pre-4.3); wasm parity = feature-reject at lowering.

**Tech Stack:** Rust (workspace of 8 crates), `thiserror` at boundaries, `serde`/`toml`, `schemars` (schema feature), `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-08-21-epic-4-4-dynamic-regions-design.md`

## Global Constraints

- **CARGO RULES (CLAUDE.md):** every cargo command is `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo <cmd> -p <crate>`. Never bare `cargo`, always `-p`, always `timeout` (test 300s, build/check 180s, clippy 240s). Prefer `cargo nextest run`; doctests via `cargo test --doc`.
- `#![forbid(unsafe_code)]` in every crate touched.
- `thiserror` at crate boundaries (`LowerError`, `ProjectConfigError`, `RuntimeError`); plain-data `CeilingViolation` inside the no_std lattice (unchanged).
- tau-domain is `#![no_std]` + `alloc`; new domain code must not use `std`.
- Capability-list order is taken from source TOML verbatim (canonical-bytes stability) — do not sort resolved caps.
- IR is an additive MINOR bump: `v2.5.0 → v2.6.0`. Schema snapshot regenerated, not hand-edited.
- No in-guest wasm capability gate is added or modified (settled by #557/#583/#582). wasm parity for Dynamic = feature-reject at lowering only.

---

### Task 1: tau-domain — `AgentKind` type + `IrFeature::Dynamic`

**Files:**
- Create: `crates/tau-domain/src/agent_kind.rs`
- Modify: `crates/tau-domain/src/lib.rs` (module decl + re-export)
- Modify: `crates/tau-domain/src/ir_feature.rs:25-46`

**Interfaces:**
- Produces: `tau_domain::AgentKind { pub name: String, pub capabilities: Vec<Capability> }`; `tau_domain::IrFeature::Dynamic` (variant + in `IrFeature::ALL`).

- [ ] **Step 1: Write the failing test** — append to `crates/tau-domain/src/ir_feature.rs` (new `#[cfg(test)] mod tests` if absent, else add):

```rust
#[cfg(test)]
mod tests {
    use super::IrFeature;

    #[test]
    fn all_contains_every_variant_including_dynamic() {
        // ALL must list Dynamic so native/host targets can execute it and a
        // new variant is compile-forced into this list.
        assert!(IrFeature::ALL.contains(&IrFeature::Dynamic));
        assert_eq!(IrFeature::ALL.len(), 5);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo test -p tau-domain --lib ir_feature 2>&1 | tail -20`
Expected: FAIL — `no variant named Dynamic` / compile error.

- [ ] **Step 3: Add the `Dynamic` variant** — in `crates/tau-domain/src/ir_feature.rs`, after the `Suspend` variant (line 33):

```rust
    /// Human-in-the-loop `Suspend { resume_signal }` (EPIC 4.1 / 4.3).
    Suspend,
    /// Bounded dynamic region `Dynamic { envelope, spawns, max_spawns,
    /// max_concurrency }` (EPIC 4.4; runtime gate 4.5).
    Dynamic,
}
```

And add to `ALL` (after `IrFeature::Suspend,`):

```rust
    pub const ALL: &'static [IrFeature] = &[
        IrFeature::Branch,
        IrFeature::Parallel,
        IrFeature::Loop,
        IrFeature::Suspend,
        IrFeature::Dynamic,
    ];
```

- [ ] **Step 4: Create the `AgentKind` type** — `crates/tau-domain/src/agent_kind.rs`:

```rust
//! Per-kind agent definitions (ADR-0024): a named, spawnable agent *kind*
//! carrying its own capability set.
//!
//! This is the static `kind → capabilities` map whose absence deferred
//! build-time `agent ⊇ spawn` enforcement in EPIC 1 story 1.5 ("no static
//! kind→agent map"). EPIC 4.4 uses it to check, at `tau check` time, that a
//! spawned kind's caps ⊆ the spawning agent's / dynamic region's effective
//! caps via the sound `capability_subset` lattice primitive.

use alloc::string::String;
use alloc::vec::Vec;

use crate::Capability;

/// A named spawnable agent kind and its capability set.
///
/// The `name` matches the string used in `Agent(Spawn { allowed_kinds })`
/// and in a dynamic region's `spawns` list. `capabilities` is the kind's
/// grant, authored as raw caps in `[agent.kinds.<name>]`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct AgentKind {
    /// The kind name (referenced by spawn allow-lists and dynamic regions).
    pub name: String,
    /// The kind's capability grant.
    pub capabilities: Vec<Capability>,
}

impl AgentKind {
    /// Construct a kind from a name and its capability grant.
    pub fn new(name: String, capabilities: Vec<Capability>) -> Self {
        Self { name, capabilities }
    }
}
```

- [ ] **Step 5: Wire the module + re-export** — in `crates/tau-domain/src/lib.rs`, add the module declaration near the other `mod` lines (keep alphabetical-ish with siblings such as `mod agent;`):

```rust
mod agent_kind;
```

and add to the public re-exports (next to the `AgentDefinition` / capability exports):

```rust
pub use agent_kind::AgentKind;
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo nextest run -p tau-domain 2>&1 | tail -20`
Expected: PASS (all tau-domain tests, incl. the new `all_contains_every_variant_including_dynamic`).

- [ ] **Step 7: Commit**

```bash
git add crates/tau-domain/src/agent_kind.rs crates/tau-domain/src/lib.rs crates/tau-domain/src/ir_feature.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(tau-domain): add AgentKind + IrFeature::Dynamic (EPIC 4.4)"
```

---

### Task 2: tau-ir — `StepRun::Dynamic` + `DynamicSpawn` + feature collection + version bump

**Files:**
- Modify: `crates/tau-ir/src/pipeline.rs:36-84` (enum + new `DynamicSpawn` struct)
- Modify: `crates/tau-ir/src/features.rs:36-63` (`collect_step` arm)
- Modify: `crates/tau-ir/src/module.rs:30-48,148-169` (version bump + pins)
- Test: `crates/tau-ir/src/pipeline.rs` (tests mod), `crates/tau-ir/src/features.rs` (tests mod)
- Regenerate: `crates/tau-ir/tests/schema_export.rs` snapshot

**Interfaces:**
- Consumes: `tau_domain::IrFeature::Dynamic` (Task 1), `crate::capability::CapabilityRequirements` (existing).
- Produces: `StepRun::Dynamic { envelope: CapabilityRequirements, spawns: Vec<DynamicSpawn>, max_spawns: u64, max_concurrency: u64 }`; `pub struct DynamicSpawn { pub kind: String, pub capabilities: CapabilityRequirements }`; `IrFormatVersion::CURRENT == "v2.6.0"`.

- [ ] **Step 1: Write the failing feature-collection test** — in `crates/tau-ir/src/features.rs` tests mod, add:

```rust
    #[test]
    fn dynamic_region_collects_dynamic_feature() {
        use crate::capability::CapabilityRequirements;
        use crate::pipeline::{DynamicSpawn, PipelineStep, StepRun};
        use crate::ids::PipelineStepId;

        let step = PipelineStep {
            id: PipelineStepId("fanout".into()),
            run: StepRun::Dynamic {
                envelope: CapabilityRequirements::default(),
                spawns: alloc::vec![DynamicSpawn {
                    kind: "researcher".into(),
                    capabilities: CapabilityRequirements::default(),
                }],
                max_spawns: 8,
                max_concurrency: 4,
            },
            input: "${input}".into(),
        };
        let feats = features_used(&module_with(alloc::vec![step]));
        assert!(feats.contains(&IrFeature::Dynamic));
        assert_eq!(feats.len(), 1);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo test -p tau-ir --lib features 2>&1 | tail -20`
Expected: FAIL — `no variant or associated item named Dynamic`.

- [ ] **Step 3: Add `DynamicSpawn` + `StepRun::Dynamic`** — in `crates/tau-ir/src/pipeline.rs`, add the import at the top (after line 10):

```rust
use crate::capability::CapabilityRequirements;
```

Add the new struct above `StepRun` (after `PipelineStep`, ~line 31):

```rust
/// One spawnable per-kind agent definition inside a [`StepRun::Dynamic`]
/// region, resolved with its capability grant so the runtime gate
/// (EPIC 4.5) is self-contained (no re-resolution against `tau.toml`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DynamicSpawn {
    /// The agent-kind name (`[agent.kinds.<kind>]`).
    pub kind: String,
    /// The kind's resolved capability grant.
    pub capabilities: CapabilityRequirements,
}
```

Add the variant after `Suspend { .. }` (line 83, inside `StepRun`):

```rust
    /// EPIC 4.4: a bounded dynamic region. Spawns up to `max_spawns` children
    /// (concurrency-capped by `max_concurrency`) drawn from `spawns`, each
    /// attenuated to `envelope`. Build-time verified: every spawn ⊆ envelope ⊆
    /// owner ⊆ root `[allow]` (see `tau check governance`). Runtime execution +
    /// membership/attenuation/bounds counters land in EPIC 4.5; the interpreter
    /// meets it with `RuntimeError::DynamicRegionRequiresRuntimeGate` until then.
    Dynamic {
        /// Region capability envelope (ceiling); every spawn ⊆ this.
        envelope: CapabilityRequirements,
        /// Spawnable per-kind agent definitions this region may launch.
        spawns: Vec<DynamicSpawn>,
        /// Hard cap on total spawns (`> 0`, enforced at author time).
        max_spawns: u64,
        /// Hard cap on concurrent spawns (`0 < n <= max_spawns`).
        max_concurrency: u64,
    },
}
```

- [ ] **Step 4: Add the `collect_step` arm** — in `crates/tau-ir/src/features.rs:36-63`, add before the closing brace of the match (after the `Suspend` arm):

```rust
        StepRun::Dynamic { .. } => {
            found.insert(IrFeature::Dynamic);
        }
```

(A dynamic region carries no nested `PipelineStep`s, so no recursion.)

- [ ] **Step 5: Bump the IR format version** — in `crates/tau-ir/src/module.rs`, add a history line after the `v2.4.0` note (line ~39) and change `CURRENT` (line 43):

```rust
// MINOR v2.6.0: StepRun gains Dynamic (bounded dynamic region; additive; EPIC 4.4).
pub const CURRENT: &'static str = "v2.6.0";
```

Update the two pin tests (`module.rs:148` and its `current()` assertion, `:169`):

```rust
    #[test]
    fn ir_format_version_is_v2_6_0() {
        assert_eq!(IrFormatVersion::CURRENT, "v2.6.0");
        assert_eq!(IrFormatVersion::current().0, "v2.6.0");
    }
```

(Rename the test fn from `ir_format_version_is_v2_5_0`. Leave `current_major_agrees_with_current_string` — major stays 2.)

- [ ] **Step 6: Run lib tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo nextest run -p tau-ir 2>&1 | tail -25`
Expected: PASS for `features`/`pipeline`/`module` tests. The schema snapshot test `schema_export` may FAIL (expected — regenerated next).

- [ ] **Step 7: Regenerate the schema snapshot**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 UPDATE_SCHEMA=1 cargo test -p tau-ir --features schema --test schema_export 2>&1 | tail -10`
Then re-run without `UPDATE_SCHEMA` to confirm green:
Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo test -p tau-ir --features schema --test schema_export 2>&1 | tail -10`
Expected: PASS. Inspect `git diff` of the snapshot file — it must show only the additive `Dynamic`/`DynamicSpawn` schema.

- [ ] **Step 8: Commit**

```bash
git add crates/tau-ir/src/pipeline.rs crates/tau-ir/src/features.rs crates/tau-ir/src/module.rs crates/tau-ir/tests/
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(tau-ir): StepRun::Dynamic + DynamicSpawn, bump IR to v2.6.0 (EPIC 4.4)"
```

---

### Task 3: tau-pkg — `[agent.kinds.*]` table + no-ceiling-gate cap bridge

**Files:**
- Modify: `crates/tau-pkg/src/project/allow.rs:136-164` (factor `bridge_cap_unchecked` / add `pub(crate) bridge_caps_any`)
- Modify: `crates/tau-pkg/src/project/project.rs` (`UncheckedProjectConfig` gains `agent.kinds`; `ProjectConfig.agent_kinds`; `AgentKindEntry`; validation)
- Test: `crates/tau-pkg/src/project/project.rs` tests mod

**Interfaces:**
- Consumes: existing `bridge_caps` internals in `allow.rs`.
- Produces: `pub(crate) fn bridge_caps_any(&BTreeMap<String, toml::Value>) -> Result<Vec<Capability>, ProjectConfigError>` (no ceiling-kind gate); `ProjectConfig.agent_kinds: BTreeMap<String, AgentKindEntry>`; `pub struct AgentKindEntry { pub name: String, pub capabilities: Vec<Capability> }`.

- [ ] **Step 1: Write the failing parse test** — in `crates/tau-pkg/src/project/project.rs` tests mod, add (adapt `parse`/`from_toml_str` helper to the one already used in that tests mod — grep the mod for the existing `ProjectConfig::` parse entry point and reuse it verbatim):

```rust
    #[test]
    fn agent_kinds_table_parses_with_capabilities() {
        let toml = r#"
[agent.kinds.researcher]
capabilities = [ { "net.http" = { hosts = ["api.crawler.test"] } } ]
"#;
        let cfg = parse_project(toml).expect("agent.kinds parses");
        let k = cfg.agent_kinds.get("researcher").expect("researcher kind present");
        assert_eq!(k.name, "researcher");
        assert_eq!(k.capabilities.len(), 1);
    }
```

> Executor note: replace `parse_project` with the tests mod's real parse helper (e.g. `ProjectConfig::from_str` / `validate_project(...)`). Grep the tests mod first; do not invent a helper.

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo test -p tau-pkg --lib agent_kinds 2>&1 | tail -20`
Expected: FAIL — `no field agent_kinds` / unknown table.

- [ ] **Step 3: Factor the cap bridge** — in `crates/tau-pkg/src/project/allow.rs`, split `bridge_cap` (136-156) so the parse half is reusable without the ceiling gate:

```rust
/// Parse one kind-as-key raw cap into a `Capability` through the single
/// domain deserializer (typo/unknown-kind/field-shape errors surface here).
/// No `[allow]` ceiling-kind gate — callers that accept any narrowable cap
/// (agent-kind grants, dynamic-region envelopes) use this directly.
pub(crate) fn bridge_cap_any(kind: &str, value: &toml::Value) -> Result<Capability, ProjectConfigError> {
    let json: JsonValue = serde_json::to_value(value)
        .map_err(|e| err(format!("raw-cap {kind:?}: not serializable: {e}")))?;
    let JsonValue::Object(mut obj) = json else {
        return Err(err(format!("raw-cap {kind:?}: value must be a table")));
    };
    obj.insert("kind".to_string(), JsonValue::String(kind.to_string()));
    serde_json::from_value::<Capability>(JsonValue::Object(obj))
        .map_err(|e| err(format!("capability {e}")))
}

/// Bridge a kind-as-key cap map into a `Vec<Capability>` with no ceiling gate.
pub(crate) fn bridge_caps_any(
    caps: &BTreeMap<String, toml::Value>,
) -> Result<Vec<Capability>, ProjectConfigError> {
    caps.iter().map(|(k, v)| bridge_cap_any(k, v)).collect()
}

fn bridge_cap(kind: &str, value: &toml::Value) -> Result<Capability, ProjectConfigError> {
    let cap = bridge_cap_any(kind, value)?;
    if !ALLOW_CEILING_KINDS.contains(&kind) {
        return Err(err(format!(
            "capability kind {kind:?} is valid but not permitted as an [allow] \
             ceiling entry (ceilings: fs.read, fs.write, fs.exec, net.http, process.spawn)"
        )));
    }
    Ok(cap)
}
```

(Keep the existing `bridge_caps` calling `bridge_cap`. The `[allow] `-prefixed error message becomes `capability ...`; if a test pins the `[allow] ` prefix, keep it by re-wrapping inside `bridge_cap` — grep `allow.rs` tests for `"[allow]"` before changing the string.)

- [ ] **Step 4: Add the unchecked + validated agent-kind types** — in `crates/tau-pkg/src/project/project.rs`:

Unchecked (near `UncheckedAgent`, ~line 73). The table is `[agent.kinds.<name>]`; model it as a nested map under an `agent` container so it does not collide with `[agents.*]`:

```rust
/// Raw `[agent.kinds.<name>]` per-kind agent definition (pre-validation).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedAgentKind {
    /// The kind's capability grant (kind-as-key raw caps, same shape as `[allow]`).
    #[serde(default)]
    pub capabilities: BTreeMap<String, toml::Value>,
}

/// Raw `[agent]` container holding the `kinds` sub-table.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedAgentContainer {
    /// `[agent.kinds.<name>]` per-kind agent definitions.
    #[serde(default)]
    pub kinds: BTreeMap<String, UncheckedAgentKind>,
}
```

Add the field to the top-level unchecked config struct (grep for `pub agents:` in the top-level unchecked struct, e.g. `UncheckedProjectConfig`, and add alongside):

```rust
    /// `[agent.kinds.*]` per-kind agent definitions (EPIC 4.4).
    #[serde(default)]
    pub agent: UncheckedAgentContainer,
```

Validated (near `AgentEntry`, ~line 925):

```rust
/// A validated per-kind agent definition.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentKindEntry {
    /// The kind name (the `[agent.kinds.<name>]` key).
    pub name: String,
    /// The kind's capability grant.
    pub capabilities: Vec<crate::domain_reexport::Capability>, // use the crate's existing Capability path
}
```

> Executor note: use the same `Capability` import path the rest of `project.rs` uses (grep `use tau_domain::` in the file). Add `agent_kinds: BTreeMap<String, AgentKindEntry>` to `ProjectConfig` (near `agents:` ~line 854).

- [ ] **Step 5: Validate agent kinds** — in the top-level `validate_*` that builds `ProjectConfig` (grep `agents:` assignment in the constructor), add:

```rust
    let agent_kinds = raw
        .agent
        .kinds
        .into_iter()
        .map(|(name, k)| {
            let capabilities = crate::project::allow::bridge_caps_any(&k.capabilities)?;
            Ok((name.clone(), AgentKindEntry { name, capabilities }))
        })
        .collect::<Result<BTreeMap<_, _>, ProjectConfigError>>()?;
```

and set `agent_kinds` in the returned `ProjectConfig`.

- [ ] **Step 6: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo nextest run -p tau-pkg 2>&1 | tail -25`
Expected: PASS incl. `agent_kinds_table_parses_with_capabilities`.

- [ ] **Step 7: Commit**

```bash
git add crates/tau-pkg/src/project/
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(tau-pkg): [agent.kinds.*] per-kind agent definitions (EPIC 4.4)"
```

---

### Task 4: tau-pkg — `PipelineRunRef::Dynamic` + dynamic-region authoring form

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs:311-343` (`UncheckedPipelineStep.dynamic`), `:405-445` (`PipelineRunRef::Dynamic`), `:1969-2095` (`validate_pipeline_step` 5th form)
- Test: `crates/tau-pkg/src/project/project.rs` tests mod

**Interfaces:**
- Consumes: `AgentKindEntry` / `bridge_caps_any` (Task 3).
- Produces: `PipelineRunRef::Dynamic { spawns: Vec<String>, ceiling: Vec<Capability>, max_spawns: u64, max_concurrency: u64, agent: Option<String> }`; new `UncheckedDynamic` struct.

- [ ] **Step 1: Write failing tests (valid + over-bounds)** — in the tests mod:

```rust
    #[test]
    fn dynamic_region_form_parses() {
        let toml = r#"
[[pipeline.steps]]
id = "fanout"
[pipeline.steps.dynamic]
spawns = ["researcher"]
ceiling = [ { "net.http" = { hosts = ["api.crawler.test"] } } ]
max_spawns = 8
max_concurrency = 4
"#;
        let cfg = parse_project(toml).expect("dynamic region parses");
        let step = &cfg.pipeline.as_ref().unwrap().steps[0];
        match &step.run {
            PipelineRunRef::Dynamic { spawns, ceiling, max_spawns, max_concurrency, agent } => {
                assert_eq!(spawns, &vec!["researcher".to_string()]);
                assert_eq!(ceiling.len(), 1);
                assert_eq!(*max_spawns, 8);
                assert_eq!(*max_concurrency, 4);
                assert!(agent.is_none());
            }
            other => panic!("expected Dynamic, got {other:?}"),
        }
    }

    #[test]
    fn dynamic_region_rejects_zero_max_spawns() {
        let toml = r#"
[[pipeline.steps]]
id = "fanout"
[pipeline.steps.dynamic]
spawns = ["researcher"]
ceiling = []
max_spawns = 0
max_concurrency = 1
"#;
        let err = parse_project(toml).expect_err("zero max_spawns rejected");
        assert!(format!("{err}").contains("max_spawns"));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo test -p tau-pkg --lib dynamic_region 2>&1 | tail -20`
Expected: FAIL — no `dynamic` field / no `PipelineRunRef::Dynamic`.

- [ ] **Step 3: Add the unchecked form** — `crates/tau-pkg/src/project/project.rs`, new struct near `UncheckedParallelBranch` (~line 356):

```rust
/// Dynamic-region form (EPIC 4.4): a bounded fan-out over spawnable kinds.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedDynamic {
    /// Kinds this region may spawn (each must be an `[agent.kinds.<name>]`).
    #[serde(default)]
    pub spawns: Vec<String>,
    /// Region capability envelope (kind-as-key raw caps).
    #[serde(default)]
    pub ceiling: BTreeMap<String, toml::Value>,
    /// Hard cap on total spawns (must be `> 0`).
    pub max_spawns: u64,
    /// Hard cap on concurrent spawns (`0 < n <= max_spawns`).
    pub max_concurrency: u64,
    /// Optional owning agent id; inserts the `agent ⊇ region` lattice link.
    #[serde(default)]
    pub agent: Option<String>,
}
```

Add the field to `UncheckedPipelineStep` (after `body`, line 342). Note `ceiling` as `BTreeMap` won't deserialize as a nested table under `[pipeline.steps.dynamic]` if authored as inline array-of-tables — keep the whole `dynamic` sub-table as one field:

```rust
    /// Dynamic-region form (EPIC 4.4): `[pipeline.steps.dynamic]`.
    #[serde(default)]
    pub dynamic: Option<UncheckedDynamic>,
```

- [ ] **Step 4: Add the validated variant** — in `PipelineRunRef` (line 445, after `Loop`):

```rust
    /// Dynamic region (EPIC 4.4). `spawns` are kind names resolved to caps
    /// during lowering; `ceiling` is the region envelope; `agent` is the
    /// optional owner for the `agent ⊇ region` link.
    Dynamic {
        /// Spawnable kind names (`[agent.kinds.<name>]`).
        spawns: Vec<String>,
        /// Region capability envelope.
        ceiling: Vec<crate::domain_reexport::Capability>, // match file's Capability path
        /// Hard total-spawn cap (`> 0`).
        max_spawns: u64,
        /// Hard concurrency cap (`0 < n <= max_spawns`).
        max_concurrency: u64,
        /// Optional owning agent id.
        agent: Option<String>,
    },
```

- [ ] **Step 5: Wire the 5th form into `validate_pipeline_step`** — `project.rs:1969-2095`:

Add detection after `has_loop` (line 1975):

```rust
    let has_dynamic = s.dynamic.is_some();
```

Include it in `form_count` (line 1978) and both error strings (mention `dynamic`):

```rust
    let form_count = [has_run, has_branch, has_parallel, has_loop, has_dynamic]
        .iter()
        .filter(|active| **active)
        .count();
```

Add the branch to the `if/else if` chain (before the final `else` Loop arm, or as a new `else if has_dynamic`):

```rust
    } else if has_dynamic {
        let d = s.dynamic.as_ref().unwrap();
        if d.spawns.is_empty() {
            return Err(bad("a dynamic region must list at least one `spawns` kind".into()));
        }
        if d.max_spawns == 0 {
            return Err(bad("a dynamic region's `max_spawns` must be greater than 0".into()));
        }
        if d.max_concurrency == 0 || d.max_concurrency > d.max_spawns {
            return Err(bad(
                "a dynamic region's `max_concurrency` must be in 1..=max_spawns".into(),
            ));
        }
        let ceiling = crate::project::allow::bridge_caps_any(&d.ceiling)
            .map_err(|e| bad(format!("dynamic region `ceiling`: {e}")))?;
        PipelineRunRef::Dynamic {
            spawns: d.spawns.clone(),
            ceiling,
            max_spawns: d.max_spawns,
            max_concurrency: d.max_concurrency,
            agent: d.agent.clone(),
        }
    } else {
```

(Adjust the final `else` that currently handles Loop so the chain stays `... else if has_loop-detected-by-fallthrough`. Simplest: change the trailing `} else {` Loop block into `} else if has_loop {` and keep a final `unreachable!()`-free structure — but `form_count == 1` already guarantees exactly one, so an explicit `else if has_dynamic { … } else { /* loop */ }` is safe. Preserve the existing Loop code verbatim in the final `else`.)

- [ ] **Step 6: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo nextest run -p tau-pkg 2>&1 | tail -25`
Expected: PASS incl. both new tests.

- [ ] **Step 7: Commit**

```bash
git add crates/tau-pkg/src/project/project.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(tau-pkg): dynamic-region authoring form -> PipelineRunRef::Dynamic (EPIC 4.4)"
```

---

### Task 5: tau-ir-lower — lower `Dynamic`, resolve kinds, wasm feature-reject

**Files:**
- Modify: `crates/tau-ir-lower/src/lower/parse.rs:528-567` (`lower_step` Dynamic arm; thread kind map)
- Modify: `crates/tau-ir-lower/src/lower/typecheck.rs:464-644` (`validate_step_run` Dynamic arm)
- Modify: `crates/tau-ir-lower/src/error.rs` (new `UnknownAgentKind` variant)
- Test: `crates/tau-ir-lower/src/lower/parse.rs` tests, `crates/tau-ir-lower/src/lower/feature_fit.rs` tests (`:105`)

**Interfaces:**
- Consumes: `PipelineRunRef::Dynamic` (Task 4), `ProjectConfig.agent_kinds` (Task 3), `StepRun::Dynamic`/`DynamicSpawn` (Task 2).
- Produces: lowered `StepRun::Dynamic`; `LowerError::UnknownAgentKind { kind: String, step: String }`.

- [ ] **Step 1: Write failing lowering tests** — in `parse.rs` tests, add a known-kind → `StepRun::Dynamic` test and an unknown-kind → `UnknownAgentKind` test. Reuse the tests mod's existing "lower a ProjectConfig" helper (grep the mod for how it builds a `ProjectConfig` and calls the lower entry point):

```rust
    #[test]
    fn dynamic_region_lowers_with_resolved_kind_caps() {
        // Build a ProjectConfig with [agent.kinds.researcher] + a dynamic
        // region spawning "researcher"; lower it; assert the emitted
        // StepRun::Dynamic embeds researcher's caps in spawns[0].capabilities.
        // (Use the tests mod's existing project-build + lower helpers.)
    }

    #[test]
    fn dynamic_region_unknown_kind_is_lower_error() {
        // Dynamic region spawns "ghost" with no [agent.kinds.ghost];
        // lowering returns LowerError::UnknownAgentKind { kind: "ghost", .. }.
    }
```

> Executor: fill these two test bodies using the tests mod's real fixtures. Do not invent helper fns — mirror an existing lowering test in the same file.

- [ ] **Step 2: Run to verify failure**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo test -p tau-ir-lower --lib dynamic_region 2>&1 | tail -20`
Expected: FAIL (compile: no `Dynamic` arm / no `UnknownAgentKind`).

- [ ] **Step 3: Add the error variant** — `crates/tau-ir-lower/src/error.rs`, mirroring the style of `LoopMaxItersZero` (line ~239):

```rust
    /// A dynamic region references an agent kind with no `[agent.kinds.<name>]`
    /// definition (EPIC 4.4).
    #[error("dynamic region in step '{step}' spawns unknown agent kind '{kind}' — declare [agent.kinds.{kind}]")]
    UnknownAgentKind {
        /// The undefined kind name.
        kind: String,
        /// The pipeline-step id of the region.
        step: String,
    },
```

- [ ] **Step 4: Thread the kind map + add the `lower_step` arm** — `parse.rs`. `lower_step` currently has signature `fn lower_step(s: &PipelineStepConfig) -> PipelineStep`. Change it to accept the resolved kind→caps map and return `Result` (so unknown-kind can fail), OR — simpler and non-invasive — resolve *before* lowering by passing a `&BTreeMap<String, Vec<Capability>>` and propagating `Result`:

```rust
fn lower_step(
    s: &tau_pkg::project::PipelineStepConfig,
    kinds: &BTreeMap<String, Vec<tau_domain::Capability>>,
) -> Result<PipelineStep, LowerError> {
    let run = match &s.run {
        PipelineRunRef::Agent(id) => StepRun::Agent(AgentId(id.clone())),
        // …existing arms, each now recursing with `?` and `kinds`, e.g.:
        PipelineRunRef::Branch { on, then, otherwise } => StepRun::Branch {
            on: lower_condition(on),
            then: then.iter().map(|st| lower_step(st, kinds)).collect::<Result<_, _>>()?,
            otherwise: otherwise.iter().map(|st| lower_step(st, kinds)).collect::<Result<_, _>>()?,
        },
        // … Parallel / Loop similarly thread `kinds` and `?` …
        PipelineRunRef::Dynamic { spawns, ceiling, max_spawns, max_concurrency, .. } => {
            let mut resolved = Vec::with_capacity(spawns.len());
            for kind in spawns {
                let caps = kinds.get(kind).ok_or_else(|| LowerError::UnknownAgentKind {
                    kind: kind.clone(),
                    step: s.id.clone(),
                })?;
                resolved.push(DynamicSpawn {
                    kind: kind.clone(),
                    capabilities: CapabilityRequirements { declared: caps.clone() },
                });
            }
            StepRun::Dynamic {
                envelope: CapabilityRequirements { declared: ceiling.clone() },
                spawns: resolved,
                max_spawns: *max_spawns,
                max_concurrency: *max_concurrency,
            }
        }
    };
    Ok(PipelineStep { id: PipelineStepId(s.id.clone()), run, input: s.input.clone() })
}
```

Update the single top-level caller of `lower_step` (grep `lower_step(` in `parse.rs`) to build `kinds` from `project.agent_kinds` (`name → capabilities.clone()`), pass it, and propagate `?`. Add imports: `use tau_ir::pipeline::DynamicSpawn; use tau_ir::capability::CapabilityRequirements; use alloc::collections::BTreeMap;` (match existing import style — this crate may be `std`; use `std::collections::BTreeMap` if so).

- [ ] **Step 5: Add the `validate_step_run` Dynamic arm** — `typecheck.rs`. The match is exhaustive with no wildcard; add a leaf-like arm (a region has no nested pipeline steps and produces an output, so nothing to scope-check here — bounds/kinds are enforced upstream):

```rust
        StepRun::Dynamic { .. } => {
            // Bounds validated at author time (tau-pkg) and kind resolution at
            // lowering (UnknownAgentKind); the region produces an output and
            // nests no pipeline steps, so no reference/scope check here.
            Ok(())
        }
```

(Confirm the arm's return type matches the surrounding arms — some return `Ok(())`, adjust if the fn returns a collected set.)

- [ ] **Step 6: Add the wasm feature-reject test** — `crates/tau-ir-lower/src/lower/feature_fit.rs` tests, mirroring `wasm_target_rejects_control_flow` (:105):

```rust
    #[test]
    fn wasm_target_rejects_dynamic_region() {
        // A pipeline with a StepRun::Dynamic used against any-wasi-strict must
        // fail feature_fit::check with FeatureUnsupported { missing: [Dynamic] }.
        // (Mirror wasm_target_rejects_control_flow's setup; swap the step for a
        // minimal Dynamic.)
    }
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo nextest run -p tau-ir-lower 2>&1 | tail -25`
Expected: PASS incl. the three new tests.

- [ ] **Step 8: Commit**

```bash
git add crates/tau-ir-lower/src/
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(tau-ir-lower): lower Dynamic region, resolve kinds, wasm feature-reject (EPIC 4.4)"
```

---

### Task 6: tau-cli governance — promote agent⊇spawn + add region lattice links (THE CORE DELTA)

**Files:**
- Modify: `crates/tau-cli/src/cmd/check/categories/governance.rs:199-336` (`lattice` fn: L3 promote, L4a, L4b, remove Note)
- Test: governance tests (grep `governance` test module / `tests/` fixtures for the existing `over_reach`/`lattice` tests to mirror)

**Interfaces:**
- Consumes: `ProjectConfig.agent_kinds` (Task 3), `PipelineRunRef::Dynamic` (Task 4), `capability_set_subset` (= `tau_domain::capability_subset`), `resolve_agent_caps`/`AgentCaps` (existing).
- Produces: findings `tau.governance.spawn_exceeds_agent`, `tau.governance.unknown_spawn_kind`, `tau.governance.region_exceeds_ceiling`, `tau.governance.spawn_exceeds_region` (all `Severity::Error`); removes `tau.governance.spawn_runtime_enforced` Note.

- [ ] **Step 1: Write the failing over-reach test (THE TDD ANCHOR)** — add a governance test: a kind whose `net.http hosts=["*"]` exceeds a region `ceiling` of `hosts=["api.crawler.test"]` must yield a `spawn_exceeds_region` Error. Mirror the existing governance test harness (grep `governance_findings(` usage in tests):

```rust
    #[test]
    fn over_reaching_spawn_in_region_fails_check() {
        // ProjectConfig with:
        //   [allow] net.http hosts = ["*"]        (root permits)
        //   [agent.kinds.greedy] net.http hosts = ["*"]
        //   a dynamic region: spawns=["greedy"], ceiling net.http hosts=["api.crawler.test"]
        // Expect a finding rule_id == "tau.governance.spawn_exceeds_region",
        // Severity::Error (greedy's ["*"] ⊄ region ceiling ["api.crawler.test"]).
        let findings = run_governance(/* built ProjectConfig + AllowConfig */);
        assert!(findings.iter().any(|f|
            f.rule_id == "tau.governance.spawn_exceeds_region"
            && f.severity == Severity::Error));
    }
```

> Executor: build the `ProjectConfig`/`CheckCtx` with the tests mod's existing fixtures. This is the plan's TDD anchor — it must be red before Step 3.

- [ ] **Step 2: Run to verify failure**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo nextest run -p tau-cli governance 2>&1 | tail -25`
Expected: FAIL (no such rule / no region check yet).

- [ ] **Step 3: Promote L3 to a real check** — in `governance.rs`, replace the `any_spawn` accumulation (lines 317-320) and the trailing Note block (324-335). Inside the `AgentCaps::Resolved { manifest, effective }` arm, after the L2 loop:

```rust
                // L3: agent ⊇ spawn, now build-time enforced (EPIC 4.4).
                // Each kind in the agent's Agent(Spawn { allowed_kinds }) must
                // be an [agent.kinds.<kind>] whose caps ⊆ the agent's effective.
                for cap in &manifest {
                    if let Capability::Agent(tau_domain::AgentCapability::Spawn { allowed_kinds }) = cap {
                        for kind in allowed_kinds {
                            match project.agent_kinds.get(kind) {
                                None => out.push(lattice_error(
                                    "unknown_spawn_kind",
                                    "tau.governance.unknown_spawn_kind",
                                    &format!("agent '{}' may spawn kind '{kind}' but no [agent.kinds.{kind}] is defined", agent.id),
                                    &tau_toml,
                                )),
                                Some(k) => {
                                    if let Err(v) = capability_set_subset(&k.capabilities, &effective) {
                                        out.push(lattice_error(
                                            "spawn_exceeds_agent",
                                            "tau.governance.spawn_exceeds_agent",
                                            &format!("agent '{}': spawn kind '{kind}' capability {} \"{}\" exceeds the agent's effective grant ({})", agent.id, v.kind, v.offender, v.reason),
                                            &tau_toml,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
```

Delete the `let mut any_spawn = false;` (line 230), the `any_spawn = true;` block (317-320), and the whole `if any_spawn { … Note … }` block (324-335). (Confirm `tau_domain::AgentCapability` is imported or path-qualify it.)

- [ ] **Step 4: Add the region lattice check (L4a + L4b)** — add a new helper called from `lattice` after the agent loop, iterating the pipeline for `PipelineRunRef::Dynamic` steps:

```rust
    // L4: dynamic-region envelope lattice (EPIC 4.4).
    //   L4a region ⊆ owner (named agent's effective, else root [allow]).
    //   L4b each spawn kind ⊆ region ceiling.
    if let Some(pipeline) = &project.pipeline {
        check_dynamic_regions(&pipeline.steps, project, allow, ctx, out);
    }
```

New fn (near `lattice`), recursing nested control-flow bodies so a region nested in a Branch/Loop/Parallel is still checked:

```rust
fn check_dynamic_regions(
    steps: &[tau_pkg::project::PipelineStepConfig],
    project: &ProjectConfig,
    allow: &AllowConfig,
    ctx: &CheckCtx,
    out: &mut Vec<CheckFinding>,
) {
    use tau_pkg::project::PipelineRunRef;
    let tau_toml = ctx.project_root.join("tau.toml");
    for step in steps {
        match &step.run {
            PipelineRunRef::Dynamic { spawns, ceiling, agent, .. } => {
                // L4a: region ceiling ⊆ owner.
                let (owner_caps, owner_desc): (Vec<_>, String) = match agent {
                    Some(a) => match project.agents.get(a).map(|ag| resolve_agent_caps(ag, ctx)) {
                        Some(AgentCaps::Resolved { effective, .. }) => (effective, format!("agent '{a}' effective grant")),
                        _ => (allow.ceiling.clone(), format!("agent '{a}' (unresolved; falling back to [allow])")),
                    },
                    None => (allow.ceiling.clone(), "[allow] ceiling".to_string()),
                };
                if let Err(v) = capability_set_subset(ceiling, &owner_caps) {
                    out.push(lattice_error(
                        "region_exceeds_ceiling",
                        "tau.governance.region_exceeds_ceiling",
                        &format!("dynamic region '{}': envelope capability {} \"{}\" exceeds {} ({})", step.id, v.kind, v.offender, owner_desc, v.reason),
                        &tau_toml,
                    ));
                }
                // L4b: each spawn kind ⊆ region ceiling.
                for kind in spawns {
                    match project.agent_kinds.get(kind) {
                        None => out.push(lattice_error(
                            "unknown_spawn_kind",
                            "tau.governance.unknown_spawn_kind",
                            &format!("dynamic region '{}' spawns kind '{kind}' but no [agent.kinds.{kind}] is defined", step.id),
                            &tau_toml,
                        )),
                        Some(k) => {
                            if let Err(v) = capability_set_subset(&k.capabilities, ceiling) {
                                out.push(lattice_error(
                                    "spawn_exceeds_region",
                                    "tau.governance.spawn_exceeds_region",
                                    &format!("dynamic region '{}': spawn kind '{kind}' capability {} \"{}\" exceeds the region envelope ({})", step.id, v.kind, v.offender, v.reason),
                                    &tau_toml,
                                ));
                            }
                        }
                    }
                }
            }
            PipelineRunRef::Branch { then, otherwise, .. } => {
                check_dynamic_regions(then, project, allow, ctx, out);
                check_dynamic_regions(otherwise, project, allow, ctx, out);
            }
            PipelineRunRef::Parallel { branches } => {
                for b in branches { check_dynamic_regions(b, project, allow, ctx, out); }
            }
            PipelineRunRef::Loop { body, .. } => check_dynamic_regions(body, project, allow, ctx, out),
            _ => {}
        }
    }
}
```

- [ ] **Step 5: Add positive tests** — a well-formed region (spawn ⊆ ceiling ⊆ root) yields **no** Error findings; an agent with a defined-and-fitting spawn kind yields no `unknown_spawn_kind`/`spawn_exceeds_agent`. Assert the old `spawn_runtime_enforced` Note is **gone**.

- [ ] **Step 6: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo nextest run -p tau-cli 2>&1 | tail -30`
Expected: PASS. If a pre-existing test asserted the `spawn_runtime_enforced` Note, update it to expect the new build-time behavior.

- [ ] **Step 7: Commit**

```bash
git add crates/tau-cli/src/cmd/check/categories/governance.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(tau-cli): build-time agent⊇spawn + dynamic-region lattice (EPIC 4.4)"
```

---

### Task 7: tau-runtime-core — interpreter meets `Dynamic` with a named error

**Files:**
- Modify: `crates/tau-runtime-core/src/interpreter/pipeline.rs:581-621,779-790` (early guard + unreachable arm)
- Modify: the `RuntimeError` enum (grep `SuspendUnsupported` to find it) — add `DynamicRegionRequiresRuntimeGate`
- Test: interpreter tests (mirror the Suspend-unsupported test)

**Interfaces:**
- Consumes: `StepRun::Dynamic` (Task 2).
- Produces: `RuntimeError::DynamicRegionRequiresRuntimeGate { step_id: String }`.

- [ ] **Step 1: Write the failing test** — mirror the existing "suspend unsupported" interpreter test: a module whose pipeline contains a `StepRun::Dynamic` step, driven by the non-suspend `run_pipeline` entry, returns `Err(RuntimeError::DynamicRegionRequiresRuntimeGate { .. })`.

```rust
    #[test]
    fn dynamic_region_errors_pending_runtime_gate() {
        // Build an IrModule with one StepRun::Dynamic step (minimal envelope +
        // one DynamicSpawn), run it, assert the named error. Mirror the
        // suspend-unsupported test's module construction.
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo test -p tau-runtime-core --lib dynamic_region 2>&1 | tail -20`
Expected: FAIL (no variant / non-exhaustive match).

- [ ] **Step 3: Add the error variant** — in the `RuntimeError` enum, near `SuspendUnsupported`:

```rust
    /// Hit a `StepRun::Dynamic` region, whose runtime gate (membership,
    /// attenuation, bounds counters) lands in EPIC 4.5. Build-time envelope
    /// verification already ran at `tau check`; execution is not yet wired.
    #[error("dynamic region '{step_id}' requires the EPIC 4.5 runtime gate (not yet implemented)")]
    DynamicRegionRequiresRuntimeGate {
        /// The pipeline-step id of the dynamic region.
        step_id: String,
    },
```

- [ ] **Step 4: Add the early-dispatch guard** — in `run_steps` (pipeline.rs), add a guard alongside the other control-flow guards (e.g. after the `Suspend` guard ~line 618, before the leaf `match` at 685):

```rust
        if let StepRun::Dynamic { .. } = &step.run {
            return Err(RuntimeError::DynamicRegionRequiresRuntimeGate {
                step_id: step.id.0.clone(),
            });
        }
```

And add `StepRun::Dynamic { .. }` to the `unreachable!` arm group at 785-790 so the leaf `match` stays exhaustive:

```rust
            StepRun::Branch { .. }
            | StepRun::Parallel { .. }
            | StepRun::Loop { .. }
            | StepRun::Suspend { .. }
            | StepRun::Dynamic { .. } => {
                unreachable!("control-flow blocks are early-dispatched")
            }
```

(Also check the suspend-only wrapper `run_pipeline`/`run_pipeline_suspendable` docs at 111-113 and 217 — extend the "supported steps" doc comment to mention Dynamic errors pending 4.5, if a test asserts on it.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo nextest run -p tau-runtime-core 2>&1 | tail -25`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-runtime-core/src/interpreter/
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(tau-runtime-core): interpreter names Dynamic runtime-gate deferral to 4.5 (EPIC 4.4)"
```

---

### Task 8: Conformance fixtures + docs + roadmap

**Files:**
- Create: conformance fixture(s) under the existing conformance fixture tree (grep `tests/` / `conformance` for where 4.2a/4.2b/4.3 fixtures live — mirror that exact layout)
- Create: `docs/explanation/dynamic-regions.md` (or `docs/how-to/`, matching sibling construct docs)
- Modify: `docs/SUMMARY.md` (add the new page — mdBook drops unlisted pages)
- Modify: `docs/superpowers/plans/vision-roadmap.md` (tick 4.4)
- Modify: `crates/tau-cli/src/cmd/check/categories/governance.rs` module doc (update the L3 line to say build-enforced)

**Interfaces:**
- Consumes: everything above (end-to-end).

- [ ] **Step 1: Locate the conformance harness** — `grep -rn "conformance\|fixture" crates/tau-cli/tests crates/*/tests | head`; identify how the 4.2a Branch / 4.3 Suspend fixtures are declared and run in CI. Mirror that mechanism exactly (do not invent a new harness).

- [ ] **Step 2: Add fixture A — well-formed region builds to dev.** A `tau.toml` with `[allow]`, `[agent.kinds.researcher]`, and a bounded dynamic region; assert `tau build --target dev` (or the harness's build entry) succeeds and the emitted IR contains a `StepRun::Dynamic`. Commit nothing yet.

- [ ] **Step 3: Add fixture B — over-reaching spawn fails `tau check`.** The greedy-kind-vs-region-ceiling case; assert non-zero exit + `spawn_exceeds_region`. This is the end-to-end anchor of the whole slice.

- [ ] **Step 4: Add fixture C — wasm rejects Dynamic.** `tau build --target wasm` on fixture A fails with `FeatureUnsupported { missing: [Dynamic] }`.

- [ ] **Step 5: Run the conformance suite**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo nextest run -p tau-cli 2>&1 | tail -30`
Expected: PASS incl. the three fixtures.

- [ ] **Step 6: Write the docs page** — `docs/explanation/dynamic-regions.md`: the lattice (`root ⊇ agent ⊇ region ⊇ spawn ⊇ tool`), the `[agent.kinds.*]` + `[pipeline.steps.dynamic]` syntax (the tau.toml example from the spec), the build-time check, and a one-line "runtime execution: EPIC 4.5" note. Add its line to `docs/SUMMARY.md` under the appropriate section.

- [ ] **Step 7: Build the book locally** (DOCS RULES)

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build 2>&1 | tail -15 && cd .. && rm -rf docs/book`
Expected: only `[INFO]` lines, no linkcheck errors.

- [ ] **Step 8: Tick the roadmap + fix the governance module doc.** In `vision-roadmap.md`, mark 4.4 done. In `governance.rs:1-6` module doc, change the L3 line from "runtime-enforced" to "build-enforced (per-kind agent definitions, EPIC 4.4)".

- [ ] **Step 9: Full gate across touched crates**

Run each (separate target dir already set):
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo nextest run -p tau-domain
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo nextest run -p tau-ir
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo nextest run -p tau-ir-lower
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo nextest run -p tau-pkg
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo nextest run -p tau-runtime-core
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo nextest run -p tau-cli
timeout 30 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo fmt --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e44 cargo clippy -p tau-cli -p tau-pkg -p tau-ir-lower -p tau-ir -p tau-domain -p tau-runtime-core -- -D warnings
```
Expected: all PASS, fmt clean, clippy clean.

- [ ] **Step 10: Commit + open PR**

```bash
git add docs/ crates/
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(4.4): dynamic-region conformance fixtures + docs; tick roadmap"
git push -u origin feat/epic-4-4-dynamic-regions
gh pr create --base main --title "feat: EPIC 4.4 — bounded dynamic regions + per-kind agent definitions" --body "<summary + spec link + emits 4.5 follow-up>"
gh pr merge <PR#> --squash --auto
```

---

## Self-Review

**Spec coverage:**
- Per-kind agent defs (ADR-0024) → Task 1 (`AgentKind`) + Task 3 (`[agent.kinds.*]`). ✓
- Dynamic-region syntax → `StepRun::Dynamic` lowering (ceiling + bounds) → Task 2 (IR) + Task 4 (authoring) + Task 5 (lowering). ✓
- Build-time lattice (root ⊇ agent ⊇ region ⊇ spawn ⊇ tool); over-reaching spawn fails `tau check` → Task 6. ✓
- Flip `Dynamic` IrFeature → Task 1. ✓
- wasm parity / feature-reject → Task 5 (Step 6). ✓
- Conformance fixture in CI + docs example + SUMMARY → Task 8. ✓
- Runtime deferred to 4.5 with named error → Task 7. ✓
- IR version bump + schema regen → Task 2. ✓

**Placeholder scan:** Test bodies in Tasks 5/6/7/8 are marked "fill using the tests mod's real fixtures" rather than invented helpers — this is deliberate (the executor must not fabricate a harness that diverges from the codebase's). Every *implementation* step has concrete code. No "add error handling"-style vagueness.

**Type consistency:** `CapabilityRequirements { declared: Vec<Capability> }` used identically in Tasks 2/5. `PipelineRunRef::Dynamic` fields (`spawns: Vec<String>`, `ceiling: Vec<Capability>`, `max_spawns`, `max_concurrency`, `agent: Option<String>`) match between Task 4 (defined) and Tasks 5/6 (consumed). `DynamicSpawn { kind, capabilities }` consistent Task 2↔5. `AgentKindEntry { name, capabilities }` consistent Task 3↔6. Rule ids (`spawn_exceeds_region`, `unknown_spawn_kind`, `spawn_exceeds_agent`, `region_exceeds_ceiling`) consistent Task 6↔8. `RuntimeError::DynamicRegionRequiresRuntimeGate` consistent Task 7.

**Known executor watch-points (call out, don't hide):**
- The exact `Capability` import path in `project.rs` (`crate::domain_reexport::Capability` is a placeholder — grep the file's real `use tau_domain::` path and use it).
- Whether `tau-ir-lower` is `std` or `no_std` (BTreeMap import path).
- The `[allow] ` error-prefix in `bridge_cap` — preserve if a test pins it.
- The real parse/lower/governance test helpers — mirror existing tests, never invent.
