# Project manifest schema

Reference for the **project-side** `tau.toml`, the file `tau init`
scaffolds at the project root. Distinct from the **package-side**
`tau.toml` shipped inside an installable package — see the
[package manifest schema](package-manifest-schema.md) for that.

The project manifest declares which agents the project knows about
and how each one is configured. The authoritative source is
`crates/tau-pkg/src/project/`.

## Overview

```toml
# Top-level keys must precede the first table header.
packages = ["anthropic@^1"]

[project]
name        = "my-project"
description = "Optional, free-form."

# Model aliases live under [allow.models.<alias>] when an [allow] ceiling
# is present, else under a top-level [models.<alias>]. A model alias's
# `backend` must name a declared package (see `packages` above).
[allow.models.default]
backend = "anthropic"
model   = "claude-haiku-4-5"

[agents.example]
display_name = "Example Agent"
package      = "https://github.com/owner/example-agent.git@^0.1"
model        = "default"   # the alias key — not the vendor model id

[agents.example.prompt]
system = """
You are an example agent. Edit me.
"""

# Optional: project-side narrowing of the package's declared capabilities.
[[agents.example.capabilities]]
kind        = "fs.read"
allow_paths = ["${PROJECT}/docs/**"]

# Optional: tool packages this agent needs at run-time.
[[agents.example.requires.tools]]
name    = "fs-read"
source  = "https://github.com/owner/fs-read.git"
version = "^0.1"
```

## Top-level blocks

| Block | Cardinality | Purpose |
|---|---|---|
| `packages` | zero or one | Top-level array of declared package strings, e.g. `["anthropic@^1"]`. Model-alias `backend` names resolve against these (and against agent `package` fields). |
| `[project]` | exactly one | project identity. |
| `[allow]` | zero or one | governance ceiling (the project *constitution*). |
| `[models.<alias>]` | any | model aliases, when no `[allow]` ceiling is declared. See [Model aliases](#model-aliases). |
| `[agents.<id>]` | any | one entry per agent the project declares. |

`packages` is a bare top-level key, so in TOML it must appear
**before** the first table header — putting it after `[project]`
makes it a `[project]` field and fails with an unknown-field error.

This page covers `[project]`, `[allow]`, `[models]` and
`[agents.<id>]`. The workflow-authoring blocks (`[tools]`,
`[steps]`, `[triggers]`, `[goals]`, `[deliverables]`) are also valid
in a project manifest but are documented elsewhere.

## Governance: the `[allow]` ceiling

`tau build` and the dev-path `tau run` are **governed by default**
(ADR-0057). A project `tau.toml` with no `[allow]` section is a hard
error, `error[GOV000]: no [allow] section declared` (exit `2`). The
`[allow]` section declares the ceiling of capabilities, model aliases,
MCP servers and tools the project permits; every capability an agent
or tool resolves to must fall inside it. Note that when `[allow]` is
present, model aliases live under `[allow.models.<alias>]` rather than
a top-level `[models]`.

Scaffold one with `tau init --allow`. Two mutually-exclusive escape
hatches waive governance and are recorded in the bundle's
`[governance]` verdict: `--allow-ungoverned` (build/run with no ceiling
at all → verdict `ungoverned`) and `--no-governance` (a ceiling exists
but skip checking it → verdict `skipped`). A valid `[allow]` with
neither flag yields verdict `governed`. The full model, including the
`[allow]` sub-tables and running an `ungoverned` bundle, is in
[Capabilities and consent](../explanation/capabilities-and-consent.md).

## `[project]`

| Field | Type | Required | Notes |
|---|---|---|---|
| `name` | string | yes | Must be non-empty after trim. Local label; doesn't need to be globally unique. |
| `description` | string | no | Free-form; default empty. |

## `[agents.<id>]`

The id `<id>` is the table key. It is what the user passes to
`tau chat <id>` and `tau run <id>`. Must be unique within the
file. Kebab-case is conventional but not enforced.

| Field | Type | Required | Notes |
|---|---|---|---|
| `display_name` | string | yes | Shown in `tau list` and run logs. Validated non-empty after trim. |
| `package` | string | yes | Git URL or `file://` to the agent's package. Validated non-empty after trim. |
| `model` | string | no | A **model alias key**. See [Model aliases](#model-aliases). |
| `[agents.<id>.requires]` | table | no | See [Requires](#requires). |
| `[agents.<id>.prompt]` | table | no | See [Prompt](#prompt). |
| `[[agents.<id>.capabilities]]` | array of tables | no | See [Capability overrides](#capability-overrides). |
| `[[agents.<id>.credentials]]` | array of tables | no | `id` + `env` pairs; see [credential providers](credential-providers.md). |
| `config` | table | no | Free-form config table forwarded to the agent's package at instantiation. |
| `[agents.<id>.context]` | table | no | Context-window policy. |
| `[agents.<id>.durable]` | table | no | Durability intent (ADR-0053). |
| `tool_refs` | array of strings | no | Tool names this agent may call; lowers to `Agent::tool_refs` in the IR. |
| `max_turns` | integer | no | Cap on agent-loop turns. |
| `max_tokens` | integer | no | Cap on tokens (input + output) across the run. |
| `produces` | array of strings | no | Artifact paths / named outputs, cross-checked against `fs-write` grants and bound to `[deliverables.*]` / `[goals.*]`. |
| `output_schema` | table | no | JSON schema for this agent's structured output. Pass-through, no deep validation. |

There is **no `llm_backend` field.** `AgentDefinition` carries an
`llm_backend` internally (ADR-0052), derived from the resolved alias,
but it is not an authoring key — writing it in `tau.toml` fails at
parse time (see [Validation](#validation)).

## Model aliases

`model` is an **alias key**, not a vendor model id.
`ProjectConfig::effective_models()` resolves it against
`[allow.models.<alias>]` when an `[allow]` ceiling exists, else against
a top-level `[models.<alias>]`. Each alias entry holds `backend` (a
package name) and `model` (the vendor string):

```toml
[allow.models.default]
backend = "anthropic"
model   = "claude-haiku-4-5"

[agents.example]
model = "default"   # the alias — not "claude-haiku-4-5"
```

The indirection is the point: swapping the vendor model for every
agent is a one-line edit, and the `[allow]` ceiling can enumerate
exactly which models the project permits. `tau init` scaffolds
`model = ""` so an unconfigured project fails loudly rather than
silently picking a default.

## Prompt

Mutually exclusive: declare `system` or `system_file`, never both.

```toml
[agents.example.prompt]
system = """
You are an example agent.
"""
```

```toml
# Or — keep long prompts out of tau.toml:
[agents.example.prompt]
system_file = "prompts/example.md"
```

| Field | Type | Notes |
|---|---|---|
| `system` | string | Inline system prompt. |
| `system_file` | path | Path (relative to `tau.toml`) to a prompt file. |

Setting both fails validation with `PromptAmbiguous`. Setting
neither yields `PromptEntry::None` (the agent runs with its
package's default prompt).

## Capability overrides

Project-side narrowing of a package's declared capabilities. Lives
under `[[agents.<id>.capabilities]]` as an array of tables, one per
capability kind to override. (The TOML key is `capabilities`; the
*concept* is an override, and the Rust type is
`UncheckedCapabilityOverride`.)

```toml
[[agents.example.capabilities]]
kind        = "fs.read"
allow_paths = ["${PROJECT}/docs/**"]
deny_paths  = ["${PROJECT}/secrets/**"]

[[agents.example.capabilities]]
kind         = "net.http"
allow_hosts  = ["api.example.com"]
deny_hosts   = []
```

| Field | Applies to `kind` | Purpose |
|---|---|---|
| `kind` | n/a — required | Capability discriminator: `fs.read`, `fs.write`, `fs.exec`, `net.http`, or `process.spawn`. |
| `allow_paths` | `fs.*` | Narrowed allow-list. Absent = "use package's allow-list verbatim". |
| `deny_paths` | `fs.*` | Globs to subtract from the effective list. |
| `allow_hosts` | `net.http` | Narrowed allow-list of hosts. |
| `deny_hosts` | `net.http` | Hosts to subtract. |
| `allow_commands` | `process.spawn` | Narrowed allow-list of commands. |
| `deny_commands` | `process.spawn` | Commands to subtract. |
| `max_bytes` | `fs.write` | Narrowed per-file write cap. |

Three guarantees:

- **An override is always a subset.** `tau run` rejects an override
  that *expands* a package's declared grant with the
  `CapabilityOverrideExpands` error.
- **Absence is verbatim.** If the agent declares no override for a
  kind the package declared, the package's grant is used as-is.
- **Cross-check at run.** The intersect-vs-manifest validation
  fires at `tau run` / `tau chat` / `tau resolve` time and at
  `tau list --capabilities` rendering time.

For the underlying capability model (declared vs granted, the
subset law), read [capabilities and
consent](../explanation/capabilities-and-consent.md).

## Requires

Optional run-time tool dependencies for an agent. Surfaces in the
project manifest so `tau resolve` can install them alongside the
agent's package.

```toml
[[agents.example.requires.tools]]
name    = "fs-read"
source  = "https://github.com/owner/fs-read.git"
version = "^0.1"

[[agents.example.requires.tools]]
name   = "shell"
source = "file:///Users/me/work/shell-tools"
```

| Field | Type | Required | Notes |
|---|---|---|---|
| `name` | string | yes | Local handle for the tool inside this agent. |
| `source` | string | yes | Parsed as `PackageSource` — git URL, scp-style, or `file://`. |
| `version` | SemVer requirement string | no | E.g. `"^0.1"`, `">=0.2, <0.4"`. Absent = take whatever the source resolves to. |

`tau_pkg::resolve_requires_tools` handles transitive resolution.
Cycles and unsatisfiable constraints fail with a guided
diagnostic.

## Validation

`UncheckedProjectConfig::validate()` returns errors of type
`ProjectConfigError`:

| Error | When it fires |
|---|---|
| `NotFound` | No `tau.toml` in scope. |
| `Read { path, source }` | Filesystem read failure other than "not found". |
| `Parse { path, source }` / `ParseStr { source }` | TOML parse failure — including an unknown field. |
| `EmptyProjectName` | `[project] name` is empty after trim. |
| `AgentValidation { id, message }` | Generic per-agent semantic failure. Covers empty `display_name`, empty `package`, and invalid `requires.tools` name/version. |
| `AllowValidation { message }` | The `[allow]` constitution is not well-formed. |
| `CapabilityOverrideExpands { id, kind, reason }` | An override expands the package's grant. *Fires later, at run-time intersect.* |
| `PromptAmbiguous { id }` | `[prompt]` has both `system` and `system_file`. |
| `RequiresToolsBareStringRejected { agent_id, index, value }` | A `[[requires.tools]]` entry used the withdrawn bare-string form instead of the struct form. |
| `ToolValidation` / `StepValidation` / `TriggerValidation` | Semantic failures in the `[tools]` / `[steps]` / `[triggers]` blocks. |

The enum is `#[non_exhaustive]`; match with a catch-all arm.

The `deny_unknown_fields` serde attribute is set on every
project-config struct, so a misspelled or withdrawn field is a
**parse-time** failure, not a silent no-op. This is why
`llm_backend = "..."` under `[agents.<id>]` does not load — serde
rejects it as an unknown field and lists the valid set in the error.

## Complete worked example

This example illustrates the agent/prompt/override schema; to `tau build`
it under governed-by-default you would add an `[allow]` ceiling covering
the capabilities its packages declare (see
[Governance](#governance-the-allow-ceiling) above), or pass
`--allow-ungoverned`.

```toml
packages = ["anthropic@^1"]

[project]
name        = "writing-helper"
description = "Two-agent pipeline: draft + critique."

# One alias, referenced by both agents below.
[models.default]
backend = "anthropic"
model   = "claude-haiku-4-5"

# Inline prompt; package pinned to a local checkout during dev.
[agents.drafter]
display_name = "Drafter"
package      = "file:///Users/me/work/drafter-agent"
model        = "default"

[agents.drafter.prompt]
system = "Write a draft based on the user's brief."

[[agents.drafter.requires.tools]]
name   = "fs-read"
source = "https://github.com/owner/fs-read.git"

# Narrowed fs.read: drafter only reads from notes/, not the whole tree.
[[agents.drafter.capabilities]]
kind        = "fs.read"
allow_paths = ["${PROJECT}/notes/**"]


# External-file prompt; same alias, so both agents share one backend instance.
[agents.critic]
display_name = "Critic"
package      = "https://github.com/owner/critic-agent.git@^0.2"
model        = "default"

[agents.critic.prompt]
system_file = "prompts/critic.md"
```

## See also

- [Package manifest schema](package-manifest-schema.md) — the
  *other* `tau.toml`, inside packages.
- [Capabilities and consent](../explanation/capabilities-and-consent.md)
  — declared vs granted; the subset law overrides obey.
- [Bootstrap a tau project](../tutorials/bootstrap-a-tau-project.md)
  — the tutorial that walks through this file end-to-end.
- [Packages](../explanation/packages.md) — what `package` URLs and
  model-alias `backend` names point at.
- [Glossary](glossary.md) — quick definitions of `agent`,
  `grant`, `override`, `scope`.
