# How-to guides

Task-oriented documentation: recipes for accomplishing a specific goal
when you already know what you want.

How-tos assume you understand the basics and are reaching for the right
flag, the right command, or the right shape of a config block. They are
short, focused, and named after the goal — not the feature.

## Project & sandbox

- [Configure the sandbox tier](configure-sandbox-tier.md) — the three
  knobs (scope config, plugin manifest, `--no-sandbox`), what each
  one controls, and quick recipes for common scenarios.

## Skills

- [Install a skill](install-a-skill.md) — `tau install <source>` from
  git, a local path, or a `file://` URL; customize-before-install via
  `tau skill import`.
- [Author a skill](author-a-skill.md) — minimal skill, capability
  declarations, bundled files, `requires_skills`, versioning, and
  testing.
- [Export a skill](export-a-skill.md) — emit a tau skill in the
  Anthropic SKILL.md format; `--strict`, `--force`, and the roundtrip
  guarantee for capability-less skills.

## Execution & durability

- [Run tau under a durable orchestrator](run-tau-under-a-durable-orchestrator.md)
  — hand a tau bundle to Temporal, Inngest, or Cloudflare Workflows as a
  safe-to-retry reentrant unit; the whole-bundle granularity caveat.

## Contributing

- [Write a tool plugin](write-a-tool-plugin.md) — eight-step walk
  through the minimum viable tool plugin (binary shim, `Tool`
  trait impl, capability extraction from `SessionContext`,
  integration test via `tau-plugin-test-support`).
- [Propose an ADR](propose-an-adr.md) — when an ADR is required
  (QG18), the template + workflow, numbering rules, and the
  24-hour cooldown.

## Coming next

How-to coverage will grow alongside the public surface. Planned recipes
include: installing and pinning LLM-backend plugins, running an agent
in serve mode from a parent application, capturing structured logs for
observability, and writing a workflow that chains agents.

## Where to look first

- New to tau? Read a [tutorial](../tutorials/README.md) first — how-tos
  assume you know the vocabulary.
- Want the *exact* schema, flag, or field? → [reference](../reference/README.md).
- Want to understand *why* something works the way it does? →
  [explanation](../explanation/README.md).
