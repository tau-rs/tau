# ADR-0068: Directory-based tool & agent definitions

**Status:** Accepted
**Date:** 2026-08-22
**Deciders:** tau core

> **Numbering note:** the design spec and the implementation's internal
> comments (`tau-pkg`'s `dirs` module, `ProjectConfig::parse_str_at`, `tau
> check`'s dirs category) refer to this feature as "ADR-0067" — that was the
> next free slot when the spec was written. By the time this page landed,
> [ADR-0067](0067-sandbox-windows-appcontainer-phase2.md) (Windows
> AppContainer adapter, Phase 2) had already merged to `main` under that
> number. This document claims **0068**, the next free slot, rather than
> overwriting an unrelated accepted ADR. The in-code comments citing
> "ADR-0067" are stale by one digit; fixing them is a follow-up (docs-only
> changes are out of scope for a docs task that must not touch Rust code).

## Context

Authoring every agent and tool inline in `tau.toml` scales poorly:
prompt-heavy agents bloat a single file, and the one-file model has no answer
for "add an agent by adding a file" — the DX that Claude Code
(`.claude/agents/*.md`), Cursor (`.cursor/rules/`), and Copilot
(`.github/prompts/`) have made the ecosystem default.

tau carries two invariants a naive directory-authoring bolt-on would break:

- **Byte-equal IR.** `tau-sdk-codegen/tests/byte_equal.rs` asserts that
  equivalent projects lower to identical IR regardless of authoring surface
  (TOML vs. TS today). A new authoring surface must converge on the same
  `ProjectConfig` before lowering, not fork a parallel path.
- **Governance is never a side door.** `[allow]`, capability fit, and
  feature fit gate every agent/tool regardless of where it was declared
  (ADR-0057, ADR-0059). A directory scan must feed the same validated model
  the inline tables feed, not bypass it.

The design lives in
[`docs/superpowers/specs/2026-08-22-dir-based-definitions-design.md`](../superpowers/specs/2026-08-22-dir-based-definitions-design.md);
this ADR records the decisions that spec made and why.

## Decision

**Explicit opt-in, never conventional.** A root `[dirs]` table declares
`agents` and/or `tools` roots as project-root-relative paths:

```toml
[dirs]
agents = "agents"   # scans agents/**/*.{md,toml}
tools  = "tools"    # scans tools/**/*.toml
```

No `[dirs]` table means no scanning — an existing project's behavior is
unchanged unless it opts in. Declared roots are validated (relative,
contained within the project root, existing, not `.`/`_`-prefixed, mutually
disjoint) before any file is read.

**Path = name, `/`-joined.** An entry's engine name is its path relative to
the kind root with the extension stripped: `agents/review/strict.md` names
agent `review/strict`. Names flow through `tool_refs`, `subflow`,
`[allow.*]` keys, traces, and `tau list` identically to inline names — there
is no separate "file path" vs. "engine name" concept to keep in sync.
References use the full name; a stale reference (e.g. after a file move)
fails the build loudly — `LowerError::UnknownToolRef` /
`UnknownSubflowTarget` (`agent "x" references unknown tool "y"` style) —
rather than resolving silently. The error names the bad reference but does
not suggest a replacement.

**A stricter charset than inline names.** Each path segment must match
`[a-z0-9_-]+`. This is deliberately narrower than the unrestricted charset
inline `[agents.X]` keys already allow — the extra strictness rules out
case-insensitive-filesystem collisions (macOS/Windows vs. Linux CI), macOS
NFD/NFC normalization drift, and collisions with the reserved
`skill.<name>.spawn` (`.`) and `__tau::goal::*` (`::`) namespaces. Inline
names are unchanged; only the directory surface is stricter than the
language.

**YAML frontmatter for `.md`, deserializing into the existing schema.**
`agents/**/*.md` files use `---`-fenced YAML frontmatter (matching
`SKILL.md` and the Claude Code convention) that deserializes directly into
`UncheckedAgent` — the exact same struct, same required fields
(`display_name`, `package`), same `deny_unknown_fields` — that inline
`[agents.X]` tables produce. There is no second, parallel agent schema to
keep in sync by hand. The markdown body becomes the agent's inline system
prompt (`prompt.system`); `name` and `prompt`/`system`/`system_file` are
forbidden in frontmatter because the path and the body already own those
roles respectively.

**Unchecked-level merge in a new root-aware entry point.** The merge happens
inside `tau-pkg`, before the single existing `validate()` pass, via
`ProjectConfig::parse_str_at(toml, project_root)`: parse the root config,
scan `[dirs]`, insert scanned entries into the same unchecked `BTreeMap`s the
inline tables populate (a name collision is a hard error), then validate
once. `ProjectConfig::from_path` delegates to it, so every real loading path
(`tau run`, `tau build`, `tau dev`, `tau check`, serve, resolve, chat, …)
becomes dirs-aware for free, without a second code path through lowering.
The rootless `ProjectConfig::parse_str` (in-memory parsing, used by tests and
library callers with no filesystem root to scan from) rejects a `[dirs]`
table outright rather than silently ignoring it. `tau-ir-lower` is
unmodified — it only ever sees a fully-merged `ProjectConfig`.

**Strict hygiene over a permissive scan.** Every file under a declared root
must be a definition (`.md` under `agents`, `.toml` under either) or must be
explicitly excluded — `_`/`.`-prefixed names (files or directories) and a
short OS-junk allowlist (`Thumbs.db`, `desktop.ini`). Anything else is a
hard build error naming the offending file and the `_` escape, not a
silently-skipped file. Symlinks are rejected outright (never followed) to
keep the scan hermetic and loop-free. This mirrors the project's existing
"strict authoring surface, versioned interchange" stance (ADR-0065) — a
directory of definitions is an authoring surface, so it fails loud on
anything unrecognized rather than guessing.

**CRLF normalization at build time.** `\r\n` → `\n` is applied to `.md`
frontmatter and body content before hashing, extended to `system_file` reads
in the same change. This closes a pre-existing gap
(`tau-ir-lower/src/lower/parse.rs`, `system_file` bytes were hashed raw) in
the same class as the earlier `*.wit` CRLF incident (#553): without it, an
`autocrlf` Windows checkout changes a prompt's asset hash and breaks `tau
verify` cross-platform.

**Containment guard, extended to `system_file`.** The same `starts_with`
canonicalized-path containment check `[dirs]` roots use is applied to
`system_file` (previously unconstrained — a latent gap closed as part of the
same change, not a new mechanism).

## Consequences

- **Names may contain `/`.** Because a dir-authored and an inline-authored
  entry with the same full name must be able to collide (equivalence
  invariant, not silently coexist), inline `[agents.X]` / `[tools.X]` keys
  now accept `/`-containing names too, via quoted TOML keys:
  `[agents."review/strict"]`. This is a small but real widening of what was
  previously an implicit "no slashes" convention for inline keys.
- **Moving a file renames the definition.** There is no identity separate
  from the path. This is the intended, Claude-Code-like DX ("the file *is*
  the thing"), but it means a rename is a breaking change to every reference
  by default. The only mitigation today is that it fails loudly at build
  time (a plain "unknown tool/agent" error, no suggested replacement) rather
  than resolving silently — there is no alias, redirect, or did-you-mean
  mechanism.
- **TS projects cannot declare `[dirs]` yet.** The TS authoring surface
  (ADR-0041) has no `dirs()` factory, so `[dirs]` is unrepresentable from
  `.ts` today. The root-aware merge seam (`parse_str_at`) does not care which
  frontend produced the TOML it scans alongside, so adding the factory later
  is additive — this ADR does not close that gap, it defers it.
- **No dir surface for governance-adjacent or control-flow tables.**
  `[allow]`, `[models]`, credentials, steps, triggers, and goals stay
  root-only by design (non-goal in the spec) — each dir file is exactly one
  entry body, so tables that aren't "one entry" are structurally
  unrepresentable, not merely unimplemented.
- **`tau check` gains a lint, not a gate.** A dir-defined file matched by
  `.gitignore` builds locally but is silently absent from CI/clones; `tau
  check dirs` warns (does not fail) because a project may legitimately
  gitignore a draft definition on purpose.
- **The in-code "ADR-0067" comments are stale.** See the numbering note
  above; a follow-up should update the three doc-comment references
  (`tau-pkg/src/project/dirs/mod.rs`, `tau-pkg/src/project/project.rs`,
  `tau-cli/src/cmd/check/result.rs`) to cite 0068 once a Rust-touching change
  lands on this branch anyway.
