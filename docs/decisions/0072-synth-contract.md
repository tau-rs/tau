# ADR-0072: The synth contract — subprocess synthesis emitting ProjectConfig JSON

**Status:** Accepted (records locked decision §10.4 of the
[2026-09-01 consolidated design](../superpowers/specs/2026-09-01-tau-authoring-ops-and-primitives-design.md);
Phase 0 ADR wave)
**Date:** 2026-09-01
**Deciders:** maintainer, via the 2026-09-01 brainstorm session
**Supersedes:** ADR-0041's static-extraction decision (swc AST analysis).
**Preserves from ADR-0041:** the one-validation-path decision (the TOML
serialization bridge's principle: every surface merges *before* the
single `validate()`), snake_case field parity with the TOML schema, and
"no parallel validation path" as a hard rule. The superseded parts are
the extraction mechanism (static AST → subprocess execution), the
file-extension dispatch (`.ts` → extractor), and the single-file
constraint.

## Context

ADR-0041 shipped declarations-only TS authoring via swc static AST
analysis: factory calls recognized as data, no JS execution. That was the
right v1 cut, but it caps the surface at object literals — no functions,
no fragments, no parameterized composition — and the 2026-08-29 framing
audit showed every stretched-static-extraction system (early CDK
prototypes, Pulumi's abandoned static mode) eventually executes the
program anyway. With ADR-0071 making TS the choreography surface,
pipelines need real composition: loops that stamp out steps, fragments
with props, imports between pipeline files.

Executing user TS raises the questions this contract answers: what runs
it, under what authority, and what stops a hostile synth program from
smuggling capability grants.

## Decision

1. **`tau.toml` stays the root.** A `[synth]` table declares
   `entry` (the program) and `format` (the interchange version). No
   `[synth]` table = no synth step (TOML-only projects pay nothing).
2. **Subprocess, not embedded runtime.** tau spawns the entry — Node/tsx
   by default, runner overridable — as a child process. The contract is
   language-agnostic: any program that emits the JSON is a valid
   frontend (Starlark stays reserved as a possible future one, §10.4).
3. **Sandboxed synthesis.** The subprocess runs under
   `tau-sandbox-native`: no network, filesystem read-only within the
   project root. Synthesis is a pure function of the repo.
4. **Output = canonical `ProjectConfig`-shaped JSON on stdout**, gated by
   `synth_format`. Strictness per ADR-0065: the synth output is an
   **authored surface** (reject unknown fields outright) even though it
   is version-gated like interchange — the author controls both ends, so
   silent field-dropping is a bug factory, not compatibility. This is
   the synth JSON strictness ruling the handoff assigned to this wave.
5. **Merge at the unchecked level, one validator.** Synth output merges
   into `UncheckedProjectConfig` exactly where `[dirs]` definitions merge
   (`ProjectConfig::parse_str_at`, ADR-0069 discipline), *before* the
   single `validate()`. Collisions with TOML-declared facts are hard
   errors, never overrides. Governance is never delegated to the SDK: a
   hostile synth program can only emit config that `tau check` fully
   re-validates against the TOML-only `[allow]` (ADR-0071).
6. **Hermeticity is checked, not assumed.** CI runs synth twice and
   requires byte-identical output (the double-synth check). Divergence
   fails the build.
7. **`tau.gen.ts`** — typed bindings for agents, models, tools,
   deterministic fns, and agent kinds — is generated inside
   `tau dev`/`tau build`, committed, and **registry-content-hash-stamped**:
   a stale gen is a loud build error (the anti-Prisma rule), never a
   silently-wrong autocomplete.
8. **Source-mapped synth errors are a v1 requirement** (design §12):
   synth failures point at `file:line` in the author's TS, never at JSON
   pointers in the emitted config.

## Consequences

- The TS surface gains real composition (fragments `(scope, id, props)`,
  imports, loops) with zero engine exposure — TS still never runs at
  runtime.
- ADR-0041's subprocess rejection ("requires external toolchain,
  breaking the no-toolchain promise") is **reversed with argument**: the
  no-toolchain promise now holds rung-by-rung — TOML-only projects never
  need Node; authors who opt into `pipelines/*.ts` already have a TS
  toolchain by definition. Progressive disclosure, not a global promise.
- `tau-ts-extract` (swc, static factories) is deleted in Phase 1 once
  the runner lands; the TOML-bridge pattern is harvested as the
  merge-at-unchecked-level discipline.
- New obligations: `schemas/project-manifest/` published + drift-tested
  (E-1) so L1 factories are generated, not hand-maintained; the sandbox
  port for synth (`crates/tau-pkg/src/install_sandbox.rs` precedent);
  double-synth CI job (E-2); lockfile v8 `[synth]` provenance
  (ADR-0075/E-4).
- Exact `[synth]` field list (a residual open item): `entry` (string,
  required), `format` (string semver, required), `runner` (string array,
  optional — defaults to the tsx invocation). Anything further is a
  format bump.

## Alternatives considered

- **Keep stretching static extraction.** Rejected: every added TS
  feature (a `.map()`, a helper fn, an import) becomes a parser feature;
  the AST interpreter converges on a JS engine written by hand, with
  worse errors.
- **Embedded JS runtime (rquickjs / deno_core) in tau-cli.** Rejected:
  binds the contract to one language, adds a Rust↔JS capability bridge
  inside the trusted binary, and couples engine releases to JS-runtime
  CVEs. The subprocess boundary is also the sandbox boundary.
- **Trust the SDK to validate (typed SDK = the contract).** Rejected:
  governance delegated to author-side code is not governance; the single
  Rust validator is the only path to `tau check`'s guarantees.
- **Lenient synth JSON (ignore unknown fields).** Rejected per
  ADR-0065's authored-surface rule; version negotiation happens through
  `synth_format`, not through silent dropping.

## References

- Design: [`2026-09-01-tau-authoring-ops-and-primitives-design.md`](../superpowers/specs/2026-09-01-tau-authoring-ops-and-primitives-design.md) §1 (synth contract), §4 (TS API), §12
- Related: ADR-0041 (superseded in part), ADR-0065 (strictness),
  ADR-0069 (merge discipline), ADR-0071 (surface roles), ADR-0073 (IR v3)
- Epics: E-1 (gen + schema), E-2 (runner) in
  [`vision-roadmap.md`](../superpowers/plans/vision-roadmap.md)
