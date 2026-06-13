# Trigger Ingress — Slice 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Compile a declarative `[trigger]` table (cron + manual kinds, with a `[trigger.*.retry]` policy) into the workflow IR and the bundle as portable, capability-safe metadata, and emit `systemd`/`k8s` host-adapter descriptors from `tau build --emit-trigger`.

**Architecture:** Triggers lower as **metadata**, parallel to capabilities. A new `tau-ir::trigger` module owns the canonical IR type `TriggerBinding`; `IrModule` gains a `triggers` field that is a **sibling of `workflow`** (so trigger-less modules hash byte-identically to today). `tau-pkg` gains a validated config representation (`TriggerEntry`) and a bundle-manifest representation (`BundleTrigger`); the bundle's `schema_version` bumps to `3` **only when triggers are present**. `tau build --emit-trigger=<adapter>` reads the lowered bindings and writes scheduler descriptors next to the bundle. The egress-only capability vocabulary is **not touched** — cron/manual need no inbound primitive (the host invokes tau as an ordinary child process).

**Tech Stack:** Rust (workspace of 8 crates). `tau-ir` is `#![no_std]` + `alloc` (gate `with-std-adapters` for the `lower` module). `serde`/`serde_json` for canonical IR bytes; hand-rolled canonical TOML for the bundle. `humantime` (already a `tau-pkg` dep) for duration-string validation.

---

## Design decisions resolved (the spec's open questions)

The framing doc (`docs/superpowers/specs/2026-06-13-trigger-ingress-and-serve-transport-framing.md`) defers two questions to the implementation spec. This plan **resolves** them as follows; the rationale is reproduced in the ADR (Task 8).

### D1 — `ir_format` bump mechanics (spec open Q2)

**Decision: no `ir_format` bump — stay `v1.0.0` always.** `IrModule.triggers` carries `#[serde(default, skip_serializing_if = "Vec::is_empty")]`, so a trigger-less module emits **no** `triggers` key and its canonical bytes — and therefore its content hash — are byte-identical to today. A trigger-bearing module appends a `triggers` array (so its hash *does* differ from trigger-less, automatically). `ir_format` is left at `IrFormatVersion::current()` (`"v1.0.0"`) in **all** cases.

**Rationale (supersedes the framing doc's "minor bump" lean).** The framing doc leaned toward a minor bump, but that predates a key observation: **triggers are inert at runtime.** A trigger decides *when/whether* the host invokes tau — a decision made *before* tau's process starts. By the time `tau-runtime-core` decodes the IR, the trigger has already done its job, so an old runtime that silently ignores a `triggers` field still executes the workflow correctly. There is no reader-side gate on `ir_format` today, and we deliberately do **not** add one (it would reject a workflow that runs fine). The only gate with teeth lives at the **bundle** layer (`schema_version`, see D3) — read at build/inspect time, where rejecting an old-tau is correct. Since nothing keys off the IR language version, bumping it would be a label with no consumer. YAGNI: leave `ir_format` at `v1.0.0`; the `triggers`-key presence already differentiates the hash, and the bundle `schema_version` already provides loud rejection.

Forward-compat read: an older module with no `triggers` key deserializes to an empty `Vec` via `#[serde(default)]`; a trigger-bearing module decoded by an old runtime silently (and harmlessly) drops the inert field.

### D2 — DLQ envelope shape (spec open Q3)

**Decision: out of scope for slice 1 — record the sink reference only.** Slice 1 compiles the *retry policy* (`max_attempts`, `backoff`, `dead_letter`) as metadata. The dead-letter **envelope** (run id, trigger name, attempt count, last error, original input hash) is a *runtime artifact* produced when a trigger actually fires and exhausts its attempts — and nothing in tau fires triggers yet (retry is host-honoured; the interpreter stays deterministic and stateless across invocations). Slice 1 therefore records `dead_letter: Option<String>` (the sink reference) and **does not** define an envelope struct. The envelope shape lands with the host-adapter runtime work (post-slice-2).

### D3 — `--emit-trigger` scope (spec open Q5)

systemd + k8s ship in slice 1 (the framing doc's "obvious v1 pair"). Both handle **cron**; **manual** triggers emit nothing (the host invokes tau directly) and are logged as skipped. k8s `CronJob.schedule` takes 5-field cron **verbatim** (exact). systemd `OnCalendar` requires translation: slice 1 implements a converter for the subset where each of the 5 fields is `*` or a plain non-negative integer (covers `0 3 * * *`). Schedules using ranges/lists/steps are **skipped with a logged warning** for systemd (k8s still emits them exactly); the bundle build still succeeds.

### D4 — bundle `schema_version` bump

**Decision: conditional bump to `3`.** A bundle with no triggers stays `schema_version = 2` and serialises byte-identically to today (no `[[trigger]]` tables). A bundle carrying ≥1 trigger is `schema_version = 3`. `BundleManifest::parse_str` accepts `{1, 2, 3}`. Mirrors D1's conditional-bump reasoning: old `tau` reading a trigger-bearing bundle must reject it loudly rather than silently drop the binding, but a trigger-less bundle stays maximally compatible.

### Non-negotiable constraint

**No inbound verb is added to `crates/tau-domain/src/package/capability.rs`.** cron and manual are egress-shaped (host invokes tau as a child process). The egress-only vocabulary is load-bearing for the NG3 argument. This plan touches `tau-domain` **only not at all**.

---

## File structure

| File | Responsibility | New/Modified |
|---|---|---|
| `crates/tau-ir/src/trigger.rs` | Canonical IR trigger types (`TriggerBinding`, `TriggerKind`, `RetryPolicy`, `Backoff`, `BackoffStrategy`) + `systemd`/`k8s` descriptor emitters | **New** |
| `crates/tau-ir/src/lib.rs` | `pub mod trigger;` + re-exports | Modified |
| `crates/tau-ir/src/module.rs` | `IrModule.triggers` sibling field (skip-empty; `ir_format` unchanged) | Modified |
| `crates/tau-ir/src/lower/parse.rs` | Read `config.triggers` → `Parsed.triggers` | Modified |
| `crates/tau-ir/src/lower/typecheck.rs` | Validate each `trigger.agent` exists in `agents` | Modified |
| `crates/tau-ir/src/lower/mod.rs` | `Parsed.triggers`; `build_module` moves triggers + picks `ir_format` | Modified |
| `crates/tau-ir/src/error.rs` | `IrError::UnknownTriggerAgent` variant | Modified |
| `crates/tau-pkg/src/project/project.rs` | `UncheckedTrigger`/`UncheckedRetry` + validated `TriggerEntry`/`RetryEntry` + `validate_trigger` + `ProjectConfig.triggers` + error variants | Modified |
| `crates/tau-pkg/src/bundle/manifest.rs` | `BundleTrigger`/`BundleRetry` + `BundleManifest.triggers`; accept `schema_version` 3 | Modified |
| `crates/tau-pkg/src/bundle/canonical.rs` | Emit `[[trigger]]` tables | Modified |
| `crates/tau-pkg/src/bundle/build.rs` | Populate `triggers` from `project_config.triggers`; conditional `schema_version` | Modified |
| `crates/tau-cli/src/cli.rs` | `BuildArgs.emit_trigger` | Modified |
| `crates/tau-cli/src/cmd/build.rs` | Surface triggers from `lower_ir`; write descriptors after build | Modified |
| `crates/tau-cli/tests/cmd_build_trigger.rs` | Integration test for `--emit-trigger=systemd` | **New** |
| `docs/decisions/0043-trigger-ingress-slice-1.md` | ADR | **New** |
| `docs/explanation/trigger-ingress.md` + `docs/SUMMARY.md` | mdBook page + index entry | **New/Modified** |

> ADR number: **0043** — confirmed during execution (`0042-cross-repo-ci-template-sync.md` already exists, so 0042 is taken). The committed doc-comments from Tasks 1–2 reference `ADR-0042` and must be corrected to `ADR-0043` in Task 8 (Step 1b).

---

## Cargo command shape (CLAUDE.md)

Every cargo invocation in this plan uses:

```
timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <cmd> -p <crate>
```

Prefer `cargo nextest run -p <crate>` for tests; `cargo test --doc -p <crate>` for doctests. Timeouts: test 300s, build/check 180s, clippy 240s.

---

### Task 1: IR trigger types (`tau-ir::trigger`)

**Files:**
- Create: `crates/tau-ir/src/trigger.rs`
- Modify: `crates/tau-ir/src/lib.rs`

- [ ] **Step 1: Write `trigger.rs` with the type definitions**

Create `crates/tau-ir/src/trigger.rs`:

```rust
//! Trigger bindings — compiled, capability-safe metadata describing how
//! tau is invoked. See the framing doc
//! `docs/superpowers/specs/2026-06-13-trigger-ingress-and-serve-transport-framing.md`
//! and ADR-0043.
//!
//! A trigger has two halves: the **substrate** (the scheduler/socket/queue,
//! owned by the host) and the **binding** (declared once, compiled, portable —
//! owned by tau). This module is the binding. It carries no inbound capability
//! and adds no executable node; it is pure metadata that rides in the canonical
//! IR (and thus participates in the content hash).
//!
//! Slice 1 ships `Cron` + `Manual`. `Webhook`/`Queue` are slice 2 (they
//! additionally require a host-adapter contract, which `tau check` will
//! enforce). The enums are `#[non_exhaustive]` so adding those kinds later is
//! a minor change.

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::ids::AgentId;

/// The kind of external event a trigger binds to.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerKind {
    /// Fires on a cron schedule. Substrate = systemd/k8s/Lambda scheduler.
    Cron,
    /// The default: tau is invoked by an external driver (a parent process,
    /// CI step, etc.). No scheduler descriptor is emitted.
    Manual,
}

/// Backoff strategy for trigger-level re-invocation (host-honoured).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackoffStrategy {
    /// Constant delay between attempts.
    Fixed,
    /// Exponentially increasing delay, capped at `Backoff::max`.
    Exponential,
}

/// Backoff parameters. Durations are stored as the author's verbatim
/// duration strings (e.g. `"30s"`, `"10m"`) — they are host-honoured
/// metadata, not values the (no_std) IR interpreter ever computes with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backoff {
    /// `fixed` or `exponential`.
    pub strategy: BackoffStrategy,
    /// Base delay, duration string (e.g. `"30s"`).
    pub base: String,
    /// Cap on the computed delay, duration string (e.g. `"10m"`).
    pub max: String,
}

/// Trigger-level re-invocation policy. This is **not** a per-node interpreter
/// retry: the host (or host adapter) re-invokes the artifact; tau's
/// interpreter stays deterministic and stateless across invocations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Total attempts including the first; `1` = no retry.
    pub max_attempts: u32,
    /// Backoff parameters.
    pub backoff: Backoff,
    /// Where a run that exhausts `max_attempts` is sent — a **sink
    /// reference** (`mcp:<name>` or an already-granted capability target),
    /// never a tau-owned queue. `None` ⇒ no dead-letter sink. The envelope
    /// shape is a runtime concern not modelled in slice 1 (see ADR-0043 §D2).
    pub dead_letter: Option<String>,
}

/// One named trigger binding. Canonically ordered by `name` within
/// `IrModule.triggers`. A trigger is metadata about how tau is invoked,
/// never an executable node.
///
/// Optional fields serialize verbatim (`None` → `null`, no skipping) to
/// match the IR's canonical-encoding discipline (see `canonical.rs`). Only
/// the module-level `triggers` `Vec` skips-when-empty, to preserve
/// trigger-less hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerBinding {
    /// Trigger name (the `[trigger.<name>]` table key).
    pub name: String,
    /// The kind of event this binds to.
    pub kind: TriggerKind,
    /// Entrypoint agent id (validated at lowering against the workflow).
    pub agent: AgentId,
    /// 5-field cron expression (cron kind only; `None` otherwise).
    pub schedule: Option<String>,
    /// IANA timezone name; defaults to `"UTC"` at config-validation time
    /// (cron kind only).
    pub timezone: Option<String>,
    /// Re-invocation policy (`None` = invoke once, no retry).
    pub retry: Option<RetryPolicy>,
}
```

- [ ] **Step 2: Add the emitter stubs at the bottom of `trigger.rs`** (filled in Task 6 — declared here so the module compiles as one unit)

Skip — emitters land in Task 6. For Task 1, the file above is complete and compiles on its own.

- [ ] **Step 3: Wire the module into `lib.rs`**

In `crates/tau-ir/src/lib.rs`, add `pub mod trigger;` in the module list (after `pub mod tool_impl;` keeps alphabetical-ish grouping; it must sit with the other `pub mod` lines) and add to the re-export block:

```rust
pub mod trigger;
```

and in the re-exports section:

```rust
pub use trigger::{Backoff, BackoffStrategy, RetryPolicy, TriggerBinding, TriggerKind};
```

- [ ] **Step 4: Add a serde round-trip unit test inside `trigger.rs`**

Append to `crates/tau-ir/src/trigger.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::AgentId;
    use alloc::string::ToString;

    fn cron_binding() -> TriggerBinding {
        TriggerBinding {
            name: "nightly".to_string(),
            kind: TriggerKind::Cron,
            agent: AgentId("summarizer".to_string()),
            schedule: Some("0 3 * * *".to_string()),
            timezone: Some("UTC".to_string()),
            retry: Some(RetryPolicy {
                max_attempts: 3,
                backoff: Backoff {
                    strategy: BackoffStrategy::Exponential,
                    base: "30s".to_string(),
                    max: "10m".to_string(),
                },
                dead_letter: Some("dlq-sink".to_string()),
            }),
        }
    }

    #[test]
    fn trigger_binding_round_trips_through_json() {
        let b = cron_binding();
        let bytes = serde_json::to_vec(&b).expect("serialize");
        let back: TriggerBinding = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(b, back);
    }

    #[test]
    fn kind_serializes_lowercase() {
        let bytes = serde_json::to_vec(&TriggerKind::Cron).unwrap();
        assert_eq!(bytes, b"\"cron\"");
        let bytes = serde_json::to_vec(&TriggerKind::Manual).unwrap();
        assert_eq!(bytes, b"\"manual\"");
    }

    #[test]
    fn manual_binding_has_no_schedule() {
        let b = TriggerBinding {
            name: "manual".to_string(),
            kind: TriggerKind::Manual,
            agent: AgentId("summarizer".to_string()),
            schedule: None,
            timezone: None,
            retry: None,
        };
        let bytes = serde_json::to_vec(&b).unwrap();
        let back: TriggerBinding = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(b, back);
    }
}
```

- [ ] **Step 5: Run the tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir trigger`
Expected: 3 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-ir/src/trigger.rs crates/tau-ir/src/lib.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(ir): add trigger binding types (slice 1)"
```

---

### Task 2: `IrModule.triggers` sibling (skip-empty) + hash preservation

**Files:**
- Modify: `crates/tau-ir/src/module.rs`
- Test: `crates/tau-ir/tests/canonical_idempotence.rs`, `crates/tau-ir/tests/canonical_cosmetics_insensitive.rs` (struct-literal updates), new `crates/tau-ir/tests/trigger_hash_preservation.rs`

- [ ] **Step 1: Write the failing hash-preservation test**

Create `crates/tau-ir/tests/trigger_hash_preservation.rs`:

```rust
//! The load-bearing invariant for slice 1: adding `triggers` to `IrModule`
//! must NOT change the canonical bytes (and thus the content hash) of any
//! trigger-less module. A trigger-bearing module must hash differently and
//! must NOT bump `ir_format` (Option B): it stays v1.0.0.

use tau_ir::trigger::{
    Backoff, BackoffStrategy, RetryPolicy, TriggerBinding, TriggerKind,
};
use tau_ir::{compute_hash, to_canonical_bytes, AgentId, IrFormatVersion, IrModule, Workflow};
use tau_ports::target::registry;

fn target() -> tau_ports::target::TargetTriple {
    registry::list_available().next().unwrap().triple
}

fn trigger_less() -> IrModule {
    IrModule {
        ir_format: IrFormatVersion::current(),
        tau_version: "0.0.0".into(),
        target: target(),
        workflow: Workflow::default(),
        triggers: Vec::new(),
    }
}

#[test]
fn trigger_less_module_emits_no_triggers_key() {
    let bytes = to_canonical_bytes(&trigger_less());
    let json = core::str::from_utf8(&bytes).unwrap();
    assert!(
        !json.contains("triggers"),
        "trigger-less module must not emit a `triggers` key: {json}"
    );
}

#[test]
fn trigger_less_hash_is_stable_known_value() {
    // Pin the trigger-less hash so a future skip-serializing regression
    // (which would silently re-hash every trigger-less module) is caught.
    // Compute once via `to_canonical_bytes` + `compute_hash` and assert the
    // two agree; the value itself is derived, not magic.
    let m = trigger_less();
    let h1 = compute_hash(&m);
    // Re-decode/re-encode round-trip must preserve the hash.
    let bytes = to_canonical_bytes(&m);
    let m2 = tau_ir::from_canonical_bytes(&bytes).unwrap();
    let h2 = compute_hash(&m2);
    assert_eq!(h1, h2, "round-trip changed the hash");
}

#[test]
fn trigger_bearing_module_changes_hash_but_keeps_ir_format() {
    let mut m = trigger_less();
    let baseline = compute_hash(&m);
    // Option B: ir_format is NOT bumped — it stays v1.0.0. The appended
    // `triggers` array is what differentiates the hash.
    m.triggers = vec![TriggerBinding {
        name: "nightly".into(),
        kind: TriggerKind::Cron,
        agent: AgentId("summarizer".into()),
        schedule: Some("0 3 * * *".into()),
        timezone: Some("UTC".into()),
        retry: Some(RetryPolicy {
            max_attempts: 3,
            backoff: Backoff {
                strategy: BackoffStrategy::Exponential,
                base: "30s".into(),
                max: "10m".into(),
            },
            dead_letter: Some("dlq-sink".into()),
        }),
    }];
    let with_trigger = compute_hash(&m);
    assert_ne!(baseline, with_trigger, "triggers must change the hash");
    assert_eq!(
        m.ir_format.0,
        IrFormatVersion::CURRENT,
        "Option B: ir_format must NOT bump for a trigger-bearing module"
    );

    // Round-trips.
    let bytes = to_canonical_bytes(&m);
    let back = tau_ir::from_canonical_bytes(&bytes).unwrap();
    assert_eq!(m, back);
}
```

- [ ] **Step 2: Run it to confirm it fails to compile** (no `triggers` field yet)

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-ir --tests`
Expected: FAIL — `IrModule has no field named triggers`.

- [ ] **Step 3: Add the field in `module.rs`**

In `crates/tau-ir/src/module.rs`, add the import at the top:

```rust
use crate::trigger::TriggerBinding;
```

> Option B: **no** `ir_format` constant is added — `IrFormatVersion::CURRENT` (`"v1.0.0"`) stays the only emitted version (ADR-0043 §D1).

Add the field to `IrModule` (as the **last** field, after `workflow`, so the serialized field order leaves `ir_format`/`tau_version`/`target`/`workflow` untouched):

```rust
    /// The workflow itself.
    pub workflow: Workflow,
    /// Trigger bindings — invocation metadata, a SIBLING of `workflow`
    /// (triggers are about *how* tau is invoked, not the call graph).
    /// `skip_serializing_if` + `default` means a trigger-less module emits
    /// no `triggers` key and hashes identically to a pre-trigger module
    /// (ADR-0043 §D1); older modules with no key read back as empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<TriggerBinding>,
```

- [ ] **Step 4: Fix the existing struct-literal constructions of `IrModule`**

`IrModule` is not `#[non_exhaustive]`, so every literal needs the new field. Update:

In `crates/tau-ir/tests/canonical_idempotence.rs`, `sample_module()` — add `triggers: Vec::new(),` after `workflow: Workflow::default(),`.

In `crates/tau-ir/tests/canonical_cosmetics_insensitive.rs` — find each `IrModule { ... }` literal and add `triggers: Vec::new(),`. (Grep first: `grep -n "IrModule {" crates/tau-ir/tests/canonical_cosmetics_insensitive.rs`.)

In `crates/tau-ir/src/lower/mod.rs`, `build_module` — handled in Task 4 (it moves `parsed.triggers` in). For now, to keep Task 2 self-contained and the crate compiling, temporarily add `triggers: Vec::new(),` to the `build_module` literal; Task 4 replaces it.

Run a grep to catch any other literal: `grep -rn "IrModule {" crates/ | grep -v target`.

- [ ] **Step 5: Run the build + the new test**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir`
Expected: all tests pass, including the 3 in `trigger_hash_preservation`.

- [ ] **Step 6: Run the doctest for `lower_project`** (it asserts `module.ir_format.0 == CURRENT` — a trigger-less project, must stay v1.0.0)

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-ir`
Expected: pass (trigger-less lowering still yields `v1.0.0`).

- [ ] **Step 7: Commit**

```bash
git add crates/tau-ir/src/module.rs crates/tau-ir/tests/
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(ir): add IrModule.triggers sibling with hash-preserving skip-empty (slice 1)"
```

---

### Task 3: `tau-pkg` config parse + validation (`[trigger.*]`)

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs`

- [ ] **Step 1: Write the failing parse/validation tests**

Add to the `tests` module in `crates/tau-pkg/src/project/project.rs`:

```rust
    #[test]
    fn parse_cron_trigger_with_retry() {
        let toml_str = r#"
            [project]
            name = "x"

            [agents.summarizer]
            display_name = "Summarizer"
            package      = "p@^0.1"
            llm_backend  = "anthropic"

            [trigger.nightly]
            kind     = "cron"
            agent    = "summarizer"
            schedule = "0 3 * * *"

            [trigger.nightly.retry]
            max_attempts = 3
            backoff      = { strategy = "exponential", base = "30s", max = "10m" }
            dead_letter  = "dlq-sink"
        "#;
        let cfg = parse(toml_str).unwrap();
        let t = cfg.triggers.get("nightly").expect("trigger present");
        assert_eq!(t.kind, "cron");
        assert_eq!(t.agent, "summarizer");
        assert_eq!(t.schedule.as_deref(), Some("0 3 * * *"));
        assert_eq!(t.timezone, "UTC"); // defaulted
        let r = t.retry.as_ref().expect("retry present");
        assert_eq!(r.max_attempts, 3);
        assert_eq!(r.backoff_strategy, "exponential");
        assert_eq!(r.dead_letter.as_deref(), Some("dlq-sink"));
    }

    #[test]
    fn parse_manual_trigger() {
        let toml_str = r#"
            [project]
            name = "x"

            [agents.summarizer]
            display_name = "S"
            package      = "p@^0.1"
            llm_backend  = "anthropic"

            [trigger.manual]
            kind  = "manual"
            agent = "summarizer"
        "#;
        let cfg = parse(toml_str).unwrap();
        let t = cfg.triggers.get("manual").unwrap();
        assert_eq!(t.kind, "manual");
        assert!(t.schedule.is_none());
        assert!(t.retry.is_none());
    }

    #[test]
    fn validate_rejects_cron_without_schedule() {
        let toml_str = r#"
            [project]
            name = "x"
            [agents.a]
            display_name = "A"
            package = "p@^0.1"
            llm_backend = "anthropic"
            [trigger.t]
            kind = "cron"
            agent = "a"
        "#;
        let Err(ProjectConfigError::TriggerValidation { name, message }) = parse(toml_str) else {
            panic!("expected TriggerValidation");
        };
        assert_eq!(name, "t");
        assert!(message.contains("schedule"), "got: {message}");
    }

    #[test]
    fn validate_rejects_manual_with_schedule() {
        let toml_str = r#"
            [project]
            name = "x"
            [agents.a]
            display_name = "A"
            package = "p@^0.1"
            llm_backend = "anthropic"
            [trigger.t]
            kind = "manual"
            agent = "a"
            schedule = "0 3 * * *"
        "#;
        let Err(ProjectConfigError::TriggerValidation { message, .. }) = parse(toml_str) else {
            panic!("expected TriggerValidation");
        };
        assert!(message.contains("manual"), "got: {message}");
    }

    #[test]
    fn validate_rejects_unsupported_kind() {
        let toml_str = r#"
            [project]
            name = "x"
            [agents.a]
            display_name = "A"
            package = "p@^0.1"
            llm_backend = "anthropic"
            [trigger.t]
            kind = "webhook"
            agent = "a"
        "#;
        let Err(ProjectConfigError::TriggerValidation { message, .. }) = parse(toml_str) else {
            panic!("expected TriggerValidation");
        };
        assert!(message.contains("webhook") || message.contains("not supported"), "got: {message}");
    }

    #[test]
    fn validate_rejects_bad_cron_field_count() {
        let toml_str = r#"
            [project]
            name = "x"
            [agents.a]
            display_name = "A"
            package = "p@^0.1"
            llm_backend = "anthropic"
            [trigger.t]
            kind = "cron"
            agent = "a"
            schedule = "0 3 * *"
        "#;
        let Err(ProjectConfigError::TriggerValidation { message, .. }) = parse(toml_str) else {
            panic!("expected TriggerValidation");
        };
        assert!(message.contains("5"), "got: {message}");
    }

    #[test]
    fn validate_rejects_bad_backoff_duration() {
        let toml_str = r#"
            [project]
            name = "x"
            [agents.a]
            display_name = "A"
            package = "p@^0.1"
            llm_backend = "anthropic"
            [trigger.t]
            kind = "cron"
            agent = "a"
            schedule = "0 3 * * *"
            [trigger.t.retry]
            max_attempts = 2
            backoff = { strategy = "fixed", base = "not-a-duration", max = "10m" }
        "#;
        let Err(ProjectConfigError::TriggerValidation { message, .. }) = parse(toml_str) else {
            panic!("expected TriggerValidation");
        };
        assert!(message.contains("duration") || message.contains("base"), "got: {message}");
    }

    #[test]
    fn no_trigger_table_keeps_triggers_empty() {
        let cfg = parse("[project]\nname = \"x\"\n").unwrap();
        assert!(cfg.triggers.is_empty());
    }
```

- [ ] **Step 2: Run to verify they fail to compile**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-pkg --tests`
Expected: FAIL — no `triggers` field, no `TriggerValidation` variant.

- [ ] **Step 3: Add the unchecked structs**

In `crates/tau-pkg/src/project/project.rs`, add the `triggers` field to `UncheckedProjectConfig` (after `steps`):

```rust
    /// Map of trigger name → unchecked trigger definition (slice 1).
    #[serde(default)]
    pub triggers: BTreeMap<String, UncheckedTrigger>,
```

Add the unchecked types (place them near the other `Unchecked*` IR-lowering structs):

```rust
/// Unchecked `[trigger.<name>]` table (slice 1).
///
/// `#[serde(deny_unknown_fields)]` catches typos. Slice-2 fields (`path`,
/// `methods`, `source`) are intentionally absent — a webhook/queue trigger
/// declared today fails fast (either on the unknown field or on the
/// unsupported-kind check in `validate_trigger`).
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedTrigger {
    /// `cron` | `manual` (slice 1).
    pub kind: String,
    /// Entrypoint agent id.
    pub agent: String,
    /// 5-field cron expression (cron only).
    #[serde(default)]
    pub schedule: Option<String>,
    /// IANA timezone name (cron only; defaults to `UTC`).
    #[serde(default)]
    pub timezone: Option<String>,
    /// Re-invocation policy.
    #[serde(default)]
    pub retry: Option<UncheckedRetry>,
}

/// Unchecked `[trigger.<name>.retry]` sub-table.
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedRetry {
    /// Total attempts including the first.
    pub max_attempts: u32,
    /// Backoff parameters.
    pub backoff: UncheckedBackoff,
    /// Sink reference for exhausted runs.
    #[serde(default)]
    pub dead_letter: Option<String>,
}

/// Unchecked `backoff` inline table.
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedBackoff {
    /// `fixed` | `exponential`.
    pub strategy: String,
    /// Base delay, duration string.
    pub base: String,
    /// Max delay, duration string.
    pub max: String,
}
```

- [ ] **Step 4: Add the validated structs**

Add near `StepEntry`:

```rust
/// Validated `[trigger.<name>]` entry produced by `validate()`.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct TriggerEntry {
    /// Trigger name (the TOML table key).
    pub name: String,
    /// `cron` | `manual`.
    pub kind: String,
    /// Entrypoint agent id (existence checked at IR lowering, not here).
    pub agent: String,
    /// 5-field cron expression (cron only).
    pub schedule: Option<String>,
    /// IANA timezone (defaults to `UTC` for cron; `None` for manual).
    pub timezone: String,
    /// Re-invocation policy.
    pub retry: Option<RetryEntry>,
}

/// Validated `[trigger.<name>.retry]` entry.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct RetryEntry {
    /// Total attempts including the first.
    pub max_attempts: u32,
    /// `fixed` | `exponential`.
    pub backoff_strategy: String,
    /// Base delay, duration string.
    pub backoff_base: String,
    /// Max delay, duration string.
    pub backoff_max: String,
    /// Sink reference for exhausted runs.
    pub dead_letter: Option<String>,
}
```

Add the `triggers` field to `ProjectConfig` (after `steps`):

```rust
    /// Map of trigger name → validated trigger entry (slice 1).
    pub triggers: BTreeMap<String, TriggerEntry>,
```

- [ ] **Step 5: Add the error variant**

Add to `ProjectConfigError`:

```rust
    /// A `[trigger.<name>]` entry failed validation.
    #[error("trigger {name:?}: {message}")]
    TriggerValidation {
        /// Trigger name that failed.
        name: String,
        /// Human-readable reason.
        message: String,
    },
```

- [ ] **Step 6: Wire validation into `validate()` + add `validate_trigger`**

In `UncheckedProjectConfig::validate`, after the `steps` loop and before `Ok(ProjectConfig { ... })`:

```rust
        let mut triggers = BTreeMap::new();
        for (name, raw) in self.triggers {
            triggers.insert(name.clone(), validate_trigger(name, raw)?);
        }
```

and add `triggers,` to the returned `ProjectConfig { ... }`.

Add the function (near `validate_step`):

```rust
fn validate_trigger(name: String, raw: UncheckedTrigger) -> Result<TriggerEntry, ProjectConfigError> {
    let err = |message: String| ProjectConfigError::TriggerValidation {
        name: name.clone(),
        message,
    };

    if raw.agent.trim().is_empty() {
        return Err(err("agent must be non-empty".into()));
    }

    // Slice 1 supports cron + manual only.
    match raw.kind.as_str() {
        "cron" => {}
        "manual" => {
            if raw.schedule.is_some() {
                return Err(err("manual triggers take no schedule".into()));
            }
            if raw.timezone.is_some() {
                return Err(err("manual triggers take no timezone".into()));
            }
        }
        "webhook" | "queue" => {
            return Err(err(format!(
                "kind {:?} is not supported yet (slice 1 supports cron and manual); \
                 webhook/queue arrive in slice 2",
                raw.kind
            )));
        }
        other => {
            return Err(err(format!(
                "unknown kind {other:?}; expected cron or manual"
            )));
        }
    }

    // cron-specific validation.
    let (schedule, timezone) = if raw.kind == "cron" {
        let sched = raw
            .schedule
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| err("cron triggers require a non-empty schedule".to_string()))?;
        let field_count = sched.split_whitespace().count();
        if field_count != 5 {
            return Err(err(format!(
                "cron schedule must have 5 whitespace-separated fields, found {field_count}"
            )));
        }
        let tz = raw.timezone.unwrap_or_else(|| "UTC".to_string());
        (Some(sched.to_string()), tz)
    } else {
        (None, String::new())
    };

    // retry validation.
    let retry = match raw.retry {
        None => None,
        Some(r) => {
            if r.max_attempts < 1 {
                return Err(err("retry.max_attempts must be >= 1".into()));
            }
            match r.backoff.strategy.as_str() {
                "fixed" | "exponential" => {}
                other => {
                    return Err(err(format!(
                        "retry.backoff.strategy {other:?} must be fixed or exponential"
                    )));
                }
            }
            // Durations are host-honoured; validate they parse so a typo
            // is caught at build time (Rust-class build-time enforcement).
            humantime::parse_duration(&r.backoff.base)
                .map_err(|e| err(format!("retry.backoff.base is not a valid duration: {e}")))?;
            humantime::parse_duration(&r.backoff.max)
                .map_err(|e| err(format!("retry.backoff.max is not a valid duration: {e}")))?;
            Some(RetryEntry {
                max_attempts: r.max_attempts,
                backoff_strategy: r.backoff.strategy,
                backoff_base: r.backoff.base,
                backoff_max: r.backoff.max,
                dead_letter: r.dead_letter,
            })
        }
    };

    Ok(TriggerEntry {
        name,
        kind: raw.kind,
        agent: raw.agent,
        schedule,
        timezone,
        retry,
    })
}
```

- [ ] **Step 7: Fix the `UncheckedProjectConfig` struct-literal in the proptest**

`UncheckedProjectConfig` is not `#[non_exhaustive]`. In the `proptests` module, the literal `UncheckedProjectConfig { project, agents, tools, steps }` must add `triggers: BTreeMap::new(),`.

Grep for other literals: `grep -rn "UncheckedProjectConfig {" crates/ | grep -v target` — fix each (there may be one in `cmd/build.rs`'s `resolve_mcp_cache` path; it deserializes via `toml::from_str`, not a literal, so likely none outside this file, but verify).

- [ ] **Step 8: Run the tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg`
Expected: all pass, including the 8 new trigger tests.

Run doctests: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-pkg`
Expected: pass (the `validate` / `parse_str` doctests still hold — `triggers` defaults empty).

- [ ] **Step 9: Commit**

```bash
git add crates/tau-pkg/src/project/project.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(pkg): parse + validate [trigger.*] config (cron/manual + retry, slice 1)"
```

---

### Task 4: Lowering — config triggers → `IrModule.triggers` + agent-exists typecheck

**Files:**
- Modify: `crates/tau-ir/src/lower/parse.rs`, `crates/tau-ir/src/lower/typecheck.rs`, `crates/tau-ir/src/lower/mod.rs`, `crates/tau-ir/src/error.rs`

- [ ] **Step 1: Write the failing lowering test**

Append to the `tests` module in `crates/tau-ir/src/lower/parse.rs` (or a new test in `lower/mod.rs` tests — parse.rs is fine since it owns `Parsed`):

Actually place these as integration-style tests in a new file `crates/tau-ir/tests/lower_triggers.rs` (uses the public `lower_project`):

```rust
//! Lowering of `[trigger.*]` config into `IrModule.triggers`.

use tau_ir::lower::{lower_project, Caches};
use tau_ir::trigger::TriggerKind;
use tau_ir::{IrError, IrFormatVersion};
use tau_pkg::project::ProjectConfig;
use tau_ports::target::registry;

fn caches() -> Caches<'static> {
    Caches {
        native_tool: &|_| None,
        mcp_contract: &|_| None,
        skill: &|_| None,
    }
}

fn target() -> tau_ports::target::TargetTriple {
    registry::list_available().next().unwrap().triple
}

#[test]
fn lowers_cron_trigger_into_module() {
    let toml = r#"
        [project]
        name = "demo"

        [agents.summarizer]
        display_name = "S"
        package      = "p@^0.1"
        llm_backend  = "anthropic"

        [trigger.nightly]
        kind     = "cron"
        agent    = "summarizer"
        schedule = "0 3 * * *"

        [trigger.nightly.retry]
        max_attempts = 3
        backoff      = { strategy = "exponential", base = "30s", max = "10m" }
        dead_letter  = "dlq-sink"
    "#;
    let config = ProjectConfig::parse_str(toml).unwrap();
    let module = lower_project(&config, &target(), &caches()).unwrap();

    assert_eq!(module.triggers.len(), 1);
    let t = &module.triggers[0];
    assert_eq!(t.name, "nightly");
    assert_eq!(t.kind, TriggerKind::Cron);
    assert_eq!(t.agent.0, "summarizer");
    assert_eq!(t.schedule.as_deref(), Some("0 3 * * *"));
    assert_eq!(t.timezone.as_deref(), Some("UTC"));
    let r = t.retry.as_ref().unwrap();
    assert_eq!(r.max_attempts, 3);
    assert_eq!(r.backoff.base, "30s");
    assert_eq!(r.dead_letter.as_deref(), Some("dlq-sink"));

    // Option B: ir_format is NOT bumped for a trigger-bearing module.
    assert_eq!(module.ir_format.0, IrFormatVersion::CURRENT);
}

#[test]
fn trigger_less_module_keeps_v1_0_0() {
    let toml = r#"
        [project]
        name = "demo"
        [agents.a]
        display_name = "A"
        package = "p@^0.1"
        llm_backend = "anthropic"
    "#;
    let config = ProjectConfig::parse_str(toml).unwrap();
    let module = lower_project(&config, &target(), &caches()).unwrap();
    assert!(module.triggers.is_empty());
    assert_eq!(module.ir_format.0, IrFormatVersion::CURRENT);
}

#[test]
fn triggers_are_sorted_by_name() {
    let toml = r#"
        [project]
        name = "demo"
        [agents.a]
        display_name = "A"
        package = "p@^0.1"
        llm_backend = "anthropic"
        [trigger.zeta]
        kind = "manual"
        agent = "a"
        [trigger.alpha]
        kind = "manual"
        agent = "a"
    "#;
    let config = ProjectConfig::parse_str(toml).unwrap();
    let module = lower_project(&config, &target(), &caches()).unwrap();
    let names: Vec<&str> = module.triggers.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["alpha", "zeta"], "canonical order is by name");
}

#[test]
fn rejects_trigger_referencing_unknown_agent() {
    let toml = r#"
        [project]
        name = "demo"
        [agents.a]
        display_name = "A"
        package = "p@^0.1"
        llm_backend = "anthropic"
        [trigger.t]
        kind = "manual"
        agent = "ghost"
    "#;
    let config = ProjectConfig::parse_str(toml).unwrap();
    let err = lower_project(&config, &target(), &caches()).unwrap_err();
    assert!(
        matches!(&err, IrError::UnknownTriggerAgent { trigger, agent }
            if trigger == "t" && agent.0 == "ghost"),
        "expected UnknownTriggerAgent; got {err:?}"
    );
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-ir --tests`
Expected: FAIL — `Parsed has no field triggers`, `no variant UnknownTriggerAgent`.

- [ ] **Step 3: Add the `IrError::UnknownTriggerAgent` variant**

In `crates/tau-ir/src/error.rs`, add (before the closing brace):

```rust
    /// A trigger binding names an entrypoint agent that is not present in
    /// the workflow.
    #[error("trigger {trigger:?} references unknown agent {agent:?}")]
    UnknownTriggerAgent {
        /// The trigger name.
        trigger: String,
        /// The unresolved entrypoint agent id.
        agent: AgentId,
    },
```

- [ ] **Step 4: Thread triggers through `Parsed`**

In `crates/tau-ir/src/lower/parse.rs`:

Add imports:

```rust
use crate::trigger::{Backoff, BackoffStrategy, RetryPolicy, TriggerBinding, TriggerKind};
```

Add the field to `Parsed`:

```rust
pub(super) struct Parsed {
    /// Partially-populated workflow (content hashes are zero pending `resolve`).
    pub(super) workflow: Workflow,
    /// Trigger bindings, canonically ordered by name (BTreeMap iteration).
    pub(super) triggers: alloc::vec::Vec<TriggerBinding>,
}
```

At the end of `parse()`, before `Ok(Parsed { ... })`, build the triggers (BTreeMap iteration is already sorted by name):

```rust
    // --- Triggers (slice 1) --------------------------------------------
    let mut triggers: alloc::vec::Vec<TriggerBinding> = alloc::vec::Vec::new();
    for (name, entry) in config.triggers.iter() {
        let kind = match entry.kind.as_str() {
            "cron" => TriggerKind::Cron,
            "manual" => TriggerKind::Manual,
            // validate_trigger already rejected anything else; defensive.
            other => {
                return Err(IrError::Parse(alloc::format!(
                    "trigger {name:?}: unsupported kind {other:?} reached lowering"
                )));
            }
        };
        let retry = entry.retry.as_ref().map(|r| RetryPolicy {
            max_attempts: r.max_attempts,
            backoff: Backoff {
                strategy: match r.backoff_strategy.as_str() {
                    "fixed" => BackoffStrategy::Fixed,
                    _ => BackoffStrategy::Exponential,
                },
                base: r.backoff_base.clone(),
                max: r.backoff_max.clone(),
            },
            dead_letter: r.dead_letter.clone(),
        });
        triggers.push(TriggerBinding {
            name: name.clone(),
            kind,
            agent: AgentId(entry.agent.clone()),
            schedule: entry.schedule.clone(),
            timezone: if entry.timezone.is_empty() {
                None
            } else {
                Some(entry.timezone.clone())
            },
            retry,
        });
    }
```

and update the return to `Ok(Parsed { workflow: Workflow { ... }, triggers })`.

- [ ] **Step 5: Validate trigger.agent in `typecheck`**

In `crates/tau-ir/src/lower/typecheck.rs`, after the existing checks and before `Ok(())`:

```rust
    // 6. Each trigger's entrypoint agent must exist.
    for trigger in parsed.triggers.iter() {
        if !parsed.workflow.agents.contains_key(&trigger.agent) {
            return Err(IrError::UnknownTriggerAgent {
                trigger: trigger.name.clone(),
                agent: trigger.agent.clone(),
            });
        }
    }
```

Fix the two `Parsed { workflow: ... }` literals in `typecheck.rs`'s own `tests` module — add `triggers: alloc::vec::Vec::new(),`.

- [ ] **Step 6: Move triggers into the module + pick `ir_format` in `build_module`**

In `crates/tau-ir/src/lower/mod.rs`, replace `build_module`:

```rust
fn build_module(parsed: crate::lower::parse::Parsed, target: &TargetTriple) -> IrModule {
    // Option B (ADR-0043 §D1): ir_format is NOT bumped — it stays v1.0.0
    // whether or not the module carries triggers. The `triggers` field's
    // skip-empty serialization preserves trigger-less hashes; the appended
    // array differentiates trigger-bearing hashes on its own.
    IrModule {
        ir_format: crate::IrFormatVersion::current(),
        tau_version: env!("CARGO_PKG_VERSION").into(),
        target: *target,
        workflow: parsed.workflow,
        triggers: parsed.triggers,
    }
}
```

Fix the `parse.rs` `tests` module's `parse(&config)` calls — those go through `parse()` which now returns `Parsed` with `triggers`; the existing tests read `parsed.workflow.*` and are unaffected (no literal to fix there since they call `parse()`). Confirm `resolve.rs` does not construct `Parsed` by literal (it mutates `parsed` in place — fine).

- [ ] **Step 7: Run all tau-ir tests + doctests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir`
Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-ir`
Expected: all pass, including the 4 in `lower_triggers`.

- [ ] **Step 8: Commit**

```bash
git add crates/tau-ir/src/lower/ crates/tau-ir/src/error.rs crates/tau-ir/tests/lower_triggers.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(ir): lower [trigger.*] into IrModule.triggers + agent-exists check (slice 1)"
```

---

### Task 5: Bundle manifest — `[[trigger]]` section + conditional `schema_version` 3

**Files:**
- Modify: `crates/tau-pkg/src/bundle/manifest.rs`, `crates/tau-pkg/src/bundle/canonical.rs`, `crates/tau-pkg/src/bundle/build.rs`

- [ ] **Step 1: Write the failing manifest tests**

In `crates/tau-pkg/src/bundle/manifest.rs` `tests` module:

```rust
    #[test]
    fn parse_str_accepts_schema_version_3() {
        let toml_str = r#"
schema_version = 3

[bundle]
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
created_at = "2026-06-13T00:00:00Z"
tau_version = "0.1.0"
target = "passthrough"

[project]
name = "x"
version = "0.1.0"
tau_toml_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[[trigger]]
name = "nightly"
kind = "cron"
agent = "summarizer"
schedule = "0 3 * * *"
timezone = "UTC"
"#;
        let m = BundleManifest::parse_str(toml_str).expect("v3 must parse");
        assert_eq!(m.schema_version, 3);
        assert_eq!(m.triggers.len(), 1);
        assert_eq!(m.triggers[0].name, "nightly");
        assert_eq!(m.triggers[0].schedule.as_deref(), Some("0 3 * * *"));
    }

    #[test]
    fn parse_str_rejects_schema_version_4() {
        let toml_str = r#"
schema_version = 4

[bundle]
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
created_at = "2026-06-13T00:00:00Z"
tau_version = "0.1.0"
target = "passthrough"

[project]
name = "x"
version = "0.1.0"
tau_toml_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
"#;
        let err = BundleManifest::parse_str(toml_str).expect_err("should reject v4");
        match err {
            BundleParseError::UnsupportedSchemaVersion { found } => assert_eq!(found, 4),
            other => panic!("expected UnsupportedSchemaVersion, got {other:?}"),
        }
    }

    #[test]
    fn trigger_bearing_manifest_round_trips_canonical() {
        let mut m = sample_manifest();
        m.schema_version = 3;
        m.triggers = vec![BundleTrigger {
            name: "nightly".into(),
            kind: "cron".into(),
            agent: "summarizer".into(),
            schedule: Some("0 3 * * *".into()),
            timezone: Some("UTC".into()),
            retry: Some(BundleRetry {
                max_attempts: 3,
                backoff_strategy: "exponential".into(),
                backoff_base: "30s".into(),
                backoff_max: "10m".into(),
                dead_letter: Some("dlq-sink".into()),
            }),
        }];
        let toml = m.to_canonical_toml();
        let parsed = BundleManifest::parse_str(&toml).expect("round-trip");
        assert_eq!(parsed.triggers, m.triggers);
        assert_eq!(parsed.schema_version, 3);
    }

    #[test]
    fn trigger_less_manifest_omits_trigger_section() {
        let m = sample_manifest(); // no triggers, schema_version 2
        let toml = m.to_canonical_toml();
        assert!(!toml.contains("[[trigger]]"), "got: {toml}");
    }
```

- [ ] **Step 2: Run to confirm failure**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-pkg --tests`
Expected: FAIL — no `triggers` field, no `BundleTrigger`/`BundleRetry`.

- [ ] **Step 3: Add the manifest structs + field**

In `crates/tau-pkg/src/bundle/manifest.rs`, add the structs (near `BundleAgent`):

```rust
/// One trigger binding carried in a v3 bundle's `[[trigger]]` section.
///
/// A host-readable mirror of the trigger metadata already inside the IR
/// payload, so an operator's host adapter can read trigger bindings without
/// decoding the canonical IR hex. Present only in `schema_version >= 3`
/// bundles (i.e. bundles whose project declared ≥1 trigger).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleTrigger {
    /// Trigger name.
    pub name: String,
    /// `cron` | `manual`.
    pub kind: String,
    /// Entrypoint agent id.
    pub agent: String,
    /// 5-field cron expression (cron only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    /// IANA timezone (cron only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Re-invocation policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<BundleRetry>,
}

/// Retry policy carried in a `[[trigger]]`'s `retry` sub-table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleRetry {
    /// Total attempts including the first.
    pub max_attempts: u32,
    /// `fixed` | `exponential`.
    pub backoff_strategy: String,
    /// Base delay, duration string.
    pub backoff_base: String,
    /// Max delay, duration string.
    pub backoff_max: String,
    /// Sink reference for exhausted runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dead_letter: Option<String>,
}
```

> Note: `String` is needed in this file — confirm `use ...` brings it (the file uses `std::collections::BTreeMap`; `String` is prelude). No new import required.

Add the field to `BundleManifest` (after `ir_payload`):

```rust
    /// Trigger bindings (slice 1). Present only when the project declared
    /// triggers; the bundle's `schema_version` is `3` whenever this is
    /// non-empty. Hashed into the bundle self-hash via the canonical TOML.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<BundleTrigger>,
```

Update `parse_str`'s version gate:

```rust
        if manifest.schema_version < 1 || manifest.schema_version > 3 {
            return Err(BundleParseError::UnsupportedSchemaVersion {
                found: manifest.schema_version,
            });
        }
```

> Replace the existing `!= 1 && != 2` check. Keep the doctest examples (`schema_version = 1`) valid.

Update `sample_manifest()` in `tests_helpers` to add `triggers: Vec::new(),` to the literal (it is `schema_version: 2`, no triggers).

- [ ] **Step 4: Emit `[[trigger]]` in canonical TOML**

In `crates/tau-pkg/src/bundle/canonical.rs`, update the import line to add `BundleTrigger`:

```rust
use crate::bundle::manifest::{
    BackendRef, BundleAgent, BundleEffectiveCapabilities, BundleManifest, BundlePackage,
    BundleTrigger, IrPayload,
};
```

In `to_canonical_toml`, after the `[[agents]]` loop and **before** the `[ir_payload]` block (fixed order: packages, agents, triggers, ir_payload):

```rust
    // [[trigger]] — emitted when present so the bindings participate in the
    // self-hash. Order is the struct's Vec order (lowering sorts by name).
    for trigger in &manifest.triggers {
        out.push('\n');
        out.push_str("[[trigger]]\n");
        write_trigger(&mut out, trigger);
    }
```

Add the writer functions:

```rust
fn write_trigger(out: &mut String, t: &BundleTrigger) {
    write_str_kv(out, "name", &t.name);
    write_str_kv(out, "kind", &t.kind);
    write_str_kv(out, "agent", &t.agent);
    if let Some(s) = &t.schedule {
        write_str_kv(out, "schedule", s);
    }
    if let Some(tz) = &t.timezone {
        write_str_kv(out, "timezone", tz);
    }
    if let Some(r) = &t.retry {
        // Inline sub-table for stable single-line emission.
        out.push_str("retry = { ");
        write!(out, "max_attempts = {}", r.max_attempts).unwrap();
        write!(out, ", backoff_strategy = {}", toml_string(&r.backoff_strategy)).unwrap();
        write!(out, ", backoff_base = {}", toml_string(&r.backoff_base)).unwrap();
        write!(out, ", backoff_max = {}", toml_string(&r.backoff_max)).unwrap();
        if let Some(dl) = &r.dead_letter {
            write!(out, ", dead_letter = {}", toml_string(dl)).unwrap();
        }
        out.push_str(" }\n");
    }
}
```

> `write!`/`toml_string`/`write_str_kv` are already in scope in this file.

- [ ] **Step 5: Populate `triggers` + conditional `schema_version` in `build.rs`**

In `crates/tau-pkg/src/bundle/build.rs`, after the `selected_agents` block (step 5.5) and before assembling the manifest (step 6), build the trigger list from the validated config:

```rust
    // Step 5.6: gather trigger bindings (slice 1). One entry per validated
    // [trigger.<name>]. BTreeMap iteration is sorted by name.
    let triggers: Vec<crate::bundle::manifest::BundleTrigger> = project_config
        .triggers
        .values()
        .map(|t| crate::bundle::manifest::BundleTrigger {
            name: t.name.clone(),
            kind: t.kind.clone(),
            agent: t.agent.clone(),
            schedule: t.schedule.clone(),
            timezone: if t.timezone.is_empty() {
                None
            } else {
                Some(t.timezone.clone())
            },
            retry: t.retry.as_ref().map(|r| crate::bundle::manifest::BundleRetry {
                max_attempts: r.max_attempts,
                backoff_strategy: r.backoff_strategy.clone(),
                backoff_base: r.backoff_base.clone(),
                backoff_max: r.backoff_max.clone(),
                dead_letter: r.dead_letter.clone(),
            }),
        })
        .collect();
```

> Add `use std::...` is not needed; `Vec` is prelude. `project_config` is in scope.

In the `BundleManifest { ... }` literal, set the schema version conditionally and add the field:

```rust
        schema_version: if triggers.is_empty() { 2 } else { 3 },
        ...
        ir_payload: opts.ir_payload,
        triggers,
```

- [ ] **Step 6: Run the bundle tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg`
Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-pkg`
Expected: all pass. The existing `parse_str_rejects_schema_version_3` test in `manifest.rs` will now FAIL (v3 is accepted) — **delete or rename it**; the new `parse_str_accepts_schema_version_3` + `parse_str_rejects_schema_version_4` replace it. The `schema_version_ninety_nine_is_rejected` test still holds.

- [ ] **Step 7: Add a build-level integration test (trigger-bearing project → v3 bundle)**

Add to `crates/tau-pkg/src/bundle/build.rs` `tests`:

```rust
    #[test]
    fn build_emits_v3_bundle_with_trigger_section() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("tau.toml"),
            r#"
[project]
name = "trig"
version = "0.1.0"

[agents.summarizer]
display_name = "S"
package = "p@^0.1"
llm_backend = "anthropic"

[agents.summarizer.prompt]
system = "hi"

[trigger.nightly]
kind = "cron"
agent = "summarizer"
schedule = "0 3 * * *"
"#,
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("tau.lock"),
            "schema_version = 6\ngenerated_by_tau_version = \"0.1.0\"\ngenerated_at = \"2024-01-01T00:00:00Z\"\n",
        )
        .unwrap();

        let artifact = build(opts(tmp.path())).expect("build");
        let m = crate::bundle::manifest::BundleManifest::parse_str(
            &std::fs::read_to_string(&artifact.path).unwrap(),
        )
        .unwrap();
        assert_eq!(m.schema_version, 3);
        assert_eq!(m.triggers.len(), 1);
        assert_eq!(m.triggers[0].name, "nightly");
        crate::bundle::hash::verify_self_hash(&m).expect("self-hash verifies");
    }

    #[test]
    fn build_trigger_less_stays_v2() {
        let tmp = tempdir().unwrap();
        happy_path_project(tmp.path());
        let artifact = build(opts(tmp.path())).expect("build");
        let m = crate::bundle::manifest::BundleManifest::parse_str(
            &std::fs::read_to_string(&artifact.path).unwrap(),
        )
        .unwrap();
        assert_eq!(m.schema_version, 2);
        assert!(m.triggers.is_empty());
    }
```

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg bundle::build`
Expected: pass.

- [ ] **Step 8: Commit**

```bash
git add crates/tau-pkg/src/bundle/
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(pkg): bundle [[trigger]] section + conditional schema_version 3 (slice 1)"
```

---

### Task 6: Descriptor emitters (`systemd` + `k8s`)

**Files:**
- Modify: `crates/tau-ir/src/trigger.rs`

- [ ] **Step 1: Write the failing emitter tests**

Append to the `tests` module in `crates/tau-ir/src/trigger.rs`:

```rust
    #[test]
    fn k8s_emits_cronjob_with_verbatim_schedule() {
        let bindings = vec![cron_binding()];
        let out = emit_k8s(&bindings, "/srv/app.tau");
        assert_eq!(out.len(), 1);
        let (fname, content) = &out[0];
        assert!(fname.ends_with("nightly.cronjob.yaml"), "got {fname}");
        assert!(content.contains("kind: CronJob"), "got {content}");
        assert!(content.contains("schedule: \"0 3 * * *\""), "got {content}");
        assert!(content.contains("summarizer"), "got {content}");
    }

    #[test]
    fn systemd_emits_timer_and_service_for_simple_cron() {
        let bindings = vec![cron_binding()];
        let out = emit_systemd(&bindings, "/srv/app.tau");
        // .service + .timer for the one cron trigger.
        assert_eq!(out.len(), 2);
        let names: alloc::vec::Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.iter().any(|n| n.ends_with("nightly.service")), "got {names:?}");
        assert!(names.iter().any(|n| n.ends_with("nightly.timer")), "got {names:?}");
        let timer = &out.iter().find(|(n, _)| n.ends_with(".timer")).unwrap().1;
        // "0 3 * * *" → OnCalendar=*-*-* 03:00:00
        assert!(timer.contains("OnCalendar=*-*-* 03:00:00"), "got {timer}");
    }

    #[test]
    fn manual_trigger_emits_nothing() {
        let bindings = vec![TriggerBinding {
            name: "m".into(),
            kind: TriggerKind::Manual,
            agent: AgentId("a".into()),
            schedule: None,
            timezone: None,
            retry: None,
        }];
        assert!(emit_systemd(&bindings, "/srv/app.tau").is_empty());
        assert!(emit_k8s(&bindings, "/srv/app.tau").is_empty());
    }

    #[test]
    fn systemd_skips_uncovertible_cron() {
        // step syntax "*/5" is outside the slice-1 converter subset.
        let bindings = vec![TriggerBinding {
            name: "fast".into(),
            kind: TriggerKind::Cron,
            agent: AgentId("a".into()),
            schedule: Some("*/5 * * * *".into()),
            timezone: Some("UTC".into()),
            retry: None,
        }];
        // systemd skips it (returns empty); k8s still emits it verbatim.
        assert!(emit_systemd(&bindings, "/srv/app.tau").is_empty());
        assert_eq!(emit_k8s(&bindings, "/srv/app.tau").len(), 1);
    }

    #[test]
    fn cron_to_oncalendar_handles_dom_month_dow() {
        // "30 4 1 6 *" → minute 30, hour 04, dom 01, month 06, any dow.
        assert_eq!(
            cron_to_oncalendar("30 4 1 6 *").as_deref(),
            Some("*-06-01 04:30:00".into()).as_deref()
        );
        // dow Monday (1) with all-* date → "Mon *-*-* HH:MM:SS"
        assert_eq!(
            cron_to_oncalendar("0 9 * * 1").as_deref(),
            Some("Mon *-*-* 09:00:00".into()).as_deref()
        );
        // unsupported step form → None
        assert_eq!(cron_to_oncalendar("*/5 * * * *"), None);
    }
```

- [ ] **Step 2: Run to confirm failure**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-ir --tests`
Expected: FAIL — `emit_k8s`/`emit_systemd`/`cron_to_oncalendar` not found.

- [ ] **Step 3: Implement the emitters in `trigger.rs`**

Add to `crates/tau-ir/src/trigger.rs` (after the type definitions, before `#[cfg(test)]`):

```rust
use alloc::format;
use alloc::string::ToString;

/// Day-of-week names systemd's `OnCalendar` expects, indexed by cron dow
/// (0 and 7 both = Sunday).
const DOW_NAMES: [&str; 8] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Translate a 5-field cron expression to a systemd `OnCalendar` value, for
/// the slice-1 subset where each field is `*` or a plain non-negative
/// integer. Returns `None` for any field using ranges (`-`), lists (`,`), or
/// steps (`/`) — the caller skips the systemd timer for such triggers and
/// logs a warning (k8s still emits the cron verbatim).
pub fn cron_to_oncalendar(schedule: &str) -> Option<String> {
    let f: Vec<&str> = schedule.split_whitespace().collect();
    if f.len() != 5 {
        return None;
    }
    // Each field must be `*` or all-ASCII-digits.
    fn field(s: &str) -> Option<Option<u8>> {
        if s == "*" {
            Some(None)
        } else if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
            s.parse::<u8>().ok().map(Some)
        } else {
            None // ranges / lists / steps → unsupported
        }
    }
    let min = field(f[0])?;
    let hour = field(f[1])?;
    let dom = field(f[2])?;
    let month = field(f[3])?;
    let dow = field(f[4])?;

    let two = |v: Option<u8>| match v {
        None => "*".to_string(),
        Some(n) => format!("{n:02}"),
    };
    // OnCalendar date+time: `[DOW ]YYYY-MM-DD HH:MM:SS` with `*` wildcards.
    let date = format!("*-{}-{}", two(month), two(dom));
    let time = format!("{}:{}:00", two(hour), two(min));
    let body = format!("{date} {time}");
    match dow {
        None => Some(body),
        Some(d) if (d as usize) < DOW_NAMES.len() => {
            Some(format!("{} {}", DOW_NAMES[d as usize], body))
        }
        Some(_) => None, // out-of-range dow
    }
}

/// Emit systemd `.service` + `.timer` descriptors for each **cron** trigger.
/// `artifact_ref` is the path the unit invokes (the built `.tau` bundle).
/// Manual triggers and cron schedules outside the converter subset produce
/// no output (the caller logs the skip). Returns `(filename, content)` pairs.
pub fn emit_systemd(bindings: &[TriggerBinding], artifact_ref: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for b in bindings {
        if b.kind != TriggerKind::Cron {
            continue;
        }
        let Some(schedule) = b.schedule.as_deref() else {
            continue;
        };
        let Some(oncalendar) = cron_to_oncalendar(schedule) else {
            continue; // caller warns
        };
        let service = format!(
            "[Unit]\n\
             Description=tau trigger '{name}' (agent {agent})\n\n\
             [Service]\n\
             Type=oneshot\n\
             ExecStart=tau run --bundle {artifact} --agent {agent}\n",
            name = b.name,
            agent = b.agent.0,
            artifact = artifact_ref,
        );
        let timer = format!(
            "[Unit]\n\
             Description=tau trigger '{name}' schedule ({schedule})\n\n\
             [Timer]\n\
             OnCalendar={oncalendar}\n\
             Persistent=true\n\n\
             [Install]\n\
             WantedBy=timers.target\n",
            name = b.name,
            schedule = schedule,
            oncalendar = oncalendar,
        );
        out.push((format!("tau-{}.service", b.name), service));
        out.push((format!("tau-{}.timer", b.name), timer));
    }
    out
}

/// Emit a k8s `CronJob` manifest for each **cron** trigger. k8s consumes
/// 5-field cron verbatim, so every cron trigger emits exactly. Manual
/// triggers produce no output.
pub fn emit_k8s(bindings: &[TriggerBinding], artifact_ref: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for b in bindings {
        if b.kind != TriggerKind::Cron {
            continue;
        }
        let Some(schedule) = b.schedule.as_deref() else {
            continue;
        };
        let manifest = format!(
            "apiVersion: batch/v1\n\
             kind: CronJob\n\
             metadata:\n\
             \x20\x20name: tau-{name}\n\
             spec:\n\
             \x20\x20schedule: \"{schedule}\"\n\
             \x20\x20jobTemplate:\n\
             \x20\x20\x20\x20spec:\n\
             \x20\x20\x20\x20\x20\x20template:\n\
             \x20\x20\x20\x20\x20\x20\x20\x20spec:\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20restartPolicy: Never\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20containers:\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- name: tau\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20image: tau:latest\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20args: [\"run\", \"--bundle\", \"{artifact}\", \"--agent\", \"{agent}\"]\n",
            name = b.name,
            schedule = schedule,
            artifact = artifact_ref,
            agent = b.agent.0,
        );
        out.push((format!("tau-{}.cronjob.yaml", b.name), manifest));
    }
    out
}
```

> The `\x20` escapes keep YAML indentation unambiguous in the source literal. Verify the rendered YAML indents with real spaces (the test only checks substrings, but spot-check the output visually once).

- [ ] **Step 4: Run the emitter tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir trigger`
Expected: the 5 emitter tests pass (plus the 3 from Task 1).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ir/src/trigger.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(ir): systemd + k8s trigger descriptor emitters (slice 1)"
```

---

### Task 7: CLI — `tau build --emit-trigger=<adapter>`

**Files:**
- Modify: `crates/tau-cli/src/cli.rs`, `crates/tau-cli/src/cmd/build.rs`
- Test: `crates/tau-cli/tests/cmd_build_trigger.rs` (new)

- [ ] **Step 1: Add the CLI flag**

In `crates/tau-cli/src/cli.rs`, add to `BuildArgs` (after `offline`):

```rust
    /// Also emit host-adapter descriptors for the project's cron triggers.
    /// `systemd` writes `.timer` + `.service` units; `k8s` writes `CronJob`
    /// manifests. Files are written next to the bundle. Manual triggers and
    /// cron schedules systemd can't auto-translate are skipped with a note.
    #[arg(long = "emit-trigger", value_name = "ADAPTER", value_parser = ["systemd", "k8s"])]
    pub emit_trigger: Option<String>,
```

> Every existing `BuildArgs { ... }` literal in tests must add `emit_trigger: None,`. Grep: `grep -rn "BuildArgs {" crates/tau-cli | grep -v target`. The unit test in `cmd/build.rs` (`args_with_target`) is one — update it.

- [ ] **Step 2: Surface triggers from `lower_ir`**

In `crates/tau-cli/src/cmd/build.rs`, change `lower_ir`'s return type to also yield the lowered triggers. Update its signature and body:

```rust
pub(crate) fn lower_ir(
    project_root: &std::path::Path,
    target: &TargetTriple,
    mcp_cache: &BTreeMap<String, tau_ir::lower::ResolvedMcpContract>,
    preloaded_config: Option<&tau_pkg::project::ProjectConfig>,
) -> (Option<IrPayload>, Vec<tau_ir::trigger::TriggerBinding>) {
```

In the `Ok(module)` arm, capture the triggers before building the payload:

```rust
        Ok(module) => {
            let triggers = module.triggers.clone();
            let bytes = tau_ir::to_canonical_bytes(&module);
            let hash_bytes = tau_ir::compute_hash(&module);
            let canonical_ir_hash = hex_lower(&hash_bytes);
            let canonical_ir_bytes_hex = hex_lower(&bytes);
            (
                Some(IrPayload {
                    ir_format: module.ir_format.0.clone(),
                    canonical_ir_hash,
                    canonical_ir_bytes_hex,
                }),
                triggers,
            )
        }
        Err(e) => {
            tracing::warn!("IR lowering failed (bundle built without IR payload): {e}");
            (None, Vec::new())
        }
```

Update every early-return in `lower_ir` (the read/parse/validate-failure paths) from `return None;` to `return (None, Vec::new());`.

Update the existing doctest/unit test `lower_ir_yields_payload_for_native_tool_project` to destructure: `let (payload, _triggers) = lower_ir(...);`.

- [ ] **Step 3: Wire the call site + descriptor emission**

In `run()`, update the `lower_ir` call:

```rust
    let (ir_payload, trigger_bindings) =
        lower_ir(&project_root, &target, &mcp_cache_ir, ts_project.as_ref());
```

After a successful `build` (inside the `Ok(artifact)` arm, after `emit_artifact`), emit descriptors when requested:

```rust
            if let Some(adapter) = &args.emit_trigger {
                emit_trigger_descriptors(adapter, &trigger_bindings, &artifact, output);
            }
```

Add the helper:

```rust
/// Write host-adapter descriptors for the lowered cron triggers next to the
/// bundle. Manual triggers and systemd-uncovertible cron schedules are noted
/// and skipped. Errors writing a descriptor are surfaced but non-fatal — the
/// bundle is already built.
fn emit_trigger_descriptors(
    adapter: &str,
    bindings: &[tau_ir::trigger::TriggerBinding],
    artifact: &BundleArtifact,
    output: &mut Output,
) {
    use tau_ir::trigger::{emit_k8s, emit_systemd, TriggerKind};

    let artifact_ref = artifact.path.display().to_string();
    let descriptors = match adapter {
        "systemd" => emit_systemd(bindings, &artifact_ref),
        "k8s" => emit_k8s(bindings, &artifact_ref),
        // value_parser restricts to these two; defensive.
        other => {
            let _ = output.error(format!("unknown --emit-trigger adapter: {other}"));
            return;
        }
    };

    if descriptors.is_empty() {
        let cron_count = bindings.iter().filter(|b| b.kind == TriggerKind::Cron).count();
        if cron_count == 0 {
            let _ = output.status("No cron triggers to emit (manual triggers need no scheduler).");
        } else {
            let _ = output.status(format!(
                "{cron_count} cron trigger(s) present, but none were emittable for {adapter} \
                 (schedules outside the auto-convertible subset are skipped)."
            ));
        }
        return;
    }

    let dir = artifact
        .path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    for (filename, content) in descriptors {
        let path = dir.join(&filename);
        match std::fs::write(&path, content.as_bytes()) {
            Ok(()) => {
                let _ = output.status(format!("Wrote trigger descriptor: {}", path.display()));
            }
            Err(e) => {
                let _ = output.error(format!("failed to write {}: {e}", path.display()));
            }
        }
    }
}
```

> Add `use tau_pkg::bundle::BundleArtifact;` — confirm it's already imported (the top of `build.rs` imports `BundleArtifact` already).

- [ ] **Step 4: Write the integration test**

Create `crates/tau-cli/tests/cmd_build_trigger.rs`:

```rust
//! `tau build --emit-trigger=systemd` writes scheduler descriptors next to
//! the bundle for a project's cron triggers.

use std::process::Command;

fn tau_bin() -> std::path::PathBuf {
    // Cargo sets CARGO_BIN_EXE_<name> for integration tests.
    env!("CARGO_BIN_EXE_tau").into()
}

#[test]
fn emit_trigger_systemd_writes_units() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(
        root.join("tau.toml"),
        r#"
[project]
name = "trig"
version = "0.1.0"

[agents.summarizer]
display_name = "S"
package = "p@^0.1"
llm_backend = "anthropic"

[agents.summarizer.prompt]
system = "hi"

[trigger.nightly]
kind = "cron"
agent = "summarizer"
schedule = "0 3 * * *"
"#,
    )
    .unwrap();
    std::fs::write(
        root.join("tau.lock"),
        "schema_version = 6\ngenerated_by_tau_version = \"0.1.0\"\ngenerated_at = \"2024-01-01T00:00:00Z\"\n",
    )
    .unwrap();

    let status = Command::new(tau_bin())
        .args(["build", "--emit-trigger", "systemd"])
        .current_dir(root)
        .status()
        .expect("run tau build");
    assert!(status.success(), "tau build failed");

    assert!(root.join("tau-nightly.service").exists(), "service unit missing");
    let timer = std::fs::read_to_string(root.join("tau-nightly.timer")).unwrap();
    assert!(timer.contains("OnCalendar=*-*-* 03:00:00"), "got: {timer}");
}
```

> Confirm the test binary name is `tau` (`CARGO_BIN_EXE_tau`). Check `crates/tau-cli/Cargo.toml` `[[bin]] name`. If different, adjust the env var. Also confirm an existing `cmd_build.rs` test uses the same `Command`-spawn pattern and mirror it (some suites use a helper like `assert_cmd`); match the established pattern in that crate rather than the raw `Command` above if one exists.

- [ ] **Step 5: Run the CLI tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli cmd_build`
Expected: existing build tests + the new `emit_trigger_systemd_writes_units` pass.

Run clippy on the touched crates:
`timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-cli -p tau-ir -p tau-pkg --all-targets`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-cli/
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(cli): tau build --emit-trigger=systemd|k8s (slice 1)"
```

---

### Task 8: ADR + mdBook docs

**Files:**
- Create: `docs/decisions/0043-trigger-ingress-slice-1.md` (0043 confirmed — 0042 is taken)
- Create: `docs/explanation/trigger-ingress.md`
- Modify: `docs/SUMMARY.md`

- [ ] **Step 1: ADR number is 0043** (confirmed during execution — `0042-cross-repo-ci-template-sync.md` exists). Use `0043` for the filename and all in-text references.

- [ ] **Step 1b: Fix the committed forward-references from Tasks 1–2**

Tasks 1 and 2 committed doc-comments that say `ADR-0042`; the real number is `0043`. Correct them:

```bash
grep -rln "ADR-0042" crates/ | grep -v target
# expected: crates/tau-ir/src/trigger.rs, crates/tau-ir/src/module.rs
```

Edit each occurrence (`crates/tau-ir/src/trigger.rs` lines ~4 and ~76; `crates/tau-ir/src/module.rs` line ~56) replacing `ADR-0042` with `ADR-0043`. These can ride in this task's docs commit, or a separate `docs: correct ADR reference` commit — either is fine since they are comment-only.

- [ ] **Step 2: Write the ADR**

Create `docs/decisions/0043-trigger-ingress-slice-1.md`:

```markdown
# 0043 — Trigger ingress, slice 1: cron + manual + retry policy

**Status:** Accepted

**Date:** 2026-06-13

**Relates to:** framing doc `docs/superpowers/specs/2026-06-13-trigger-ingress-and-serve-transport-framing.md`, [ADR-0037](0037-workflow-ir.md) (workflow IR), the egress-only capability vocabulary, NG3/NG5/NG6.

## Context

The framing doc takes the position *compile the trigger; delegate the
substrate.* This ADR records the implementation decisions for the first
slice: the self-contained kinds (`cron`, `manual`) plus a `[trigger.*.retry]`
policy and `tau build --emit-trigger=systemd|k8s`. `webhook`/`queue` (which
require a host-adapter contract that `tau check` enforces) are slice 2.

## Decisions

### D1 — `triggers` is a sibling of `workflow`; `ir_format` is NOT bumped

`IrModule` gains `triggers: Vec<TriggerBinding>` as a sibling of `workflow`
(triggers are about *invocation*, not the call graph). The field carries
`#[serde(default, skip_serializing_if = "Vec::is_empty")]`, so a trigger-less
module emits no `triggers` key and its canonical bytes — and content hash —
are byte-identical to a pre-trigger module; a trigger-bearing module appends a
`triggers` array, which differentiates its hash on its own.

`ir_format` stays `v1.0.0` in all cases — it is **not** bumped. The framing
doc leaned toward a minor bump, but **triggers are inert at runtime**: a
trigger decides when/whether the host invokes tau, a decision made before
tau's process starts, so an old runtime that silently ignores the `triggers`
field still executes the workflow correctly. There is no reader-side gate on
`ir_format` and we deliberately add none (it would reject a runnable
workflow). The gate with teeth is the bundle `schema_version` (§D3), read at
build/inspect time. Since nothing keys off the IR language version, a bump
would be a label with no consumer — so we leave it at `v1.0.0` (YAGNI). An
unconditional bump was independently disqualified: it would re-hash every
existing trigger-less module.

### D2 — DLQ envelope shape deferred

Slice 1 compiles the retry *policy* (`max_attempts`, `backoff`,
`dead_letter` sink reference). The dead-letter *envelope* is a runtime
artifact produced when a trigger fires and exhausts its attempts; nothing in
tau fires triggers yet (retry is host-honoured), so the envelope shape lands
with the host-adapter runtime work.

### D3 — bundle `schema_version` bumps to 3 only when triggers are present

A trigger-less bundle stays `schema_version = 2` and serialises identically
to today. A trigger-bearing bundle is `3`, so an old `tau` rejects it loudly
rather than silently dropping the binding. `BundleManifest` accepts `{1,2,3}`.

### D4 — systemd needs a cron→OnCalendar converter; k8s takes cron verbatim

k8s `CronJob.schedule` consumes 5-field cron exactly. systemd `OnCalendar`
does not, so slice 1 ships a converter for the subset where each field is `*`
or a plain integer. Schedules outside that subset are skipped for systemd
(with a logged note) but still emit exactly for k8s.

### Non-goal cross-check

No inbound capability verb was added. cron/manual are egress-shaped: the host
invokes tau as a child process. The egress-only vocabulary remains
load-bearing for NG3. Retry is host-honoured (NG6 — no durable state in
core); `dead_letter` is a sink reference, never a tau-owned store.

## Consequences

`tau.toml` authors can declare cron/manual triggers that compile into the
content-hashed artifact and reproduce across targets. `tau build
--emit-trigger` generates the scheduler wiring; the operator owns the
substrate. Slice 2 adds `webhook`/`queue` + the `tau check` host-adapter rule.
```

- [ ] **Step 3: Write the explanation page**

Create `docs/explanation/trigger-ingress.md` — a Diátaxis *explanation* page covering: the substrate/binding split, the `[trigger]` schema (cron + manual + retry), how it lowers into the IR + bundle, and `--emit-trigger`. Keep it to the slice-1 surface; note webhook/queue as "slice 2, not yet shipped." Reuse the framing doc's tables. (Author ~60–100 lines; mirror the prose style of an existing `docs/explanation/*.md`.)

- [ ] **Step 4: Add to `SUMMARY.md`**

Add a line under the appropriate Explanation section in `docs/SUMMARY.md`:

```markdown
- [Trigger ingress](explanation/trigger-ingress.md)
```

And under the decisions/ADR list:

```markdown
- [0043 — Trigger ingress, slice 1](decisions/0043-trigger-ingress-slice-1.md)
```

- [ ] **Step 5: Build the book**

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build`
Expected: only `[INFO]` lines; no broken-link errors. Then `rm -rf docs/book`.

- [ ] **Step 6: Commit**

```bash
git add docs/
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "docs(trigger): ADR-0043 + explanation page for trigger ingress slice 1"
```

---

## Final verification (before PR)

- [ ] **Workspace-scoped test sweep of the four touched crates**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir -p tau-pkg -p tau-cli
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-ir -p tau-pkg -p tau-cli
```

- [ ] **Conformance crate still green** (proves trigger-less IR hashes unchanged end-to-end)

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-conformance
```

- [ ] **clippy + fmt**

```
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-ir -p tau-pkg -p tau-cli --all-targets
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt --check
```

- [ ] **Deep gate** (Rust changed → run before PR per CLAUDE.md)

```
lefthook run deep-gate
```

- [ ] **Open the PR against `main`**

```bash
gh pr create --base main --title "feat: trigger ingress slice 1 (cron + manual + retry + --emit-trigger)" --body "<summary + link to spec PR #330 + ADR-0043>"
```

---

## Self-review (run after drafting, before execution)

**Spec coverage (slice 1 scope = cron + manual + `[trigger.*].retry` + `--emit-trigger=systemd|k8s`):**

| Spec element | Task |
|---|---|
| `[trigger.<name>]` parse (cron/manual) | Task 3 |
| `[trigger.*.retry]` policy (max_attempts/backoff/dead_letter) | Tasks 1, 3 |
| `TriggerBinding` in IR, sibling of `workflow` | Tasks 1, 2 |
| `ir_format` mechanics — no bump, forward-compat read (open Q2) | Task 2 (D1) |
| trigger-less modules hash identically | Task 2 |
| Lowering = metadata only, no new nodes | Task 4 |
| canonical + hash include triggers; conformance preserved | Tasks 2, 5, final-verify |
| Bundle `trigger` + `tau.trigger` section, schema bump | Task 5 (D3/D4) |
| `tau build --emit-trigger` systemd + k8s | Tasks 6, 7 |
| webhook/queue EXCLUDED | enforced by `validate_trigger` rejection (Task 3) |
| no inbound capability verb | constraint honoured — `tau-domain` untouched |

**Placeholder scan:** every code step shows complete code; no TBD/TODO-as-implementation. (The one deliberate deferral — DLQ envelope — is documented as out-of-scope, not stubbed.)

**Type consistency:** `TriggerBinding`/`RetryPolicy`/`Backoff`/`BackoffStrategy`/`TriggerKind` defined once (Task 1), used verbatim in Tasks 2/4/6/7. Config-side `TriggerEntry`/`RetryEntry` (Task 3) → IR types (Task 4) → `BundleTrigger`/`BundleRetry` (Task 5): the three-layer mirror matches the existing `UncheckedAgent`/`Agent`/`BundleAgent` pattern. `ir_format` stays `IrFormatVersion::CURRENT` everywhere (Option B — no new constant). `IrError::UnknownTriggerAgent` defined in Task 4, asserted in Task 4 tests. `emit_systemd`/`emit_k8s`/`cron_to_oncalendar` defined Task 6, called Task 7.
