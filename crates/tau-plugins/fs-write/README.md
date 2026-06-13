# `fs-write` tool plugin

Write or edit a single absolute path under the calling agent's
`fs.write` capability scope. Write-side mirror of
[`fs-read`](../fs-read/README.md).

## Trust model (v0.1, sandboxing deferred)

Runs **unsandboxed** on the host process. The runtime enforces the
capability check at dispatch; the plugin enforces glob-allowlist
scoping + `max_bytes` at invoke time. No memory / CPU / network
isolation (Constitution G12 / ROADMAP Tier 3). Treat installed
plugins as host-equivalent code.

## Usage

Declare the agent's grant in `tau.toml`:

```toml
[[agents.<id>.requires]]
plugin = "fs-write"

[[agents.<id>.capabilities]]
kind = "fs.write"
paths = ["${PROJECT}/src/**"]
max_bytes = 1048576          # optional
```

### write mode — create or truncate

```json
{ "mode": "write", "path": "/abs/path", "contents": "<base64 bytes>" }
```

### edit mode — replace old_str with new_str

```json
{ "mode": "edit", "path": "/abs/path", "old_str": "...", "new_str": "...", "replace_all": false }
```

`old_str` must match **exactly once** unless `replace_all` is true.

Response (both modes):

```json
{ "bytes_written": 1234, "path": "/abs/path" }
```

## Validation rules

- Path must be **absolute**, contain no `..` segments, no NUL bytes.
- Path must match the agent's `fs.write` allow-globs and not its
  deny-globs (deny wins).
- Decoded write size / post-edit file size must not exceed `max_bytes`
  when the grant sets it.

These are **`BadArgs`** (RPC-rejected). Filesystem outcomes — IO
errors, base64 decode failure, `old_str` not-found / ambiguous — are
returned as `ToolResult { is_error: true }` so the LLM may retry.
`edit` requires an existing, UTF-8-decodable file.

## See also

- Spec: [`docs/superpowers/specs/2026-06-13-fs-write-edit-plugin-design.md`](../../../docs/superpowers/specs/2026-06-13-fs-write-edit-plugin-design.md)
- Sibling: [`fs-read`](../fs-read/README.md)
- ADR-0008 §5 (IPC vocabulary).
