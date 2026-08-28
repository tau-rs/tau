# Glossary

Single-page reference for the vocabulary used across the book.
Each entry is one or two sentences with a link to where the concept
is treated in detail.

Definitions here are *normative* — they pin the meaning tau core
uses. If two places in the book seem to disagree, this page is the
tiebreaker; file an issue against the page that drifted.

## A

**Adapter** — a concrete implementation of the `Sandbox` port that
turns a `SandboxPlan` into a kernel-enforced wrapped `Command`.
Today's adapters: `native` (Linux landlock+seccomp), `darwin`
(macOS `sandbox-exec`), `windows` (scaffold), `container` (Docker /
Podman), `passthrough` (identity wrap). See
[sandboxing](../explanation/sandboxing.md#adapters-how-a-tier-becomes-enforcement).

**Agent** — a configured `(package, llm_backend, system_prompt,
grant)` tuple that the runtime can instantiate and run. That tuple is
the *runtime* shape (`AgentDefinition`); it is not the authoring
shape. An *agent definition* is written in a project's `tau.toml`
under `[agents.<id>]`, where the model is named by the `model` alias
key — there is no `llm_backend` key in `tau.toml`; the runtime derives
it from the resolved alias. See [project manifest
schema](project-manifest-schema.md).

## B

**Bundle** — the content-hashed deployment artifact produced by
`tau build --target <triple>` (Phase 2). Pins all resolved package
versions + effective capabilities + the target sandbox triple.
See [tau as a language](../explanation/tau-as-language.md).

## C

**Capability** — a typed grant of a specific kind of access (e.g.
`fs.read` with paths, `net.http` with hosts). Capabilities are
*positive* (everything is denied except what's granted), *typed*
(schema evolution is non-breaking), and *inert until granted*. See
[capabilities and consent](../explanation/capabilities-and-consent.md).

**CapabilityShape** — the *kind* of enforcement a capability
requires from a sandbox adapter, with the payload erased
(`FilesystemRead`, `NetworkHttp`, `AgentSpawn`, etc.). The
resolver uses shapes to match plans against adapter advertisements.
See
[capabilities](../explanation/capabilities-and-consent.md#capabilityshape-the-resolvers-vocabulary).

**Consent** — the user's affirmative accept of a package's declared
capabilities at install time. Recorded in the lockfile; escalation
requires re-install (G14).

**Custom (capability / kind)** — an escape hatch for variants not
yet typed in core. Tracked in the
[escape-hatches registry](../explanation/escape-hatches.md). No
kernel-level enforcement is wired for Custom shapes.

## G

**Grant** — what a package actually receives after consent +
project-side narrowing. The lockfile records the grant; the
runtime + sandbox enforce the grant. Distinct from *declared*
(what the manifest asks for). See
[declared vs granted](../explanation/capabilities-and-consent.md#declared-vs-granted).

## K

**Kind** — the role a package fulfils in the runtime. Today's
seven: `llm-backend`, `tool`, `skill`, `pipeline`, `mcp-server`,
`storage`, `sandbox`. The runtime loads a different trait
implementation per kind. See
[packages](../explanation/packages.md#the-seven-kinds).

## L

**Lockfile** — the resolved truth for an install. Records
`<name, version, source, content-hash, granted-capabilities>` per
package. `tau verify` re-hashes the installed content against this
file to detect drift. Lives at `<scope>/lockfile.toml`.

## M

**Manifest** — `tau.toml`. There are two kinds:
- **Package manifest** — sits inside a package; declares name,
  version, source, kind, dependencies, capabilities, sandbox
  requirements. See
  [package manifest schema](package-manifest-schema.md).
- **Project manifest** — sits at the project root; declares
  agents (`[agents.<id>]`), prompts, capability overrides. See
  [project manifest schema](project-manifest-schema.md).

## O

**Orchestration** — composing multiple agents inside one run via
the kernel's virtual tools (`task.*`, `agent.<kind>.spawn`,
`run.*`). The LLM of the parent agent decides what to spawn next.
See [multi-agent
orchestration](../explanation/multi-agent-orchestration.md).
Compare with **workflow**.

**Override (capability)** — a project-side narrowing of a
package's declared capability set, declared under
`[[agents.<id>.capabilities]]`. Must be a subset of the
package's declared set; overrides that *expand* fail validation.
See [project manifest
schema](project-manifest-schema.md#capability-overrides).

## P

**Package** — the unit of extension in tau (G2). A manifest plus
the artifacts the manifest describes. The only way to add
functionality is `tau install <source>` (G7). See
[packages](../explanation/packages.md).

**Pipeline** — a package whose `kind = "pipeline"` orchestrates
multiple agents through a *methodology*. Not a general-purpose
workflow engine (NG5). Distinct from `tau-workflow`'s linear
workflows; pipelines run inside an agent's tool loop and a
pipeline package can ship any orchestration pattern it wants.

**Port** — the trait a kind of plugin implements: `LlmBackend`,
`Tool`, `Storage`, `Sandbox`. Plugins declare which port they
provide via `[plugin] provides = "..."` in their manifest.

## R

**Run** — an execution of an agent (or workflow). Has a root
agent, a tree of child agents, a shared task list, and a JSONL
trace. Ends when every spawned agent completes or a budget cap
fires (orchestration invariant 5).

## S

**Sandbox plan** — a `(tier, required_shapes)` tuple derived from
intersecting a scope's sandbox config with a plugin's manifest.
The resolver maps a plan to an adapter at spawn time. See
[sandboxing](../explanation/sandboxing.md#resolution).

**Serve mode** — tau running as a long-lived subprocess speaking
JSON-RPC 2.0 over NDJSON-framed stdio. One of tau's two public
surfaces (G6). See [serve mode](../explanation/serve-mode.md) and
the [protocol reference](serve-mode-protocol.md).

**Scope** — a tau installation root. Two kinds: **global**
(`~/.tau`) and **project** (a `.tau/` directory walked up from
cwd). Project scope overrides global per package (G8). Each
scope has its own lockfile + sandbox config.

**Shape** — see **CapabilityShape**.

**Skill** — a package whose `kind = "skill"` ships a `SKILL.md`
(prompt content) plus `tau.toml` (typed metadata). Invoked from
an agent as a child via `skill.<name>.spawn`. See
[two-layer skills](../explanation/two-layer-skills.md).

**Source** — where a package lives. `PackageSource::Git { location,
rev }` covers every install mode today (https / ssh / scp-style /
file://). See [package manifest
schema](package-manifest-schema.md#source).

## T

**Task** — a unit of work in an orchestrated run's shared
`TaskList`. Has a hierarchical id (`1`, `1.1`, `1.1.a`) and a
lock with owner + lease + heartbeat.

**Tier** — the strength of sandbox isolation: `none`, `light`,
`strict`. Tier is an *intent* level; the resolver picks an
adapter to satisfy it. See
[sandboxing](../explanation/sandboxing.md#the-tier-vocabulary).

**Trace** — the append-only JSONL log of every think / tool call /
completion event in a run. Lives at
`<scope>/.tau/runs/<run-id>.jsonl`. Monotonic; never edited.

## V

**Virtual tool** — a tool call the kernel intercepts before plugin
dispatch (`task.*`, `agent.<kind>.spawn`, `run.snapshot`). Looks
like an ordinary tool from the agent's LLM context.

## W

**Workflow** — a deterministic linear pipeline of steps
(`agent.run` and `tool.call`), authored as `workflows/<name>.toml`
and persisted as JSONL. Compare with **orchestration**: workflows
are externally driven, orchestration is in-run-driven. See
[workflows](../explanation/workflows.md).
