# ADR-0075: The ops lane — env `local`, pins, `tau plan`/`apply`, run-or-refuse

**Status:** Accepted (records locked decisions §10.1/§10.6/§10.7 of the
[2026-09-01 consolidated design](../superpowers/specs/2026-09-01-tau-authoring-ops-and-primitives-design.md);
Phase 0 ADR wave)
**Date:** 2026-09-01
**Deciders:** maintainer, via the 2026-09-01 brainstorm session
**Builds on:** ADR-0043 ("compile the trigger, delegate the substrate"),
the `tau mcp pin`/diff precedent (`crates/tau-cli/src/cmd/mcp/`),
ADR-0055/0056 (the frozen IR is what a pin can point at)

## Context

tau can build and run a sealed artifact but has no ops verbs around it:
nothing records *which* build is the one this machine should run, nothing
diffs a proposed change against that record in review, and deployment to
even the local machine (cron/systemd) is hand-rolled. The 2026-08-29
framing audited the IaC field (Terraform/Pulumi/Argo) and the 2026-09-01
design locked an **ops-lane-first** v1: IaC semantics on the current
machine, before any fleet story. The IR makes this possible — pinning
unresolved config would pin sand; the pin points at a frozen, resolved,
content-hashed identity (design §2).

## Decision

1. **The machine is environment `local`** — implicit until a second
   environment exists (v2 `environments/promote` rides the same model).
   The pin lives in a **committed, secret-free**
   `.tau/envs/local.state.toml`.
2. **`tau plan`** — semantic diff in IR vocabulary: source-vs-pin,
   pin-vs-pin, `--check` for CI. Rendering rule (design §12 plan signal
   discipline): **capability changes always first**; only governance
   deltas may be loud. A versioned JSON twin lives in `schemas/plan/`
   (generated + drift-tested like `schemas/ir/`). **Exit codes:**
   `0` no change · `2` changes · `3` **widens capabilities** · `1` error.
   Exit 3 is the CI gate primitive (a PR that widens power fails a
   `tau plan --check` gate until a human approves).
3. **`tau apply`** — **atomic per repo**: one bundle, one pin, applied
   or not; `--pipeline` slicing is the explicit escape valve, not the
   default. Apply emits **substrate adapters** from `[trigger]`
   declarations — systemd-user timers first, k8s later — per ADR-0043's
   compile-the-trigger rule. Adapter retry-policy encoding is
   **deliberately dropped in v1** (a residual open item settled here:
   emitted units carry no `Restart=`/backoff policy beyond substrate
   defaults; the encoding folds into the v2 Time/trigger ADR, where
   retry semantics arrive with `Sleep`/`retry-catch`).
4. **Wasm bundles are run-or-refuse per environment.** Structural
   capabilities cannot be narrowed post-build; narrowing is
   host-sandbox-tier only; different capability profiles = different
   *declared* builds, pinned separately. Build-once-promote is preserved,
   never violated by mutation.
5. **`[[moved]]` records** (in `tau.toml`) declare renames. They drive
   both `tau plan` rendering (rename-not-replace) and checkpoint/journal
   remap on resume. A rename without a moved record is honestly a
   delete+add — loud in plan output.
6. **`tau record` / `tau replay` / `tau inspect`** complete the lane:
   journal views (ADR-0074) and the permission-sheet capability card
   (`tau inspect`, with `--attempt` to demonstrate denial — design §12).
7. **Provenance:** `tau-lock.toml` gains a `[synth]` section — SDK
   version, gen hash, fragment resolved SHAs — as **lockfile schema v8,
   additive**. State-file field list (residual open item settled here):
   the pin records `ir_hash`, `bundle_path`, `applied_at` (UTC),
   `ir_format`, `lockfile_hash`, per-trigger adapter unit names, and the
   `tau` version that applied — nothing secret, nothing host-specific
   beyond unit names; anything further is a state-format version bump.

## Consequences

- Review changes power-first in CI (`plan --check`, exit 3), pin what
  runs, apply atomically, and demonstrate denial — the maker's-loop v1
  center (design §10.1).
- The plan JSON twin becomes an integration surface for policy tooling
  (ADR-0077); its schema is a versioned contract from day one.
- Obligations: `schemas/plan/` generation + drift test; state-file
  parser with the additive-versioning discipline (lockfile precedent);
  systemd-user adapter emission + round-trip test; `[[moved]]`
  validation (unknown old-id = error); journal/checkpoint remap keyed by
  moved records (E-4).
- Deliberate v1 narrowness accepted: one environment, one machine,
  systemd-user only; no drift *detection* daemon (plan is on-demand);
  promote/fleet ride the same pin model in v2/v3 without rework.

## Alternatives considered

- **Terraform-style separate state backend + providers.** Rejected for
  v1: a state backend, locking, and provider protocol serve
  multi-operator fleets; the committed pin file gives single-repo
  ops the same review semantics at zero infrastructure.
- **Mutable narrowing of a built wasm bundle per environment.**
  Rejected: it breaks the artifact seal and the build-once-promote
  invariant; narrowed profiles are different declared builds.
- **Textual diff of IR JSON as `plan`.** Rejected: unreadable at
  capability granularity and unable to enforce the
  capability-changes-first discipline; the diff must be semantic, in IR
  vocabulary, with a stable JSON twin.
- **Apply per pipeline by default.** Rejected: shared vocabulary
  (agents/models/allow) makes partial applies a coherence hazard; the
  repo is the unit of truth (atomic), slicing is opt-in.

## References

- Design: [`2026-09-01-tau-authoring-ops-and-primitives-design.md`](../superpowers/specs/2026-09-01-tau-authoring-ops-and-primitives-design.md) §5 (ops lane), §2, §12
- Related: ADR-0043, ADR-0053/0074, ADR-0055/0056, ADR-0072 (provenance
  inputs), ADR-0073 (pipeline addressing)
- Epics: E-3 (plan/record/inspect), E-4 (pins/apply/moved/lockfile) in
  [`vision-roadmap.md`](../superpowers/plans/vision-roadmap.md)
