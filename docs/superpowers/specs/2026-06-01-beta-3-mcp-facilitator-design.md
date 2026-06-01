# β.3 — MCP facilitator: design

**Status:** Locked design; ready for implementation plan.
**Date:** 2026-06-01.
**Scope:** ROADMAP §β.3 — MCP host runtime + capability gate at the contract
boundary + Workflow-IR integration via the existing `ToolImpl::Mcp` variant.
**Supersedes:** nothing. **Builds on:** β.1 (`tau-runtime-core` extraction —
PRs #256 / #257 / #260 / #261 / #262, ADR footnotes 0006/0014/0024-0034),
β.2 (workflow IR — ADR-0037), the existing sandbox stack
(`tau-sandbox-{native, darwin, container, windows}` + `process_gate`), and
the bundle format (Phase 2 §C — ADR-0035).
**Authority for forward-looking framings cited inline:**
[`tau-philosophy.md`](../../explanation/tau-philosophy.md) — *Three convictions*
§2 (harness everywhere, inference + credentials delegated) and §3
(capability-safe by construction); *The architecture, in one picture*
(the "MCP FACILITATOR" block); *Two tool kinds, one rule*.

## In one breath

> tau adds an in-process MCP host runtime — protocol surface in `tau-mcp`,
> tokio runtime + transports in `tau-mcp-tokio`. External MCP servers
> (stdio subprocess or Streamable HTTP) are contracted at build time
> (`contract_hash` pins the full `tools/list`), expanded into per-tool IR
> entries during lowering, and dispatched at runtime through an
> `McpBridge` that composes with the existing `ToolDispatcher`. Per-tool
> capability enforcement attaches at two points: OS-level via the existing
> `ProcessGate::Sandbox::wrap_spawn` for stdio servers, contract-level
> per-call for both transports. Sampling routes to the agent's
> `LlmBackend` through a default-deny allowlist; roots routes to an
> explicit `roots = [...]` field that is build-time-consistency-checked
> against the tool's `fs.read` caps. The five in-tree bespoke plugins are
> untouched (Lane 1 preserved). DoD: one off-the-shelf weather MCP server
> is contracted by an agent, capability-enforced end-to-end, round-trips
> a forecast.

---

## 1. Motivation and authority

ROADMAP §β.3 names MCP facilitator as the second post-β.1 work stream
(parallel to β.2 / β.4 / β.5 / β.8 after β.1 lands). The
philosophy doc enshrines two-tool-kinds (native vs MCP-contracted) and
positions tau-as-facilitator: tau owns the *contract* to external MCP
servers, never the implementation. This spec implements that contract
surface.

The philosophy doc explicitly stars **sampling** and **roots** as the
inbound handlers where the capability gate and delegated inference
attach. β.3 ships those two plus `tools/call` plus the
`notifications` / `cancellation` infrastructure that every handler
shares. `resources/read`, `elicitation`, `prompts/get` are deferred to
β.3.1 — the inbound-dispatch machinery built for sampling + roots
generalizes additively to them.

The gap β.3 fills is the one the philosophy doc names explicitly:
**"MCP's capabilities are protocol-feature negotiation; its authorization
is OAuth-scoped remote access. Neither sandboxes a tool's filesystem,
network, or exec at runtime."** β.3 is what makes contracted MCP tools
honor the per-tool capability declaration the same way native tools do.

## 2. Crate layout

Two new crates. Both follow the β.1 core/tokio split discipline — `tau-mcp`
is `no_std`-friendly so γ.5 wasm and embassy shells can adopt it; only
`tau-mcp-tokio` carries transport + lifecycle code that depends on the
tokio runtime.

```
crates/tau-mcp/                       NEW; no_std-friendly
├─ protocol/                          MCP wire types — JSON-RPC envelopes
│                                     (request, response, notification),
│                                     `initialize` / `tools/list` /
│                                     `tools/call` / `sampling/createMessage`
│                                     / `roots/list` / `notifications/*` /
│                                     `cancellation`. Pure serde types; no I/O.
├─ contract/                          Schema + cap-declaration types;
│                                     canonical-hash function
│                                     (Hash256 = SHA-256 of canonical JSON
│                                     per β.2 canonical-encoder rules);
│                                     pinned-contract file I/O helpers
│                                     (`.tau/mcp/<name>.contract.json`);
│                                     envelope ∩ contract intersection logic.
├─ host/                              `HostHandlers` trait — slots for the
│                                     server-initiated request handlers
│                                     (sampling, roots, plus future
│                                     resources/elicitation/prompts).
│                                     Default-deny baseline impls.
├─ cassette/                          Transport-agnostic message-level
│                                     recorder + replayer (JSONL format —
│                                     spec in §11).
└─ Transport TRAIT                    `send_message(msg) → ()` +
                                      `next_message() → Option<Message>`,
                                      `no_std`-friendly so wasm transports
                                      can implement it later.

deps: tau-domain, tau-ports (capability types), serde, serde_json, hashbrown
NO deps on: tokio, reqwest, hyper, tau-runtime-*
```

```
crates/tau-mcp-tokio/                 NEW; tokio runtime + transports
├─ transport_stdio/                   Subprocess MCP servers. Spawn goes
│                                     through `tau_runtime_tokio::process_gate
│                                     ::Sandbox::wrap_spawn(&mut cmd, plan)`
│                                     — exact reuse of the in-tree sandbox
│                                     stack.
├─ transport_http/                    Streamable HTTP MCP client. Reqwest +
│                                     SSE chunk parsing. Per-call net.http
│                                     cap enforcement via wire-level host
│                                     pinning.
├─ host_lifecycle/                    spawn/dial → handshake (initialize +
│                                     tools/list) → keepalive → shutdown;
│                                     exposes live `McpClient` handles.
└─ bridge.rs                          `McpBridge` — composable
                                      `ToolDispatcher` adapter. Knows a
                                      `BTreeMap<ToolId, (Arc<McpClient>,
                                      server_tool_name, caps)>`; routes
                                      `invoke(tool_id, args)` calls to the
                                      right server.

deps: tau-mcp, tau-runtime-tokio, tokio, reqwest, hyper, wiremock (dev)
```

Adjacent (existing) crates receive small additions:

| Crate | Change |
|---|---|
| `tau-ir` | `ToolImpl::Mcp` gains `server_tool_name: String`; new port trait `McpContractResolver` (no I/O); `lower/resolve.rs` gains the MCP expansion stage. |
| `tau-pkg` | `UncheckedTool` gains optional `sampling` + `roots` fields; `ToolBody::Mcp(String)` URL parser distinguishes `stdio:<command>` from URI scheme; lockfile schema v6 → v7 (per-MCP-entry `{url, contract_hash, expanded_tools, pinned_contract?}`); `PinnedContract` (de)serializer. |
| `tau-cli` | `cmd/build.rs` wires the live resolver + `--offline` mode; `cmd/run/ir_dispatcher.rs` `ForwardingDispatcher` composes with `McpBridge`; `cmd/run/bundle.rs` uses the same bridge for bundle dispatch; new `cmd/mcp/{pin,ls,show,refresh,diff}.rs`; `tau check` gains an `mcp_contracts` phase. |
| `tau-runtime-core` | **UNCHANGED.** No new port trait; `ToolDispatcher` stays MCP-agnostic; `DispatcherTool::invoke`'s `Native | Mcp` arm is unchanged (forwards to `ToolDispatcher`, which the McpBridge implements). |
| `tau-runtime-tokio` | `plugin_host` mod-doc header gains one line acknowledging β.3 shipped. Otherwise unchanged. |
| `tau-ir-conformance` | Adds fixture #07 — cassette-replay weather scenario; cross-mode test. |
| `docs/decisions/0038-mcp-facilitator.md` | NEW. |

## 3. Authoring surface

The author writes one `[tools.<name>]` block per MCP server. Tau handles
discovery + expansion.

```toml
# stdio MCP server — local subprocess
[tools.weather]
mcp = "stdio:npx --yes @modelcontextprotocol/server-weather"
capabilities = [
    { kind = "net.http", host = "api.weather.com" },
    { kind = "fs.read",  path = "/tmp/mcp-cache" },
]
sampling.models = ["claude-haiku-4-5"]  # default empty → sampling refused
roots = ["/tmp/mcp-cache"]              # default [] → roots/list returns []

# HTTP MCP server — remote / SaaS
[tools.search]
mcp = "https://mcp.search.example.com"
capabilities = [{ kind = "net.http", host = "api.search.example.com" }]
# sampling.models omitted = server cannot invoke sampling
# roots omitted = server gets [] from roots/list
```

URL-scheme discrimination:

| Scheme | Transport | Spawn? |
|---|---|---|
| `stdio:<command>` | stdio | yes — `tau-mcp-tokio::transport_stdio` |
| `https://…` | Streamable HTTP | no — `tau-mcp-tokio::transport_http` |
| `http://…` | Streamable HTTP (plaintext) | no — accepted but warned at build |
| anything else | error | n/a |

`mcp = "stdio:..."` was deliberately chosen over `mcp = { kind = "stdio",
command = "..." }` for parity with the existing one-line URL form
`mcp = "https://..."`. The string `stdio:` prefix is a scheme-like
sentinel; future transports (`ws://...`) extend by URI scheme.

## 4. IR shape

`ToolImpl::Mcp` evolves to carry the per-tool routing info:

```rust
// crates/tau-ir/src/tool_impl.rs
pub enum ToolImpl {
    Native { fn_ref: NativeFnRef, content_hash: Hash256 },
    Mcp {
        url: String,
        contract_hash: Hash256,
        capability_subset: CapabilityRequirements,
        server_tool_name: String,  // NEW — the name passed on the MCP wire
    },
    Subflow { target: AgentId },
    Step { id: StepId },
}
```

IR `ToolId` convention for expanded MCP entries: `"<entry>.<server-tool>"`,
e.g. `weather.get_forecast`. The agent's `tool_refs` is rewritten by
`lower/resolve.rs` to reference the expanded ids. The agent author still
writes `tool_refs = ["weather"]` in `tau.toml`; expansion happens during
lowering.

Guard: server-tool names containing `.` are rejected with a
`McpBuildError::ServerToolNameContainsDot` error during expansion. Real MCP
servers don't use `.` in tool names; this is a forward-defense guard against
ToolId namespace collisions. v0 forbids it; if a real server emerges that
uses `.`, the fix is to introduce an alternate separator (likely `::`) and
extend the discriminator at parse time.

## 5. Lowering: build-time data flow

`tau build` runs the existing parse stage, then the resolve stage gains
the MCP expansion below before typecheck.

```
PARSE                    →  RESOLVE (MCP additions)               →  TYPECHECK
─────                       ─────────────────────────                 ─────────

ToolImpl::Mcp {             For each ToolImpl::Mcp:                  Existing checks
  url,                                                                pass unchanged
  contract_hash: 0..0,        if --offline + pinned file exists →    against the
  capability_subset:            PinnedResolver reads                  expanded ToolId
    <envelope>,                 .tau/mcp/<name>.contract.json         set.
  server_tool_name: ""        else →
}                               McpContractResolver impl
                                (tau-mcp-tokio, live handshake)
                              → tools/list response

                            Canonical-hash the FULL tools/list
                            payload → Hash256 = contract_hash

                            Per server-tool, intersect:
                              envelope ∩ contract.<tool>.caps

                            Consistency checks:
                              roots ⊆ fs.read caps
                              sampling.models = ∅ AND server
                                contract requires sampling →
                                build error

                            Emit one ToolImpl::Mcp per server-tool:
                              ToolId("<entry>.<tool>") {
                                url,
                                contract_hash,
                                capability_subset = intersection,
                                server_tool_name,
                                spec = contract.<tool>.input_schema,
                              }

                            Rewrite agent.tool_refs:
                              ["weather"] →
                                ["weather.get_current",
                                 "weather.get_forecast",
                                 ...]
```

Build-time invariants enforced at this stage:

| Invariant | Error variant |
|---|---|
| Contract reachable at build (live mode) | `McpBuildError::ContractUnreachable` |
| Envelope ⊇ every server-tool's declared caps | `McpBuildError::EnvelopeCoversContract` (with missing-caps detail) |
| `roots` ⊆ tool's `fs.read` caps | `McpBuildError::RootsExceedFsCaps` |
| Server contract requires sampling AND `sampling.models` is empty | `McpBuildError::SamplingRequiredByContract` |
| `--offline` mode + pinned file missing | `McpBuildError::PinnedContractMissing` |
| `--offline-strict` mode + pinned hash ≠ live hash | `McpBuildError::PinnedContractStale` |
| server-tool name contains `.` | `McpBuildError::ServerToolNameContainsDot` |

Exit code 64 (validation); all renderers (human / JSON / SARIF) per the
existing `tau check` aggregator from PR #161.

## 6. Lockfile schema v7

Per-MCP-entry persists what `tau build` resolved, so `tau verify --bundle`
and the runtime drift check can re-validate without re-handshaking.

```toml
# .tau/Tau.lock (lockfile schema_version = 7)
schema_version = 7

[[packages]]
# … existing package fields …

[packages.mcp.weather]
url = "stdio:npx --yes @modelcontextprotocol/server-weather"
contract_hash = "9f2e…<hex 64>"
pinned_contract = ".tau/mcp/weather.contract.json"  # optional

[[packages.mcp.weather.expanded_tools]]
name = "get_current"
caps = [{ kind = "net.http", host = "api.weather.com" }]
schema_hash = "0a1b…"

[[packages.mcp.weather.expanded_tools]]
name = "get_forecast"
caps = [{ kind = "net.http", host = "api.weather.com" }]
schema_hash = "2c3d…"
```

Migration from v6 is mechanical: existing v6 lockfiles with no MCP entries
upgrade silently (empty `mcp` map). Existing v6 fixtures stay green.

`tau-pkg::MAX_SUPPORTED_LOCKFILE_SCHEMA_VERSION` bumps from 6 to 7. The
`SchemaTooNew` error path that already exists handles forward compatibility
with no new code.

## 7. Bundle format

The Phase 2 §C bundle format (ADR-0035) adds:

- **Embedded pinned contracts.** Every MCP entry's pinned contract JSON is
  embedded in the bundle so `tau run --bundle` works offline (no live
  handshake needed; runtime still re-hashes the bundle-embedded contract
  and checks against the lockfile-stored hash).
- **Bundle format version bump** following the same discipline as
  lockfile v6 → v7. Existing bundles without MCP entries upgrade trivially.

## 8. Runtime: agent boot + per-turn dispatch

### 8.1 — Boot

`ForwardingDispatcher::new(...)` (existing tau-cli code) is extended:

```text
1. Existing: build BTreeMap<ToolId, Arc<dyn Tool>> for Native/Subflow/Step.
2. NEW: build BTreeMap<ToolId, (McpClient, server_tool_name, caps)> by
   walking ir.tools and grouping ToolImpl::Mcp entries by URL.
3. Per distinct MCP server URL:
   a. Look up lockfile entry {url, contract_hash, expanded_tools}.
   b. Look up tau.toml [tools.<name>] for {sampling.models, roots, caps}.
   c. Construct CapabilityPlan from caps (same shape plugin_host uses today).
   d. tau-mcp-tokio::host_lifecycle::open(url_parsed, plan):
      - stdio: transport_stdio::spawn(cmd, &plan) → wraps via ProcessGate::
        Sandbox::wrap_spawn → child runs under landlock / seccomp / sbpl.
      - http: transport_http::connect(url, &plan) → reqwest client + per-call
        host-pinning interceptor.
   e. Handshake: initialize + tools/list (live every boot — necessary to
      detect drift; not optional even with pinned contract).
   f. RE-HASH live tools/list response; compare against lockfile.contract_hash.
      Mismatch → ContractDriftAtBoot hard error; refuse to start.
   g. Construct HostHandlers impl with:
      - sampling: wraps agent's LlmBackend, filtered by sampling.models.
      - roots: returns tau.toml `roots` (already build-time-checked).
      - notifications: routes progress + log into the existing trace stream.
      - cancellation: hooked to agent's RunOutcome::Aborted signal.
   h. Spawn the inbound-dispatch task that pumps server-initiated requests
      through the HostHandlers.
4. Wrap (URL → McpClient + HostHandlers) collections in an McpBridge:
   impl ToolDispatcher for McpBridge { … } — routes ToolId lookups.
5. ForwardingDispatcher composes:
   if McpBridge.routes(id) → mcp_bridge.invoke
   else → existing plugin_host route
```

### 8.2 — Per-turn LLM call

```text
agent_loop builds LLM-facing tools/list from ir.tools (existing).
LLM sees `weather.get_forecast`, etc., each with its server-declared schema.

LLM calls weather.get_forecast {lat, lon}.

DispatcherTool::invoke for ToolId("weather.get_forecast"):
  match tool_impl:
    ToolImpl::Native | ToolImpl::Mcp → forward to ToolDispatcher
    Subflow / Step → existing paths

ForwardingDispatcher::invoke routes to McpBridge.

McpBridge::invoke:
  - look up entry (McpClient, server_tool_name, caps) by ToolId
  - cap_gate.check_outbound(&caps, &args) — refuse args that violate caps
  - client.tools_call(server_tool_name, args).await
  - convert MCP ToolResult content → serde_json::Value (preserving the
    body-shape symmetry from PR #277 — Value::String preserved as raw
    text, not double-quoted)

Result flows back to agent_loop unchanged (same path as Native).
```

### 8.3 — Inbound server-initiated requests (sampling)

```text
server sends sampling/createMessage {messages, modelPreferences}.

McpClient::inbound_dispatch_task routes by method → HostHandlers::sampling.

HostHandlers::sampling impl (composed in tau-cli):
  - allowlist = self.sampling_models
  - if allowlist.is_empty() → return InboundSamplingNotAllowed error to server
  - if modelPreferences.requested_model is set AND not in allowlist →
    InboundSamplingRefused
  - else pick allowlist[0] (modelPreferences ignored in v0; β.3.1 adds it)
  - call agent's LlmBackend::generate(messages, model)
  - budget account into the agent's token meter
  - send response back to server

server resumes its tools/call processing → eventually sends tools/call
response → agent_loop continues.
```

### 8.4 — Cancellation propagation

```text
Parent agent abort (existing RunOutcome::Aborted signal):
  → HostHandlers::cancellation
  → notifications/cancelled sent to every in-flight MCP server
  → in-flight tools/call futures unblock with ToolError::Cancelled.

Server-initiated notifications/cancelled for an in-flight tools/call:
  → McpBridge's pending invocation resolves with ToolError::Cancelled.
```

## 9. Capability gate enforcement

Two enforcement points per MCP server. Both are necessary; neither is
sufficient alone.

| Point | stdio MCP server | HTTP MCP server |
|---|---|---|
| Spawn-time (OS boundary) | `ProcessGate::Sandbox::wrap_spawn` → landlock / seccomp / sandbox-exec / podman per the existing four sandbox adapters. fs.* caps enforced by OS. net.* caps are advisory on Linux (seccomp can't filter network egress); logged in §13 open gaps. | n/a — no spawn. |
| Per-call (contract boundary) | `McpBridge::cap_gate.check_outbound` per call (build-time intersection means args can't request more than the granted envelope; runtime is defense-in-depth). | Same `check_outbound` PLUS reqwest middleware enforces net.http host pinning at the wire. |

Default-deny posture for inbound handlers:

| Inbound | Default-allow? | Author opt-in | Build-time check |
|---|---|---|---|
| `sampling/*` | **no** | `sampling.models = [...]` | server contract demands sampling AND allowlist empty → error |
| `roots/list` | **no** (returns `[]`) | `roots = [...]` | declared roots ⊆ `fs.read` caps |
| `notifications/*` | yes (observational) | n/a | n/a |
| `cancellation/*` | yes (control plane) | n/a | n/a |

## 10. CLI surface

```
tau mcp pin <name> [--from <transport-args>]
  Connects to the MCP server, captures handshake + tools/list,
  writes .tau/mcp/<name>.contract.json. Lockfile entry references it
  on the next `tau build`.

tau mcp ls
  Lists every [tools.<name>] mcp = "..." entry with: url, transport,
  pinned-y/n, last-pinned-at, lockfile contract_hash prefix, expanded
  tool count.

tau mcp show <name>
  Prints the captured contract (tools/list snapshot, per-tool caps,
  schemas). Reads from pinned file if present, else live handshake.
  --json | --human | --sarif (consistent with `tau check`).

tau mcp refresh <name>
  Re-runs handshake, writes a NEW pinned file, prints a diff vs prior.
  Does NOT touch lockfile.

tau mcp diff <name>
  Shows the diff between (live contract) and (lockfile contract_hash)
  without modifying anything.

tau check (existing aggregator from PR #161) gains:
  mcp_contracts: verifies every locked MCP entry's pinned contract (if
  present) matches the lockfile contract_hash; refuses build if
  mismatched.
```

CLI module layout: `tau-cli/src/cmd/mcp/{mod, pin, ls, show, refresh,
diff}.rs`, mirroring `tau-cli/src/cmd/skill/` from Skills-3 (PR #66).

## 11. Cassette format

Transport-agnostic message-level JSONL, lives in `tau-mcp::cassette`.
Captures observable MCP-message traffic at the handler-dispatch boundary
(above the transport layer), so the same cassette replays under any
transport (stdio, HTTP, future ws) and any host shell (tokio,
wasm, embassy).

```jsonl
{"version":1}
{"dir":"in", "kind":"request",      "id":0, "method":"initialize", "payload":{...}}
{"dir":"out","kind":"response",     "id":0, "payload":{"protocolVersion":"2025-03-26",...}}
{"dir":"in", "kind":"request",      "id":1, "method":"tools/list", "payload":{}}
{"dir":"out","kind":"response",     "id":1, "payload":{"tools":[{"name":"get_forecast",...}]}}
{"dir":"in", "kind":"request",      "id":2, "method":"tools/call", "payload":{"name":"get_forecast","arguments":{...}}}
{"dir":"out","kind":"notification","payload":{"method":"notifications/progress","params":{...}}}
{"dir":"out","kind":"response",     "id":2, "payload":{"content":[{"type":"text","text":"Sunny, 72°F"}]}}
```

`dir` (from the cassette's recording POV): `"in"` = message arriving INTO
the cassette from the host side (i.e. the host sent it to the server);
`"out"` = message emitted OUT of the cassette to the host side (i.e. the
server's reply or server-initiated request). Mnemonic: replay direction
is `out` — the cassette is the server stand-in, so what it emits is what
the host receives. `kind`: `"request"` | `"response"` | `"notification"`.
Matching is on `(method, normalized_args)`; the replayer normalizes
argument JSON (sort keys, strip whitespace) before comparison.

`tau_mcp::cassette::Replayer` supports injecting server-initiated requests
at a configured turn (used to test sampling + roots).

Versioning: a `{"version":N}` first line. v0 cassettes are version 1.
Replayer accepts version 1 only in v0; reading higher versions errors;
older format (no version line) accepted with `--legacy-cassette` flag
during the β.3 → β.3.1 transition only.

## 12. Testing strategy

| Layer | Crate | Scope |
|---|---|---|
| Unit — protocol | `tau-mcp` | serde round-trip per message variant; canonical-hash determinism + golden vectors |
| Unit — contract | `tau-mcp` | envelope ∩ contract under all cap kinds; roots ⊆ fs.read; pinned-file round-trip |
| Unit — cassette | `tau-mcp` | match-by-(method, args); inbound-injection; out-of-order delivery |
| Unit — host handlers | `tau-mcp` | default-deny; sampling.models=[] refuses; roots=[] returns []; cancellation propagates |
| Unit — bridge | `tau-mcp-tokio` | McpBridge::invoke routes by ToolId; OutboundCapDenied path; TransportClosed → ToolError::Internal |
| Integration — stdio | `tau-mcp-tokio` | spawn → handshake → tools/list → call → shutdown against in-tree mock-mcp-server binary |
| Integration — HTTP | `tau-mcp-tokio` | same shape via wiremock-rs; SSE chunking |
| Integration — sandbox | `tau-mcp-tokio` | stdio server with restrictive caps; forbidden path access; sandbox refuses |
| Contract — facilitator | `tau-cli` | tau build → cassette-replay tau run → forecast result reaches agent |
| Contract — drift | `tau-cli` | tau build cassette A; tau run cassette B with different tools/list → ContractDriftAtBoot |
| Contract — default-deny | `tau-cli` | cassette injects sampling; tau.toml sampling.models = [] → InboundSamplingNotAllowed |
| Contract — envelope | `tau-cli` | cassette tool has cap envelope doesn't cover → EnvelopeCoversContract at build |
| CLI — verbs | `tau-cli` | pin / ls / show / refresh / diff each get an integration test |
| Conformance | `tau-ir-conformance` | fixture #07 — cassette-replay weather scenario; cross-mode under DevMode + BundleMode |
| Determinism | `tau-mcp` | same canonical input → same hash across platforms (β.2 invariants); same cassette → same trace events across runs (β.6 dress rehearsal) |

CI lanes (per ROADMAP §β.3 line 668):

```
test (mcp-facilitator / linux)        runs all of the above
test (mcp-facilitator / macos)        stdio path only; HTTP skipped (== linux)
test (mcp-facilitator / windows)      both paths; stdio sandbox stub (Reserved)
```

Two new in-tree fixtures support the test surface:

- `crates/tau-mcp-tokio/tests/fixtures/mock-mcp-server/` — small Rust
  binary speaking MCP over stdio deterministically. Avoids depending on
  `npx` / Node.js in CI.
- `crates/tau-ir-conformance/cases/07-mcp-weather/` — cassette-based
  weather scenario reusing the β.2 conformance harness.

## 13. Out of scope + open gaps

### Deferred to β.3.1 (additive; no architecture change)

- `resources/read` inbound handler (cap: tau.toml `resources` URI globs).
- `elicitation` inbound handler (cap: UX path; tau dev only initially).
- `prompts/get` inbound handler (cap: explicit prompt registry).
- `modelPreferences` honored on sampling (currently ignored; allowlist-only).
- Per-server sampling token budget enforcement (field reserved; v0 no
  enforcement).

### Deferred to β.5 (credential chain)

- Full MCP server auth (OAuth, bearer tokens, etc.) via the credential
  chain. v0 supports `Authorization: Bearer <env-var>` as a stopgap.

### Deferred to β.6 (conformance gate)

- The full fan-monitor scenario (canonical β.6 workflow). β.3 ships
  fixture #07 as a stepping stone; full cross-target gate is β.6 work.

### Deferred to β.7 (AOT codegen)

- Compile-time `tools/call` dispatch. v0 interpreter dispatches by
  ToolId lookup; AOT lowers each MCP tool to a direct call site.

### Known design holes (logged, non-blocking)

- **Windows stdio sandbox is stub-only.** Pre-existing gap
  ([Windows sandbox v2 deferred from PR #46]). Docs note "stdio MCP on
  Windows runs without OS-level cap enforcement" until sandbox-windows
  Phase 2.
- **net.\* OS-level enforcement is advisory on Linux.** seccomp can't
  filter outbound network easily; v0 stdio MCP relies on contract-level
  host pinning + wire-level enforcement (HTTP) for net.http caps. Logged
  in ADR-0038. Resolved properly when "namespaced network egress" lands
  as a separate sub-project — not on the Phase β roadmap yet.
- **Sampling delegation budget attribution.** v0 routes through the
  agent's `LlmBackend` and the agent's token meter — the cost lives on
  the agent's budget. Per-server budget cap is a deferred enforcement;
  β.4 (context manager) is the natural place to add per-server
  accounting.
- **IR ToolId namespace collision via `.`.** v0 forbids `.` in
  server-tool names during expansion (build error). Real MCP servers
  don't use `.`; forward-defense only.

## 14. Migration / coexistence

```
Lane 1 — bespoke plugins (5 in-tree)            UNCHANGED
  fs-read, shell, anthropic, ollama, openai
  Load via tau_runtime_tokio::plugin_host (existing).
  No code in tau-mcp / tau-mcp-tokio touches plugin_host.
  Their integration tests stay green; integration smoke test
  in PR-5 and PR-6 gates this.

Lane 2 — MCP facilitator (NEW)                  NEW
  Any [tools.<name>] mcp = "..." routes through tau-mcp-tokio.
  External servers contracted via Streamable HTTP or local stdio.
  No in-tree plugin migrates in β.3.

Lane 3 — native tools (compiled-in)             UNTOUCHED
  β.2-defined ToolImpl::Native path remains the way to register
  in-process Rust tools through the Runtime builder.

ForwardingDispatcher composes:
                  ┌──────────────────────────────────────┐
                  │  ForwardingDispatcher (tau-cli)      │
                  │                                      │
  tool_id ────▶   │  if McpBridge.routes(id) → mcp       │
                  │  else if plugin_host.has(id) → plug  │
                  │  else → ToolNotRegistered            │
                  └──────────────────────────────────────┘

Per-plugin migration triggers (ROADMAP-aligned; NONE land in β.3):
  fs-read     → native (compiled-in)              wasm-component target (β.7)
  shell       → native                            same as fs-read
  anthropic   → LlmBackend impl OR MCP server     β.5 credential chain
  ollama      → same                              same
  openai      → same                              same
```

`tau-runtime-tokio::plugin_host` mod-doc header gains one line acknowledging
β.3 shipped. No `--deprecated` CLI warning; would spam users since no
replacement is ready for fs-read/shell.

## 15. PR phasing

Six PRs, ordered by dependency. Worktree-isolated per the standing setup.

| PR | Scope | Depends on | Estimate |
|---|---|---|---|
| **PR-1** | Crate scaffolds + protocol types. NEW: `crates/tau-mcp/` (lib.rs, protocol/, contract/ shells), `crates/tau-mcp-tokio/` (lib.rs, transport_stdio/ shell), ADR-0038 placeholder, this spec committed. Protocol types complete with serde round-trip tests; contract canonical-hash works against golden vectors. Pure-add: workspace builds, no runtime integration. | — | ~2-3 days |
| **PR-2** | stdio transport + lifecycle + in-tree fixture server. `tau-mcp-tokio::transport_stdio` (spawn via ProcessGate), `host_lifecycle` (handshake, keepalive, shutdown). NEW: `crates/tau-mcp-tokio/tests/fixtures/mock-mcp-server/`. Integration tests: lifecycle, handshake timeout, cancellation, sandbox refusal. | PR-1 | ~3-4 days |
| **PR-3** | HTTP transport + cassette + replay. `transport_http` (Streamable HTTP + SSE parsing). `tau-mcp::cassette` (Recorder + Replayer). Dev-dep: wiremock-rs. Integration tests: HTTP lifecycle, SSE streaming, cassette round-trip, inbound-injection. | PR-1 | ~3-4 days |
| **PR-4** | Lowering integration + lockfile v7 + `tau build` wiring. `tau-ir`: `ToolImpl::Mcp.server_tool_name`; `lower/resolve.rs` expansion; `McpContractResolver` port. `tau-pkg`: ToolBody discriminator; sampling/roots fields; lockfile v6→v7; PinnedContract. `tau-mcp::contract::PinnedResolver`. `tau-mcp-tokio` live resolver. `tau-cli cmd/build.rs` wires resolver + `--offline` path. Migration test: existing fixtures still build green. | PR-1 | ~4-5 days |
| **PR-5** | Bridge + ForwardingDispatcher + runtime drift check. `tau-mcp-tokio::bridge.rs` McpBridge; HostHandlers impls (sampling allowlist, roots). `tau-cli cmd/run/ir_dispatcher.rs` ForwardingDispatcher composition. Boot-time drift check. Bundle dispatch path. Outbound cap-gate enforcement. Default-deny tests. | PR-4 | ~4-5 days |
| **PR-6** | CLI verbs + conformance fixture #07 + ADR-0038 finalize + docs. NEW: `tau-cli cmd/mcp/{pin,ls,show,refresh,diff}.rs`. `tau check mcp_contracts` phase. Conformance fixture #07 (cassette-replay weather; DevMode + BundleMode). Finalize ADR-0038. Docs: 2 mdBook pages ("MCP servers" how-to, "tau mcp" reference). DoD verified end-to-end. | PR-4, PR-5 | ~3-4 days |

Cumulative scope: ~3-4 weeks of work, mirroring β.2's 9-PR shape.
β.3.1 follow-up adds resources/elicitation/prompts (~1 week, additive).

Critical path: PR-1 → (PR-2 ∥ PR-3 ∥ PR-4) → PR-5 → PR-6. PR-2/3/4 are
independent once PR-1 lands; can fan out 3-way.

## 16. Definition of done

Per ROADMAP §β.3:

- [ ] An off-the-shelf weather MCP server (in-tree fixture exercising the
      shape; PR-6 also includes a documented manual test against a real
      `@modelcontextprotocol/server-weather` for the canonical
      demonstration) is contracted by an agent declared in tau.toml.
- [ ] The agent's call round-trips: `tau run` produces a forecast in the
      assistant text.
- [ ] The server's capability declaration is enforced:
      - filesystem caps gated at OS (Linux/macOS) for stdio transport
      - network caps host-pinned at the wire for HTTP transport
      - sampling refused when `sampling.models` is empty
      - roots returns `[]` when `roots` is unset
- [ ] All five in-tree bespoke plugins still load and pass their integration
      tests (Lane 1 preserved).
- [ ] Conformance fixture #07 passes under both DevMode and BundleMode.
- [ ] `tau build` fails with a clear, typed error on any envelope /
      contract / roots / sampling violation.
- [ ] `tau verify --bundle` re-validates contracts against pinned
      contracts and refuses bundles whose contracts drifted.

---

## Locked architectural decisions (recap)

| # | Decision | Choice |
|---|---|---|
| Q1 | Crate layout | `tau-mcp` (no_std-friendly) + `tau-mcp-tokio` (transports + lifecycle) |
| Q2 | Transports v0 | stdio + Streamable HTTP (no SSE) |
| Q3 | `contract_hash` resolution | Handshake-at-build default; `tau mcp pin` + `tau build --offline` escape |
| Q4 | Handler scope v0 | tools/call + sampling + roots + notif/cancel infra; resources/elicitation/prompts → β.3.1 |
| Q5 | IR shape for multi-tool servers | Build-time expansion; `ToolImpl::Mcp` gains `server_tool_name`; namespaced ToolId |
| Q6 | Sampling + roots semantics | Allowlist (default-deny) + explicit roots field with build-time ⊆ fs.read check |
| Q7 | stdio sandbox model | Reuse `ProcessGate::wrap_spawn`; URL-scheme discriminates transport |
| Q8 | Cassette format | Transport-agnostic message-level JSONL in `tau-mcp::cassette` |
