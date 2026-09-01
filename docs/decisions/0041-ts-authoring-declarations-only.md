# ADR-0041: β.8 TS authoring surface — declarations-only via static AST analysis

> **AMENDED — SUPERSEDED IN PART (2026-09-01).** The **static-extraction
> decision** (swc AST analysis, `tau-ts-extract`, `.ts` file-extension
> dispatch, single-file constraint) is superseded by the **synth
> contract** (ADR-0072: sandboxed subprocess emitting ProjectConfig
> JSON), and the **surface scope** (TS factories spanning agents, tools,
> MCP) is superseded by the **three-surface split** (ADR-0071: TS
> authors choreography only). The δ.2 runtime-JS/QuickJS deferral is
> **killed outright** (never lands — design §8). What **carries
> forward**: the one-validation-path rule (merge before the single
> `validate()`, no parallel validation), snake_case parity with the TOML
> schema, and rejection of an embedded JS runtime in tau-cli.
> `tau-ts-extract` is deleted in Phase 1 (epic E-1).

**Status:** Accepted; superseded in part by ADR-0071 + ADR-0072 (see banner)
**Date:** 2026-06-10
**Supersedes:** none

## Context

The 2026-05-29 philosophy doc names TS as a sugar layer over the canonical
IR. The β.8 ROADMAP entry adds the `@tau/sdk` factory functions but is
ambiguous about whether TS code is statically analyzed or runtime-executed.

After β.7 (tau dev REPL) shipped, the choice becomes load-bearing for β.6
(conformance gate). Two interpretations are honest:

1. **Declarations-only via swc static AST analysis** — TS file is parsed,
   factory calls recognized as data, no JS execution. Tool bodies remain
   Rust-native (referenced via `native: "ReadTemp"` string).
2. **Full Vercel-DX feel** — embed a JS runtime (rquickjs / deno_core),
   execute the TS file, factories build IR objects at runtime, inline
   tool bodies (`run: async () => ...`) work in dev mode.

## Decision

Ship **declarations-only via swc static AST analysis** for β.8 v1. Defer
the runtime JS execution path (and inline tool bodies) to δ.2.

Specific decisions:
- New workspace crate `tau-ts-extract` does the TS → ProjectConfig
  conversion via swc 41.x.
- Snake_case fields throughout — matches TOML 1:1 to keep the
  conformance test (TOML↔TS byte-equal IR) simple.
- File-extension dispatch in `cmd/project_load.rs` —
  `.ts` → TS extractor; everything else → TOML.
- `contextManager` factory exists in the SDK shape but rejects at
  parse time with a `Deferred` error (β.4 prerequisite).
- Multi-file TS imports rejected with helpful hint; v1.1 work.
- The TS extractor uses a TOML serialization bridge internally:
  emit a TOML string from the AST, then call `ProjectConfig::parse_str`.
  This reuses ALL existing validation — no parallel validation path.

## Consequences

**Positive:**
- β.8 ships in ~2 hours, comparable to β.7's footprint.
- No embedded JS engine in tau-cli; ~3 MB swc dep is the only binary cost.
- TOML↔TS conformance is straightforward (same lower path, same canonical
  encoder, byte-equal check).
- β.7.5 (IR-to-wasm AOT) doesn't need to handle in-wasm JS execution.

**Negative:**
- Users expecting Vercel AI SDK-style `run: async () => ...` get a
  rejection at parse time. The error message points them at the
  `native:` reference pattern + notes the δ.2 plan.
- Multi-file projects must wait for v1.1 (single-file constraint).

## Alternatives considered

- **Embed rquickjs for runtime JS** — rejected for v1 because it adds
  ~600KB binary + a Rust↔JS capability bridge (significant scope). δ.2
  picks this up.
- **Subprocess to tsx / Node** — rejected because it requires external
  toolchain ("no toolchain required" promise from the philosophy doc).
- **CamelCase TS fields with auto-mapping to snake_case TOML** —
  rejected because the conformance test would need a canonicalization
  layer; snake_case-on-both-sides is simpler and consistent.

## References

- Spec: `docs/superpowers/specs/2026-06-10-beta-8-ts-authoring-design.md`
- Plan: `docs/superpowers/plans/2026-06-10-beta-8-ts-authoring.md`
- Philosophy: `docs/explanation/tau-philosophy.md` (TS sugar over IR)
- Related ADRs: 0037 (workflow IR), 0040 (β.7 tau dev REPL)
