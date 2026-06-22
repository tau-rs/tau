# EPIC 6.1 — Durability intent knob — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `durable = "survive-restarts"` (intent) that the host resolves to a concrete checkpoint granularity + store per target, with `tau check --target X` printing the resolution.

**Architecture:** The IR carries the *intent* (`Durability::Intent`) or the existing *explicit* form (`Durability::Explicit`). A pure `resolve_durability(&Durability, &TargetTriple)` in `tau-runtime-core` maps intent → concrete per target. The host (`ir_dispatcher`) resolves with `TargetTriple::host()` at run; `tau check --target X` resolves and prints. Bundles stay portable (one IR, host-sized durability).

**Tech Stack:** Rust (8-crate workspace), serde, the existing `tau check` finding model, the `tau-ir-conformance` harness.

**Spec:** `docs/superpowers/specs/2026-06-22-epic-6-1-durability-intent-knob-design.md`

## Global Constraints

- **Cargo rules (repo CLAUDE.md):** every cargo invocation is `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo <cmd> -p <crate>`. Use `target/agent-impl` for implementation, `target/agent-test` if running a second build concurrently. Prefer `cargo nextest run -p <crate>`; doctests via `cargo test -p <crate> --doc`. Timeouts: test 300s, build/check 180s.
- **Commits:** conventional, imperative, scoped. Use `git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "..."` (the lefthook test step can corrupt git identity; `--no-verify` is acceptable here — CI is the gate).
- **Branch:** `feat/epic-6-1-durability-intent-knob` (already created; the spec commit is its first commit).
- **Transient workspace breakage is expected.** Task 1 changes the `Durability` shape; `tau-ir-lower` (Task 3) and `tau-runtime-core` (Task 4) do not compile until their tasks land. Each task verifies ONLY its own crate with `-p`. Full-workspace green is verified in Task 8.
- **No `unsafe`.** `tau-runtime-core` is `#![no_std]` + alloc — the resolver must be `core`/`alloc`-only (no `std`): use `&'static str` for reasons, no `String` formatting that needs `std`.
- **IR format bump:** v2.2.0 → **v2.3.0** (MINOR; the `Durability` field already exists and is optional, the shape of its value grows).

---

### Task 1: `tau-ir` — `Durability` becomes `Intent | Explicit`; IR bump v2.3.0

**Files:**
- Modify: `crates/tau-ir/src/durable.rs` (replace the struct with a tagged enum; add `DurabilityIntent`; re-point constructors)
- Modify: `crates/tau-ir/src/module.rs:33-36` (`CURRENT` → `v2.3.0`) and `:100-102` (drift test)
- Modify: `crates/tau-ir/src/canonical.rs:66` (test asserts `v2.3.0`)

**Interfaces:**
- Produces:
  - `enum Durability { Intent(DurabilityIntent), Explicit { checkpoint: CheckpointGranularity, store: DurableStore } }` (`#[non_exhaustive]`, serde externally-tagged snake_case → JSON `{"intent":"survive-restarts"}` / `{"explicit":{"checkpoint":"per_turn","store":"file"}}`)
  - `enum DurabilityIntent { SurviveRestarts }` (`#[non_exhaustive]`, serde rename `"survive-restarts"`)
  - `Durability::per_turn_file() -> Durability` (now returns the `Explicit` variant — unchanged signature)
  - `Durability::new(CheckpointGranularity, DurableStore) -> Durability` (now returns `Explicit` — unchanged signature)
  - `CheckpointGranularity` and `DurableStore` unchanged.

- [ ] **Step 1: Write the failing tests** (replace the `#[cfg(test)] mod tests` block in `crates/tau-ir/src/durable.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_round_trips() {
        let d = Durability::per_turn_file();
        let json = serde_json::to_string(&d).expect("serialize");
        let back: Durability = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(d, back);
    }

    #[test]
    fn explicit_serializes_tagged_snake_case() {
        let d = Durability::per_turn_file();
        let json = serde_json::to_string(&d).expect("serialize");
        // externally tagged: {"explicit":{"checkpoint":"per_turn","store":"file"}}
        assert!(json.contains("\"explicit\""), "got: {json}");
        assert!(json.contains("per_turn"), "got: {json}");
        assert!(json.contains("\"file\""), "got: {json}");
    }

    #[test]
    fn explicit_per_tool_call_round_trips() {
        let d = Durability::new(CheckpointGranularity::PerToolCall, DurableStore::File);
        let json = serde_json::to_string(&d).expect("serialize");
        assert!(json.contains("per_tool_call"), "got: {json}");
        let back: Durability = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(d, back);
    }

    #[test]
    fn intent_round_trips_and_serializes_kebab() {
        let d = Durability::Intent(DurabilityIntent::SurviveRestarts);
        let json = serde_json::to_string(&d).expect("serialize");
        // {"intent":"survive-restarts"}
        assert!(json.contains("\"intent\""), "got: {json}");
        assert!(json.contains("survive-restarts"), "got: {json}");
        let back: Durability = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(d, back);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ir durable`
Expected: FAIL to compile (`Durability::Intent` / `DurabilityIntent` undefined).

- [ ] **Step 3: Replace the type definitions** in `crates/tau-ir/src/durable.rs` (lines 19-45, the `struct Durability` + its `impl`). Keep the module doc comment and the `CheckpointGranularity` / `DurableStore` enums (lines 47-77) unchanged.

```rust
/// Durable-execution config attached to an [`crate::node::Agent`].
///
/// Either a high-level **intent** (the host picks granularity + store per
/// target — EPIC 6.1) or the **explicit** A-minimal form (ADR-0053).
/// Absent (`None` on the agent) is byte-stable with pre-A-minimal modules.
#[non_exhaustive]
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Durability {
    /// High-level intent. The host resolves it to a concrete granularity +
    /// store for the run/build target (see `tau_runtime_core::durable_resolve`).
    Intent(DurabilityIntent),
    /// Explicit escape hatch: the author names the mechanism directly.
    Explicit {
        /// How often a checkpoint is committed.
        checkpoint: CheckpointGranularity,
        /// Where checkpoints are written.
        store: DurableStore,
    },
}

impl Durability {
    /// Construct the explicit form from parts. Required because the enum is
    /// `#[non_exhaustive]` — crates outside `tau-ir` cannot use the variant
    /// struct-literal directly.
    pub fn new(checkpoint: CheckpointGranularity, store: DurableStore) -> Self {
        Self::Explicit { checkpoint, store }
    }

    /// The A-minimal default: explicit per-turn checkpoints to the filesystem.
    pub fn per_turn_file() -> Self {
        Self::new(CheckpointGranularity::PerTurn, DurableStore::File)
    }
}

/// High-level durability intent. The host sizes it per target.
///
/// `#[non_exhaustive]`: more intents are additive `MINOR` `ir_format` bumps.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum DurabilityIntent {
    /// "This run must survive a process restart." Resolves per target to the
    /// coarsest checkpoint + store the target can durably provide.
    #[serde(rename = "survive-restarts")]
    SurviveRestarts,
}
```

- [ ] **Step 4: Bump the IR format version.** In `crates/tau-ir/src/module.rs`, update the comment block + constant (around lines 33-36):

```rust
    // MINOR v2.3.0: Durability gains the `Intent(survive-restarts)` variant
    // (EPIC 6.1) alongside the explicit form. Optional field, additive shape.
    pub const CURRENT: &'static str = "v2.3.0";
```

Update the drift test (around lines 100-102):

```rust
    fn ir_format_version_is_v2_3_0() {
        assert_eq!(IrFormatVersion::CURRENT, "v2.3.0");
        assert_eq!(IrFormatVersion::current().0, "v2.3.0");
    }
```

(Rename the `fn ir_format_version_is_v2_2_0` test to `..._v2_3_0` to match.)

In `crates/tau-ir/src/canonical.rs` update the assertion at line 66:

```rust
        assert_eq!(m.ir_format.0, "v2.3.0");
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir`
Expected: PASS (all durable, module, canonical tests green). Also run doctests:
Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ir --doc`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-ir/src/durable.rs crates/tau-ir/src/module.rs crates/tau-ir/src/canonical.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify \
  -m "feat(epic-6.1): tau-ir Durability intent variant + ir_format v2.3.0"
```

---

### Task 2: `tau-pkg` — accept the intent scalar; `DurableEntry` becomes an enum

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs` — `UncheckedDurable` (line ~212), `DurableEntry` (line ~750), validation block (line ~1470)
- Test: inline `#[cfg(test)]` in the same file (sibling to `durable_accepts_per_tool_call` at line ~4287)

**Interfaces:**
- Consumes: nothing from earlier tasks (`tau-pkg` does not depend on `tau-ir`).
- Produces:
  - `enum UncheckedDurable { Intent(String), Explicit(UncheckedDurableExplicit) }` (`#[serde(untagged)]`)
  - `struct UncheckedDurableExplicit { checkpoint: String, store: String }` (`deny_unknown_fields`)
  - `enum DurableEntry { Intent(String), Explicit { checkpoint: String, store: String } }`
  - `AgentEntry.durable: Option<DurableEntry>` (field type unchanged; variant shape new)

- [ ] **Step 1: Write the failing tests** (add next to `durable_accepts_per_tool_call`, ~line 4287)

```rust
    #[test]
    fn durable_accepts_intent_string() {
        let toml = r#"
            packages = []
            [project]
            name = "p"
            [models.m]
            backend = "b"
            model = "m"
            [agents.a]
            display_name = "A"
            package = "p@^0.1"
            model = "m"
            durable = "survive-restarts"
        "#;
        let cfg = parse(toml).expect("valid intent durable");
        let agent = cfg.agents.get("a").expect("agent a");
        match agent.durable.as_ref().expect("durable present") {
            DurableEntry::Intent(s) => assert_eq!(s, "survive-restarts"),
            other => panic!("expected Intent, got {other:?}"),
        }
    }

    #[test]
    fn durable_rejects_unknown_intent_string() {
        let toml = r#"
            packages = []
            [project]
            name = "p"
            [models.m]
            backend = "b"
            model = "m"
            [agents.a]
            display_name = "A"
            package = "p@^0.1"
            model = "m"
            durable = "make-it-immortal"
        "#;
        let err = parse(toml).expect_err("unknown intent must fail");
        assert!(
            format!("{err}").contains("survive-restarts"),
            "error should name the accepted intent, got: {err}"
        );
    }

    #[test]
    fn durable_explicit_table_still_parses() {
        let toml = r#"
            packages = []
            [project]
            name = "p"
            [models.m]
            backend = "b"
            model = "m"
            [agents.a]
            display_name = "A"
            package = "p@^0.1"
            model = "m"
            [agents.a.durable]
            checkpoint = "per_turn"
            store = "file"
        "#;
        let cfg = parse(toml).expect("valid explicit durable");
        let agent = cfg.agents.get("a").expect("agent a");
        match agent.durable.as_ref().expect("durable present") {
            DurableEntry::Explicit { checkpoint, store } => {
                assert_eq!(checkpoint, "per_turn");
                assert_eq!(store, "file");
            }
            other => panic!("expected Explicit, got {other:?}"),
        }
    }
```

Update the existing `durable_accepts_per_tool_call` test (line ~4287) to match the enum: replace its `durable.checkpoint` field reads with a `DurableEntry::Explicit { checkpoint, .. }` match asserting `checkpoint == "per_tool_call"`.

- [ ] **Step 2: Run to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg durable --no-run`
Expected: FAIL to compile (`DurableEntry::Intent` undefined; `UncheckedDurable` is still a struct).

- [ ] **Step 3: Replace `UncheckedDurable`** (lines ~212-219) with the untagged enum:

```rust
/// `[agents.<id>.durable]` — either a bare intent string
/// (`durable = "survive-restarts"`) or the explicit `{ checkpoint, store }`
/// table (ADR-0053). Untagged: serde tries `Explicit` (a table) first, then
/// `Intent` (a string).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum UncheckedDurable {
    /// `[agents.<id>.durable] { checkpoint, store }`.
    Explicit(UncheckedDurableExplicit),
    /// `durable = "survive-restarts"`.
    Intent(String),
}

/// Explicit durable table. `deny_unknown_fields` so a typo'd key fails the
/// build rather than being silently dropped.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedDurableExplicit {
    /// Checkpoint granularity. A-minimal accepts `"per_turn"` or `"per_tool_call"`.
    pub checkpoint: String,
    /// Durable store. A-minimal accepts only `"file"`.
    pub store: String,
}
```

> Note: with `#[serde(untagged)]`, list `Explicit` before `Intent` so a table is never mis-parsed as a string. A bare string can only match `Intent`.

- [ ] **Step 4: Replace `DurableEntry`** (lines ~750-756) with the validated enum:

```rust
/// Validated `[agents.<id>.durable]` (ADR-0053 + EPIC 6.1). Present only
/// when the agent opts into durable execution.
#[derive(Debug, Clone)]
pub enum DurableEntry {
    /// Validated intent string (currently only `"survive-restarts"`).
    Intent(String),
    /// Validated explicit form. `checkpoint ∈ {per_turn, per_tool_call}`, `store == "file"`.
    Explicit {
        /// Validated checkpoint granularity.
        checkpoint: String,
        /// Validated durable store.
        store: String,
    },
}
```

- [ ] **Step 5: Replace the validation block** (lines ~1470-1496) so it produces the enum:

```rust
    let durable: Option<DurableEntry> = match raw.durable {
        None => None,
        Some(UncheckedDurable::Intent(s)) => {
            if s != "survive-restarts" {
                return Err(ProjectConfigError::AgentValidation {
                    id: id.clone(),
                    message: format!(
                        "durable {s:?} unsupported (accepts \"survive-restarts\" or an explicit {{ checkpoint, store }} table)"
                    ),
                });
            }
            Some(DurableEntry::Intent(s))
        }
        Some(UncheckedDurable::Explicit(d)) => {
            if d.checkpoint != "per_turn" && d.checkpoint != "per_tool_call" {
                return Err(ProjectConfigError::AgentValidation {
                    id: id.clone(),
                    message: format!(
                        "durable.checkpoint {:?} unsupported (accepts \"per_turn\" or \"per_tool_call\")",
                        d.checkpoint
                    ),
                });
            }
            if d.store != "file" {
                return Err(ProjectConfigError::AgentValidation {
                    id: id.clone(),
                    message: format!(
                        "durable.store {:?} unsupported (A-minimal accepts only \"file\")",
                        d.store
                    ),
                });
            }
            Some(DurableEntry::Explicit { checkpoint: d.checkpoint, store: d.store })
        }
    };
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg durable`
Expected: PASS (intent, unknown-intent, explicit, per_tool_call tests green).

- [ ] **Step 7: Commit**

```bash
git add crates/tau-pkg/src/project/project.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify \
  -m "feat(epic-6.1): tau-pkg accepts durable intent scalar + explicit table"
```

---

### Task 3: `tau-ir-lower` — `lower_durable` maps the validated enum to IR

**Files:**
- Modify: `crates/tau-ir-lower/src/lower/parse.rs:436-449` (`lower_durable`) and the test at `:720-754`

**Interfaces:**
- Consumes: Task 1 `tau_ir::durable::{Durability, DurabilityIntent, CheckpointGranularity, DurableStore}`; Task 2 `tau_pkg::project::project::DurableEntry`.
- Produces: `lower_durable(&AgentEntry) -> Option<tau_ir::durable::Durability>` (signature unchanged).

- [ ] **Step 1: Write the failing test** — add an intent test next to `lower_durable_maps_per_tool_call` (~line 720), and retarget the existing per_tool_call test's field reads.

```rust
    #[test]
    fn lower_durable_maps_intent() {
        let toml = r#"
packages = []
[project]
name = "p"
[models.m]
backend = "b"
model = "m"
[agents.a]
display_name = "A"
package = "p@^0.1"
model = "m"
durable = "survive-restarts"
"#;
        let project = tau_pkg::project::project::ProjectConfig::parse_str(toml).expect("parse");
        let agent_entry = project.agents.get("a").expect("agent a");
        let durable = super::lower_durable(agent_entry).expect("durable present");
        assert_eq!(
            durable,
            tau_ir::durable::Durability::Intent(tau_ir::durable::DurabilityIntent::SurviveRestarts)
        );
    }
```

For `lower_durable_maps_per_tool_call` (~line 747-753), replace the field reads with:

```rust
        let durable = super::lower_durable(agent_entry).expect("durable present");
        assert_eq!(
            durable,
            tau_ir::durable::Durability::Explicit {
                checkpoint: tau_ir::durable::CheckpointGranularity::PerToolCall,
                store: tau_ir::durable::DurableStore::File,
            }
        );
```

- [ ] **Step 2: Run to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ir-lower lower_durable --no-run`
Expected: FAIL to compile (`lower_durable` still reads `d.checkpoint` on a `DurableEntry` that is now an enum).

- [ ] **Step 3: Rewrite `lower_durable`** (lines ~436-449):

```rust
fn lower_durable(entry: &tau_pkg::project::AgentEntry) -> Option<tau_ir::durable::Durability> {
    use tau_ir::durable::{CheckpointGranularity, Durability, DurabilityIntent, DurableStore};
    use tau_pkg::project::project::DurableEntry;
    match entry.durable.as_ref()? {
        DurableEntry::Intent(s) => {
            // tau-pkg validated the string; the mapping is total.
            debug_assert_eq!(s, "survive-restarts");
            Some(Durability::Intent(DurabilityIntent::SurviveRestarts))
        }
        DurableEntry::Explicit { checkpoint, store } => {
            // tau-pkg validated both strings; wildcard arms are defence-in-depth.
            let checkpoint = match checkpoint.as_str() {
                "per_tool_call" => CheckpointGranularity::PerToolCall,
                _ => CheckpointGranularity::PerTurn,
            };
            let store = match store.as_str() {
                _ => DurableStore::File,
            };
            Some(Durability::Explicit { checkpoint, store })
        }
    }
}
```

> The `DurableEntry` path may need a `use` of the concrete module path — match the existing import style at the top of `parse.rs` (it already references `tau_pkg::project::AgentEntry`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-lower durable`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ir-lower/src/lower/parse.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify \
  -m "feat(epic-6.1): lower durable intent + explicit to IR enum"
```

---

### Task 4: `tau-runtime-core` — per-target resolver + DurableHandles granularity

**Files:**
- Create: `crates/tau-runtime-core/src/durable_resolve.rs`
- Modify: `crates/tau-runtime-core/src/lib.rs` (add `pub mod durable_resolve;` and re-export)
- Modify: `crates/tau-runtime-core/src/interpreter/tool_dispatch.rs:30-38` (`DurableHandles` gains `checkpoint`)
- Modify: `crates/tau-runtime-core/src/interpreter/agent_loop.rs:558` (read `handles.checkpoint`)

**Interfaces:**
- Consumes: Task 1 `tau_ir::durable::{Durability, DurabilityIntent, CheckpointGranularity, DurableStore}`; `tau_ports::target::TargetTriple`, `tau_ports::target::lookup`.
- Produces:
  - `struct ResolvedDurability { checkpoint: CheckpointGranularity, store: DurableStore, support: Support, from_intent: Option<DurabilityIntent> }`
  - `enum Support { Honored, Unsupported { reason: &'static str } }`
  - `fn resolve_durability(&Durability, &TargetTriple) -> ResolvedDurability`
  - `impl ResolvedDurability { fn require_supported(self) -> Result<Self, DurabilityUnsupported> }`
  - `struct DurabilityUnsupported { reason: &'static str, target: alloc::string::String }` (impls `core::fmt::Display`)
  - `DurableHandles.checkpoint: CheckpointGranularity` (new public field)

- [ ] **Step 1: Write the failing tests** — create `crates/tau-runtime-core/src/durable_resolve.rs` with the test module first (and an empty stub so the file is in the build):

```rust
//! Per-target durability resolution (EPIC 6.1).
//!
//! The IR carries a [`Durability`] *intent* (or an explicit form). The host
//! resolves it to a concrete `(checkpoint, store)` for a given
//! [`TargetTriple`] at run time (`ir_dispatcher`) and at build/check time
//! (`tau check --target`). Keeping resolution here — the one `no_std` crate
//! that sees both `tau_ir::durable` and `tau_ports::target` — means `tau
//! check` prints exactly what the runtime will do (the transparency bar).

use alloc::string::{String, ToString};
use tau_ir::durable::{CheckpointGranularity, Durability, DurabilityIntent, DurableStore};
use tau_ports::target::TargetTriple;

// (definitions added in Step 3)

#[cfg(test)]
mod tests {
    use super::*;

    fn honored_for(t: &TargetTriple) {
        let d = Durability::Intent(DurabilityIntent::SurviveRestarts);
        let r = resolve_durability(&d, t);
        assert_eq!(r.checkpoint, CheckpointGranularity::PerTurn);
        assert_eq!(r.store, DurableStore::File);
        assert!(matches!(r.support, Support::Honored), "target {t} should honor");
        assert_eq!(r.from_intent, Some(DurabilityIntent::SurviveRestarts));
    }

    #[test]
    fn every_registered_target_honors_survive_restarts() {
        for entry in tau_ports::target::list_all() {
            honored_for(&entry.triple);
        }
    }

    #[test]
    fn explicit_resolves_to_itself_on_a_registered_target() {
        let d = Durability::Explicit {
            checkpoint: CheckpointGranularity::PerToolCall,
            store: DurableStore::File,
        };
        let r = resolve_durability(&d, &TargetTriple::PASSTHROUGH);
        assert_eq!(r.checkpoint, CheckpointGranularity::PerToolCall);
        assert_eq!(r.store, DurableStore::File);
        assert!(matches!(r.support, Support::Honored));
        assert_eq!(r.from_intent, None);
    }

    #[test]
    fn unregistered_target_is_unsupported_and_require_errs() {
        use tau_ports::capability_gate::CapabilityTier;
        use tau_ports::target::adapter_family::AdapterFamily;
        use tau_ports::target::platform::Platform;
        // A triple not present in the registry (no shipping store).
        let off = TargetTriple {
            platform: Platform::Windows,
            adapter_family: AdapterFamily::Wasi,
            tier: CapabilityTier::None,
        };
        let d = Durability::Intent(DurabilityIntent::SurviveRestarts);
        let r = resolve_durability(&d, &off);
        assert!(matches!(r.support, Support::Unsupported { .. }));
        assert!(r.clone().require_supported().is_err());
    }
}
```

> Confirm the exact module paths for `Platform` / `AdapterFamily` / `CapabilityTier` against `crates/tau-ports/src/target/` and `capability_gate` when wiring imports — adjust the `use` lines if the re-export path differs.

- [ ] **Step 2: Run to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-runtime-core durable_resolve --no-run`
Expected: FAIL — `resolve_durability`, `Support`, `ResolvedDurability` undefined (and `agent_loop.rs:558` still reads `d.checkpoint`, which also fails to compile; fix in Step 4).

- [ ] **Step 3: Add the definitions** to `durable_resolve.rs` (above the test module):

```rust
/// Whether a target can honor a requested durability.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    /// The target provides the resolved granularity + store.
    Honored,
    /// The target cannot durably provide the requested store. `tau check
    /// --target` reports Error; the runtime refuses to start the run.
    Unsupported {
        /// Static, human-readable reason.
        reason: &'static str,
    },
}

/// Concrete durability resolved for a specific target (EPIC 6.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDurability {
    /// Resolved checkpoint granularity.
    pub checkpoint: CheckpointGranularity,
    /// Resolved durable store.
    pub store: DurableStore,
    /// Whether the target honors the request.
    pub support: Support,
    /// `Some(..)` when the author used an intent; `None` for an explicit form.
    pub from_intent: Option<DurabilityIntent>,
}

/// Error returned by [`ResolvedDurability::require_supported`].
#[derive(Debug, Clone)]
pub struct DurabilityUnsupported {
    /// Why the target cannot honor the request.
    pub reason: &'static str,
    /// The target that could not honor it (for the error message).
    pub target: String,
}

impl core::fmt::Display for DurabilityUnsupported {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "target `{}` cannot honor the requested durability: {}",
            self.target, self.reason
        )
    }
}

impl ResolvedDurability {
    /// Convert an `Unsupported` resolution into an error; pass `Honored`
    /// through. Used by the host to refuse a run it cannot make durable, and
    /// by `tau check` to fail the build.
    pub fn require_supported(self, target: &TargetTriple) -> Result<Self, DurabilityUnsupported> {
        match &self.support {
            Support::Honored => Ok(self),
            Support::Unsupported { reason } => Err(DurabilityUnsupported {
                reason,
                target: target.to_string(),
            }),
        }
    }
}

/// Resolve a [`Durability`] against a target.
///
/// - `Explicit { checkpoint, store }` resolves to itself; `support` checks the
///   target can provide `store`.
/// - `Intent(SurviveRestarts)` maps to the coarsest checkpoint + store the
///   target durably provides.
///
/// A-minimal policy: every triple in the target registry provides the `File`
/// store (host filesystem or host-mediated wasi preopen), so all registered
/// targets honor `survive-restarts` → `PerTurn + File`. Any triple absent from
/// the registry has no shipping store and is `Unsupported`. The policy
/// diverges the moment a `Kv` store or a no-persistence target lands.
pub fn resolve_durability(d: &Durability, target: &TargetTriple) -> ResolvedDurability {
    let provides_file = target_provides_file(target);
    match d {
        Durability::Intent(intent @ DurabilityIntent::SurviveRestarts) => ResolvedDurability {
            checkpoint: CheckpointGranularity::PerTurn,
            store: DurableStore::File,
            support: if provides_file {
                Support::Honored
            } else {
                Support::Unsupported {
                    reason: "target has no durable file store for survive-restarts",
                }
            },
            from_intent: Some(*intent),
        },
        Durability::Explicit { checkpoint, store } => ResolvedDurability {
            checkpoint: *checkpoint,
            store: *store,
            support: match store {
                DurableStore::File if provides_file => Support::Honored,
                DurableStore::File => Support::Unsupported {
                    reason: "target has no durable file store",
                },
            },
            from_intent: None,
        },
    }
}

/// A-minimal: a target provides the `File` store iff it is a registered triple.
fn target_provides_file(target: &TargetTriple) -> bool {
    tau_ports::target::lookup(target).is_some()
}
```

> `require_supported` takes `&TargetTriple` — update the test in Step 1 (`r.clone().require_supported()` → `r.clone().require_supported(&off)`).

Wire the module in `crates/tau-runtime-core/src/lib.rs`:

```rust
pub mod durable_resolve;
pub use durable_resolve::{resolve_durability, ResolvedDurability, Support, DurabilityUnsupported};
```

(Match the file's existing `pub mod` / `pub use` grouping and ordering.)

- [ ] **Step 4: Add `checkpoint` to `DurableHandles` and consume it.** In `crates/tau-runtime-core/src/interpreter/tool_dispatch.rs` (struct at line 30):

```rust
pub struct DurableHandles {
    /// Where turn checkpoints are written/read.
    pub store: Arc<dyn tau_ports::CheckpointStore>,
    /// Run id used to key checkpoints and as the `--resume` handle.
    pub run_id: String,
    /// When resuming, the latest checkpoint to rehydrate from; `None` for a
    /// fresh durable run.
    pub resume: Option<tau_ports::TurnCheckpoint>,
    /// Host-resolved checkpoint granularity (EPIC 6.1). The host resolves the
    /// agent's `Durability` for its target and passes the concrete value here,
    /// so the core never resolves intent itself.
    pub checkpoint: tau_ir::durable::CheckpointGranularity,
}
```

In `crates/tau-runtime-core/src/interpreter/agent_loop.rs`, replace line 558:

```rust
            run_options.durable_granularity = Some(handles.checkpoint);
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core durable`
Expected: PASS (resolver tests + existing `durable_*` stream tests). Also confirm the crate still builds `no_std`:
Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-runtime-core --no-default-features`
Expected: builds (no `std` pulled by the resolver).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-runtime-core/src/durable_resolve.rs crates/tau-runtime-core/src/lib.rs \
  crates/tau-runtime-core/src/interpreter/tool_dispatch.rs \
  crates/tau-runtime-core/src/interpreter/agent_loop.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify \
  -m "feat(epic-6.1): per-target durability resolver + DurableHandles granularity"
```

---

### Task 5: `tau-cli` — dispatcher resolves at run; bails on Unsupported

**Files:**
- Modify: `crates/tau-cli/src/cmd/ir_dispatcher.rs` — the durable block (~line 218), `with_durable` signature + the `durable_*` fields (~lines 360-390), `checkpointing()` (~line 616)

**Interfaces:**
- Consumes: Task 4 `tau_runtime_core::{resolve_durability}` + `tau_ports::target::TargetTriple::host()`; Task 4 `DurableHandles.checkpoint`.
- Produces: the dispatcher now resolves `entry_agent.durable` for `TargetTriple::host()`, refuses the run on `Unsupported`, and passes the resolved checkpoint into `DurableHandles`.

This task makes `tau-cli` compile again (Task 4 added the `checkpoint` field that the dispatcher's `DurableHandles { .. }` construction must now supply).

- [ ] **Step 1: Add a `durable_checkpoint` field + extend `with_durable`.** In the `ForwardingDispatcher` struct (near the `durable_store` / `durable_run_id` / `durable_resume` fields, ~line 360):

```rust
    /// Host-resolved checkpoint granularity (EPIC 6.1). Set via
    /// [`Self::with_durable`] alongside the store/run id.
    durable_checkpoint: Option<tau_ir::durable::CheckpointGranularity>,
```

Initialise it to `None` everywhere the other `durable_*` fields are initialised (the two constructors at ~lines 373 and 404).

Extend `with_durable` (~line 382) to take the granularity:

```rust
    pub(crate) fn with_durable(
        mut self,
        store: Arc<dyn tau_ports::CheckpointStore>,
        run_id: String,
        resume: Option<tau_ports::TurnCheckpoint>,
        checkpoint: tau_ir::durable::CheckpointGranularity,
    ) -> Self {
        self.durable_store = Some(store);
        self.durable_run_id = Some(run_id);
        self.durable_resume = resume;
        self.durable_checkpoint = Some(checkpoint);
        self
    }
```

Update `checkpointing()` (~line 616):

```rust
    fn checkpointing(&self) -> Option<DurableHandles> {
        Some(DurableHandles {
            store: self.durable_store.clone()?,
            run_id: self.durable_run_id.clone()?,
            resume: self.durable_resume.clone(),
            checkpoint: self.durable_checkpoint?,
        })
    }
```

- [ ] **Step 2: Resolve in the durable block.** Replace the head of the `if entry_agent.durable.is_some()` block (~line 218) so it resolves first and bails on Unsupported, then threads the resolved checkpoint into `with_durable` at the end of the block (~line 256):

```rust
    let mut dispatcher = ForwardingDispatcher::new(llm_backends, tools_by_id);
    if let Some(durability) = entry_agent.durable.as_ref() {
        // EPIC 6.1: resolve the agent's durability for THIS host's target.
        // The runtime refuses to start a run it cannot make durable — symmetric
        // with `tau check --target` failing at build time.
        let host_target = tau_ports::target::TargetTriple::host();
        let resolved = tau_runtime_core::resolve_durability(durability, &host_target)
            .require_supported(&host_target)
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        let store: Arc<dyn tau_ports::CheckpointStore> = Arc::new(
            tau_runtime_tokio::FileCheckpointStore::new(scope.path().to_path_buf()),
        );
        let (durable_run_id, resume) = match &args.resume {
            // ... unchanged run-id / resume logic ...
        };
        dispatcher = dispatcher.with_durable(store, durable_run_id, resume, resolved.checkpoint);
    }
    let dispatcher = Arc::new(dispatcher);
```

(Keep the existing run-id minting + `--resume` load logic verbatim between the `let store` and the final `with_durable` call.)

- [ ] **Step 3: Build to verify it compiles**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-cli`
Expected: builds (the `DurableHandles { checkpoint }` field is now supplied).

- [ ] **Step 4: Run the durable CLI tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli`
Expected: PASS (no durable regressions; `TargetTriple::host()` resolves Honored on the CI host).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-cli/src/cmd/ir_dispatcher.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify \
  -m "feat(epic-6.1): runtime resolves durability per host target; refuse if unsupported"
```

---

### Task 6: `tau-cli check` — `Severity::Note` + per-target durability resolution output

**Files:**
- Modify: `crates/tau-cli/src/cmd/check/result.rs` (`Severity::Note` variant + `compute_exit`)
- Modify: `crates/tau-cli/src/cmd/check/output/human.rs:59-61`, `json.rs:38-40` + `:80-82`, `sarif.rs:16-18` + `:32-34` (add `Note` arms)
- Modify: `crates/tau-cli/src/cmd/check/categories/sandbox.rs` (emit durability findings in the `--target` branch)
- Test: `crates/tau-cli/tests/` — a new integration test for `tau check --target` durability output (follow the pattern of an existing `check` integration test)

**Interfaces:**
- Consumes: Task 4 `tau_runtime_core::{resolve_durability, Support}`; `ctx.project.agents` (`tau_pkg` `AgentEntry` with `durable: Option<DurableEntry>`); `ctx.target: Option<TargetTriple>`.
- Note: the check path needs the *IR* `Durability` to resolve, but `ctx.project` carries the *tau-pkg* `DurableEntry`. Lower the agent's `DurableEntry` to `tau_ir::durable::Durability` for resolution. The cheapest, drift-free way is to reuse the same intent/explicit mapping: add a small `pub fn durable_entry_to_ir(&DurableEntry) -> tau_ir::durable::Durability` in `tau-ir-lower` (extracted from Task 3's `lower_durable` body) and call it from both `lower_durable` and here. (If `tau-cli` already depends on `tau-ir-lower`, reuse it; confirm with `grep tau-ir-lower crates/tau-cli/Cargo.toml`.)

- [ ] **Step 1: Write the failing test.** Add `crates/tau-cli/tests/cmd_check_durability_target.rs`. Model the harness on an existing check integration test (e.g. a test that builds a temp project + lockfile and runs `tau check`). The assertion:

```rust
// Build a temp project whose entry agent has `durable = "survive-restarts"`,
// run `tau check --target any-wasi-strict`, assert stdout contains the
// resolution line and the exit code is unaffected (0/2/3 per other findings,
// never raised by the Honored durability note).
assert!(stdout.contains("survive-restarts"));
assert!(stdout.contains("per_turn"));
assert!(stdout.contains("any-wasi-strict"));
```

> If standing up a full project+lockfile in a test is heavy, instead add a focused unit test inside `sandbox.rs` for a helper `durability_findings(agents, &target) -> Vec<CheckFinding>` (see Step 4) — assert it yields one `Severity::Note` finding with `rule_id == "tau.durability.resolved"` whose `summary` contains `survive-restarts` and `per_turn`, and that an `Explicit` agent yields a finding too. This is the lighter, deterministic option and is the recommended primary test; the end-to-end CLI test is optional if the harness exists.

- [ ] **Step 2: Run to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-cli durability --no-run`
Expected: FAIL — `Severity::Note` / `durability_findings` undefined.

- [ ] **Step 3: Add `Severity::Note`.** In `crates/tau-cli/src/cmd/check/result.rs` (the `Severity` enum, ~line 59):

```rust
pub enum Severity {
    /// A real bug the user can fix without running a setup command.
    /// Contributes to exit code 2.
    Error,
    /// A setup-needed condition (e.g., missing package).
    /// Contributes to exit code 3.
    NeedsSetup,
    /// Informational; does not affect exit code.
    Warning,
    /// Purely informational transparency (e.g. resolved durability).
    /// Does not affect exit code. Renders as SARIF `note`.
    Note,
}
```

Add the `Note` arm to `compute_exit` (~line 141): `Severity::Note => {}` (no effect).

In `output/human.rs` (~line 59-61): add `Severity::Note => {}` to the counter match (Notes are not counted as fixable/needs-setup; render them as plain informational lines in the body — match how the human renderer lists findings and give Notes a neutral prefix).

In `output/json.rs`: add `Severity::Note => {}` to the counter match (~line 38-40) and `Severity::Note => "note"` to the string mapping (~line 80-82).

In `output/sarif.rs`: add `Severity::Note => "note"` to both matches (~line 16-18 and ~32-34).

- [ ] **Step 4: Emit durability findings in the `--target` branch** of `categories/sandbox.rs`. Inside `if let Some(target) = &ctx.target {` (after the `let profile = entry.profile();` setup, before the per-plugin loop), iterate the project's agents:

```rust
        // EPIC 6.1: print the host-resolved durability per durable agent.
        if let Some(project) = &ctx.project {
            findings.extend(durability_findings(&project.agents, target));
        }
```

Add the helper at the bottom of `sandbox.rs`:

```rust
/// Build the per-agent durability resolution findings for `tau check --target`.
/// Honored → an informational `Note`; Unsupported → an `Error`.
fn durability_findings(
    agents: &std::collections::BTreeMap<String, tau_pkg::project::AgentEntry>,
    target: &tau_ports::target::TargetTriple,
) -> Vec<CheckFinding> {
    use tau_runtime_core::Support;
    let mut out = Vec::new();
    for (id, agent) in agents {
        let Some(entry) = agent.durable.as_ref() else { continue };
        let durability = tau_ir_lower::durable_entry_to_ir(entry);
        let resolved = tau_runtime_core::resolve_durability(&durability, target);
        let form = if resolved.from_intent.is_some() { "intent" } else { "explicit" };
        let ckpt = match resolved.checkpoint {
            tau_ir::durable::CheckpointGranularity::PerTurn => "per_turn",
            tau_ir::durable::CheckpointGranularity::PerToolCall => "per_tool_call",
            _ => "per_turn",
        };
        let store = match resolved.store {
            tau_ir::durable::DurableStore::File => "file",
            _ => "file",
        };
        let detail = match resolved.from_intent {
            Some(_) => format!("survive-restarts → {ckpt} checkpoints, {store} store"),
            None => format!("explicit {ckpt} + {store}"),
        };
        match resolved.support {
            Support::Honored => out.push(CheckFinding {
                category: CheckCategory::Sandbox,
                severity: Severity::Note,
                rule_id: "tau.durability.resolved",
                summary: format!("{id}: {detail}  [resolved for {target}]"),
                detail: None,
                location: None,
                remediation: None,
                structured: json!({
                    "kind": "DurabilityResolved",
                    "agent": id,
                    "form": form,
                    "checkpoint": ckpt,
                    "store": store,
                    "support": "honored",
                    "target": target.to_string(),
                }),
            }),
            Support::Unsupported { reason } => out.push(CheckFinding {
                category: CheckCategory::Sandbox,
                severity: Severity::Error,
                rule_id: "tau.durability.unsupported",
                summary: format!("{id}: target `{target}` cannot honor durability: {reason}"),
                detail: None,
                location: None,
                remediation: None,
                structured: json!({
                    "kind": "DurabilityUnsupported",
                    "agent": id,
                    "support": "unsupported",
                    "reason": reason,
                    "target": target.to_string(),
                }),
            }),
        }
    }
    out
}
```

> The `Support::Unsupported` arm cannot fire for a registered target today (the `--target` value is validated as registered in `check/mod.rs`). It exists so the first non-persistent target fails loudly. Keep both arms.

Add `pub fn durable_entry_to_ir(entry: &tau_pkg::project::project::DurableEntry) -> tau_ir::durable::Durability` to `tau-ir-lower` (extract from Task 3's `lower_durable` and have `lower_durable` call it). Confirm `tau-cli` depends on `tau-ir-lower` (it does for the build path); if not, add it to `crates/tau-cli/Cargo.toml`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli durability`
Expected: PASS. Also re-run any check-output snapshot tests (`help_snapshots`, sarif/json renderer tests) and update snapshots if the new `Note` arm changed any rendered output:
Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli check`
Expected: PASS (review any `insta` snapshot diffs; accept only durability-line additions).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-cli/src/cmd/check/ crates/tau-ir-lower/src/lower/parse.rs crates/tau-cli/tests/
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify \
  -m "feat(epic-6.1): tau check --target prints resolved durability (Severity::Note)"
```

---

### Task 7: `tau-ts-extract` — emit the intent scalar

**Files:**
- Modify: `crates/tau-ts-extract/src/lower.rs:364-375` (durable re-emission)
- Test: `crates/tau-ts-extract/src/lower.rs` inline tests, or the conformance test in Task 8

**Interfaces:**
- Consumes: nothing new (it re-emits TOML text). The captured `durable` JSON Value is either a string (`"survive-restarts"`) or an object (`{checkpoint, store}`).
- Produces: TS `durable: "survive-restarts"` re-emits as the top-level agent key `durable = "survive-restarts"`; the object form re-emits as the `[agents.<id>.durable]` sub-table (unchanged).

- [ ] **Step 1: Write the failing test** — add an inline unit test in `lower.rs` asserting the emitted TOML for a string-valued durable:

```rust
    #[test]
    fn durable_intent_string_emits_top_level_key() {
        // expr_to_json on a string literal yields Value::String.
        let durable = serde_json::Value::String("survive-restarts".into());
        // Use the same emission path the lowerer uses; assert the rendered
        // agent TOML contains `durable = "survive-restarts"` and NOT a
        // `[...durable]` sub-table header.
        let toml = render_agent_with_durable("a", &durable); // small test helper or call the public extract path
        assert!(toml.contains("durable = \"survive-restarts\""), "got: {toml}");
        assert!(!toml.contains("[agents.a.durable]"), "intent must not open a sub-table; got: {toml}");
    }
```

> If there is no convenient unit seam, cover this via the conformance fixture in Task 8 instead and skip this inline test — note that choice in the commit message.

- [ ] **Step 2: Run to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ts-extract durable --no-run`
Expected: FAIL (current code only handles the object form).

- [ ] **Step 3: Handle the string form** in `lower.rs` (~line 364). The scalar must be emitted as a top-level agent key (it is valid because nothing but the `[agents.<id>]` header precedes it):

```rust
        if let Some(durable) = &agent.durable {
            match durable {
                // Intent scalar: `durable = "survive-restarts"` — a top-level
                // agent key (must precede any sub-table for valid TOML).
                serde_json::Value::String(intent) => {
                    out.push_str(&format!("durable = {}\n", toml_str(intent)));
                }
                // Explicit object: re-emit the sub-table (unchanged).
                _ => {
                    out.push_str(&format!("[agents.{}.durable]\n", toml_key(name)));
                    if let Some(checkpoint) = durable.get("checkpoint").and_then(|v| v.as_str()) {
                        out.push_str(&format!("checkpoint = {}\n", toml_str(checkpoint)));
                    }
                    if let Some(store) = durable.get("store").and_then(|v| v.as_str()) {
                        out.push_str(&format!("store = {}\n", toml_str(store)));
                    }
                }
            }
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ts-extract durable`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ts-extract/src/lower.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify \
  -m "feat(epic-6.1): tau-ts-extract emits durable intent scalar"
```

---

### Task 8: Conformance fixture + TOML↔TS conformance + docs; full-workspace green

**Files:**
- Create: `crates/tau-ir-conformance/fixtures/18_durable_intent/{workflow.toml,mock_llm.jsonl,expected_report.json}`
- Create: `crates/tau-ts-extract/tests/fixtures/durable_intent_conformance/{tau.toml,project.ts}`
- Modify: `crates/tau-ts-extract/tests/durable_conformance.rs` (add an intent-form test, or a sibling file)
- Modify: `docs/reference/` durable page + `docs/decisions/0053-*` cross-reference note (and `docs/SUMMARY.md` if a new page is added)

**Interfaces:**
- Consumes: the full pipeline from Tasks 1-7.

- [ ] **Step 1: Create the conformance fixture `18_durable_intent`.** Copy `16_durable_per_turn`'s three files; in `workflow.toml` replace the `[agents.fan.durable]` sub-table with the intent scalar on the agent and rename the project:

`crates/tau-ir-conformance/fixtures/18_durable_intent/workflow.toml`:
```toml
packages = ["mock-llm"]

[project]
name = "fixture-18"

[models.mock-1]
backend = "mock-llm"
model = "mock-1"

[agents.fan]
display_name = "Fan Controller"
package      = "fan-ctrl@^0.1"
model        = "mock-1"
tool_refs    = ["read_temp"]
max_turns    = 2
# EPIC 6.1: intent form. Behaviourally identical to fixtures 01 / 16 — the
# durable block is additive IR metadata; under the conformance harness no
# store is wired so no checkpoints are written.
durable = "survive-restarts"

[tools.read_temp]
native      = "ReadTemp"
description = "Read the current temperature."
capabilities = []
```

Copy `mock_llm.jsonl` and `expected_report.json` from fixture 16 verbatim (same observable behavior).

- [ ] **Step 2: Run the conformance suite to verify fixture 18 conforms and 16/17 stay green**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-conformance`
Expected: PASS — `18_durable_intent` cross-mode conforms; `16_durable_per_turn` and `17_durable_per_tool_call` still pass (now under the `Explicit` tag).

- [ ] **Step 3: Add the TOML↔TS intent conformance fixture + test.** Create `crates/tau-ts-extract/tests/fixtures/durable_intent_conformance/tau.toml`:
```toml
packages = ["mock-llm"]
[project]
name = "durable-intent-fixture"
[models.mock-1]
backend = "mock-llm"
model = "mock-1"
[agents.fan]
display_name = "Fan"
package = "p@^0.1"
model = "mock-1"
durable = "survive-restarts"
```
And `project.ts` mirroring it (follow the existing `durable_conformance/project.ts` shape, with `durable: "survive-restarts"`). Add a test fn to `durable_conformance.rs` (mirroring the existing one) that lowers both and asserts byte-equal canonical IR, plus a sanity assert that `fan.durable == Some(Durability::Intent(DurabilityIntent::SurviveRestarts))`.

- [ ] **Step 4: Run the TS conformance test**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ts-extract durable`
Expected: PASS (intent + explicit both byte-equal across TOML/TS).

- [ ] **Step 5: Docs.** Update the durable reference page (find it: `grep -rl "durable" docs/reference docs/how-to`) to document the intent form, the per-target resolution, and `tau check --target`'s durability line. Add a one-line forward-reference in `docs/decisions/0053-turn-level-checkpoint-resume.md` pointing at EPIC 6.1 (intent knob). If a new page is added, register it in `docs/SUMMARY.md`. Build the book:

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build && cd .. && rm -rf docs/book`
Expected: only `[INFO]` lines; linkcheck clean.

- [ ] **Step 6: Full-workspace verification.** Confirm the whole branch builds + the durable surface is green end-to-end:

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build --workspace` (one-off full build; this is the documented exception to the `-p` rule — verifying branch integrity before PR)
Expected: builds clean.
Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir -p tau-pkg -p tau-ir-lower -p tau-runtime-core -p tau-cli -p tau-ts-extract -p tau-ir-conformance`
Expected: all PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/tau-ir-conformance/fixtures/18_durable_intent crates/tau-ts-extract/tests docs/
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify \
  -m "test(epic-6.1): conformance fixture 18 + TOML↔TS intent conformance + docs"
```

---

## Self-Review

**Spec coverage:**
- §1 Authoring surface → Task 2 (untagged enum + validation).
- §2 IR enum + version bump → Task 1.
- §3 Lowering → Task 3.
- §4 Resolver + crate placement (`tau-runtime-core`) → Task 4.
- §5 `tau check --target` output + `Severity::Note` → Task 6.
- §6 Runtime honest wiring → Tasks 4 (core seam) + 5 (host resolve/bail).
- §7 TS parity → Task 7 + Task 8 (conformance).
- Open choice (hard-fail Unsupported) → Task 4 (`require_supported`) + Task 5 (run bail) + Task 6 (check Error).
- Testing matrix (spec §Testing) → covered across Tasks 1-8; the `18_durable_intent` fixture + the TOML↔TS intent fixture are the conformance items.

**Type consistency:** `Durability::{Intent,Explicit}`, `DurabilityIntent::SurviveRestarts`, `ResolvedDurability{checkpoint,store,support,from_intent}`, `Support::{Honored,Unsupported{reason}}`, `DurableHandles.checkpoint`, `with_durable(..., checkpoint)`, `DurableEntry::{Intent,Explicit}`, `UncheckedDurable::{Explicit,Intent}`, `durable_entry_to_ir`, `resolve_durability`, `require_supported(&TargetTriple)` — used consistently across tasks.

**Known soft spots (flagged, not placeholders):**
- Task 6's end-to-end CLI test depends on an existing temp-project check harness; the unit-level `durability_findings` test is the guaranteed-deterministic primary coverage.
- Task 7's inline unit test depends on a render seam; if absent, Task 8's TS conformance fixture is the fallback coverage.
- `Support::Unsupported` is unreachable for registered targets today (by design — the future-proofing path); tested only via a constructed off-registry triple in Task 4.
