# Spec — EPIC 2.3: freeze the WIT host world + ports↔WIT drift test

**Story:** 2.3 (issue #385), milestone EPIC 2 — Lock the two contracts (public ABIs).
**Date:** 2026-06-22
**Accept:** ports↔WIT drift test green; the minimal 3-function host surface is frozen.
**Builds on:** ADR-0056 — the **embedding contract** is the WIT host world, versioned
by its WIT package version. This story locks and drift-guards that contract.

## Purpose

ADR-0055/0056 name the WIT host world as tau's second public contract and say it is
"generated from the ports, never hand-maintained, so it cannot drift." This story
makes that real — but via a **drift test**, not literal codegen, for a concrete
reason stated below. The 3-function host surface is frozen and provably kept in
correspondence with the `tau-ports` traits it projects.

## Context that constrains the design

- `wit/tau-run.wit` already exists (β.7.5 PR-C #362), hand-written:
  `package tau:run@0.1.0`, an `interface host` with exactly 3 functions, and a
  `world runner` that imports `host` and exports `run`.
- The 3 host functions project 3 `tau-ports` traits:

  | WIT host fn | signature | tau-ports trait |
  |---|---|---|
  | `complete` | `func(request-json: string) -> result<string, string>` | `llm::LlmBackend::complete` (JSON-serialized `CompletionRequest`/`Response`) |
  | `now-millis` | `func() -> u64` | `time::Clock::now` |
  | `next-u64` | `func() -> u64` | `random::RandomSource` |

- `crates/tau-wasm-guest/src/host_ports.rs` already implements `LlmBackend`/`Clock`/
  `RandomSource` on top of the WIT-generated host imports, and `tau-wasm-host`
  satisfies the same 3 imports. So a **signature** drift between the 3 WIT functions
  and those 3 ports already breaks compilation today, on both sides.
- `wit-bindgen 0.58` (WIT→Rust) and `wasmtime 45` are workspace deps; `wit-parser`
  is available transitively via `wit-bindgen`.

## Decision (Approach B — authored canonical `.wit` + parse-based drift/freeze test)

### Why a drift test, not codegen

ADR-0055 says "generated from the ports." But there is **no Rust-trait→WIT
generator** (wit-bindgen only goes WIT→Rust), and the boundary deliberately uses
`string` (serialized JSON) for `complete` precisely to avoid mapping rich Rust
types into WIT. Literal "parse the traits, emit WIT" is therefore both intractable
and unnecessary. "No drift" is delivered by a **test**, and the `.wit` stays the
clean, readable, canonical published artifact external embedders consume (via
wit-bindgen / jco). ADR-0055's "generated" wording is aspirational shorthand for
"provably non-drifting"; the 2.4 policy doc records this clarification (same move as
2.1 sharpening 0055's "+ ports API" phrasing). Package name/version stay
`tau:run@0.1.0` (ADR-0056's `tau:host` was illustrative of the *mechanism*, not a
naming mandate; `tau:run` accurately names a package that carries both the host
imports and the `run` export; 0.x is honest while the `run` export payload is
unsettled).

### Components

1. **The host-port registry (the single Rust declaration of the surface).** A small
   const in the test (or a `pub` item in `tau-wasm-host`) listing the host-crossing
   ports and their WIT function names:
   `&[("complete", "LlmBackend"), ("now-millis", "Clock"), ("next-u64", "RandomSource")]`.
   This is the Rust-side source the WIT is checked against.

2. **The drift/freeze test** (in `tau-wasm-host`, std; `wit-parser` as a dev-dep).
   It parses `wit/tau-run.wit` and asserts:
   - the `host` interface contains **exactly** the 3 functions named in the registry,
     with their exact signatures (params + result shape) — frozen;
   - `package tau:run@0.1.0` is present (the embedding contract version carrier);
   - the `runner` world imports `host` and exports `run` (presence only — the `run`
     payload is intentionally not frozen).
   Any add/remove/rename/signature-change to the host interface fails the test — the
   freeze is a deliberate tripwire forcing an intentional edit + review.

3. **Signature-drift guard (already present, documented here).** `host_ports.rs`'s
   `LlmBackend`/`Clock`/`RandomSource` impls over the WIT imports fail to compile if
   any of the 3 signatures stop matching their port. The test's doc comment points at
   this link so the two guards are understood as a pair.

### The honest residual gap (accepted)

Rust has no reflection, so the test **cannot** mechanically detect "a new
host-crossing port was added to `tau-ports` without a corresponding WIT function." The
guard against that is the freeze test as a tripwire (the frozen surface + registry
must be edited deliberately) plus design review — not a compiler proof. A stronger
guarantee would require generating `host_ports.rs` from a port registry; that is more
machinery than a 3-function surface warrants and is out of scope. 2.3 freezes and
cross-checks the *existing* surface tightly and makes *growing* it a conscious,
test-breaking act.

### Published-contract symmetry with 2.2

- A docs reference page `docs/reference/wit-host-world.md` (added to `SUMMARY.md`)
  documenting the embedding contract, the frozen 3-function surface, the port
  mapping, and the path to `1.0.0` (when the `run` export payload settles).
- A CI lane running the drift/freeze test, gated like 2.2's `schema-conformance`.

## Consequences / obligations

- New dev-dependency: `wit-parser` on `tau-wasm-host` (std; test-only).
- 2.4's policy doc records: the WIT package is `tau:run` (not `tau:host`); "generated"
  in ADR-0055 means "provably non-drifting"; the host surface is frozen, the `run`
  export is not; graduation to `1.0.0` is gated on the export payload settling.

## Out of scope (later stories)

- Freezing / settling the `run` export payload (a later β.7.5 story).
- Capability→WIT world generation at `tau build wasm` (EPIC 3.2).
- Renaming to `tau:host` / bumping to `1.0.0` (Q2 deferred both).
- Generating `host_ports.rs` from a port registry (the stronger ports-growth guard).
