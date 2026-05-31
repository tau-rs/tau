# ADR-0027 — Skills discovery (Skills-3)

> **Updated 2026-05-31 (β.1.4):** runtime references below are split
> between `tau-runtime-core` (no_std + alloc kernel) and
> `tau-runtime-tokio` (host shell).

**Status:** Accepted 2026-05-13.
**Branch / PR:** `feat/skills-3-discovery` (PR pending).
**Spec:** `docs/superpowers/specs/2026-05-13-skills-3-discovery-design.md`.
**Plan:** `docs/superpowers/plans/2026-05-13-skills-3-discovery.md`.
**Depends on:** ADR-0025 (Skills-1), ADR-0026 (Skills-2).

## Context

Third of 6 sub-projects from ROADMAP §16 (Skills as first-class
packages, Constitution G10). Skills-1 (ADR-0025) shipped the manifest
types + parser; Skills-2 (ADR-0026) wired the install pipeline +
lockfile cache. Skills-3 surfaces installed skills to the user via
`tau skill list` and `tau skill show <name>`.

## Decision

Two subcommands:
- **`tau skill list`** — enumerate installed skills (lockfile only;
  zero per-skill disk reads, consumes Skills-2's
  `LockedSkill.frontmatter` cache).
- **`tau skill show <name>`** — inspect one skill (lockfile + one
  `tau.toml` disk read; optional `--body` adds one `SKILL.md` read).

Both subcommands support `--json` for canonical machine-readable
output (matches existing `tau resolve --json` / `tau verify --json`
convention). Both default to human-formatted terminal output.

`show --body` **renders markdown via termimad by default**, matching
`tau chat`'s precedent. `--raw` flag opts out for pipe / grep / diff
workflows. The trade-off favors the interactive default (most users
running `tau skill show --body` are reading interactively, not
piping).

Unknown-name handling: Levenshtein distance ≤ 2 surfaces a
"did you mean…?" suggestion; otherwise prints the full installed
list. Exit 2 in both cases (matches existing `tau install`
not-found convention).

No new lockfile schema bump in Skills-3. `tau.toml` is re-read from
disk on each `show` invocation (small + fast). Caching capabilities
+ requires_tools + requires_skills in the lockfile would require
Skills-2 schema churn for a marginal perf gain — rejected.

## Alternatives considered

### Default to raw markdown for `--body`

Considered: pipe-friendly out of the box, no rendering hop. Rejected:
- Tau's existing precedent (`tau chat` streams rendered markdown) is
  render-by-default. Skills should match.
- The interactive case is the most common one. Render makes content
  scannable; raw markdown forces the user to mentally parse.
- `--raw` is one flag away for scripting workflows.

### `tau skill uninstall <name>` alias

Considered: discoverability — a user running `tau skill list` might
look for a parallel `tau skill uninstall`. Rejected:
- Pure alias for the generic `tau uninstall <name>` — no
  skill-specific behavior.
- API surface bloat.

### Cache `capabilities` + `requires_tools` + `requires_skills` in lockfile

Considered: would make `show` zero-disk-read like `list`. Rejected:
- Skills-2 just shipped lockfile schema v5. Bumping to v6 for
  marginal perf is churn-without-justification.
- `tau.toml` reads are fast; one file, small payload.
- Drift would require another sha256 cache (Skills-2 has one for
  SKILL.md content; adding another for `tau.toml` is more surface).

### `--format <template>` flag

Considered: power-user feature for scripting. Rejected:
- `--json | jq` covers the same use case better.
- Template language is its own design problem.
- YAGNI for v1.

### `--sort=installed-at|version|name`

Considered: useful for "what did I install recently?" workflows.
Rejected for v1:
- Feature surface for the initial ship.
- Easy additive enhancement when a user asks.
- Alphabetic by name is the most predictable default.

## Consequences

- `tau-cli`'s public surface grows by `Command::Skill` +
  `SkillSubcommand` + `SkillListArgs` + `SkillShowArgs`.
- New `crates/tau-cli/src/cmd/skill/` module tree (5 files).
- No `tau-pkg` or `tau-domain` changes. Skills-2's `LockedSkill`
  schema is consumed read-only.
- `cargo-deny` allow-list unchanged (no new external deps).
- `tau install <skill-pkg>` + `tau skill list` + `tau skill show`
  now form a complete user-facing workflow for skill packages.
- Skills-4 (runtime invocation) is unblocked.

## Out of scope (deferred to Skills-4+)

- **Runtime invocation** — `agent.<skill-name>.spawn` resolution →
  Skills-4 (the most strategic remaining sub-project).
- **Agent Skills spec export / import** → Skills-5.
- **Reference skill packages + user docs** → Skills-6.
- **`--sort` / filter flags** — additive enhancement when needed.
- **User-customizable termimad skins** — Skills-5 or separate
  ROADMAP item.

## References

- Spec: `docs/superpowers/specs/2026-05-13-skills-3-discovery-design.md`
- Plan: `docs/superpowers/plans/2026-05-13-skills-3-discovery.md`
- Skills-1 ADR: `docs/decisions/0025-skills-foundation.md`
- Skills-2 ADR: `docs/decisions/0026-skills-install-pipeline.md`
- ROADMAP §16
