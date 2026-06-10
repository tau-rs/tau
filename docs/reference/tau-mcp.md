# `tau mcp` — MCP contract management

The `tau mcp` family of subcommands manages MCP server contracts: pinning,
listing, inspecting, refreshing, and drift-checking. For the design
rationale see [ADR-0038](../decisions/0038-mcp-facilitator.md).

## Synopsis

```
tau mcp pin <NAME> [--from <URL>] [--json]
tau mcp ls [--json]
tau mcp show <NAME> [--json | --sarif]
tau mcp refresh <NAME> [--json]
tau mcp diff <NAME> [--json]
```

## URL schemes

Every `--from URL`, every `[tools.<name>] mcp = "..."` value in
`tau.toml`, and every cassette path accept the same four schemes:

| Scheme | Example | Notes |
|---|---|---|
| `stdio:` | `stdio:npx --yes weather-mcp` | argv after `stdio:` (whitespace-split). Subprocess; OS-level sandbox applied. |
| `https://` | `https://mcp.example.com/v1` | Streamable HTTP over HTTPS (production). |
| `http://` | `http://localhost:8080/mcp` | Plain HTTP. Accepted; `tau build` emits a warning. |
| `cassette:` | `cassette:./fixtures/weather.jsonl` | JSONL cassette. Path is relative to the project root or absolute. |

## Verbs

### `tau mcp pin <NAME>`

Probes the MCP server, captures the handshake + `tools/list` response,
and writes `.tau/mcp/<NAME>.contract.json`. If a pin file already exists
it is overwritten.

**Arguments:**

| Argument / flag | Type | Description |
|---|---|---|
| `<NAME>` | positional string | Tool name. Must match a `[tools.<NAME>]` block in `tau.toml`. |
| `--from <URL>` | optional string | Override the URL. Defaults to the `mcp = "..."` value in `tau.toml`. Accepts all four URL schemes. |
| `--json` | flag | Emit machine-readable JSON instead of human output. |

**Human output:** one line summarising the server URL, pin file path, tool
count, and the first 16 hex characters of the contract hash.

**JSON output:**

```json
{
  "ok": true,
  "name": "weather",
  "path": ".tau/mcp/weather.contract.json",
  "url": "stdio:npx --yes weather-mcp",
  "contract_hash_hex": "0123…abcdef…",
  "tools_count": 2
}
```

**Exit codes:** 0 on success; non-zero on probe or I/O failure.

### `tau mcp ls`

Enumerates every pinned contract file found under `.tau/mcp/` in the
current project. Results are sorted alphabetically by name.

**Arguments:**

| Flag | Description |
|---|---|
| `--json` | Emit machine-readable JSON. |

**Human output:** one line per pin — name, URL, server name, tool count,
and a 16-character hash prefix.

**JSON output:**

```json
{
  "pins": [
    {
      "name": "weather",
      "url": "stdio:npx --yes weather-mcp",
      "server_name": "weather",
      "tools_count": 2,
      "contract_hash_hex": "0123…",
      "path": ".tau/mcp/weather.contract.json"
    }
  ]
}
```

**Exit codes:** 0 always (an empty directory is not an error).

### `tau mcp show <NAME>`

Reads `.tau/mcp/<NAME>.contract.json` and renders the full pinned
contract. Does not probe the live server.

**Arguments:**

| Argument / flag | Type | Description |
|---|---|---|
| `<NAME>` | positional string | Tool name. |
| `--json` | flag | Render the `PinnedContract` as canonical JSON. Mutually exclusive with `--sarif`. |
| `--sarif` | flag | Render as a SARIF 2.1.0 document. Mutually exclusive with `--json`. |

**Human output:** structured key-value block with name, URL, server name +
version, full hash, tool count, and an indented list of tool names.

**JSON output:** the `PinnedContract` struct serialised directly (see
[File formats](#file-formats) below).

**SARIF output:** a SARIF 2.1.0 document. Single tool (`tau-mcp`), single
rule (`tau-mcp/show`), zero results. The pinned contract payload is
embedded at `runs[0].properties.embedded`.

```json
{
  "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
  "version": "2.1.0",
  "runs": [{
    "tool": {
      "driver": {
        "name": "tau-mcp",
        "informationUri": "https://github.com/LEBOCQTitouan/tau",
        "rules": [{ "id": "tau-mcp/show" }]
      }
    },
    "results": [],
    "properties": { "embedded": { ... } }
  }]
}
```

**Exit codes:** 0 on success; non-zero if the pin file is missing or
unparseable.

### `tau mcp refresh <NAME>`

Re-probes the live server and overwrites `.tau/mcp/<NAME>.contract.json`.
Reports whether the contract changed relative to the previous pin.

**Arguments:**

| Argument / flag | Type | Description |
|---|---|---|
| `<NAME>` | positional string | Tool name. |
| `--json` | flag | Emit machine-readable JSON describing the diff. |

**Human output:** one line with `(CHANGED)`, `(NEW)`, or `(unchanged)` and
the old/new hash prefix + tool count.

**JSON output:**

```json
{
  "name": "weather",
  "url": "stdio:npx --yes weather-mcp",
  "changed": true,
  "new_hash": "abcd…",
  "prev_hash": "0123…",
  "tools_count": 3
}
```

`changed` is `true` when the new hash differs from the previous pin's
hash. It is always `true` when no previous pin existed.
`prev_hash` is `null` when there was no previous pin.

**Exit codes:** 0 on success; non-zero on probe or I/O failure.

**Note:** refresh only updates the pin file. Re-run `tau build` to update
`Tau.lock` to reflect the new contract.

### `tau mcp diff <NAME>`

Reads the pin file at `.tau/mcp/<NAME>.contract.json` and probes the live
server, then compares the two contract hashes. Does not modify any file.

**Arguments:**

| Argument / flag | Type | Description |
|---|---|---|
| `<NAME>` | positional string | Tool name. |
| `--json` | flag | Emit machine-readable JSON. |

**Human output:** one line if no drift (`no drift: …`); a multi-line block
if drift is detected showing pin hash, live hash, tool counts, and server
versions.

**JSON output:**

```json
{
  "name": "weather",
  "drift": false,
  "pin_hash": "0123…",
  "live_hash": "0123…",
  "pin_tools": 2,
  "live_tools": 2,
  "pin_server_version": "1.0",
  "live_server_version": "1.0"
}
```

**Exit codes:** 0 (no drift) or 64 (drift detected). Non-zero for probe or
I/O failures.

## Related: `tau check mcp-contracts`

The `mcp-contracts` category is available as a standalone `tau check`
phase or as part of the full `tau check` aggregator.

```
tau check mcp-contracts
tau check                  # runs all categories
```

Walks `Tau.lock` entries that have `pinned_contract: Some(path)` and
verifies:

1. The pin file referenced by the lockfile exists.
2. The pin file parses as a valid `PinnedContract`.
3. The pin's self-hash is internally consistent
   (`PinnedContract::verify_self_hash`).
4. The pin's `contract_hash_hex` matches `LockedMcpEntry.contract_hash`
   in `Tau.lock`.

Rule IDs emitted:

| Rule ID | Severity | Meaning |
|---|---|---|
| `tau.mcp.contract.missing` | Error | Pin file referenced by lockfile is absent. Remedy: `tau mcp pin <name>` |
| `tau.mcp.contract.malformed` | Error | Pin file exists but is unparseable. Remedy: `tau mcp refresh <name>` |
| `tau.mcp.contract.self_drift` | Error | Pin's internal hash doesn't match its own content. Remedy: `tau mcp refresh <name>` |
| `tau.mcp.contract.lockfile_drift` | Error | Pin's hash doesn't match the lockfile's `contract_hash`. Remedy: `tau mcp refresh <name>`, then `tau build` |

All findings are `Severity::Error` and contribute to exit code 2.

## File formats

### `.tau/mcp/<name>.contract.json` (`PinnedContract` schema v1)

```json
{
  "schema_version": 1,
  "url": "stdio:npx --yes weather-mcp",
  "contract_hash_hex": "0123…abcdef (64 hex chars)",
  "contract": {
    "protocol_version": "2025-03-26",
    "server_info": {
      "name": "weather",
      "version": "1.0"
    },
    "tools": [
      {
        "name": "get_forecast",
        "description": "Get a weather forecast.",
        "input_schema": { "type": "object", "properties": { ... } },
        "caps": []
      }
    ]
  }
}
```

`contract_hash_hex` is the SHA-256 (lowercase hex, 64 characters) of the
canonical JSON encoding of the `contract` field. The canonical encoding
sorts object keys and strips insignificant whitespace (per the β.2
canonical-encoder rules).

### `Tau.lock` v7 — `[[mcp_entries]]`

```toml
[[mcp_entries]]
entry            = "weather"
url              = "stdio:npx --yes weather-mcp"
contract_hash    = "0123…abcdef"
pinned_contract  = ".tau/mcp/weather.contract.json"

[[mcp_entries.expanded_tools]]
name        = "get_forecast"
caps        = ["net.http,host=api.weather.com"]
schema_hash = "fedc…3210"
```

`pinned_contract` is optional; it is absent when no pin was written (for
example, after a live build without `tau mcp pin`).

`expanded_tools` entries are the per-server-tool records written during
`tau build`. Each `schema_hash` is the SHA-256 of the tool's `input_schema`
canonical JSON; used for the runtime drift check.

### Cassette format (`.jsonl`)

A cassette is a JSONL file. The first line must be a version header:

```json
{"version": 1}
```

Subsequent lines are `CassetteMessage` records:

```json
{"dir": "in",  "kind": "request",      "id": 0, "method": "initialize", "payload": {...}}
{"dir": "out", "kind": "response",     "id": 0, "payload": {"protocolVersion": "2025-03-26", ...}}
{"dir": "in",  "kind": "request",      "id": 1, "method": "tools/list",  "payload": {}}
{"dir": "out", "kind": "response",     "id": 1, "payload": {"tools": [...]}}
{"dir": "in",  "kind": "request",      "id": 2, "method": "tools/call",  "payload": {"name": "get_forecast", "arguments": {...}}}
{"dir": "out", "kind": "notification", "payload": {"method": "notifications/progress", "params": {...}}}
{"dir": "out", "kind": "response",     "id": 2, "payload": {"content": [{"type": "text", "text": "Sunny, 22 °C"}]}}
```

Field reference:

| Field | Values | Notes |
|---|---|---|
| `dir` | `"in"` or `"out"` | `"in"` = host sent to server (request going in). `"out"` = cassette emits to host (server's reply). |
| `kind` | `"request"`, `"response"`, `"notification"` | JSON-RPC message kind. |
| `id` | integer (optional) | Present on requests and their matching responses; absent on notifications. |
| `method` | string (optional) | Present on requests and notifications. |
| `payload` | object | The `params` (requests/notifications) or `result` (responses) value. |

The replayer matches on `(method, normalized_args)`, where normalized
means object keys are sorted and insignificant whitespace is removed.
Requests that do not match any cassette entry fail with a replay error.

## See also

- [How to use MCP servers in tau](../how-to/mcp-servers.md)
- [ADR-0038](../decisions/0038-mcp-facilitator.md) — the design rationale
- [Capabilities and consent](../explanation/capabilities-and-consent.md)
- [Package manifest schema](package-manifest-schema.md)
