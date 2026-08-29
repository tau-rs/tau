# Directory-based tool & agent definitions

Date: 2026-08-22
Status: draft (pending review)

## Motivation

Authoring every agent and tool inline in `tau.toml` scales poorly: prompt-heavy
agents bloat the file, and the one-file model has no answer for "add an agent
by adding a file" — the DX that Claude Code (`.claude/agents/*.md`), Cursor
(`.cursor/rules/`), and Copilot (`.github/prompts/`) have made the ecosystem
default. This spec adds a directory-based authoring surface for **agents** and
**tools** as pure sugar over the existing `ProjectConfig` model: a dir file is
an alternative serialization of the same table entry, merged before lowering.
IR, governance, capability fit, and `tau verify` semantics are unchanged.

## Non-goals

- No generic `include = [...]` fragment mechanism.
- No dir surface for `[allow]`, `[models]`, credentials (governance/scope
  stay root-only), nor steps/triggers/goals (YAGNI; the design extends to
  them later without change).
- No project-local skill dirs — skills keep the package install path.
- No dir-per-definition asset bundles (Claude-Code-skills style); a compatible
  future extension.
- No TS-authoring dir emitter; `.ts` projects get `[dirs]` for free (see
  Merge seam).

## Opt-in: the `[dirs]` table

Discovery is explicit, never conventional. Root `tau.toml`:

```toml
[dirs]
agents = "agents"   # scans agents/**/*.{md,toml}
tools  = "tools"    # scans tools/**/*.toml
```

Both keys optional; unknown keys rejected (`deny_unknown_fields`).

Validation of each declared root:
- Relative path only; after join + canonicalize it must remain inside the
  project root (`starts_with` guard, #536 pattern). Absolute paths and
  escapes are errors.
- Must exist (declared ⇒ present). An existing empty dir is valid (zero defs).
- Must not start with `.` or `_` (reserves `.tau/` scope state; avoids a root
  its own hygiene rules would ignore).
- Roots must not overlap each other (no nesting one inside the other).

The same PR adds the identical containment guard to `system_file`, which is
currently unconstrained (latent gap).

## Naming: path = name

An entry's engine name is its path relative to the kind root, extension
stripped, segments joined with `/`:

```
agents/triage.md            → agent "triage"
agents/review/strict.md     → agent "review/strict"
agents/perf/strict.md       → agent "perf/strict"     (distinct; never ambiguous)
tools/github/search.toml    → tool  "github/search"
```

References use the full name everywhere (`tool_refs`, `subflow`,
`[allow.tools]` keys, traces, `tau list`). Moving a file renames the
definition; dangling references fail the build loudly (e.g. `agent "x"
references unknown tool "y"`), not silently.

Segment charset: `[a-z0-9_-]+` per segment (lowercase ASCII; no dots, no
spaces, no Unicode). Rationale: kills case-insensitive-filesystem collisions
(macOS/Windows vs Linux CI), macOS NFD/NFC drift, clashes with the
`skill.<name>.spawn` virtual-tool namespace (`.`) and `__tau::goal::*`
reserved ids (`::`). Windows `\` is normalized to `/` before naming.

Names with `/` become legal for inline definitions too (quoted keys:
`[agents."review/strict"]`) — required for dir↔inline equivalence. Inline
names keep today's unrestricted charset; the dir surface is stricter than the
language. Verified: no existing charset validation, IR ids are plain strings,
WIT worlds are capability-named — no id plumbing changes. The one
name-becomes-path site, MCP contract pins, nests naturally:
tool `github/search` → `.tau/mcp/github/search.contract.json` (no clash with a
sibling `github.contract.json`).

## Scan rules (strict hygiene)

Recursive, deterministic (sorted walk). Within a root:

- `*.md` (agents root only) and `*.toml` are definitions and MUST parse —
  a broken file is a build error, never skipped.
- Entries (files or dirs) whose name starts with `_` or `.` are ignored
  wholesale (the deliberate "not a definition" escape: `_README.md`,
  `_drafts/`; also covers `.DS_Store`).
- OS-junk allowlist ignored: `Thumbs.db`, `desktop.ini`.
- Anything else — other extensions, `*.md` under the tools root — is a hard
  error naming the file and the `_` escape.
- Symlinks are never followed: hard error (loops, root escape, hermeticity).
- A dir containing only ignored entries produces nothing and is not an error.

## File formats

### `agents/**/*.md` — YAML frontmatter + body-as-prompt

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

(Field names and requiredness are exactly the `[agents.X]` schema —
`display_name` and `package` are required today and stay required.)

- Frontmatter is YAML between `---` fences (matches `SKILL.md` and Claude
  Code convention) and deserializes into the **full existing agent schema**
  (`UncheckedAgent`) — same fields, same validation, no second dialect.
  `deny_unknown_fields` applies per file.
- The `---` fences are required even when empty (a fence-less `.md` is a
  stray doc: error suggests adding frontmatter or `_`-prefixing).
- Forbidden in frontmatter (each a targeted error):
  - `name` — the path is the name (single source of truth).
  - `prompt` (i.e. `system` / `system_file`) — the body IS the system prompt.
- The body (may be empty ⇒ same as no prompt) is injected as the agent's
  inline system prompt (`prompt.system`), lowering to `PromptSource::Inline`
  exactly like an inline-authored twin — hermetic, byte-equal, and requiring
  no new `PromptEntry` variant. (Large-prompt asset promotion stays a future
  option; inline is what `system` produces today.)
- YAML behavior pinned by conformance tests: duplicate keys error; unknown
  fields error; `yes`/`no`/`on` and `08`-style scalars resolve as the serde
  target type or error — never silently coerce; anchors/merge keys rejected.

### `agents/**/*.toml` and `tools/**/*.toml` — plain TOML

The file body is exactly the contents of the corresponding
`[agents.X]` / `[tools.X]` table (no wrapping table header). `name` is
forbidden; agents' `system` / `system_file` ARE allowed here (no md body to
conflict with). `system_file` and MCP `roots` paths resolve relative to the
**project root**, same as inline definitions, and are containment-guarded.

## Equivalence & merge seam

Invariant: `agents/review/strict.md` ≡ `[agents."review/strict"]` with
`prompt.system` set to the body. Both converge on `ProjectConfig` before
`lower_project`, so the byte-equal-IR invariant
(`tau-sdk-codegen/tests/byte_equal.rs`) extends to the dir surface: dir-authored
and inline-authored projects lower to identical IR.

Implementation seam: the merge happens at the **unchecked level inside
`tau-pkg`** — a new root-aware entry point
`ProjectConfig::parse_str_at(toml, project_root)` parses the root config,
scans `[dirs]`, inserts scanned entries into the unchecked maps (collision =
error), then runs the single existing `validate()` pass.
`ProjectConfig::from_path` delegates to it (root = the manifest's parent), so
every `from_path` call site (`load_project`, serve, resolve, chat, check, …)
becomes dirs-aware for free. The rootless `parse_str` errors if `[dirs]` is
present. Lowering (`tau-ir-lower`) is untouched.

TS frontend, v1 deviation: `.ts` projects cannot declare `[dirs]` yet — the
TS authoring surface has no `dirs()` factory, so the table is unrepresentable
there. The root-aware seam is ready; adding the factory is a follow-up.

## Collisions

Only full-name collisions exist, and all are hard errors (never last-wins):

- `review/strict.md` + `review/strict.toml` in the same dir;
- a dir file vs an inline `[agents."review/strict"]`;
- (cross-kind is NOT a collision: tool `x/y` and agent `x/y` are separate
  namespaces today and remain so.)

## Governance

Merged-then-gated: dir entries pass through the same `[allow]` ceiling,
`capability_fit`, and feature-fit as inline ones. Discovery is never a
capability side-door. `[allow]`, `[models]`, and any non-entry table are
unrepresentable in dir files by construction (each file is one entry body).

## Determinism & reproducibility

- Sorted scan; entries land in the existing `BTreeMap`s.
- CRLF: `\r\n` → `\n` normalized in `.md` frontmatter and body **before**
  hashing (verified gap: `system_file` bytes are hashed raw in
  `tau-ir-lower/src/lower/parse.rs:114-134`, so an autocrlf Windows checkout
  changes asset hashes and breaks `tau verify` cross-platform — same class as
  the #553 `*.wit` incident). The same normalization is applied to
  `system_file` reads in this PR.
- Dir files are build inputs like `system_file`: `tau build` reads them at
  lower time; bundles stay self-contained (prompts become assets; config
  becomes IR). `tau verify` rebuild-and-compare picks them up from source.
- `tau check` gains a lint: definition file matched by `.gitignore` (builds
  locally, absent in CI).

## Impacted components

| Component | Change |
|---|---|
| `tau-pkg` (`project/`) | `[dirs]` table parse + validation; scan/merge module; name-charset + collision errors; frontmatter (YAML) deserialize into `UncheckedAgent`/`UncheckedTool`; containment guards |
| `tau-cli` `project_load.rs` | TOML branch switches to the root-aware `from_path` |
| `tau-cli` `build.rs` | MCP contract pin paths nest for `/`-names (`create_dir_all` parent) |
| `tau-cli` `error_render.rs` | new error copy (collision, hygiene, fence, forbidden-field, containment) |
| `tau-ir-lower` | CRLF normalization on prompt bytes (only change) |
| `tau-sdk-codegen` | byte-equal test: dir-authored ≡ inline-authored |
| `tau check` | gitignored-definition lint; dirs category |
| `tau dev` | watch `[dirs]` roots recursively (move = remove + add) |
| docs | authoring how-to + reference page (+ `SUMMARY.md` entries), ADR |

## Testing

- Unit: scan hygiene (each error class), naming (charset, `\` normalization,
  extension stripping), collision matrix (md/toml/inline), `[dirs]`
  validation (escape, overlap, missing, `.`/`_` root), frontmatter forbidden
  fields, YAML conformance pins, CRLF normalization idempotence.
- Integration: dir project lowers; byte-equal vs inline-authored twin;
  `tau verify` green after CRLF-mangled checkout simulation; MCP pin nesting;
  `tau list`/trace show full path-names; build fails with an unknown-ref
  error after a referenced definition file is moved.
- Cross-platform: case-collision fixture rejected by charset rule on Linux
  (where it would otherwise pass).
