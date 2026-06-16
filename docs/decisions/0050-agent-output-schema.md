# ADR-0050: Agent `output_schema` — additive field plumbing (IR v1.3.0)

**Status:** Accepted
**Date:** 2026-06-16
**Deciders:** Titouan (architect), implementing session

## Context

Deterministic steps already carry an `output_schema: Option<serde_json::Value>`
field (introduced in the deliverables-and-goals track). Agents did not.

A downstream judge-compatibility check — planned for a later task — needs to
cross-reference a deliverable's judge agent against a declared output schema:
the judge must produce output that conforms to the schema the deliverable
expects. Without an `output_schema` field on the agent, that check has no
schema to compare against; the check would be forced to defer to runtime, which
conflicts with tau's Rust-like build-time enforcement principle (any check that
_could_ run at build time _must_ run at build time).

This ADR records only the additive field plumbing — not the downstream
judge-compatibility check itself.

## Decision

Add `output_schema: Option<serde_json::Value>` to three layers, mirroring
the existing `[steps.*].output_schema` pattern:

1. **`tau-pkg`** — `UncheckedAgent` / `AgentEntry`: a new optional TOML key
   `output_schema`, deserialized as a raw JSON-compatible value. No deep
   JSON-schema validation at this layer; the field is stored verbatim.

2. **`tau-ir` IR node** — `Agent`: a new optional field
   `output_schema: Option<serde_json::Value>` gated with
   `#[serde(default, skip_serializing_if = "Option::is_none")]`.
   The lowering stage (`lower_project`) threads the value from `AgentEntry`
   into the IR node with no transformation. The IR format version is bumped
   `v1.2.0 → v1.3.0` — a MINOR/additive bump per the ADR-0006 / `module.rs`
   semver discipline (MAJOR = breaking, MINOR = additive optional field,
   PATCH = spec-only). A v1.2.0 reader ignores the absent-when-`None` key; a
   v1.3.0 reader handles both. All pre-existing conformance fixtures' canonical
   bytes are unchanged (verified by the full `tau-ir-conformance` suite, which
   runs fixtures 01–14 after this change with 28/28 passing).

3. **`tau-ts-extract`** — `outputSchema` extraction: static AST analysis
   of `.ts` project files extracts the `outputSchema` field from agent
   declaration objects and includes it in the extracted `UncheckedAgent`,
   matching the TOML surface.

All three layers are pass-through — no deep validation, no runtime behavior
change.

## Consequences

**Positive:**

- Unblocks the downstream judge-compatibility build-time check without
  requiring another round of field-plumbing at that point.
- Consistent surface: TOML, IR, and TS authoring now all carry
  `output_schema` on both steps and agents.
- IR version bump is MINOR/additive, so existing bundles (v1.2.0) continue
  to load correctly in a v1.3.0 reader with no migration.

**Neutral / obligations:**

- `tau_ir::node::Agent` **keeps its `Eq` derive** — `serde_json::Value`
  implements `Eq`, so `Option<serde_json::Value>` does not force dropping
  `Eq`. No public-API regression.
- The field is inert at runtime until the downstream judge-compat task
  consumes it. Tests confirm it round-trips but do not assert on its
  semantic effect (there is none yet).
- Conformance fixture 14 (`14_agent_output_schema`) is the acceptance test:
  it mirrors fixture 01 (agent + one native tool, two turns) with an
  `output_schema` added to the agent, and asserts DevMode + BundleMode
  cross-mode equivalence, proving the schema survives the canonical
  encode/decode cycle unchanged.

**No negative consequences identified.**

## Alternatives considered

- **Validate the JSON schema value at parse time** (e.g., check that it is a
  valid JSON Schema object). Rejected for this phase: validation adds a
  `jsonschema` / `boon` dependency at the `tau-pkg` parse layer and introduces
  a new error variant consumers must handle. The downstream judge-compat check
  will know exactly what shape it requires; general schema validation is
  deferred until that check is specified. Mirroring the existing step
  `output_schema` precedent (which is also unvalidated at parse time) keeps the
  two surfaces consistent.

- **Represent `output_schema` as a typed `JsonSchema` struct rather than
  `serde_json::Value`**. Rejected: no agreed-upon schema-validation AST
  exists in the codebase today, and defining one is out of scope for an
  additive field that is currently inert. `serde_json::Value` is the
  existing convention for unvalidated JSON blobs at this layer (see
  `[steps.*].output_schema`).

- **Defer the field entirely until the judge-compat task is specified**.
  Rejected: the judge-compat task explicitly needs this field to be already
  present in the TOML surface, IR, and TS surface — adding it mid-spec would
  widen that PR unnecessarily. The field is small, additive, and its plumbing
  is independently testable (fixture 14).
