# Design: widening the `AgentId` grammar for namespaced agent names

**Date:** 2026-08-29
**Issue:** [#715](https://github.com/tau-rs/tau/issues/715)
**Decision record:** [ADR-0070](../../decisions/0070-agent-id-grammar.md)

The rationale — why widen rather than sanitize, why `PackageName` stays put,
why the scanner defers to the domain type — lives in ADR-0070. This spec
records the implementation surface, the test plan, and the sequencing.

## Problem

`tau_domain::AgentId`'s grammar (`[a-z]` then `[a-z0-9-]`, 1..=64 bytes)
rejects both `/` and `_`. The `[dirs]` scanner's grammar (`[a-z0-9_-]+` per
`/`-joined segment) accepts both. Three failures follow:

| # | surface | symptom |
|---|---|---|
| 1 | `bundle/build.rs:182` | `tau build` exits 2 — "agent `review/strict` has invalid id" |
| 2 | `cmd/resolve_helpers.rs:34,68` | `tau resolve` **panics** on `expect("AgentId from validated entry")` |
| 3 | `interpreter/agent_loop.rs:504-515` | silent `unwrap_or_else` collapse to a phantom `ir-agent`, losing token attribution (#538) and the run span (#731) |

Failure 3 is undocumented in the issue and is the same defect #734 fixed for
generated ids. Failures 1 and 2 also reproduce on inline
`[agents."review/strict"]`, so this is not a `[dirs]`-only bug.

## Target grammar

```
AgentId  := segment ( '/' segment )*        1..=64 bytes total
segment  := [a-z0-9] [a-z0-9_-]*
```

| input | verdict |
|---|---|
| `researcher`, `agent-123` | accept (unchanged) |
| `review/strict`, `perf/strict`, `a/b/c` | accept (new) |
| `my_agent` | accept (new) |
| `2fa`, `2fa/check` | accept (new — leading rule relaxed to `[a-z0-9]`) |
| `""` | `Empty` |
| `/a`, `a/`, `a//b` | `EmptySegment { pos }` (new variant) |
| `-foo`, `a/-b`, `_x` | `InvalidLeadingCharacter { ch }` |
| `A/b`, `a/B`, `a b`, `a.b` | `InvalidCharacter { ch, pos }` |
| 65+ bytes | `TooLong { max: 64, got }` |

`PackageName` is unchanged.

## Changes

### 1. `crates/tau-domain/src/error.rs`

Add `AgentIdError::EmptySegment { pos: usize }`. The enum is
`#[non_exhaustive]` (`error.rs:64`), so this is additive.

### 2. `crates/tau-domain/src/id.rs`

Rewrite `impl FromStr for AgentId` to validate per segment. Length and
emptiness checks stay on the whole string; the leading-character rule applies
to each segment. Update the type's doc comment (it currently claims "same
grammar as `PackageName`" — no longer true) and the module header.

### 3. `crates/tau-pkg/src/project/dirs/scan.rs`

At Step 7, where `segments` are joined into `name`, the agents root validates
through the domain type:

```rust
if matches!(kind, Kind::Agents) {
    tau_domain::AgentId::from_str(&name).map_err(|e| ProjectConfigError::DefFile {
        file: file_disp.clone(),
        reason: format!("invalid agent name {name:?}: {e}"),
    })?;
}
```

`valid_segment` stays as-is and keeps guarding both roots' per-segment
hygiene; this is an additional whole-name check on agents only. The error
names the file, which is what the author has to rename.

### 4. `crates/tau-cli/src/cmd/resolve_helpers.rs`

Both `expect("AgentId from validated entry")` sites (`:34` and `:68`) become
`?` with context naming the offending id. Both enclosing functions already
return `anyhow::Result<()>`, and the CLI maps that to exit 2.

### 5. `crates/tau-pkg/src/bundle/{manifest.rs, build.rs}`

- `parse_str`'s accepted range becomes `1..=6`.
- New invariant, alongside the existing trigger/governance ones: if any
  `BundleAgent.id` uses the widened charset (contains `/` or `_`, or starts
  with a digit), `schema_version` must be `>= 6`; otherwise
  `BundleParseError::AgentIdSchemaVersionMismatch`.
- `build.rs`'s `schema_version` computation (`build.rs:367-370`) gains the
  widened-id condition at the top of its precedence chain. A project whose
  agent ids are all pre-widening keeps its current version — no churn for
  ordinary projects.

### 6. `crates/tau-runtime-core/src/interpreter/dynamic.rs`

Comment only. Annotate the `x-` re-anchor (`:225-231`) as now being
`PackageName`-load-bearing only, so a later reader who checks `AgentId` alone
does not remove it. `assert_legal` continues to assert both types.

## Test plan

**`tau-domain` unit** (`id.rs::agent_id_tests`) — every row of the grammar
table above. The existing `rejects_invalid_leading` loses its `"1agent"` case
and keeps `"-abc"` and `"Abc"`.

**`tau-pkg` scan** (`dirs/scan.rs` tests)
- `agents/review/strict.md` scans clean, yielding name `review/strict`.
- `agents/my_agent.md` scans clean.
- `agents/-foo.md` fails with a `DefFile` error naming the file.
- A path exceeding 64 bytes fails with a `DefFile` error carrying `TooLong`.
- `tools/-x/y.toml` still scans clean — tools are unaffected.

**`tau-pkg` bundle**
- Building a project with `agents/review/strict.md` yields
  `BundleAgent { id: "review/strict" }` and `schema_version = 6`.
- A flat-only project keeps its pre-existing `schema_version`.
- `parse_str` rejects a manifest declaring a widened id at `schema_version = 5`.

**`tau-cli` integration** (`tests/cmd_build_dirs.rs`)
- **Delete** `nested_agent_name_is_a_known_bundle_gap`, per its own doc
  comment.
- Extend `build_ships_dir_defined_agent_and_verify_agrees` to use
  `agents/review/strict.md`.
- New: `tau resolve` on a project with a nested agent exits cleanly — the
  direct regression test for failure 2.
- New: `tau build --agent review/strict` slices to that one agent.
- New: `tau verify --bundle` round-trips a nested-agent bundle, exercising
  `reproduce.rs:169`'s `selected_agents` reparse.

## Docs

- **New:** ADR-0070.
- **`docs/SUMMARY.md`:** add the ADR-0070 line (mdBook silently skips pages
  absent from SUMMARY).
- **ADR-0069** Consequences (`:163-174`): the "`/`-containing agent name
  cannot reach a bundle yet" bullet becomes a pointer to ADR-0070.
- **`docs/how-to/define-agents-and-tools-in-directories.md`:** delete the
  `Today's limit` blockquote (`:56-58`); rewrite the first Gotcha
  (`:168-176`) from "keep agent names flat" to the shipped rule, including
  the agents-vs-tools leading-character difference and the 64-byte bound on
  nesting depth.

## Sequencing

1. ADR + spec + SUMMARY, draft PR with `Closes #715` pushed early.
2. `tau-domain` grammar + error variant + unit tests (self-contained; the
   rest depends on it).
3. `resolve_helpers.rs` typed errors — independent of 2, lands either way.
4. `scan.rs` whole-name validation + scan tests.
5. Bundle `schema_version` 6 + invariant + tests.
6. `tau-cli` integration tests; delete the containment test.
7. `dynamic.rs` comment; docs rewrites; mdBook build.

Steps 2-6 each keep the workspace green on their own, so the branch stays
bisectable.

## Out of scope

- `PackageName` — deliberately untouched (ADR-0070, Decision 2).
- Tool-name validation — `tau_ir::ToolId` stays unvalidated by design.
- An explicit nesting-depth cap — the 64-byte `AgentId` bound covers it.
- `#726`, `#717`, and the other open `[dirs]` follow-ups — unrelated seams.
