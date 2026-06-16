# ADR-0051 — tau-ir crate split: a structural no_std boundary

**Status:** Accepted (β.7.5)
**Date:** 2026-06-16
**Supersedes / amends:** none. Complements ADR-0046 (wasm AOT).

## Context

β.7.5 requires `tau-runtime-core` (the executor-agnostic kernel) to
cross-compile to `wasm32-wasip2` so it can run inside the wasm guest.
`tau-runtime-core` declares `#![no_std]`, but it never actually built for
a no_std target — no CI lane exercised it — and had drifted.

Two leaks of `tau-pkg` (→ `tokio`, `fs4` → `rustix 0.38.44`, which uses
`feature(wasip2)` rejected on stable, plus tokio's `compile_error!` on
wasm) reached the kernel:

1. **Direct** — `tau-runtime-core` depended on `tau-pkg` for skill
   resolution. Removed by the `SkillResolver` port (PR-A): skill
   resolution is now a `tau-ports` port with a host-side
   `TauPkgSkillResolver` adapter, mirroring `CapabilityResolver`.
2. **Transitive** — `tau-pkg → tau-ir → tau-runtime-core`. `tau-ir`'s
   **default** feature `with-std-adapters` gated a `lower` module that
   depends on `tau-pkg`. Member-level `default-features = false` on the
   inherited workspace dep did **not** reliably suppress it (feature
   unification), and `tau-ir` could not compile `--no-default-features`
   at all (`IrError::McpBuild` referenced the gated `lower` module
   unconditionally).

## Decision

Split `tau-ir` along a **crate boundary**, not a feature flag:

- **`tau-ir`** — pure `#![no_std] + alloc`. IR types, `check`, `context`,
  `pipeline`, `template`, `canonical`/`hash`, `message` (std adapters
  behind an opt-in, **non-default** `with-std-adapters` that no longer
  pulls `tau-pkg`). Builds for `wasm32-wasip2`.
- **`tau-ir-lower`** — std-side. The `lower` pass
  (`tau_pkg::ProjectConfig → IrModule`) and its `LowerError`. Linked by
  `tau-cli`, `tau-pkg`, `tau-conformance`, `tau-ir-conformance`,
  `tau-ts-extract`.

`tau-runtime-core` depends only on pure `tau-ir` (default-features
off) and now cross-compiles to `wasm32-wasip2`.

**Why a crate boundary, not a feature flag:** a feature is an interface
the build resolver can silently violate — exactly what happened with
`default-features = false` under unification. A crate boundary is one it
cannot. The kernel's std-freedom must be *structural*, matching tau's
"any check that could run at build time must run at build time"
discipline. A CI guard (`runtime-core-no-std` job) builds
`tau-runtime-core` for `wasm32-wasip2` so the boundary cannot re-drift.

## Consequences

- The kernel is genuinely substrate-agnostic — the precondition for the
  β.6 "one engine, N profiles" conformance claim.
- `IrError` stays in `tau-ir` (the pure `check` pass raises most of its
  variants); only the lowering-specific `McpBuild` variant moved to
  `tau-ir-lower::LowerError`.
- Consumers of lowering import `tau-ir-lower` instead of `tau_ir::lower`.
- One more workspace crate (38 members).
