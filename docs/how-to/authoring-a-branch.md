# Authoring a conditional branch

A `[[pipeline.steps]]` entry is normally a **leaf** — it runs one agent, tool,
deterministic step, or check:

```toml
[[pipeline.steps]]
id  = "triage"
run = "agent:triage"
```

To route between two paths, make the step a **branch** instead. A branch has a
condition (`branch = { … }`) and two nested step arrays, `then` and
`otherwise`:

```toml
[[pipeline.steps]]
id    = "triage"
run   = "agent:triage"
input = "${input}"

[[pipeline.steps]]
id     = "route"
branch = { evaluates = "steps.triage.output", check = "matches", pattern = "(?i)urgent" }

  [[pipeline.steps.then]]
  id    = "escalate"
  run   = "agent:oncall"
  input = "${steps.triage.output}"

  [[pipeline.steps.otherwise]]
  id    = "ack"
  run   = "agent:writer"
  input = "${steps.triage.output}"
```

If `steps.triage.output` matches the pattern, the `then` arm runs; otherwise
the `otherwise` arm runs.

## The condition

`branch` reuses the same predicate vocabulary as `[goals.*]` (see
[Assert pipeline postconditions](assert-pipeline-postconditions.md)).
`evaluates` names the value to test — a `steps.<id>.output` reference or a
filesystem path — and one predicate selector decides the verdict:

| `check`        | companion field | holds when …                       |
| -------------- | --------------- | ---------------------------------- |
| `exists`       | —               | the locus resolves                 |
| `non_empty`    | —               | it resolves and is non-empty       |
| `equals`       | `equals`        | it equals the literal              |
| `matches`      | `pattern`       | it matches the regex               |
| `min_count`    | `min_count`     | it has at least N non-empty lines  |
| `schema_valid` | `schema`        | it validates against the schema    |

Or use `fn = "<crate>::<path>"` for a registered native predicate instead of
`check`.

## One-armed branches

Omit `otherwise` to do nothing when the condition is false:

```toml
[[pipeline.steps]]
id     = "maybe-review"
branch = { evaluates = "steps.draft.output", check = "non_empty" }

  [[pipeline.steps.then]]
  id  = "review"
  run = "agent:reviewer"
```

## Scope

Branch arms share the pipeline's flat output namespace: an arm step may read
any earlier step's `${steps.<id>.output}`, and later steps may read an arm's
output by id. A condition may only read outputs produced **before** the branch.
These rules are checked at build time — a forward or out-of-scope reference
fails `tau build`.

> Deeply nested or expression-heavy control flow is better authored in
> TypeScript (`tau-ts-extract`), which lowers to the same IR. TOML branches are
> intended for shallow, declarative routing.
