# ADR-0070: `AgentId` grammar — namespaced authored names, sanitized generated ones

**Status:** Accepted
**Date:** 2026-08-29
**Deciders:** tau core

> **Numbering note:** ADR numbers in this repo are claimed by *merge* order,
> not branch-start order — 0067 and 0068 were both taken mid-flight while
> [ADR-0069](0069-directory-based-definitions.md) was in review, forcing two
> renumbers. This document claimed 0070 when its PR opened; re-verify before
> merge.

## Context

`tau_domain::AgentId` and `tau_domain::PackageName` share one grammar: an
ASCII identifier of 1..=64 bytes, leading character `[a-z]`, remainder
`[a-z0-9-]`. `AgentId` has enforced that since the first `tau-domain` commit,
when the only agent names in existence were inline `[agents.researcher]`
table keys.

[ADR-0069](0069-directory-based-definitions.md) introduced `[dirs]`, where an
entry's engine name **is** its path relative to the kind root:
`agents/review/strict.md` names agent `review/strict`. The scanner's own
charset is `[a-z0-9_-]+` per path segment, joined with `/`. Those two
grammars disagree: the scanner accepts `/` and `_`, the domain type accepts
neither.

The disagreement was invisible until #725 made the build pipeline
`[dirs]`-aware, at which point it produced three distinct failures:

- **`tau build` exits 2.** `bundle/build.rs` parses each project agent id
  into a `tau_domain::AgentId` for `BundleAgent.id`; `review/strict` fails
  manifest assembly.
- **`tau resolve` panics.** `cmd/resolve_helpers.rs` called
  `AgentId::from_str(&entry.id).expect("AgentId from validated entry")` on
  the same string — a panic on user-authored input.
- **The interpreter silently mis-attributes.** `agent_loop.rs` converts the
  IR agent id to both `PackageName` and `AgentId` with
  `.unwrap_or_else(|_| … "ir-agent" …)`. An id the grammar rejects does not
  fail; it collapses to a phantom agent named `ir-agent`, taking that
  agent's token accounting (#538) and run span (#731) with it.

The third failure is the same defect #731 / #734 fixed from the other end.
`_` is rejected too, so even a flat `agents/my_agent.md` — a name the scanner
explicitly permits — dies at the bundle boundary.

ADR-0069 documented the gap in its Consequences and deferred the decision;
`crates/tau-cli/tests/cmd_build_dirs.rs::nested_agent_name_is_a_known_bundle_gap`
pinned the boundary so it could not shift silently. This ADR takes the
deferred decision.

### The precedent this ADR declines to follow

Commit `b91366c59` (#734, "make dynamic-region child ids legal domain agent
ids") hit the same wall two commits earlier and resolved it by **sanitizing
into the existing grammar**: `dynamic.rs::sanitize_component` folds every
character outside `[a-z0-9-]` to `-`, and `child_agent_id` assembles
`{region_step}-{kind}-{entry}-{index}`, re-anchoring with an `x-` prefix when
the result would not start with a letter.

That is correct **for generated ids** and wrong **for authored ones**, and
the distinction is not stylistic:

- A generated child id is assembled from opaque `tau.toml` step names that no
  user ever expects to read back. Sanitizing destroys information, but the
  `-{entry}-{index}` suffix independently guarantees uniqueness, so nothing
  observable is lost. Truncation, as `child_agent_id`'s own doc comment puts
  it, "costs legibility, never correctness."
- An authored name has no such suffix. The `[dirs]` contract, stated verbatim
  in its how-to, is that distinct paths yield distinct names:

      agents/review/strict.md     -> agent "review/strict"
      agents/perf/strict.md       -> agent "perf/strict"     (distinct; never ambiguous)

  Applying `sanitize_component` to those gives `review-strict` and
  `perf-strict` — which then collide with an authored `agents/review-strict.md`
  and `agents/perf-strict.md`. Sanitizing an authored name is not lossy-but-safe;
  it is **non-injective**, and injectivity is the property the whole feature
  was designed to provide.

Sanitizing is right where uniqueness is supplied by a counter and wrong where
uniqueness is supplied by the name itself. #734 is the first case, this is the
second, and the two coexist without contradiction.

## Decision

**1. Widen `AgentId` — and only `AgentId` — to a `/`-separated namespaced
grammar.**

```
AgentId  := segment ( '/' segment )*        1..=64 bytes total
segment  := [a-z0-9] [a-z0-9_-]*
```

`review/strict`, `perf/strict`, `my_agent`, `a/b/c`, and `2fa` are all
legal. `""`, `/a`, `a/`, `a//b`, `-foo`, `a/-b`, `A/b`, and anything over 64
bytes are not. Empty segments are reported as a new
`AgentIdError::EmptySegment { pos }` variant; the enum is `#[non_exhaustive]`,
so this is a minor change.

The leading-character rule relaxes from `[a-z]` to `[a-z0-9]` so that the
domain grammar accepts every segment shape the `[dirs]` scanner can produce,
rather than agreeing with it on three characters out of four. A leading `-`
stays illegal: `tau build --agent -foo` must not be ambiguous with a flag.

**2. `PackageName` is not touched.** A package name is a registry identity
(`fs-tools@1.2.0`) resolved against a lockfile and a package index, not a
project-local namespace. It keeps `[a-z]` + `[a-z0-9-]`. This is the first
point at which the two grammars diverge, and the divergence is deliberate:
they answer different questions.

Consequently `dynamic.rs::child_agent_id`'s `x-` re-anchor
(`dynamic.rs:225-231`) becomes **`PackageName`-load-bearing only** — `AgentId`
would now accept a digit-leading child id, `PackageName` still would not, and
`assert_legal` asserts both. It must not be simplified away on the grounds
that `AgentId` no longer needs it.

**3. The `[dirs]` scanner validates agent names through the domain type, not
through a second charset.** At the point where `scan.rs` joins path segments
into a name, the agents root calls `tau_domain::AgentId::from_str` and reports
a `ProjectConfigError::DefFile` naming the offending **file**:

```
error: agents/-foo.md: invalid agent name "-foo": agent id must start with a
       letter or digit, got '-'
error: agents/a/b/c/d/e/f/g/verylongname.md: invalid agent name "a/b/c/…":
       agent id exceeds 64 characters: got 71
```

There is now exactly one implementation of the agent-name grammar, and the
authoring surface cannot accept a name the bundle boundary will later refuse.
The length cap doubles as the nesting-depth bound; no separate depth limit is
introduced, because a second bound is a third grammar to keep in sync.

**Tool names are unchanged** — the tools root keeps `[a-z0-9_-]+` per segment
with no leading-character rule. The asymmetry is principled: an agent name
becomes a typed identity in the bundle (`BundleAgent.id: AgentId`), a tool
name stays a free-form string (`tau_ir::ToolId` is an unvalidated newtype by
design, per ADR-0069). Agents are validated at the scan because there is a
downstream type to satisfy; tools are not because there is not.

**4. `resolve_helpers.rs` returns an error instead of panicking.** Both
`expect("AgentId from validated entry")` sites become typed failures. This
holds independently of the grammar: a panic on user-authored input is a bug
under every grammar, wide or narrow.

**5. A bundle carrying a widened agent id declares `schema_version = 6`.**
`BundleManifest::parse_str` accepts 1..=6 and enforces the invariant that any
agent id outside the pre-widening charset requires `>= 6`. A bundle whose
agent ids are all still `[a-z][a-z0-9-]*` continues to be emitted at its
existing version, so ordinary projects see no version churn.

This is an error-quality measure, not a correctness one, and the ADR is
explicit about the difference from the `[[trigger]]` (v3) and `[governance]`
(v4) bumps. Those fields are `#[serde(default)]`, so an old tau would
**silently drop** them — the version gate is the only thing standing between
a stale binary and a wrong answer. `BundleAgent.id` cannot be dropped: an old
tau reaches `AgentId::deserialize` and hard-fails regardless. The bump only
converts

    Error: TOML parse error … agent id contains invalid character '/' at byte 6

into

    Error: unsupported bundle schema_version: 6

It is adopted because the discipline is established at this boundary and a
reader of `parse_str` will expect to find it, not because anything is unsound
without it.

## Consequences

**Positive**

- `[dirs]`' headline example works. `agents/review/strict.md` builds, slices
  (`tau build --agent review/strict`), verifies, and runs.
- The silent `ir-agent` collapse of the **agent id** in `agent_loop.rs`
  becomes unreachable for authored names, closing the #715 half of the defect
  #731 closed from the generated-id side. Per-agent token attribution and run
  spans key on that id, so they are now correct for namespaced agents.

  The adjacent `pkg_name` conversion on the same lines still falls back:
  `PackageName` stays narrow by Decision 2, so a namespaced id yields the
  synthetic `PackageId("ir-agent", 0.0.0)` that `AgentDefinition::new`
  requires. That is not a regression — it was already the behaviour for every
  id `PackageName` rejects, and it was the behaviour for `review/strict`
  before this change — and it is not an attribution surface. It is called out
  here so the asymmetry on those five lines reads as deliberate.
- `tau resolve` no longer panics on any project it can parse.
- The scanner and the domain type share one grammar with one implementation.
  The class of bug this ADR fixes — an authoring surface accepting a name a
  later stage rejects — is structurally eliminated for agents, not merely made
  rarer.

**Negative**

- `AgentId` now accepts digit-leading names (`2fa`, `123`). The existing
  `rejects_invalid_leading` case for `"1agent"` is deleted. Nothing depends on
  an agent id being non-numeric, but it is a real relaxation of a rule that
  held since the first commit.
- `AgentId` and `PackageName` no longer share a grammar, so
  `agent_loop.rs`'s paired conversion can now succeed for one and fail for the
  other. Both call sites already fall back independently; the `x-` re-anchor
  in `dynamic.rs` is annotated as `PackageName`-load-bearing so it is not
  removed by a later reader who checks only `AgentId`.
- Bundles with namespaced agent ids are unreadable by any tau older than this
  change. That is unavoidable in either direction — such a bundle was
  unbuildable before — and `schema_version = 6` makes the refusal legible.

**Neutral / bounded**

- Blast radius is smaller than `AgentId`'s file count suggests. Six crates
  reference `tau_domain::AgentId`; four sites construct one from user input.
  Both downstream identifier types are unvalidated — `tau_ports::AgentId` is
  `pub type AgentId = String` and `tau_ir::ids::AgentId` is a transparent
  newtype whose doc comment already says "validation is the lowering pass's
  responsibility, not the type's". No `.wit` file names an agent id. Traces,
  session and suspension state, the plugin protocol, and the wasm guest
  boundary are all untouched.

**New obligations**

- `nested_agent_name_is_a_known_bundle_gap` is deleted, as its own doc comment
  instructs, and `build_ships_dir_defined_agent_and_verify_agrees` is extended
  to a nested name.
- ADR-0069's Consequences bullet and the `[dirs]` how-to's `Today's limit`
  blockquote and first Gotcha are rewritten from "known gap" to the shipped
  rule.
- Inline `[agents."review/strict"]` was always legal TOML and always failed at
  the bundle boundary; it now works, so the inline and directory surfaces stay
  interchangeable as ADR-0069 requires.

## Alternatives considered

**Narrow the `[dirs]` scanner to `[a-z0-9-]` and forbid nested agent
directories.** Cheapest option (~20 lines), needs no schema bump, and is
consistent with #734's "make the name fit the grammar" instinct. Rejected on
three counts. It fixes one surface, not the bug: inline
`[agents."review/strict"]` is legal at the project-config layer and would
still exit 2 at manifest assembly and still panic `tau resolve`, so #715's
title would remain accurate after the fix. It makes `agents/` and `tools/`
obey different rules inside one feature with no principle behind the
difference beyond which one happens to have a validating type downstream —
tools would keep nesting and keep `_` while agents lost both. And it deletes
the headline capability of a feature that shipped the previous day, turning
ADR-0069's "this decision is deferred" into "this decision is declined"
without new information.

**Split identity: keep `AgentId` narrow, carry the authored path as a display
name, derive the engine id by sanitizing.** Rejected because
`sanitize_component` is not injective: `review/strict` and `review-strict` map
to the same id, so a project containing both either fails with a confusing
collision or silently merges two agents. A hash suffix restores uniqueness at
the cost of ids no human can type, which breaks `tau build --agent`. The
deeper defect is that every reference site — `tool_refs`, `subflow`,
`[allow.*]` keys, `--agent`, traces, `tau list` — must name either the display
form (requiring a fallible reverse lookup, ambiguous exactly when it matters)
or the engine form (making "the path is the name" false). ADR-0069 chose one
name precisely to avoid a path/name pair to keep in sync.

**Widen `PackageName` alongside `AgentId`, keeping them identical.** Rejected:
it would let a package name contain `/`, which collides with registry and
path conventions for no benefit — no package is authored by directory
position. The two types were identical by coincidence of implementation, not
by requirement; #734's `assert_legal` asserts both only because a dynamic
child id is used as both.

**Adopt `AgentId::MAX_LEN` per segment plus an explicit nesting-depth cap.**
Rejected: a third bound to keep in sync with the other two, which is the
failure mode this ADR exists to remove. The 64-byte total already bounds
depth, enforced once, with an error that names the file.
