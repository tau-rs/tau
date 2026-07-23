# ADR-0065: Unknown-input policy — strict authoring surfaces, versioned interchange

**Status:** Accepted
**Date:** 2026-07-19
**Deciders:** tau core

## Context

An audit of the frontend (D7-B, audit B2) found **five inconsistent behaviors**
for unknown or malformed input across tau's deserialization surfaces:

1. A typo'd capability kind silently became `Capability::Custom` in package
   manifests, but was rejected in `[allow]`.
2. A wrong field on a known capability kind was silently dropped
   (`unwrap_or_default` conflation).
3. `#[serde(deny_unknown_fields)]` was present on some leaf structs but absent
   on the top-level authoring structs (`UncheckedProjectConfig`,
   `UncheckedAgent`, `UncheckedManifest`, …), so unknown top-level keys were
   silently ignored.
4. Lowering mapped an unknown `PromptEntry` to an **empty prompt** and an
   unknown determinism string to `Pure` — the most permissive class.
5. `[allow]` maintained its own capability-kind whitelist, free to drift from
   the manifest parser.

These are all instances of the same anti-pattern: **silently accepting input
tau does not understand**, which contradicts tau's build-time-enforcement
philosophy (a check that *can* run at build time *must*).

## Decision

**One rule, split by who writes the input.**

- **AUTHORING** (humans: `tau.toml`, root `[allow]`, package manifests) →
  unknown input is an **error**.
- **INTERCHANGE** (machines: lockfile, bundle, IR) → **version-gated
  acceptance**. Within an accepted format version, unknown fields are still
  rejected (canonical, hashed formats have no slack); a *newer* declared
  version is an explicit upgrade error, never silent dropping.

This ADR is written whole, but implemented in halves:

- **Authoring half** — implemented across D7-B PR1–PR3 (this work).
- **Interchange half** — the version-gate-at-load for lockfile/bundle/IR — is
  **D8's** handoff. It is *not* implemented here; this ADR states the rule so
  D8 has a single reference.

### Authoring half — what shipped (D7-B)

**Struct strictness (PR1).** `#[serde(deny_unknown_fields)]` on every
non-`flatten` authoring struct. Structs that use `#[serde(flatten)]`
(`UncheckedAllow`, `UncheckedToolAllow`) cannot use `deny_unknown_fields`;
their strictness stays in `validate_allow`.

**Field-shape strictness (PR1).** The hand-written `Capability` deserializer no
longer conflates fields: each kind requires exactly its fields and rejects
foreign or unknown fields, naming the offender and the expected shape.
`net.http` requires `hosts` — either a non-empty list, or the any-host escape
hatch `hosts = "any"` (typed `NetHosts { Any, List }`); the bare
`{ kind = "net.http" }` form, which previously meant deny-all, is now an error.

**Kind strictness + explicit `custom.` (PR2).** An unrecognized capability kind
is an error with a Levenshtein did-you-mean and a pointer to the escape-hatch
docs. A plugin-defined capability must opt in *explicitly* via a `custom.`
kind prefix → [`Capability::Custom`](../explanation/escape-hatches.md#capability-custom).
`[allow]` and package manifests share the **single** capability parser
(Decision 4-A): typo/did-you-mean/field-shape live in one place, and `[allow]`'s
deliberately narrower ceiling kind-set is a *post-parse* semantic gate — "one
parser" ≠ "same accepted kinds".

**Forward-vocabulary opt-in (PR2).** A package manifest may declare a top-level
`vocab_version` (a monotonic `u32` generation). Only when it is *newer* than
this build's `KNOWN_VOCAB` do unrecognized (non-`custom.`) kinds parse — as a
distinct, fail-closed [`Capability::Forward`](../explanation/escape-hatches.md#capability-forward)
variant (subsumes nothing, subsumed by nothing but an exact match / an
`Any` ceiling), surfaced by `tau check` as an info finding. An undeclared
`vocab_version` means the current vocabulary, strict. This preserves the
Phase-2 §D forward-compatibility path: an older tau can inspect/install a
newer-vocab plugin fail-closed instead of hard-failing. Package manifests are
the only authored artifact that travels between tau versions, so they alone get
this opt-in; the root `tau.toml`/`[allow]` stay strict-current with no escape.

**Lowering arms (PR3).** The two silent lowering defaults become structured
errors (`LowerError::UnsupportedPromptKind`, `LowerError::UnknownDeterminism`),
mirrored into the drifting `tau-ir` `IrError` copy (D11 consolidation debt).

### Interchange half — D8 (not implemented here)

The lockfile/bundle/IR readers gain a **version gate at load**: a known format
version parses strictly (unknown fields rejected); a newer format version is an
explicit "upgrade tau" error. See D8's handoff.

## Consequences

- **Positive:** every unknown input on an authoring surface now fails loudly at
  build time with an actionable message; `[allow]` and manifests can no longer
  diverge on kind handling; forward-compat is explicit and fail-closed;
  any-host egress is a typed, honest declaration rather than a magic empty list.
- **Negative / migration:** existing manifests/fixtures using the bare
  `net.http` form, an unprefixed custom kind, or an unknown top-level key now
  fail to parse and were migrated. The `NetHosts` change rippled through every
  sandbox adapter (each now folds capabilities into a proxy `HostPolicy` where
  `Any` = unrestricted egress).
- The former non-namespaced-`Custom` *warning* is obsolete — a `Custom`
  capability is namespaced by construction (`custom.` prefix), enforced at parse.

## Alternatives considered

- **Keep the permissive Custom fallback for unknown kinds.** Rejected: it is the
  exact silent-drop the audit flagged, and it let typos reach downstream
  governance errors instead of failing at the typo.
- **Represent any-host with a `["*"]` sentinel** rather than a typed `NetHosts`.
  Rejected: the three enforcement layers disagreed on `"*"`, and a sentinel
  breaks the lattice's own subset check; a typed `Any` is correct-by-construction.
- **Fully collapse `[allow]` into the manifest parser.** Rejected: it would
  widen `[allow]` to accept `agent.spawn`/`task_list`/`custom.*` as ceilings,
  a semantic expansion with no defined meaning; the post-parse gate preserves
  `[allow]`'s scope.
