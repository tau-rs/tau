# ADR-0037: Workflow IR — typed, content-hashed, phased lowering

**Status:** Accepted

**Date:** 2026-06-01

**Deciders:** titouanlebocq

**Spec:** [`docs/superpowers/specs/2026-05-31-workflow-ir-design.md`](../superpowers/specs/2026-05-31-workflow-ir-design.md).

## Context

ROADMAP §β.2 — the workflow IR is tau's compiler thesis made concrete.
Phase α.1 (Framing D) enumerated D-1 through D-7b; the design spec settled
each. This ADR records the binding decisions for durability.

## Decision

- **D-1:** Typed full node taxonomy (Agent + Tool + Deterministic +
  Subflow).
- **D-2:** New `tau_ir::Message` + bidirectional adapter to `tau_domain::Message`.
- **D-3:** WASI + tau custom-section capability lowering.
- **D-3b:** Strict build-time refusal, no override.
- **D-4:** Monolithic component per workflow.
- **D-5:** Phased: interpret v0 (β.2), AOT v1 (β.7/γ.x).
- **D-6:** Per-target hashing; `ir_format` separate from `tau_version`.
- **D-7a:** Multiset side-effect conformance.
- **D-7b:** ~6 fixtures.

## Consequences

- `tau-ir` is a new `no_std` + `alloc` crate alongside `tau-runtime-core`.
- `tau_ir::IrModule` + `tau_ir::Workflow` + `tau_ir::Node` + variants
  (Agent, Tool, Deterministic, Subflow) form the canonical IR shape.
- `BundleManifest::schema_version` bumped 2 → 3 with the new `ir_payload`
  field; legacy v1 and v2 bundles still parse and run.
- `tau verify --bundle` extends to compare IR canonical bytes via
  `tau_ir::canonical_bytes()` + SHA-256 hash.
- The conformance suite in `crates/tau-ir-conformance/` becomes a permanent
  CI gate (β.6 lane). **v0 ships scaffold + 1 fixture; remaining 5 fixtures +
  bundle_mode runner deferred to β.2.6.1** once `tau run --bundle` interpreter
  dispatch is unstubbed (from β.2.5).
- `tau run --bundle` parses `ir_payload` and logs the entry agent, but
  **interpreter dispatch is deferred to β.2.6.1**.
- The IR is the canonical intermediate form; `tau.toml` and `workflows/*.toml`
  are source dialects that lower into it. Round-trip determinism is enforced:
  the same manifest re-lowered must produce identical IR bytes.

## Open questions deferred to future ADRs

- AOT codegen design (β.7).
- TS sugar surface (β.8 / δ.2).
- Multi-workflow composition (`SubflowKind::Compose`).
- Multi-format reader policy (whether tau N reads bundles from N-1).
