# ADR-0059: ir_format acceptance window + walked feature-fit

**Status:** Proposed
**Date:** 2026-07-19
**Deciders:** tau maintainers

## Context

`ir_format` is carried on every `IrModule` but never enforced. Two concrete holes exist,
both verified against `main @ d678802d`:

1. **Silent forward-incompatibility.** `from_canonical_bytes`
   (`crates/tau-ir/src/canonical.rs:31`) is a plain `serde_json::from_slice`. `IrModule`
   and its nested types do not use `deny_unknown_fields` (the only use in the crate is
   `budget.rs:12`). So a newer runtime's semantics-bearing optional field — exactly what
   `durable` once was (ADR-0053) — is silently dropped when decoded by an older `tau`.
   The CLI bundle path only logs `ir_format`, and only on `--dry-run`
   (`crates/tau-cli/src/cmd/run.rs:118-123`); the wasm guest never inspects it
   (`crates/tau-wasm-guest/src/guest.rs:104`). Nothing rejects the mismatch; the older
   runtime proceeds on an incomplete picture of the module's semantics.

2. **Published-but-unrunnable schema.** `ir_format` v2.4.0 (ADR-0058) publishes
   `Branch`/`Parallel`/`Loop`/`Suspend`, but no backend executes them: the interpreter
   returns `RuntimeError::Internal` on all four
   (`crates/tau-runtime-core/src/interpreter/pipeline.rs:316-347`), and the wasm guest
   supports only single-agent, no-pipeline modules (`guest.rs:106-119`). An external
   producer targeting the published JSON Schema can therefore build a module that is
   valid by the schema but unrunnable by every backend, and only discovers this mid-run.

This is the interchange half of the build-time capability policy: `ir_format` versioning
is a wire/decode-level compatibility contract (ADR-0056), while feature-fit is a
build/load-level capability contract (in the spirit of `capability_fit.rs`, ADR-0036).
There is no separate ADR filed for the feature-fit half; this ADR covers both.

## Decision

### Interchange half (PR1, shipped on branch `feat/ir-format-acceptance-window`)

`from_canonical_bytes` becomes a two-phase closed decode:

1. **Peek `ir_format` only.** A minimal partial-decode struct (no
   `deny_unknown_fields`) extracts just the `ir_format` string, so an unknown field
   introduced by a newer minor version does not mask the version check behind a generic
   serde error.
2. **Apply the semver acceptance window.** Accept iff
   `major == CURRENT.major ∧ minor ≤ CURRENT.minor`:
   - a newer minor (`found.minor > CURRENT.minor`, same major) → `DecodeError::FormatTooNew`
   - a different major → `DecodeError::FormatMajorMismatch`
   - missing or unparseable `ir_format` → `DecodeError::BadFormat`
3. **Closed full decode.** Only once the module falls inside the accepted window,
   decode the whole `IrModule` with `#[serde(deny_unknown_fields)]` applied across the
   full IR type tree (`IrModule`, `Workflow`, and nested step/pipeline/check/trigger
   types). An unknown field surviving into an accepted window means the module is
   corrupt or lying about its own format, and is rejected (`DecodeError::Serde`).

`CURRENT` is `v2.4.0` (ADR-0058's version). The gate is wired at every
`from_canonical_bytes` call site: CLI `run --bundle`, the wasm guest's `Err(string)` arm,
and the conformance loader.

### Feature-fit half (PR2, stacked follow-up on `feat/ir-feature-fit`)

A walked `required_features(&IrModule) -> BTreeSet<IrFeature>` — derived by recursing the
module's actual structure, not by reading a declared list carried in the module — is
checked against a backend's supported-feature set at two points:

- **BUILD**, in `tau-ir-lower`: after lowering, `required_features(module)` must be a
  subset of the target's supported features, resolved target-aware via
  `tau_ports::target::registry`. This is strict, with no override flag, mirroring the
  existing `capability_fit` precedent (ADR-0036).
- **LOAD**, at the single `tau-runtime-core` interpreter chokepoint that both the native
  CLI and the wasm guest funnel through: `required_features(module)` must be a subset of
  `SUPPORTED_FEATURES`, else a structured load error, not a mid-run
  `RuntimeError::Internal`.

As shipped in PR2: the LOAD error is `RuntimeError::UnsupportedFeature { features }`, and
the mid-run `RuntimeError::Internal` control-flow arms in `pipeline.rs` are kept as
defense-in-depth (now unreachable from any gated path). Build and load read one shared
table, `tau_ir::feature::backend_features(AdapterFamily)`, which returns the same
interpreter set for every adapter family today; the interpreter's `SUPPORTED_FEATURES`
const is tied to that table by a drift-guard test, and a `wasm32-wasip2` round-trip test
proves the guest rejects an unsupported-feature module across the WIT boundary. Because
`AdapterFamily` is `#[non_exhaustive]` in `tau-ports`, `backend_features` keeps an explicit
arm per known family plus a documented wildcard — a family added upstream lands on the
wildcard (same set) rather than failing to compile, so the obligation to update the sets
travels with the code review, not the type checker.

Both halves share one ADR (Decision 3a below).

### Locked decisions

- **1a — no diagnostic-code prefixes.** Errors are `thiserror` variants with prose
  messages (`DecodeError::FormatTooNew { found, supported_up_to }`, etc.), matching every
  existing `IrError` / `LowerError`. No `error[IR001]`-style bracket-code convention is
  introduced — the repo has none today, and a one-off namespace for this feature alone
  would be inconsistent.
- **2a — wasm guest error surface unchanged.** The guest keeps returning
  `result<string, string>` across the WIT boundary (`wit/tau-host.wit:26` is unchanged).
  A rejection is the `Display` string of the underlying error. No structured WIT error,
  no ABI churn.
- **3a — two stacked PRs.** PR1 (this branch: version gate + closed decode) is
  independently mergeable and valuable on its own — it closes hole 1 outright. PR2
  (feature-fit, `feat/ir-feature-fit`) stacks on PR1 and closes hole 2. ADR-0059 and the
  conformance README refresh land with PR1; this ADR documents both halves so the
  decision record is not split across two documents for one design.

## Design finding (from implementation)

The version gate is load-bearing at the wasm guest and at any direct
`from_canonical_bytes` consumer. On the CLI `tau run --bundle` path, however, it is
pre-empted by an earlier check: that path re-lowers the live cwd source and cross-checks
canonical-byte hashes (which include `ir_format`) *before* ever decoding the bundle's IR
as JSON, so a forward-incompatible bundle is caught as source divergence first, not by
the decode gate. `tau verify --bundle` never decodes IR as JSON at all — it only re-hashes
bytes — so it needs no gate at all. The decode gate is therefore the primary defense for
the wasm guest and any bundle-only consumer, and a secondary defense (unreachable in
practice, but still correct) on the `run --bundle` path.

## Key finding on backends

There is effectively one execution backend today: the `tau-runtime-core` interpreter,
which the wasm guest reuses rather than reimplementing (the guest's single-agent,
no-pipeline limit is a workflow-shape constraint orthogonal to feature support, not a
second backend with its own feature set). Consequently PR2 needs one
`SUPPORTED_FEATURES` set and one load-time enforcement chokepoint, not two parallel
sets that could drift from each other.

## Consequences

**Positive:**

- A newer-but-incompatible bundle is rejected at decode with a clear message, instead of
  silently dropping semantics-bearing fields (closes hole 1).
- `deny_unknown_fields` now spans the full `tau-ir`-owned IR type tree, turning "unknown
  field" from a silently-ignored condition into a hard decode error within an accepted
  window (see the known gap below for the one out-of-crate subtree not yet covered).
- Feature support in PR2 is derived by walking the module (ground truth), not by trusting
  a declared list that could lie or drift.

**Negative / obligations:**

- New conformance fixtures are required: unknown top-level field, unknown nested field,
  `ir_format` minor+1, `ir_format` major+1 — all rejected.
- `schemas/ir/conformance/README.md` is refreshed to `v2.4.0` (it was stale at v2.3.0
  while the schema and tests were already v2.4.0).
- **Known gap — the `tau_domain::Capability` subtree.** `Capability` (reached from
  `IrModule` via `CapabilityRequirements.declared`) is deserialized by a hand-written impl
  (`capability.rs`) over a `RawCapability` struct that uses `#[serde(flatten)] rest:
  BTreeMap` to preserve unknown keys as `Custom`-capability params. That flatten catch-all
  means unknown keys on the *known* capability kinds (`fs.read`, `net.http`, …) are
  currently absorbed rather than rejected, so the closed decode stops at the `tau-ir` crate
  boundary. The exposure is bounded — the version window already blocks cross-version minor
  field-drop, and absorbed keys are dropped, never honored (no privilege escalation) — but
  a hand-crafted same-version module can still carry stray keys inside a capability object.
  Closing this requires distinguishing known-kind (reject unknown keys) from the
  `Custom`-kind fallback (preserve them), a focused change to the security-critical
  sandbox-grant deserializer with `tau.toml` manifest blast radius; it is tracked as a
  follow-up rather than rushed into this PR.
- **Schema tightening without a version bump.** Regenerating `tau-ir.v2.4.0.schema.json`
  with `additionalProperties: false` across the tree flips the published schema from open
  to closed for external producers on an unchanged `$id`/version. Although ADR-0056 treats
  the published schema as part of the semver stability surface, this does *not* warrant an
  `ir_format` bump: emitted canonical bytes are unchanged (decode-side tightening only —
  `to_canonical_bytes` and all round-trip goldens are untouched), the schema was previously
  under-specified (silently open), and the acceptance window rejects newer minors
  regardless.
- Forward-looking note: `#[serde(untagged)]` types (e.g. a future `PromptSource` from D6-B)
  interact with `deny_unknown_fields` in non-obvious ways — untagged enums try each arm in
  order, and `deny_unknown_fields` on the arms changes which arm matches an ambiguous
  input, so each untagged arm would need an explicit accept and reject test. No `untagged`
  or `flatten` types exist in `tau-ir/src` today, so this is a caution for when such a type
  lands, not a current requirement.
- Whoever lands execution for `Branch`/`Parallel`/`Loop`/`Suspend` (the EPIC 4.2
  interpreter work — issue #399, now closed, though execution is not yet on `main`: the
  `pipeline.rs` control-flow arms still return `RuntimeError::Internal`) MUST add those
  variants to both `SUPPORTED_FEATURES` (in `tau-runtime-core`) and `backend_features` (in
  `tau-ir`) at the same time. Two shipped tests guard this: the drift-guard
  (`supported_features_matches_shared_table`) fails if the const and the shared table are
  updated one-sidedly, and the load honesty test
  (`branch_module_rejected_at_load_not_mid_run`) fails the moment a Branch module becomes
  executable, forcing whoever adds execution to update the feature sets rather than let
  them silently drift from what the interpreter actually runs.

## Alternatives considered

**A. `error[IRxxx]` diagnostic-code prefixes.** Rejected (Decision 1a). No such
convention exists anywhere in the codebase's error types; introducing one for this
feature alone would be a one-off namespace inconsistent with every existing `IrError` /
`LowerError` variant, which use plain prose `thiserror` messages.

**B. Structured WIT error for the wasm guest.** Rejected (Decision 2a). The guest's
`result<string, string>` boundary already gives a clear, catchable rejection via the
`Display` string. A structured WIT error type would require ABI churn (WIT world change,
host-embedder updates) disproportionate to the goal of a clear rejection message.

**C. A declared feature list carried in the module.** Rejected. A module could declare a
feature list that lies about or drifts from what it actually uses; walking the module's
actual structure to derive `required_features` is ground truth and cannot drift from
what the module contains.

**D. Single combined PR for both halves.** Rejected (Decision 3a). The interchange half
(version gate + closed decode) is independently valuable and mergeable on its own, and
closes hole 1 without waiting on the larger feature-fit design. Splitting into stacked
PRs keeps blast radius small per PR while still landing one ADR that documents the full
design.

## Cross-references

- ADR-0058 — IR structured control-flow blocks (`Branch`/`Parallel`/`Loop`/`Suspend`,
  `ir_format` v2.4.0) — the schema surface this ADR gates.
- ADR-0056 — The two contracts are the semver stability surface — the versioning
  convention this ADR's acceptance window enforces.
- ADR-0036 — Capability vocabulary forward-compatibility — precedent for
  `capability_fit`-style strict subset checks, mirrored by the feature-fit half.
- Design spec: `docs/superpowers/specs/2026-07-19-ir-format-acceptance-window-design.md`
- EPIC 4.2 (#399) — lands execution for the four control-flow variants; must flip
  `SUPPORTED_FEATURES` once it does.
