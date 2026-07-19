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
[project]
name        = "my-project"
description = "Optional, free-form."

[agents.example]
display_name = "Example Agent"
package      = "https://github.com/owner/example-agent.git@^0.1"
llm_backend  = "@tau/anthropic"

[agents.example.prompt]
system = """
You are an example agent. Edit me.
"""

# Optional: project-side narrowing of the package's declared capabilities.
[[agents.example.capability_overrides]]
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
| `[project]` | exactly one | project identity. |
| `[allow]` | zero or one | governance ceiling (the project *constitution*). |
| `[agents.<id>]` | any | one entry per agent the project declares. |

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
| `display_name` | string | yes | Shown in `tau list` and run logs. |
| `package` | string | yes | Git URL or `file://` to the agent's package. Validated non-empty after trim. |
| `llm_backend` | string | yes | Package name or git URL for the LLM-backend plugin. |
| `[agents.<id>.requires]` | table | no | See [Requires](#requires). |
| `[agents.<id>.prompt]` | table | no | See [Prompt](#prompt). |
| `[[agents.<id>.capability_overrides]]` | array of tables | no | See [Capability overrides](#capability-overrides). |
| `config` | table | no | Free-form config table forwarded to the agent's package at instantiation. |

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
under `[[agents.<id>.capability_overrides]]` as an array of
tables, one per capability kind to override.

```toml
[[agents.example.capability_overrides]]
kind        = "fs.read"
allow_paths = ["${PROJECT}/docs/**"]
deny_paths  = ["${PROJECT}/secrets/**"]

[[agents.example.capability_overrides]]
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
| `ProjectNameEmpty` | `[project] name` is empty after trim. |
| `AgentPackageEmpty { id }` | `package` is empty after trim. |
| `AgentLlmBackendEmpty { id }` | `llm_backend` is empty after trim. |
| `PromptAmbiguous { id }` | `[prompt]` has both `system` and `system_file`. |
| `CapabilityOverrideExpands { id, kind, reason }` | An override expands the package's grant. *Fires later, at run-time intersect.* |
| `RequiresToolNameEmpty { id, index }` | A `[[requires.tools]]` entry has empty `name`. |

The `deny_unknown_fields` serde attribute is set on every
project-config struct: typos in field names fail at parse time
with a clear error rather than being silently ignored.

## Complete worked example

This example illustrates the agent/prompt/override schema; to `tau build`
it under governed-by-default you would add an `[allow]` ceiling covering
the capabilities its packages declare (see
[Governance](#governance-the-allow-ceiling) above), or pass
`--allow-ungoverned`.

```toml
[project]
name        = "writing-helper"
description = "Two-agent pipeline: draft + critique."

# Inline prompt; package + LLM-backend pinned to local checkouts during dev.
[agents.drafter]
display_name = "Drafter"
package      = "file:///Users/me/work/drafter-agent"
llm_backend  = "https://github.com/owner/tau-anthropic.git@^0.1"

[agents.drafter.prompt]
system = "Write a draft based on the user's brief."

[[agents.drafter.requires.tools]]
name   = "fs-read"
source = "https://github.com/owner/fs-read.git"

# Narrowed fs.read: drafter only reads from notes/, not the whole tree.
[[agents.drafter.capability_overrides]]
kind        = "fs.read"
allow_paths = ["${PROJECT}/notes/**"]


# External-file prompt; same llm_backend (would resolve to one instance).
[agents.critic]
display_name = "Critic"
package      = "https://github.com/owner/critic-agent.git@^0.2"
llm_backend  = "https://github.com/owner/tau-anthropic.git@^0.1"

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
- [Packages](../explanation/packages.md) — what `package` /
  `llm_backend` URLs point at.
- [Glossary](glossary.md) — quick definitions of `agent`,
  `grant`, `override`, `scope`.
