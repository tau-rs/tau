# EPIC 6.1 — Durability intent knob (`durable = "survive-restarts"`)

**Status:** Approved (brainstorm) — ready for `writing-plans`
**Date:** 2026-06-22
**Roadmap:** `docs/superpowers/plans/vision-roadmap.md` § EPIC 6, story 6.1
**Builds on:** ADR-0053 (durable execution A-minimal, PR #373)

## Problem

A-minimal (ADR-0053) shipped per-agent durable execution, but the only
authoring surface is the *mechanism*:

```toml
[agents.worker.durable]
checkpoint = "per_turn"   # or "per_tool_call"
store      = "file"
```

The author must name the granularity and store. That is backwards: the
author knows their *intent* ("this run must survive a restart"); the
*host* knows what granularity + store it can actually provide for a given
target. Worse, the runtime already ignores the declared `store` — the
dispatcher hardcodes `FileCheckpointStore` whenever `durable.is_some()`
(`ir_dispatcher.rs:218`), so today's "store" field is decorative and the
real resolution is hidden.

Story 6.1 closes this: add an **intent knob** `durable = "survive-restarts"`,
have the **host resolve** intent → concrete granularity + store **per
target**, and make `tau check --target X` **print** the resolution so
there is *no hidden behavior*.

NG5 framing (ROADMAP): durability is *delegated-canonical + opt-in
host-sized tiers*. The intent knob is the "host-sized" surface; the
explicit form stays as the power-user escape hatch.

## Decision

**Carry the intent in the IR; the host resolves it per-target at run and
at `tau check --target` time (Option B).** The bundle stays portable —
one IR runs anywhere, and each host sizes durability to what it can
provide. Transparency (the acceptance bar) is met by `tau check
--target X` printing the resolution. This needs no per-target *lowering*
infrastructure (EPIC 3, not yet built).

Rejected alternatives:

- **Bake concrete per-target into the IR at build (Option A).** Makes the
  bundle target-specific — a `survive-restarts` bundle built for
  `linux-native-strict` could not move to `any-wasi-strict`. Fights the
  "one portable component" vision and depends on EPIC 3 lowering infra
  that does not exist yet.
- **Authoring sugar resolved to a fixed default at lowering (Option C).**
  `tau check --target X` would print the *same* thing for every target —
  "host-resolved per target" would be theater. Weakest on the acceptance
  intent.

## Design

### 1. Authoring surface (the authoring contract change)

`durable` accepts **either** a scalar intent **or** the existing explicit
table:

```toml
# Intent form (new, recommended) — host sizes it per target:
[agents.worker]
durable = "survive-restarts"

# Explicit form (the shipped #373 escape hatch — unchanged):
[agents.power-user.durable]
checkpoint = "per_tool_call"
store      = "file"
```

`tau-pkg`'s `UncheckedDurable` becomes an untagged enum:

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum UncheckedDurable {
    /// `durable = "survive-restarts"` — a bare string intent.
    Intent(String),
    /// `[agents.<id>.durable] { checkpoint, store }` — explicit.
    Explicit(UncheckedDurableExplicit),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedDurableExplicit {
    pub checkpoint: String,
    pub store: String,
}
```

Validation (in `project.rs`, replacing the current `durable` block at
~line 1470) produces a validated `DurableEntry`:

```rust
pub enum DurableEntry {
    Intent(String),                          // validated against known intents
    Explicit { checkpoint: String, store: String },
}
```

- Intent: must be `"survive-restarts"`. Any other string →
  `AgentValidation { message: "durable \"X\" unsupported (accepts \
  \"survive-restarts\" or an explicit { checkpoint, store } table)" }`.
- Explicit: unchanged rules — `checkpoint ∈ {per_turn, per_tool_call}`,
  `store == "file"` (`deny_unknown_fields` already rejects typos).

### 2. IR (`tau-ir/src/durable.rs`)

`Durability` becomes a tagged enum:

```rust
#[non_exhaustive]
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Durability {
    /// High-level intent — the host picks granularity + store per target.
    Intent(DurabilityIntent),
    /// Explicit escape hatch (the shipped #373 form).
    Explicit {
        checkpoint: CheckpointGranularity,
        store: DurableStore,
    },
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum DurabilityIntent {
    #[serde(rename = "survive-restarts")]
    SurviveRestarts,
}
```

`CheckpointGranularity` and `DurableStore` are unchanged. The existing
`Durability::per_turn_file()` / `Durability::new(..)` constructors are
re-pointed at the `Explicit` variant (call sites in `tau-ir-lower` tests
and `tau-runtime-*` update accordingly).

JSON shape (self-describing, serde-tagged):

```json
{ "intent": "survive-restarts" }
{ "explicit": { "checkpoint": "per_turn", "store": "file" } }
```

**IR format version:** MINOR bump (current → next minor; the
authoritative current value is read from `tau-ir` at implementation
time, e.g. v2.1.0 → v2.2.0). Drift tests in `tau-ir` and the
`tau-runtime-tokio` mirror assert the new value. Existing fixtures that
used the explicit form re-serialize under the `Explicit` tag — their
canonical bytes shift, which is the honest signal of an IR-format change
(same treatment as #373's v2.0.0 → v2.1.0).

### 3. Lowering (`tau-ir-lower/src/lower/parse.rs`)

`lower_durable` maps the validated `DurableEntry` to the IR enum:

```rust
fn lower_durable(entry: &AgentEntry) -> Option<tau_ir::durable::Durability> {
    use tau_ir::durable::*;
    match entry.durable.as_ref()? {
        DurableEntry::Intent(s) => {
            // tau-pkg validated the string; map total.
            debug_assert_eq!(s, "survive-restarts");
            Some(Durability::Intent(DurabilityIntent::SurviveRestarts))
        }
        DurableEntry::Explicit { checkpoint, store } => {
            let c = match checkpoint.as_str() {
                "per_tool_call" => CheckpointGranularity::PerToolCall,
                _ => CheckpointGranularity::PerTurn,
            };
            let s = match store.as_str() {
                _ => DurableStore::File, // only "file" validated
            };
            Some(Durability::Explicit { checkpoint: c, store: s })
        }
    }
}
```

### 4. Per-target resolution (the host-resolved part)

A pure resolver maps a `Durability` against a `TargetTriple`'s durability
capability:

```rust
pub struct ResolvedDurability {
    pub checkpoint: CheckpointGranularity,
    pub store: DurableStore,
    pub support: Support,
    /// `Some` when the author used an intent; `None` for explicit.
    pub from_intent: Option<DurabilityIntent>,
}

pub enum Support {
    /// Target fully provides the resolved granularity + store.
    Honored,
    /// Target cannot honor the request. `tau check --target` → Error
    /// (hard-fail; see "Open choice resolved" below).
    Unsupported { reason: &'static str },
}

pub fn resolve_durability(d: &Durability, t: &TargetTriple) -> ResolvedDurability;
```

- `Durability::Explicit { checkpoint, store }` → resolves to itself;
  `support` checks the target provides `store`.
- `Durability::Intent(SurviveRestarts)` → maps via the target's
  durability capability (table below).

**Crate placement.** The resolver needs both `tau_ir::durable` types and
`tau_ports::target::TargetTriple`. `tau-ir` depends only on `tau-domain`;
`tau-ports` does not depend on `tau-ir`. `tau-runtime-core` already
depends on *both* (`options.rs` references `tau_ir::durable` and
`tau_ports::orchestration`) and is `#![no_std]`-clean for a pure
function over enums. The resolver + `ResolvedDurability` + `Support` live
in a new `tau-runtime-core/src/durable_resolve.rs`, re-exported from the
crate root, reachable by both the host (`tau-runtime-tokio`) and `tau
check` (`tau-cli`). The per-target *policy* is a small `match` on the
triple's `(platform, adapter_family)`; it does not need a new field on
the `tau-ports` registry entry for A-minimal (avoids leaking tau-ir
durability vocabulary into `tau-ports`).

**Resolution table** (genuinely target-keyed; uniform today, diverges the
moment `DurableStore::Kv` or a no-persistence target lands):

| target | `survive-restarts` resolves to | support |
|---|---|---|
| `linux-native-strict`, `darwin-native-strict`, `linux-container-strict` | per_turn + file | Honored |
| `any-wasi-strict` | per_turn + file (host-mediated preopen) | Honored |
| `passthrough` | per_turn + file | Honored |
| `windows-native-strict` (Reserved) | per_turn + file | Honored (static) |

### 5. `tau check --target X` output (the transparency requirement)

Gated on `--target`. Folded into the existing `sandbox` category's
`--target` branch (`categories/sandbox.rs`) — already the target-aware
code path, the place an operator looks for per-target facts. For each
agent that declares `durable`:

```
$ tau check --target any-wasi-strict
  durability  worker: survive-restarts → per_turn checkpoints, file store  [resolved for any-wasi-strict]
  durability  power-user: explicit per_tool_call + file                    [resolved for any-wasi-strict]
```

- `Support::Honored` → an **informational** finding at a new
  `Severity::Note` level, `rule_id = "tau.durability.resolved"`.
  Transparency, not validation: it does not change the exit code.
  `Severity` today is `{Error, NeedsSetup, Warning}` — none fit (a
  resolution line is not a lint warning, and SARIF would mis-level it).
  Add `Severity::Note` (SARIF `note` level; ignored by `compute_exit`,
  same as `Warning`). This touches the `Severity` enum + its match arms
  in `compute_exit` and the three output renderers (human/json/sarif) —
  all small, exhaustive matches.
- `Support::Unsupported { reason }` → **`Severity::Error`**,
  `rule_id = "tau.durability.unsupported"`, fails `tau check` (exit 2).

Structured payload (JSON/SARIF) carries `{ agent, form: "intent" |
"explicit", intent?, checkpoint, store, support, target }`.

The sandbox category currently iterates *plugin packages*; durability
iterates *agents* (`ctx.project.agents`). The durability lines are
emitted in the same `--target` block, before the per-plugin loop, so the
two concerns stay visually grouped but independently computed.

### 6. Runtime (honest, not hardcoded)

`ir_dispatcher.rs` (~line 218) stops hardcoding. When `entry_agent.durable`
is `Some`:

1. `let resolved = resolve_durability(&durability, &TargetTriple::host());`
2. If `resolved.support` is `Unsupported`, fail the run with a clear error
   (the host cannot honor the declared intent) — symmetric with the
   build-time `tau check` failure.
3. Pick the store from `resolved.store` (today always
   `FileCheckpointStore`, but now *driven by* the resolution, not a
   literal), and pass `resolved.checkpoint` as `RunOptions.durable_granularity`.

Behavior is identical today (file + per_turn/per_tool_call) but is now
declared, resolved, and traceable rather than hidden.

### 7. TypeScript authoring parity (`tau-ts-extract`)

The `.ts` surface already carries the explicit `durable` field
(ADR-0053). Add the scalar intent form so `durable: "survive-restarts"`
in TS lowers byte-equal to the TOML intent form. TOML↔TS canonical IR
conformance fixture covers it.

## Open choice — resolved

**When a target cannot honor `survive-restarts`, `tau check --target X`
hard-fails (Error), it does not warn-and-degrade.** Consistent with the
"any check that *could* run at build time *must*" + "no silent
degradation, escape hatches must be explicit" principles. No current
target hits this branch, but `Support::Unsupported` and the Error path
exist so the first non-persistent target fails loudly. A future
`--allow-degraded-durability` escape hatch can relax this explicitly; it
is out of scope for 6.1.

## Testing

- **tau-pkg:** intent string parses to `DurableEntry::Intent`; explicit
  table still parses; unknown intent string fails with the documented
  message; intent + explicit are mutually exclusive (a table cannot also
  be a string — serde untagged guarantees this).
- **tau-ir:** `Durability::Intent` / `Explicit` round-trip; snake_case
  JSON tags (`intent` / `explicit`); IR-version drift test asserts the
  new minor.
- **tau-ir-lower:** intent lowers to `Durability::Intent(SurviveRestarts)`;
  explicit lowers to `Durability::Explicit{..}` (existing test retargeted).
- **resolver (tau-runtime-core):** every Available triple resolves
  `survive-restarts` → per_turn + file + Honored; explicit resolves to
  itself; a synthetic Unsupported case (via a test-only triple or a
  forced policy arm) yields `Support::Unsupported`.
- **tau check:** `--target X` prints the durability line for a durable
  agent (human + JSON snapshots); Honored does not change exit code;
  Unsupported fails. Without `--target`, no durability line (unchanged).
- **runtime:** a `survive-restarts` agent runs and checkpoints (existing
  kill-and-resume DoD test, retargeted from the explicit form).
- **conformance:** a new fixture (`17_durable_intent`) cross-mode
  conforms; existing `16_durable_per_turn` stays green (now under the
  `Explicit` tag). TOML↔TS byte-equal for the intent form.

## Out of scope (6.1)

- `--allow-degraded-durability` escape hatch (named when the first
  non-persistent target lands).
- `DurableStore::Kv` and any additional intents (additive minor bumps).
- Per-target *lowering* / build-time baking (Option A; EPIC 3 territory).
- Story 6.2 (compose-with-orchestrator how-to) and 6.3 (A-full, gated).

## Acceptance (story 6.1)

`durable = "survive-restarts"` parses, lowers, and runs; `tau check
--target X` prints the resolved granularity + store per target; no hidden
behavior (the runtime store/granularity is driven by the same resolver
that `check` prints). Explicit form preserved.
