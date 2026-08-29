# Define agents and tools in directories

Authoring every agent and tool inline in `tau.toml` scales poorly: prompt-heavy
agents bloat the file, and there is no "add an agent by adding a file" flow.
`[dirs]` adds that flow as pure sugar over the existing `[agents.*]` /
`[tools.*]` tables — a directory-defined entry is an alternative serialization
of the same table entry, merged into the project config before validation. IR,
governance, capability fit, and `tau verify` semantics are unchanged.

## Opt-in: the `[dirs]` table

Discovery is explicit, never conventional. Declare it in the root `tau.toml`:

```toml
[dirs]
agents = "agents"   # scans agents/**/*.{md,toml}
tools  = "tools"    # scans tools/**/*.toml
```

Both keys are optional; unknown keys are rejected. Each declared root must be:

- a relative path that stays inside the project root after joining and
  canonicalizing (no absolute paths, no `..` escapes);
- present on disk (declared ⇒ must exist; an existing empty directory is
  valid and simply yields zero definitions);
- not starting with `.` or `_` (that namespace is reserved for the hygiene
  escape hatch below, and for `.tau/` scope state);
- disjoint from the other declared root (no nesting one inside the other).

`[dirs]` requires a project root to scan from. `ProjectConfig::parse_str`
(the in-memory, rootless parse used by tests and library callers) rejects a
`[dirs]` table outright; `ProjectConfig::from_path` — used by every real
loading path (`tau run`, `tau build`, `tau dev`, `tau check`, …) — always has
a root and scans normally.

## Naming: path = name

An entry's engine name is its path relative to the kind root, with the
extension stripped and segments joined with `/`:

```
agents/triage.md            → agent "triage"
agents/review/strict.md     → agent "review/strict"
agents/perf/strict.md       → agent "perf/strict"     (distinct; never ambiguous)
tools/github/search.toml    → tool  "github/search"
```

References use this full name everywhere — `tool_refs`, `subflow`,
`[allow.tools]` keys, traces, `tau list`. **Moving a file renames the
definition**: any reference to the old name becomes dangling and fails the
build loudly — e.g. `agent "review" references unknown tool "github/search"`
— rather than resolving silently. There is no suggestion of the new name in
the error; you have to know what you renamed it to.

Nesting works for both kinds. (It did not until
[ADR-0070](../decisions/0070-agent-id-grammar.md) widened the agent-id
grammar — before that a `/` or `_` in an *agent* name failed `tau build`.)

Each path segment (directory or file stem) must match `[a-z0-9_-]+` —
lowercase ASCII, digits, hyphen, underscore; no dots, no spaces, no Unicode.
This is stricter than inline `[agents.X]` naming on purpose: it rules out
case-insensitive-filesystem collisions (macOS/Windows vs. Linux CI), macOS
NFD/NFC drift, and clashes with the `skill.<name>.spawn` virtual-tool
namespace (`.`) and the `__tau::goal::*` reserved ids (`::`). A Windows `\` in
a path is normalized to `/` before the name is derived, so names are portable
across platforms.

**Agent** names carry one extra rule, checked at scan time against
`tau_domain::AgentId`: every segment must start with a letter or digit (so
`agents/-draft.md` is refused — an id must never be mistakable for a CLI
flag), and the whole `/`-joined name must be at most 64 bytes. That length
cap is also the only bound on nesting depth; there is no separate depth
limit. Tool names have neither rule — an agent name becomes a typed
identity in the bundle, a tool name stays a free-form string.

Names containing `/` are also legal for **inline** definitions, using a
quoted TOML key — required so a dir-authored and an inline-authored entry can
have the same name:

```toml
[agents."review/strict"]
display_name = "Strict Reviewer"
package      = "anthropic@^1"
```

Inline names otherwise keep today's unrestricted charset; only the directory
surface enforces `[a-z0-9_-]+`.

## `agents/**/*.md` — YAML frontmatter + body-as-prompt

```markdown
---
display_name: Strict Reviewer
package: anthropic@^1
model: fast
tool_refs: ["github/search"]
max_turns: 8
---
You are a strict code reviewer. …
```

- The frontmatter is YAML between `---` fences (the `SKILL.md` / Claude Code
  convention) and deserializes into the **full existing `[agents.X]` schema**
  — same fields, same validation, no second dialect. `display_name` and
  `package` are required, exactly as inline. Unknown fields are rejected.
- The `---` fences are required even when the frontmatter is empty. A file
  with no fence is treated as a stray doc, not a definition: the error
  suggests adding frontmatter or prefixing the file with `_` to ignore it.
- Two frontmatter keys are forbidden, each with a targeted error:
  - `name` — the file path *is* the name; there is no second place to
    declare it.
  - `prompt` (i.e. `system` / `system_file`) — the markdown body **is** the
    system prompt, so a frontmatter `prompt` would silently fight it.
- The body (which may be empty, equivalent to no prompt) becomes the agent's
  inline system prompt, lowering to `PromptSource::Inline` exactly like an
  inline-authored twin — hermetic and byte-equal, no new prompt-source
  variant.
- YAML parsing is strict: duplicate keys error, unknown fields error,
  ambiguous scalars (`yes`/`no`/`on`, `08`-style numbers) resolve as the
  target field's type or error — never silently coerce — and anchors/merge
  keys are rejected.

## `agents/**/*.toml` and `tools/**/*.toml` — plain TOML

The file body is exactly the contents of the corresponding `[agents.X]` /
`[tools.X]` table, without the wrapping table header:

```toml
# agents/review/thorough.toml
display_name = "Thorough Reviewer"
package      = "anthropic@^1"

[prompt]
system = "Review every file in the diff, not just the hunk."
```

`name` is forbidden here too (same reason as the `.md` case). Unlike `.md`
files, `.toml` agent files have no separate body, so `system` / `system_file`
**are** allowed in the table. `system_file` paths (and MCP tool `roots`
paths) resolve relative to the project root and are containment-guarded, the
same as inline definitions.

## Hygiene rules (strict scan)

The scan is recursive and deterministic (entries are visited in sorted
order). Within a declared root:

- `*.md` (agents root only) and `*.toml` files must parse as definitions — a
  broken file is a build error, never silently skipped.
- Any file or directory whose name starts with `_` or `.` is ignored
  wholesale. This is the deliberate "not a definition" escape hatch:
  `_README.md`, `_drafts/` (an entire draft subtree), and it also covers
  `.DS_Store`.
- `Thumbs.db` and `desktop.ini` are ignored as known OS junk.
- Anything else — an unexpected extension, or a `.md` file under the tools
  root — is a hard error naming the file and pointing at the `_` escape.
- Symlinks are never followed: encountering one (file or directory) is a
  hard error, to keep the scan hermetic and loop-free.
- A directory containing only ignored entries produces nothing and is not an
  error.

## Collisions

Only full-name collisions exist, and every one is a hard error — never
last-wins:

- `review/strict.md` and `review/strict.toml` in the same root;
- a directory file and an inline `[agents."review/strict"]` entry with the
  same name.

Cross-kind name reuse is *not* a collision: tool `x/y` and agent `x/y` are
separate namespaces today, exactly as with inline definitions.

## Gotchas

- **A bundle carrying a namespaced agent id declares `schema_version = 6`.**
  Nested and underscored *agent* names (`review/strict`, `my_agent`) work end
  to end since [ADR-0070](../decisions/0070-agent-id-grammar.md), but a
  bundle that contains one cannot be read by a tau older than that change —
  it refuses with `unsupported schema_version: 6` rather than a charset
  complaint. A project whose agent names are all plain kebab-case keeps its
  previous bundle version, so this only bites once you adopt the wider
  charset. (Before ADR-0070 such a name failed `tau build` with exit 2 and
  made `tau resolve` panic; if you are reading an older copy of this page,
  that is the limitation it describes.)
- **Moving a file renames the definition.** There is no separate identity —
  the path is the name. Update every `tool_refs` / `subflow` / `[allow.*]`
  reference before (or immediately after) moving a file; a stale reference
  fails `tau build` loudly (an `agent "x" references unknown tool "y"`
  style error) rather than silently resolving to nothing — but the error
  does not suggest the new name.
- **CRLF is normalized at build time.** `\r\n` is rewritten to `\n` in both
  the YAML frontmatter and the markdown body before hashing, so a prompt
  authored (or checked out) with Windows line endings hashes identically to
  the same prompt on Linux/macOS — `tau verify` stays green across an
  autocrlf checkout.
- **`tau check` has a `dirs` category** that warns when a definition file
  under a `[dirs]` root is matched by `.gitignore`: it builds fine locally
  but is silently absent from a fresh clone or CI, because nothing tracks
  it.
- **`tau dev` watches `[dirs]` roots recursively.** A file move is observed
  as a remove-then-add, matching the "moving renames the definition"
  semantics above — the REPL reloads with the old name gone and the new name
  present.
- **`.ts`-authored projects cannot declare `[dirs]` yet.** The TS authoring
  surface has no `dirs()` factory, so the table is unrepresentable there;
  this is a known v1 gap, not an oversight.

## See also

- [ADR-0069 — Directory-based tool & agent definitions](../decisions/0069-directory-based-definitions.md)
- [Project manifest schema](../reference/project-manifest-schema.md)
