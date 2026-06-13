# `fs-write` tool plugin — design

Date: 2026-06-13
Status: accepted (brainstorming)

## Summary

A capability-gated, in-tree Tool plugin that **mutates** a single
absolute path under the calling agent's `fs.write` capability scope.
It is the write-side mirror of the existing
[`fs-read`](../../../crates/tau-plugins/fs-read/README.md) plugin:
same crate shape, same path-validation + glob-admission machinery,
same two-tier error model, same SDK runner.

One plugin exposes one tool (`fs-write`) with **two modes** selected by
a `mode` discriminator:

- **`write`** — create-or-truncate a file with full base64 `contents`.
- **`edit`** — replace `old_str` with `new_str` (Claude-Code-style),
  default exactly-once, with an explicit `replace_all` opt-in.

Both modes are authorized by a single `fs.write` allowlist grant.

## Motivation

`fs-read` lets agents read files within a glob scope; there is no
in-tree, capability-gated way to *write*. This plugin closes that gap
while staying within the established Tool-plugin shape (ADR-0008 IPC,
`tau_plugin_sdk::run_tool_with_config`, `tau.toml` manifest) so it
installs and sandboxes exactly like `fs-read` (`required_tier =
"strict"`).

## Non-goals

- No append / patch / multi-file / rename operations (YAGNI; `write`
  + `edit` cover the agent-loop need).
- No Codex-style context-anchored diff format (larger surface than
  mirroring `fs-read` warrants).
- No new sandbox work — runs unsandboxed on host like `fs-read` v0.1,
  same trust caveats (Constitution G12 / ROADMAP Tier 3).

## Topology (decision A)

One crate, one binary, one `Tool` impl, one tool name — a 1:1 mirror
of `fs-read`:

```
crates/tau-plugins/fs-write/
├── Cargo.toml          [[bin]] fs-write-plugin  [lib] fs_write_plugin_lib
├── tau.toml            [plugin] bin = "fs-write-plugin"
│                       [[capabilities]] kind = "fs.write", paths = []
│                       [sandbox] required_tier = "strict"
├── Dockerfile          (builds fs-write-plugin; mirrors fs-read)
├── .dockerignore
├── README.md
├── src/
│   ├── lib.rs          pub mod config; pub mod plugin; pub(crate) mod path_check;
│   ├── main.rs         run_tool_with_config::<FsWritePlugin>(...)
│   ├── config.rs       FsWriteConfig {}  (empty, #[non_exhaustive])
│   ├── path_check.rs   validate_path + admit + admit_with_deny  (ported verbatim)
│   └── plugin.rs       FsWritePlugin : Tool   name() -> "fs-write"
└── tests/invoke.rs     FakeStdioPeer integration tests
```

Rejected alternatives:
- **B (two tool names in one process)** — the SDK runner drives exactly
  one `Tool` impl; multiplexing two names departs from the `fs-read`
  shape.
- **C (two crates)** — 2× boilerplate, duplicated `path_check`/`config`,
  two grants for one logical capability (file mutation). Over-engineered.

## Tool schema (decision: discriminated `oneOf`)

The schema is a discriminated union keyed on `mode`. Each branch pins
`mode` to a `const`, fixes its own `required` set, and sets
`additionalProperties: false` so a cross-mode field mix
(e.g. `old_str` in `write`) is a schema-level reject, not a silent
ignore.

```jsonc
{
  "type": "object",
  "oneOf": [
    {
      "title": "write",
      "properties": {
        "path":     { "type": "string", "description": "Absolute path. No `..`. Created or truncated." },
        "mode":     { "const": "write" },
        "contents": { "type": "string", "description": "Base64-encoded file bytes (symmetric with fs-read)." }
      },
      "required": ["path", "mode", "contents"],
      "additionalProperties": false
    },
    {
      "title": "edit",
      "properties": {
        "path":        { "type": "string", "description": "Absolute path. No `..`. Must already exist." },
        "mode":        { "const": "edit" },
        "old_str":     { "type": "string", "description": "Exact substring to replace. Non-empty." },
        "new_str":     { "type": "string", "description": "Replacement text. May be empty to delete." },
        "replace_all": { "type": "boolean", "default": false,
                         "description": "Replace every occurrence. Default false → old_str must match exactly once." }
      },
      "required": ["path", "mode", "old_str", "new_str"],
      "additionalProperties": false
    }
  ]
}
```

### Schema ↔ parser single source of truth

The Rust args type is the same discriminated union, so schema and
parser cannot drift:

```rust
#[derive(Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum WriteArgs {
    Write { path: String, contents: String },
    Edit  { path: String, old_str: String, new_str: String,
            #[serde(default)] replace_all: bool },
}
```

`tag = "mode"` + `deny_unknown_fields` enforces on the parse side what
`oneOf` + `additionalProperties:false` enforces on the schema side. A
drift test feeds each canonical example through
`serde_json::from_value::<WriteArgs>` and asserts the variant, plus a
negative test that `mode:"write"` + `old_str` fails to parse.

### Response shape (both modes)

```json
{ "bytes_written": 42, "path": "/abs/path" }
```

`bytes_written` mirrors `fs-read`'s `size` field (the write-side
analog). Returned as `ToolContent::Json` with `is_error: false`.

## Edit semantics (decision 3a)

`old_str` matching is **exact substring** (byte-for-byte, indentation
included — same constraint Claude Code documents).

| | `replace_all=false` (default) | `replace_all=true` |
|---|---|---|
| 0 matches | `is_error:true` "not found" | `is_error:true` "not found" |
| 1 match | replace | replace |
| ≥2 matches | `is_error:true` "matched N times; add context or set replace_all" | replace all N |

- Empty `old_str` is rejected (Tier ① `BadArgs`) — it would match at
  every boundary.
- `edit` requires the file to **already exist**; a missing file is an
  IO error → Tier ② `is_error:true`.
- The model disambiguates a genuine 2-match case either by widening
  `old_str` to a unique window or by setting `replace_all:true` — the
  exact contract Claude Code's Edit tool exposes.

## Error model (decision 3b) — two tiers, mirroring `fs-read`

```
① STATIC  (args / shape / scope)  → ToolError::BadArgs        → RPC error
② RUNTIME (touches the filesystem) → ToolResult{is_error:true} → RPC ok + flag
```

**Tier ① `ToolError::BadArgs`** (request malformed or not permitted —
do not retry verbatim):
- missing / unknown `mode`; malformed args (`deny_unknown_fields` /
  `additionalProperties`)
- path empty / NUL byte / not absolute / `..` traversal
- path out of `fs.write` scope (deny wins, per spec §9 shape)
- empty `old_str`
- decoded size (write) or post-edit length (edit) over `max_bytes`

**Tier ② `ToolResult{is_error:true}`** (well-formed and allowed, but the
filesystem outcome failed — model may fix and retry):
- IO error (parent dir missing, permission denied, edit on nonexistent
  file)
- base64 decode failure of `contents`
- edit `old_str` 0 matches, or ≥2 with `replace_all:false`

`BadArgs` reason strings follow `fs-read`'s `path_check::BadArgs::reason()`
convention (`"fs-write: ..."`).

## `max_bytes` enforcement (decision 3c)

The one piece of net-new logic versus a straight read→write port.

- `init` captures `max_bytes` from the `FsCapability::Write` grant(s)
  into the session alongside the globs (path-independent, like the
  glob flattening).
- **write mode**: checked against the decoded byte length of
  `contents`, *before* touching the file.
- **edit mode**: checked against the resulting file length *after*
  applying the replacement in memory, *before* writing back.
- Over-cap → Tier ① `BadArgs`
  (`"fs-write: write of N bytes exceeds max_bytes cap of M"`) — it is a
  capability-scope violation, same class as out-of-glob.
- `max_bytes = None` → no cap.

**Multiple grants**: mirror how `paths` flatten — *most permissive
wins*. The effective cap is `None` if any `fs.write` grant is
uncapped, else the maximum of the present caps. (Flagged as an edge
case so it is not a silent hole.)

## Session state

```rust
pub struct FsWriteSession {
    allowed_globs: Vec<String>,   // from FsCapability::Write.paths (flattened)
    denied_globs:  Vec<String>,   // from deny_entries["fs.write"]
    max_bytes:     Option<u64>,   // net-new vs fs-read
}
```

`capabilities()` declares the structural `fs.write` cap with empty
`paths` (built via JSON deser because `FsCapability::Write` is
`#[non_exhaustive]`), exactly as `fs-read` declares `fs.read`. The
kernel verifies the agent has *some* `fs.write`; the plugin does the
fine-grained glob + size check against `ctx.granted_capabilities`.

## Wiring (in-tree plugin)

- Add `crates/tau-plugins/fs-write` to the workspace `members` list in
  the root `Cargo.toml` (next to `fs-read`, `shell`).
- `tau.toml` manifest declares `provides = "tool"`, `kind =
  "rust-cargo"`, `bin = "fs-write-plugin"`, the `fs.write` capability,
  and `required_tier = "strict"`.
- No runtime registry edits needed beyond the workspace member — the
  plugin is discovered/installed via its `tau.toml` like `fs-read`.

## Testing (TDD)

Unit (in-crate, mirror `fs-read`):
- `config.rs` — default / empty-object / unknown-field-rejected.
- `path_check.rs` — full port of `fs-read`'s suite (empty, NUL,
  relative, traversal, admit, admit_with_deny, deny-wins).
- `plugin.rs` — `extract_fs_write_paths` + `extract_max_bytes`
  (flatten paths, most-permissive cap, no-grant empty); `WriteArgs`
  parse table (each variant, `replace_all` default, cross-mode field
  rejected, unknown mode rejected); edit-apply helper (0 / 1 / N
  matches × `replace_all`; empty `old_str`).

Integration (`tests/invoke.rs`, via `FakeStdioPeer`, mirror `fs-read`):
- write to tempfile succeeds; `bytes_written` correct; bytes on disk
  match.
- write out-of-glob-scope → `BadArgs` RPC error.
- write over `max_bytes` → `BadArgs` RPC error.
- edit exactly-once succeeds; disk reflects replacement.
- edit 0 matches → `is_error:true`.
- edit ≥2 matches, `replace_all:false` → `is_error:true`.
- edit ≥2 matches, `replace_all:true` → success, all replaced.
- traversal path rejected (`#[cfg(unix)]`).
- deny overrides allow (`#[cfg(unix)]`).

Each behavior gets a failing test first, then the implementation.

## Open questions

None — all five design forks resolved (topology A, base64 contents,
exactly-once + `replace_all`, two-tier errors, `max_bytes` most-
permissive-wins).
