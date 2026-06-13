# 0044 — Trigger ingress, slice 1: cron + manual + retry policy

**Status:** Accepted

**Date:** 2026-06-13

**Relates to:** the framing doc `docs/superpowers/specs/2026-06-13-trigger-ingress-and-serve-transport-framing.md`, [ADR-0043](0043-trigger-ingress.md) (the trigger-ingress stance), [ADR-0037](0037-workflow-ir.md) (workflow IR), the egress-only capability vocabulary, NG3 / NG5 / NG6.

## Context

The framing doc takes the position *compile the trigger; delegate the substrate.* This ADR records the implementation decisions for the first slice: the self-contained kinds (`cron`, `manual`) plus a `[trigger.*.retry]` policy and `tau build --emit-trigger=systemd|k8s`. `webhook` / `queue` (which require a host-adapter contract that `tau check` enforces) are slice 2.

## Decisions

### D1 — `triggers` is a sibling of `workflow`; `ir_format` is NOT bumped

`IrModule` gains `triggers: Vec<TriggerBinding>` as a sibling of `workflow` (triggers are about *invocation*, not the call graph). The field carries `#[serde(default, skip_serializing_if = "Vec::is_empty")]`, so a trigger-less module emits no `triggers` key and its canonical bytes — and content hash — are byte-identical to a pre-trigger module; a trigger-bearing module appends a `triggers` array, which differentiates its hash on its own.

`ir_format` stays `v1.0.0` in all cases — it is **not** bumped. The framing doc leaned toward a minor bump, but **triggers are inert at runtime**: a trigger decides when/whether the host invokes tau, a decision made before tau's process starts, so an old runtime that silently ignores the `triggers` field still executes the workflow correctly. There is no reader-side gate on `ir_format` and we deliberately add none (it would reject a runnable workflow). The gate with teeth is the bundle `schema_version` (D3), read at build/inspect time. Since nothing keys off the IR language version, a bump would be a label with no consumer — so we leave it at `v1.0.0`. An unconditional bump was independently disqualified: it would re-hash every existing trigger-less module.

### D2 — DLQ envelope shape deferred

Slice 1 compiles the retry *policy* (`max_attempts`, `backoff`, `dead_letter` sink reference). The dead-letter *envelope* is a runtime artifact produced when a trigger fires and exhausts its attempts; nothing in tau fires triggers yet (retry is host-honoured), so the envelope shape lands with the host-adapter runtime work.

### D3 — bundle `schema_version` bumps to 3 only when triggers are present

A trigger-less bundle stays `schema_version = 2` and serialises identically to today. A trigger-bearing bundle is `3`, so an old tau rejects it loudly rather than silently dropping the binding. `BundleManifest` accepts `{1, 2, 3}`, and `parse_str` additionally rejects a trigger-bearing bundle whose `schema_version < 3` — the invariant is enforced at parse time, not merely produced by `tau build`.

### D4 — systemd needs a cron→OnCalendar converter; k8s takes cron verbatim

k8s `CronJob.schedule` consumes 5-field cron exactly. systemd `OnCalendar` does not, so slice 1 ships a converter for the subset where each field is `*` or a plain integer. Schedules outside that subset are skipped for systemd (with a logged note) but still emit exactly for k8s. `tau.toml` validation additionally range-checks any plain-integer cron field (e.g. rejects hour `25`) at build time.

### Non-goal cross-check

No inbound capability verb was added. cron/manual are egress-shaped: the host invokes tau as a child process. The egress-only vocabulary remains load-bearing for NG3. Retry is host-honoured (NG6 — no durable state in core); `dead_letter` is a sink reference, never a tau-owned store.

## Consequences

`tau.toml` authors can declare cron/manual triggers that compile into the content-hashed artifact and reproduce across targets. `tau build --emit-trigger` generates the scheduler wiring; the operator owns the substrate. Slice 2 adds `webhook` / `queue` plus the `tau check` host-adapter rule.
