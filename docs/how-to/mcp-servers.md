# How to use MCP servers in tau

Tau projects can reference Model Context Protocol (MCP) servers as tools.
This guide walks through adding a server, pinning its contract for
reproducibility, and detecting drift in CI.

If you are not yet familiar with the capability model that governs what
each tool is allowed to do, read
[Capabilities and consent](../explanation/capabilities-and-consent.md)
first.

## 1. Add a server to `tau.toml`

Declare the server under `[tools.<name>]` using the `mcp` field:

```toml
[tools.weather]
mcp = "stdio:npx --yes @modelcontextprotocol/server-weather"
capabilities = [
    { kind = "net.http", host = "api.weather.com" },
]
```

The `mcp` field accepts four URL schemes:

| Scheme | Example | Notes |
|---|---|---|
| `stdio:<argv>` | `stdio:npx --yes weather-mcp` | Most ecosystem servers. Spawned as a subprocess; OS-level capability enforcement via the existing sandbox stack (landlock / seccomp on Linux, sandbox-exec on macOS). |
| `https://…` | `https://mcp.example.com/v1` | Streamable MCP over HTTPS (production). |
| `http://…` | `http://localhost:8080/mcp` | Plain HTTP (development only). `tau build` emits a warning. |
| `cassette:<path>` | `cassette:./fixtures/weather.jsonl` | Recorded MCP traffic replayed from a JSONL file. For offline and test use. |

The `capabilities` envelope declares the maximum permissions you grant the
server. tau enforces this at the OS level (for stdio) and at the wire level
(for HTTP).

### Sampling and roots (optional)

By default, the server cannot invoke model sampling and receives an empty
roots list. Opt in explicitly:

```toml
[tools.weather]
mcp = "stdio:npx --yes weather-mcp"
capabilities = [{ kind = "net.http", host = "api.weather.com" }]
sampling.models = ["claude-haiku-4-5"]
roots = ["/tmp/weather-cache"]
```

`roots` must be a subset of the tool's `fs.read` capabilities — `tau
build` enforces this at build time.

## 2. Pin the contract

Run `tau mcp pin` to probe the server and capture its capability surface:

```
tau mcp pin weather
```

This writes `.tau/mcp/weather.contract.json`. Commit this file. Subsequent
`tau build` calls use the pinned contract so the server is not re-probed
when running `--offline`.

If you know the URL at pin time but it differs from `tau.toml` (for
example, a staging server), pass `--from`:

```
tau mcp pin weather --from https://staging.mcp.example.com/v1
```

## 3. Build the project

```
tau build
```

During the build, tau:

1. Resolves each `[tools.<name>] mcp = "..."` entry against the pinned
   contract (or probes the live server if no pin exists).
2. Verifies the `capabilities` envelope covers every tool the server
   declares.
3. Expands `tool_refs = ["weather"]` into per-server-tool IR entries
   (`weather.get_current`, `weather.get_forecast`, etc.).
4. Writes the resolved contract hash into `Tau.lock`.

If any invariant is violated, `tau build` fails with a typed error and a
human-readable remediation message.

## 4. Use the tool in an agent

Reference the entry name in `tool_refs`; tau handles the expansion:

```toml
[agents.forecaster]
display_name = "Forecaster"
tool_refs    = ["weather"]
```

The agent's prompt can request `get_forecast` (or whatever tool names the
server exposes) by name. The MCP bridge handles dispatch transparently.

## 5. Refresh when the server changes

Re-probe and overwrite the pin:

```
tau mcp refresh weather
```

Human output reports whether the contract changed. Pass `--json` for
machine-readable output (includes a `changed: bool` field). After
refreshing, re-run `tau build` to update `Tau.lock`.

To inspect drift without touching files, use `diff`:

```
tau mcp diff weather
```

Exit code 0 means no drift; exit code 64 means the live server's contract
differs from the pin.

## 6. Detect drift in CI

Add a `tau check` step to your CI pipeline:

```
tau check mcp-contracts
```

Or run all categories at once:

```
tau check
```

The `mcp-contracts` category walks `Tau.lock`'s MCP entries and verifies:

1. Each pin file referenced by the lockfile is present.
2. Each pin's self-hash is internally consistent.
3. Each pin's hash matches the `contract_hash` recorded in `Tau.lock`.

A mismatch exits with code 2 (fixable). The usual remedy is:

```
tau mcp refresh <name>
tau build
```

then commit the updated pin and lockfile.

## 7. Offline workflow: cassettes

For deterministic offline testing, record a cassette from a live session
and reference it:

```toml
[tools.weather]
mcp = "cassette:./fixtures/weather.jsonl"
```

Cassettes are JSONL files containing recorded MCP message traffic. The
first line must be `{"version":1}`. See
[`tau mcp` reference](../reference/tau-mcp.md#cassette-format) for the
full schema.

Cassette paths are resolved relative to the project root (where `tau.toml`
lives).

## See also

- [`tau mcp` reference](../reference/tau-mcp.md) — every flag for every
  verb
- [ADR-0038](../decisions/0038-mcp-facilitator.md) — the design rationale
- [Capabilities and consent](../explanation/capabilities-and-consent.md) —
  the grant model that governs what a tool can access
- [Write a tool plugin](write-a-tool-plugin.md) — the bespoke plugin
  alternative (MessagePack-RPC subprocess, same sandbox stack)
