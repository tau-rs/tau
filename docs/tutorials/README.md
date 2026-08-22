# Tutorials

Learning-oriented documentation: lessons that take a newcomer through a
meaningful first experience with tau.

A tutorial is a guided exercise. You leave it having *done* something,
with the confidence that you understand the moving parts. Tutorials
favour narrative, working code, and an explicit "what you just learned"
arc over completeness.

## Available tutorials

- [Bootstrap a tau project](bootstrap-a-tau-project.md) — `tau init`,
  read the generated `tau.toml`, discover the CLI verbs, and trace
  what each step of the agent loop does. Start here if you have
  never used tau before.
- [Build your first skill](build-your-first-skill.md) — author a
  `praise-poet` skill from scratch: `SKILL.md` + `tau.toml`, install it
  locally, invoke it from an agent, and export it for an Anthropic
  consumer. The fastest path to understanding how tau packages real
  agent behaviour.
- [The north-star in action](the-north-star-in-action.md) — walk the
  cross-epic demo fixture: an `[allow]`-governed Branch + Loop pipeline,
  its over-reaching negative twin, the governed bundle roundtrip, and
  the wasm feature-fit gate — every claim enforced in CI.

## Coming next

The Phase 1 roadmap (`../../ROADMAP.md`) tracks the next batch of
tutorials. Likely additions: writing your first LLM-backend plugin and
wiring tau into a parent application via serve mode.

## Looking for something else?

- Need a specific recipe? → [how-to guides](../how-to/README.md)
- Need precise facts about a config field or schema? →
  [reference](../reference/README.md)
- Want the design rationale behind a feature? →
  [explanation](../explanation/README.md)
