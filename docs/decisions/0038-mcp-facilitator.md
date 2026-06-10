# ADR-0038 — MCP Facilitator (β.3)

**Status:** Accepted
**Date:** 2026-06-10
**Supersedes:** none (finalises the placeholder shipped in β.3 PR-1, #280)

## Context

tau's plugin protocol predated the Model Context Protocol (MCP) ecosystem.
The philosophy pivot of 2026-05-29 (`docs/explanation/tau-philosophy.md`)
named MCP as the canonical multi-vendor tool contract that tau should adopt
as a first-class equal of the bespoke plugin protocol. The β.3 sub-project
introduces an MCP-client facilitator so IR programs can reference MCP
servers via `[tools.<name>] mcp = "..."` without changing the IR itself.

This ADR records the as-shipped reality across the six β.3 PRs (#280–#284,
#287, and this PR-6).

## Decision

Adopt MCP as a first-class tool contract on equal footing with the bespoke
plugin protocol. Specifically:

### 1. Two new crates: `tau-mcp` and `tau-mcp-tokio`

Following the β.1 core/tokio split discipline:

- **`tau-mcp`** — `no_std`-friendly. Protocol wire types (JSON-RPC
  envelopes, `initialize`, `tools/list`, `tools/call`,
  `sampling/createMessage`, `roots/list`, `notifications/*`,
  `cancellation`). Contract types, canonical-hash function, pinned-contract
  I/O helpers, `HostHandlers` trait, and the transport-agnostic cassette
  recorder/replayer.
- **`tau-mcp-tokio`** — tokio runtime + transports. stdio subprocess spawn,
  Streamable HTTP client, `host_lifecycle::open` dispatch, and
  `McpBridge` (composable `ToolDispatcher` adapter).

### 2. Transport surface — four URL schemes

| URL scheme | Module | Use case |
|---|---|---|
| `stdio:<argv>` | `tau-mcp-tokio::transport_stdio` | subprocess MCP server (most ecosystem servers) |
| `http://…` | `tau-mcp-tokio::transport_http` | plain-HTTP Streamable MCP (accepted; `tau build` warns) |
| `https://…` | `tau-mcp-tokio::transport_http` | HTTPS Streamable MCP (production) |
| `cassette:<path>` | `tau-mcp-tokio::host_lifecycle::cassette_dial` | recorded MCP traffic replayed from JSONL |

`host_lifecycle::open(url, plan, gate, options)` dispatches on the parsed
`McpUrl` variant and returns a unified `McpClient`.

### 3. Pinned contracts (`.tau/mcp/<name>.contract.json`)

Every referenced server has its `ServerContract` captured at install time
as a `PinnedContract` (schema v1, defined in
`tau_mcp::contract::pinned`). Carries:

- `schema_version: 1`
- `url`: server URL (matches `[tools.<name>] mcp = "..."`)
- `contract_hash_hex`: canonical SHA-256 of the contract body (lowercase hex)
- `contract`: the full `ServerContract` snapshot

Self-integrity is verified via `PinnedContract::verify_self_hash`. Used by
`tau build --offline`, `tau verify --bundle`, and the `tau check
mcp-contracts` phase (PR-6).

### 4. IR shape (β.2 `ToolImpl::Mcp` extended)

`ToolImpl::Mcp` gains `server_tool_name: String` so the IR carries the
per-tool routing info needed at runtime without a runtime lookup.

IR `ToolId` convention for expanded MCP entries: `"<entry>.<server-tool>"`,
e.g. `weather.get_forecast`. The author writes `tool_refs = ["weather"]`;
lowering rewrites it to `["weather.get_current", "weather.get_forecast",
...]` after the `tools/list` expansion stage.

Server-tool names containing `.` are rejected at build time with
`McpBuildError::ServerToolNameContainsDot` (forward-defense against
`ToolId` namespace collisions).

### 5. Lockfile v7

Adds `mcp_entries: Vec<LockedMcpEntry>` to `LockFile`. Each entry records:

- `entry`: tool name from `[tools.<entry>]`
- `url`: resolved MCP server URL
- `contract_hash`: hex SHA-256 of the canonical resolved contract
- `pinned_contract: Option<String>`: path to the pin file (relative to
  project root)
- `expanded_tools: Vec<LockedMcpExpandedTool>`: server-side tool names +
  cap shapes + schema hashes

v6→v7 migration is silent (an empty `mcp_entries` is correct for v6
projects). `SchemaTooNew` handles forward compat without new code.

### 6. Build-time invariants

| Invariant | Error variant |
|---|---|
| Contract reachable at build (live mode) | `McpBuildError::ContractUnreachable` |
| Envelope ⊇ every server-tool's declared caps | `McpBuildError::EnvelopeCoversContract` |
| `roots` ⊆ tool's `fs.read` caps | `McpBuildError::RootsExceedFsCaps` |
| Server contract requires sampling AND `sampling.models` is empty | `McpBuildError::SamplingRequiredByContract` |
| `--offline` mode + pinned file missing | `McpBuildError::PinnedContractMissing` |
| `--offline-strict` mode + pinned hash ≠ live hash | `McpBuildError::PinnedContractStale` |
| Server-tool name contains `.` | `McpBuildError::ServerToolNameContainsDot` |

### 7. Runtime: boot + per-turn dispatch

At `ForwardingDispatcher::new`, for each distinct MCP server URL:

1. Read `{url, contract_hash, expanded_tools}` from the lockfile entry.
2. Read `{sampling.models, roots, caps}` from `tau.toml [tools.<name>]`.
3. Construct `CapabilityPlan` and call `host_lifecycle::open`.
4. Handshake (initialize + `tools/list`); re-hash and compare against
   `lockfile.contract_hash`. Mismatch → `ContractDriftAtBoot` hard error.
5. Construct `HostHandlers` with sampling allowlist + roots + trace
   notifications + abort cancellation.
6. Wrap in `McpBridge` (`impl ToolDispatcher`).

`ForwardingDispatcher` composes: `McpBridge` routes first; bespoke
`plugin_host` handles the rest.

### 8. Capability gate — two enforcement points

| Point | stdio MCP server | HTTP MCP server |
|---|---|---|
| Spawn-time (OS boundary) | `ProcessGate::Sandbox::wrap_spawn` → landlock / seccomp / sandbox-exec / podman | n/a — no spawn |
| Per-call (contract boundary) | `McpBridge::cap_gate.check_outbound` | same + reqwest middleware enforces `net.http` host pinning |

Default-deny inbound handlers:

| Inbound | Default-allow? | Author opt-in |
|---|---|---|
| `sampling/*` | no | `sampling.models = [...]` in `tau.toml` |
| `roots/list` | no (returns `[]`) | `roots = [...]` in `tau.toml` |
| `notifications/*` | yes (observational) | n/a |
| `cancellation/*` | yes (control plane) | n/a |

### 9. CLI surface (PR-6)

| Verb | Effect |
|---|---|
| `tau mcp pin <name> [--from URL]` | Probe a server, write `.tau/mcp/<name>.contract.json` |
| `tau mcp ls [--json]` | Enumerate pinned contracts |
| `tau mcp show <name> [--json\|--sarif]` | Show one pin (human / JSON / SARIF) |
| `tau mcp refresh <name> [--json]` | Re-probe and overwrite the pin; report changed/unchanged |
| `tau mcp diff <name> [--json]` | Read-only drift check. Exit 0 unchanged, exit 64 drift |
| `tau check mcp-contracts` | Aggregator phase: verify pin self-hashes + lockfile drift |

CLI modules live at `tau-cli/src/cmd/mcp/{mod,pin,ls,show,refresh,diff}.rs`,
mirroring the `cmd/skill/` layout from Skills-3 (PR #66).

### 10. Drift defence-in-depth — three independent checks

1. **`PinnedContract::verify_self_hash`** — internal pin integrity (the
   pin's `contract_hash_hex` matches a fresh hash of its own `contract`
   field).
2. **`tau check mcp-contracts`** — pin vs lockfile (the pin's hash matches
   `LockedMcpEntry.contract_hash`).
3. **Runtime drift check** — live server vs lockfile hash at `McpClient`
   construction time (every boot).

Any mismatch fails closed.

### 11. Cassette format

Transport-agnostic message-level JSONL in `tau-mcp::cassette`. First line:
`{"version":1}`. Subsequent lines are `CassetteMessage` records with `dir`
(`"in"` = host→server, `"out"` = server→host), `kind`
(`"request"` | `"response"` | `"notification"`), optional `id`, `method`,
and `payload`. Replayer matches on `(method, normalized_args)` (sorted keys,
stripped whitespace).

### 12. Conformance fixture #07

`crates/tau-ir-conformance/fixtures/07_mcp_weather_cassette/` exercises a
`cassette:` URL through the IR-level conformance harness (DevMode +
cross-mode equivalence with BundleMode).

## Consequences

**Positive:**

- Users can adopt arbitrary MCP servers without writing bespoke plugin code.
- Cassettes make MCP-backed tools fully testable and reproducible without
  network or subprocess at test time.
- The bespoke plugin protocol is now slated for eventual
  replacement-by-MCP per the philosophy pivot; β.3 is the foundation for
  that work.
- All five in-tree bespoke plugins are unchanged (Lane 1 preserved).

**Negative:**

- Three drift checks must be kept in lockstep across `tau-mcp`, `tau-pkg`,
  and `tau-cli`. Conformance fixtures #02 and #07 plus
  `cassette_dial.rs` integration tests act as the canaries.
- Each pinned contract is committed to the repo (small JSON files, ~5–50 KB
  each).
- Windows stdio sandbox is stub-only (pre-existing gap from ADR-0023); stdio
  MCP on Windows runs without OS-level cap enforcement until
  sandbox-windows Phase 2.
- `net.*` OS-level enforcement is advisory on Linux (seccomp cannot filter
  outbound network egress); v0 stdio MCP relies on contract-level host
  pinning and wire-level enforcement (HTTP) for `net.http` caps.

## Alternatives considered

- **Streamable HTTP only, no stdio:** rejected — most MCP servers in the
  ecosystem ship as stdio-only subprocesses.
- **No cassettes, mock MCP servers per test:** rejected — would duplicate
  test infrastructure and give weaker compliance guarantees than replaying
  real protocol traffic.
- **Inline contracts in lockfile (no `.tau/mcp/` files):** rejected —
  contracts can be 50 KB+ each; this would bloat the lockfile and make it
  hard to read.
- **MCP-only (deprecate plugin protocol immediately):** rejected as
  out-of-scope for β.3; tracked separately as future work.

## References

- β.3 PR-1 (#280) — crate scaffolds + protocol types + ADR placeholder
- β.3 PR-2 (#281) — stdio transport + host lifecycle + mock-mcp-server fixture
- β.3 PR-3 (#283) — HTTP Streamable transport + CassetteTransport
- β.3 PR-4 (#284) — lowering + lockfile v7 + `tau build` wiring
- β.3 PR-5 (#287) — McpBridge + WiredHostHandlers + runtime drift check
- β.3 PR-6 (this PR) — CLI verbs + conformance fixture #07 + this ADR + docs
- Spec: `docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md`
- Philosophy: `docs/explanation/tau-philosophy.md`
- Adjacent ADRs: ADR-0030 (Skills-6), ADR-0034 (target triple registry),
  ADR-0035 (bundle format), ADR-0037 (workflow IR β.2)
