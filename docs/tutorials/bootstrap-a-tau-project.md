# Bootstrap a tau project

This tutorial walks through creating a fresh tau project, exploring
the generated `tau.toml`, and understanding how the agent declaration
fits together with packages and capabilities. By the end you will:

- Have a working `tau.toml` in a project directory.
- Understand the `[project]` / `[agents.<id>]` / `[agents.<id>.prompt]`
  layout.
- Know which `tau` verbs to reach for at each step.
- Have a concrete map of what needs to happen between here and a
  running agent.

> **Phase 0 honest framing.** At the time of writing, tau core is
> Phase 1. The five real plugin packages (`anthropic`, `openai`,
> `ollama`, `fs-read`, `shell`) ship as workspace binaries, not as
> publicly-installable git URLs with full manifests. End-to-end
> `tau install <real-llm-backend>` against a hosted URL is not yet
> a tested user flow. This tutorial focuses on the part of the flow
> that *is* working today (`tau init`, project layout, agent
> declarations) and shows you how to inspect the rest with the CLI.
> See `ROADMAP.md` for plugin-distribution status.

## Step 1: scaffold the project

Pick (or create) a directory and run:

```bash
mkdir my-project && cd my-project
tau init --allow
```

You'll see:

```
created /path/to/my-project/tau.toml
hint: add `.tau/` to your .gitignore
```

`tau init` is idempotent only on first run; a subsequent call without
`--force` errors out with `tau.toml already exists`. Add `--dry-run`
to preview the file without writing.

The `--allow` flag scaffolds a **governed** project — one with an
`[allow]` constitution declaring the ceiling of capabilities the
project permits. `tau build` and the dev-path `tau run` are governed
by default (ADR-0057): a `tau.toml` with no `[allow]` section is a
hard error, `error[GOV000]: no [allow] section declared` (exit 2).
`tau init --allow` gives you a starting ceiling so you never hit that
wall. (A bare `tau init` still works for exploring the layout, but
building it later requires either adding `[allow]` or passing
`--allow-ungoverned`.) See
[Capabilities and consent](../explanation/capabilities-and-consent.md)
for the full governance model.

The hint matters: tau-pkg installs packages into the project's
`.tau/` directory as machine-local state (per ADR-0004 §6). Treat
it like `node_modules/` or `target/` — gitignore it.

## Step 2: read the scaffolded `tau.toml`

Open `tau.toml`:

```toml
[project]
name = "my-project"

# Governance ceiling (ADR-0057). `tau build` refuses to build unless every
# capability your agents and tools use is within this [allow] section. The
# scaffold seeds it with the commented least-privilege union of your installed
# packages' declared capabilities — uncomment and narrow what you actually need.
[allow]
#   "fs.read" = { paths = ["./**"] }
#   "net.http" = { hosts = ["api.example.com"] }

# When an [allow] ceiling is declared, model aliases live under [allow.models]:
#   [allow.models.default]
#   backend = "anthropic"
#   model   = "claude-haiku-4-5"

[agents.example]
display_name = "Example Agent"
package      = ""
model        = ""

[agents.example.prompt]
system = """
You are an example agent. Edit this prompt to give yourself a job.
"""
```

Four blocks:

### `[allow]`

The project's governance ceiling (ADR-0057). `tau build` and dev
`tau run` are governed by default: they refuse to run unless every
capability the project's agents and tools resolve to falls inside
this section. `tau init --allow` seeds it with the commented
least-privilege union of your installed packages' declared
capabilities — you uncomment and narrow what you need. Model aliases,
when you add them, live under `[allow.models.<alias>]` rather than a
top-level `[models]`. Full model: [Capabilities and
consent](../explanation/capabilities-and-consent.md).

### `[project]`

Identifies the project. Just a name today; more fields land as the
project model grows. The name doesn't need to be unique across the
internet — it's a local label.

### `[agents.<id>]`

Each agent the project knows about appears as a sub-table under
`[agents.<id>]`. The id (`example` here) is what you pass to
`tau run` and `tau chat`:

```bash
tau chat example      # start an interactive REPL with this agent
tau run example "..." # one-shot
```

Two fields are load-bearing:

- `package` — a git URL pointing at the agent's package. The
  package's manifest declares the agent's default capabilities,
  default LLM backend, default system prompt. Today, with no
  published-package ecosystem, you'd typically point this at a
  local `file://` URL during development.
- `model` — a *model alias*, not a vendor model id. The alias is a key
  into the project's model table: `[allow.models.<alias>]` in a
  governed project (one with an `[allow]` ceiling), or top-level
  `[models.<alias>]` in an ungoverned one. Each alias entry names the
  concrete `backend` package and the vendor `model` string it resolves
  to, so agent blocks reference a policy ("default", "fast") instead of
  hard-coding a vendor string in every agent:

  ```toml
  [allow.models.default]
  backend = "anthropic"
  model   = "claude-haiku-4-5"

  [agents.example]
  model = "default"   # the alias — not "claude-haiku-4-5"
  ```

The example scaffold leaves both empty so `tau chat example` fails
loudly with "package must be non-empty" instead of silently picking
something. The intent is *you*, the project author, set these.

### `[agents.<id>.prompt]`

The agent's system prompt. Two mutually-exclusive forms:

```toml
[agents.example.prompt]
system = "..."

# or

[agents.example.prompt]
system_file = "prompts/example.md"
```

Setting both surfaces `PromptAmbiguous` at load. The `system` form
is convenient for short prompts; `system_file` keeps long prompts
out of `tau.toml` and lets you put them under version control as
plain Markdown.

## Step 3: discover the CLI

Before wiring up an actual agent, get a feel for what `tau` can do.
`--help` lists every verb:

```bash
tau --help
```

The ones you'll reach for first:

| Verb | Purpose |
|---|---|
| `tau init` | scaffold a `tau.toml` (you just used this) |
| `tau install <url>` | install a package from a git URL into the active scope |
| `tau list` | show installed packages |
| `tau resolve` | re-derive the lockfile from `tau.toml` |
| `tau check` | pre-flight validation of the whole project (config, lockfile, packages, sandbox, plugins, skills, MCP contracts, governance) in one CI/IDE-friendly verb |
| `tau chat <agent-id>` | interactive REPL with the agent |
| `tau run <agent-id> "<prompt>"` | one-shot invocation |
| `tau verify` | check installed packages match the lockfile (content hashes) |
| `tau plugin describe <name>` | low-level: show a plugin's declared capabilities |
| `tau sandbox probe` | show which sandbox adapters are available on this host |

Each verb has its own `--help`. `tau install --help` lists the
flags (`--global`, `--dry-run`, `--force`, `--yes`).

## Step 4: explore your scope

Even with no agents wired up, your project now has a scope:

```bash
ls -la .tau/   # nothing yet
tau resolve    # creates .tau/ and the empty lockfile
```

`tau resolve` is the verb that turns `tau.toml`'s declarations into
a concrete lockfile. It clones every declared package source — each
agent's `package` plus the project's top-level `packages` list, which
is where the `backend` named by a model alias comes from — walks
dependencies, and writes
`.tau/lockfile.toml` + `.tau/config.toml`. Today, with empty
`package` fields, it will fail with a guided error — that's
expected. It tells you exactly what `tau.toml` field is missing.

When you do have a working source URL, the lockfile is what tau
actually reads at run time. `tau.toml` declares intent; the
lockfile is the resolved truth, hashed and version-pinned.

## Step 5: understand the agent loop (conceptual)

```mermaid
sequenceDiagram
    actor User
    participant CLI as <code>tau chat</code>
    participant Kernel as Runtime kernel
    participant Sandbox as Sandbox adapter
    participant LLM as LLM-backend<br/>plugin
    participant Tool as Tool plugin

    User->>CLI: tau chat example
    CLI->>CLI: load tau.toml + lockfile
    CLI->>Sandbox: resolve adapter per plugin
    Sandbox->>LLM: spawn (wrapped)
    Sandbox->>Tool: spawn (wrapped)
    loop until /exit
        User->>CLI: prompt
        CLI->>Kernel: Message
        Kernel->>LLM: completion request
        LLM-->>Kernel: stream tokens + tool calls
        Kernel->>Tool: tool.call (capability-checked)
        Tool-->>Kernel: result
        Kernel-->>CLI: tokens
        CLI-->>User: rendered output
    end
    User->>CLI: /exit
    CLI->>LLM: kill (drop)
    CLI->>Tool: kill (drop)
```

Here's what would happen on a fully-wired `tau chat example`:

1. **Load `tau.toml`**: parse `[agents.example]`.
2. **Resolve**: read the lockfile, locate the agent's package and
   LLM backend.
3. **Sandbox-check**: for each plugin involved, pick an adapter
   (`native` / `darwin` / `container` / `passthrough`) per the
   tier model.
4. **Spawn plugins**: the LLM backend and any tool plugins spawn
   as subprocesses under the chosen sandbox adapter.
5. **REPL**: each user prompt becomes a `Message`, sent to the
   kernel, which routes to the LLM backend, processes the
   response (tool calls are dispatched, capabilities are
   checked), and streams tokens back.
6. **Shutdown**: on `/exit` or Ctrl-D the runtime drops, plugin
   processes are killed.

The whole loop is exercised end-to-end in
`crates/tau-cli/tests/cmd_chat*.rs` using the in-repo `echo-llm`
plugin (a toy backend that replays canned responses). That test
suite is the executable reference for the lifecycle described
above.

## Where to go next

You now know the shape. The natural next directions:

- **Want to write a reusable behaviour for an agent?** Read
  [Build your first skill](build-your-first-skill.md) — a complete
  worked tutorial that ships an end-to-end working artifact.
- **Want to harden the project before you wire it up?** Read
  [Configure the sandbox tier](../how-to/configure-sandbox-tier.md).
- **Want to understand why the model has this shape?** Read
  [Packages](../explanation/packages.md) and
  [Capabilities and consent](../explanation/capabilities-and-consent.md).
- **Tracking what's shipping?** `ROADMAP.md` at the repo root.

## Reference

- [`CONSTITUTION.md`](../../CONSTITUTION.md) — G1–G17 explain why the
  project / agent / package separation is the way it is.
- [Packages](../explanation/packages.md) — the package model the
  `package` field and a model alias's `backend` point at.
- [Capabilities and consent](../explanation/capabilities-and-consent.md)
  — what the agent is actually allowed to do once it loads.
- [Sandboxing](../explanation/sandboxing.md) — what happens between
  spawn and run.
