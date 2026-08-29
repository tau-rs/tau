# Execution Trace TUI — design

**Status:** draft for review
**Date:** 2026-08-21
**Author:** brainstormed with Titouan
**Related:** #469 / #528 (live trace render CLI), `tau-ports` `TraceEvent`, `tau-observe` vocabulary, EPIC 4.x interpreter

## 1. Motivation

`tau run` today renders orchestration progress as npm/cargo-style
scrolling lines (`output_orchestration.rs`) — one padded line per
event, "no TUI, no cursor magic." That is fine for a tail but gives no
sense of *shape*: what ran concurrently, where wall-clock went, which
tool calls were capability-gated and how, or what a model was reasoning
about before it acted.

The ask is a **Chrome-DevTools-Network-style execution trace** for a
`tau run` — a waterfall of spans on a shared time axis — fused with a
**DeepSeek-style "thinking trace"**: expandable model reasoning inline
on the waterfall. Benchmarking (LangSmith, Langfuse, OpenAI Traces,
Arize Phoenix, DevTools) confirmed no existing tool ships both
first-class, and that a workflow engine with tau's capability lattice
can surface things LLM-tracers cannot (per-span governance verdicts).

This is the **author-facing** tool (debug/observe your own workflows).
A polished end-user "watch it think" frontend is explicitly deferred
(§9).

## 2. Goals / non-goals

### Goals
- A **terminal TUI** (ratatui) that renders a `tau run` as a waterfall:
  rows for agent turns, tool calls, control-flow (branch/parallel/loop),
  and suspend/resume; timing bars on a shared axis; token columns.
- **Capability-verdict badges** on tool spans: `allow` / `clamp` /
  `drop`, tau's differentiator.
- **Live and post-mortem are one path.** The TUI is a pure renderer
  over `.tau/runs/<run_id>.jsonl`; tail-following a growing file gives
  the live view, a finished file gives the post-mortem view. Same code.
- A **`tau trace <run_id>`** subcommand that opens the TUI on any run —
  live (attach), finished (re-open), or copied from elsewhere.
- **M2:** an expandable **💭 thinking trace** per model span (collapsed
  by default, token/latency badge, raw|summarized flag).

### Non-goals (this design)
- No browser/web surface. No `--serve`, no static-HTML export, no
  persistent `tau-gateway`. (The design keeps the render *model*
  separable so a future web frontend can reuse it — §9.)
- No trace diff, no replay-from-span, no evals/scores. Noted as future
  differentiators (§9); not built here.
- No change to how runs are *executed* — this is purely observation.

## 3. The reframe (why two things became one)

The user described "a DeepSeek IDE trace view like Chrome's Network
panel." Research showed those are two distinct lineages:

- **Thinking trace** — what DeepSeek actually ships (the DeepThink
  "Thoughts" collapsible panel; API `reasoning_content` vs `content`).
  A reasoning drill-down. No waterfall.
- **Network-style waterfall** — from LLM observability (LangSmith et
  al.) and DevTools. Span tree + timeline. No reasoning drill-down.

Our design **fuses** them: a waterfall whose model spans expand into
their thinking trace. M1 delivers the waterfall; M2 adds the reasoning
drill-down.

## 4. Architecture

Three units, dependencies pointing inward (hexagonal):

```
 .tau/runs/<id>.jsonl  ──(tail-follow | full-read)──▶  tau-trace (model)  ──▶  TUI frontend (tau-cli)
   source of truth              ingestion                 pure logic              ratatui rendering
   (exists today)                                    no runtime deps         no runtime deps
```

### 4.1 `tau-trace` crate (new) — model + ingestion, pure & headless

Responsibility: parse a stream of persisted `TraceEvent` JSON lines into
a **normalized, renderable trace model**, and keep it updated as new
lines arrive.

- **Input:** an iterator/stream of `TraceEvent` (deserialized from
  jsonl lines). The crate does **not** open files or subscribe to the
  runtime — a caller feeds it lines. This keeps it testable headless
  and reusable by a future web frontend.
- **Output:** a `TraceModel` — an ordered tree of `Span`s with resolved
  start/end times, durations, token counts, parent/child nesting, and
  status. Plus incremental `apply(event)` for live updates.

```rust
pub struct Span {
    pub id: SpanId,
    pub kind: SpanKind,          // Agent | Tool | Reasoning | Branch | Parallel | Loop | Suspend
    pub label: String,           // "agent: planner", "net.http GET api.…"
    pub start: DateTime<Utc>,    // derived: ts - duration for completed spans
    pub end: Option<DateTime<Utc>>,
    pub tokens: Option<u64>,
    pub capability: Option<CapabilityVerdict>,   // Allow | Clamp | Drop  (tool spans)
    pub parent: Option<SpanId>,
    pub status: SpanStatus,      // Running | Ok | Failed
    pub detail: SpanDetail,      // kind-specific payload for the detail pane
}

pub enum CapabilityVerdict { Allow, Clamp { to: String }, Drop { reason: String } }
```

`TraceModel` owns time-axis math (min start → max end → per-span bar
offset/width) so the frontend is dumb pixels.

### 4.2 TUI frontend (in `tau-cli`, e.g. `cmd/trace.rs` + a `tui` module)

Responsibility: open the jsonl (tail-follow or full-read), deserialize
lines, feed `tau-trace`, and render the `TraceModel` with ratatui +
crossterm. New workspace dependencies: `ratatui`, `crossterm`.

Layout (from the approved mockup):
- **Toolbar:** run id + pipeline name; filter chips (All / Errors /
  Tools / 💭); a `/` search box.
- **Waterfall pane:** columns `Name · Tokens · Dur · Cap` + timeline
  bars. Tree indentation for nesting; ▸/▾ to fold agent subtrees.
- **Detail pane:** on select (↑↓ + enter) — full inputs/outputs, timing
  breakdown, capability decision + deciding policy, error text.
- Keybindings: `↑↓` select, `enter` expand/detail, `/` search, `f`
  cycle filter, `q` quit. Live runs auto-scroll unless the user has
  scrolled up (DevTools behavior).

### 4.3 Wiring into `tau run`

`tau run` gains an opt-in live TUI (the scrolling-line printer stays the
default so non-interactive/CI output is unchanged):

```
tau run app.tau              # default: existing npm-style live lines
tau run app.tau --tui        # live waterfall TUI (this feature)
tau trace <run_id>           # open TUI on any run (live-attach or finished)
tau trace --last             # most-recent run under ./.tau/runs
```

`--tui` and `--json`/non-TTY are mutually exclusive (guard at arg
parse). `tau trace` detects TTY; without one it errors with a hint
(future: dump normalized JSON).

## 5. Data model mapping (what a row is, and where it comes from)

| Span kind | Source event (today) | Bar | Extra |
|---|---|---|---|
| Agent turn | `TraceEventKind::Turn { duration_ms, tokens }` + `ts` — **emitted today** | `[ts-duration, ts]` | token count |
| Tool call | `TraceEventKind::ToolCall` — **defined but NOT emitted today; M1 adds the producer, see §6** | `[ts-duration, ts]` | capability badge |
| Branch/Parallel/Loop | `pipeline.step` structure | span extent | control-flow marker |
| Suspend/resume | `Suspend` outcome / resume | gap marker | resume signal |
| Spawn (nesting) | `TraceEventKind::Spawn { child_id, agent_kind }` | — | establishes parent/child |
| 💭 Reasoning (M2) | `RunEvent::ReasoningDelta` (new) | reasoning extent | raw\|summarized, token count |

**Time-axis note:** persisted `TraceEvent`s carry a completion `ts` and
a `duration_ms`; the renderer derives bar **start = ts − duration_ms**.
A still-running span (started, not yet completed) is drawn open-ended to
"now". This is the one bit of nontrivial model logic and gets unit
tests.

## 6. Known instrumentation gap — tool-call trace events are not emitted

This is the one place M1 is **not** a pure renderer, and it is larger
than a single field. Two facts, verified in the tree:

1. **`TraceEventKind::ToolCall` has no producer.** The only variants
   emitted into the orchestration trace today are `Spawn`, `Turn`,
   `Completion`, `OrphanedTasksAtTermination`, and the budget variants
   (producers in `run.rs`, `stream.rs`, `orchestration.rs`). Tool-call
   timing exists only in the `tracing` spans (`tool.invoke`,
   `dispatch.tool`) and the single-agent `RunEvent::ToolCallStarted`/
   `ToolCallCompleted` pair — neither of which lands in the persisted
   `.tau/runs/<id>.jsonl` the TUI reads. So **the waterfall's tool rows
   need a producer.**
2. **The capability verdict lives only in the tracing layer.** The
   decision exists as `tau-observe` events (`capability.allow`,
   `capability.deny`, …) on the `tracing`/OTLP sink, not in any
   `TraceEvent`. `ToolCall.status` is a generic `String`.

M1 therefore includes a scoped **instrumentation task** at the tool
dispatch site (the code path already wrapped by the `tool.invoke` /
`dispatch.tool` spans, which already has the tool name, timing, ok/error
status, and the computed capability verdict in hand):

- Add a `capability: Option<CapabilityVerdict>` field to
  `TraceEventKind::ToolCall` (`CapabilityVerdict` = `Allow | Clamp { to }
  | Drop { reason }`).
- Emit a `TraceEvent { kind: ToolCall { .. } }` at dispatch completion,
  threading the verdict the dispatcher already computes for the
  `capability.*` tracing event.

This is a `tau-ports` public-type change → **semver-minor bump** (prior
tau-ports field-add precedent). It is *not* backend/model work, but it
is real instrumentation and is called out so M1 is honestly scoped.

Fallback: ship M1 with agent/spawn/turn rows only and no tool rows,
adding tool rows in M1.5. **Not recommended** — tool rows + capability
badges are the point of a Network-style view.

## 7. Milestones

### M1 — Execution waterfall TUI
1. `tau-trace` crate: `Span`/`TraceModel`, `apply(event)`, time-axis
   math, jsonl line → `TraceEvent` deserialization helper. Headless
   unit tests over recorded fixtures.
2. Tool-call instrumentation (§6): `CapabilityVerdict` type + a
   `capability` field on `TraceEventKind::ToolCall`, **and a producer**
   that emits the `ToolCall` trace event (name, duration, ok/error,
   verdict) at the dispatch site.
3. TUI frontend in `tau-cli`: ratatui/crossterm; toolbar, waterfall
   pane, detail pane, keybindings, filter, search, tail-follow.
4. `tau run --tui` + `tau trace <run_id>` / `--last` wiring.
5. Tests: model unit tests; a TUI smoke test via ratatui's
   `TestBackend` (render a fixture trace, assert buffer regions).

### M2 — 💭 Thinking trace
1. `tau-ports`: additive `ContentBlock::Reasoning(String)` and
   `CompletionChunk::Reasoning { delta }` (seam already documented at
   `llm.rs:248`; both enums are `#[non_exhaustive]`).
2. One reasoning-capable backend adapter surfaces reasoning tokens into
   the new chunk (parse provider `reasoning_content` / `<think>` per
   provider). Which adapter = first open question for the M2 plan.
3. `RunEvent::ReasoningDelta { delta }` (schema is `#[non_exhaustive]`,
   versioned — additive) + a `TraceEvent` reasoning projection so it
   reaches the jsonl.
4. TUI: `Reasoning` span kind, collapsed-by-default drill-down with
   token/latency badge and raw|summarized flag; `💭` filter chip.
5. Tests: reasoning chunk → run event → trace model → rendered span.

## 8. Error handling

- **Malformed / partial jsonl line** (live tail may read a half-written
  line): skip-and-retry on the next fsync boundary; never panic. Lines
  are fsync'd per event, so a partial read is transient.
- **Unknown event kind** (`TraceEvent` is versioned; a newer run opened
  by an older TUI): render as a generic "unknown span," don't drop the
  run. Forward-compatible.
- **No TTY / `--json` with `--tui`:** arg-parse error with a hint.
- **Missing run id / empty `.tau/runs`:** friendly error listing
  available run ids.
- **Terminal resize:** ratatui re-layout; bars recompute from the
  time-axis model (no cached pixels).

## 9. Deferred / future (kept possible, not built)

The `tau-trace` model crate is deliberately frontend-agnostic so these
don't require rework:
- **End-user "watch it think" frontend** (the second frontend from the
  original "both" decision) — likely a browser surface; revisit its
  delivery (`--serve` SSE vs persistent gateway) then.
- **`tau trace --serve`** — ephemeral localhost + SSE live browser
  waterfall (same model crate, different frontend).
- **Trace diff** (`tau trace --diff a b`) — pairs with `tau verify`
  reproducibility.
- **Replay from a span** — leverages IR-digest + suspend/resume.
- **Evals/scores on spans**, **export/permalink**.

## 10. Testing strategy

- `tau-trace`: pure unit tests over checked-in `.jsonl` fixtures
  (recorded from real runs) — time-axis math, nesting, live `apply`
  incrementality, malformed-line resilience.
- Capability enrichment: assert the verdict round-trips jsonl → model.
- TUI: ratatui `TestBackend` golden-buffer assertions on a fixture
  trace (deterministic, no real terminal).
- M2: end-to-end reasoning chunk → `ReasoningDelta` → span, plus
  raw|summarized flag rendering.
- Everything is testable headless; nothing here is "not testable."

## 11. Open questions for the plan phase
1. `tau-trace` as a new crate vs a module in an existing crate.
   (Recommend new crate: keeps ratatui out of the model, reusable by a
   web frontend.)
2. M2: which backend adapter is the first reasoning source?
3. Do control-flow spans (branch/loop/parallel) already reach the jsonl,
   or do they need a projection like reasoning does? (Verify in plan;
   agent/tool/spawn are confirmed present.)

## 12. Clamp rows (M1.5 deferred item 2) — design

M1 shipped `Allow` rows; M1.5 shipped `Drop` rows and deferred `Clamp`
after finding that `CapabilityVerdict::Clamp` has **no runtime
producer**. This section is the design for that producer.

### 12.1 What a clamp actually is (finding)

There is no per-call narrowing anywhere in the runtime. The only
meet-clamp is decided **once, at MCP setup**:

```text
tau.toml [tools.<entry>].capabilities            (envelope)
        │
        ▼  setup_mcp_runtime (tau-cli cmd/ir_dispatcher.rs)
mcp_capability_plan:
    net caps ← tau_domain::meet(envelope_net, [allow.mcp.<entry>].hosts)
        │
        ▼
CapabilityPlan ──► mcp_open ──► OS boundary (sandboxed server spawn)
        │                        ▲ enforcement happens here, once
        ▼
McpBackedTool (carries DECLARED st.caps only)
        │
        ▼  per call, tau-runtime-core stream.rs
check_capabilities_for_tool(agent_grant ⊇ declared)   — pass/fail only
        ▼
TraceEvent ToolCall { capability: Allow }             — always Allow today
```

The kernel's per-call gate compares the *agent's grant* against the
tool's *declared* caps; the clamped plan never reaches it. Therefore a
`clamp:<to>` row **means**: "this call executed under authority that was
narrowed at open time to `<to>`." It surfaces a standing open-time
decision on each affected per-call row; it is observability only — no
enforcement change.

**Reachability today, corrected.** The diagram above describes the
kernel dispatch site (`tau-runtime-core/src/stream.rs`), and that site
is real and testable — it is what §12.4 instruments. But it is not, in
fact, where `tau run`'s MCP calls go today. MCP tools are registered
into `ForwardingDispatcher` (the IR path), and IR agents wrap every
tool in `DispatcherTool`
(`tau-runtime-core/src/interpreter/agent_loop.rs`, ~line 482), which
intentionally advertises **empty** capabilities and does not forward
`effective_capabilities()` — this is the issue-#581 pinned contract,
guarded by `tests/ir_dispatch_gate_inert.rs`, and is not something this
design changes. Separately, IR-interpreter runs never populate
`options.orchestration_state`, so they emit no `TraceEvent::ToolCall`
at all, clamped or otherwise. Net effect: the §12 producer is correct
and covered by its own tests, but on its own it is dormant for the
stock `tau run` CLI. Both gaps — no capability forwarding on the IR
path, no trace sink on IR runs — are closed by the reachability wiring
designed in §13 (issue #631). That wiring is fully shipped, but the
clamp row is still not observable end-to-end, because of two
pre-existing production bugs entirely upstream of #631:

- **#712** — the real MCP handshake always reports empty declared
  capabilities (`ServerContract::from_handshake` is called with a
  hardcoded `|_| Vec::new()` extractor), so `tool_effective_capabilities`
  always returns `None` and no clamp can ever be computed outside a test.
- **#714** — `tau run --bundle` re-lowers with an empty MCP contract
  cache and fails `IrSourceDivergence` before reaching the interpreter,
  so it cannot run any MCP-tool project at all, clamped or not.

Prior-art audit (why this shape): object-capability systems attach
narrowed authority to the object itself (Capsicum fd rights,
ocap-attenuated proxies; in-repo precedent `AttenuatedDispatcher` in
`tau-runtime-core/src/interpreter/attenuate.rs`), and per-operation
audit records annotate every operation with the standing policy decision
(Kubernetes audit `authorization.k8s.io/decision` annotations, Envoy
access-log matched-policy fields). The design below composes exactly
those two patterns. What it is *not* is enforcement-point logging
(SELinux-AVC-style "this call tried host X and was blocked") — that
would require egress events from the sandbox proxy and is future work
(§12.6).

### 12.2 Authority on the object: `Tool::effective_capabilities()`

Additive, defaulted method on the `Tool` trait (tau-ports):

```rust
/// The authority this tool actually runs under, when narrower than
/// `capabilities()` (e.g. an MCP entry meet-clamped by the
/// `[allow.mcp.<entry>].hosts` ceiling at open time).
/// `None` (default) = not narrowed; runtime authority == declared.
fn effective_capabilities(&self) -> Option<&[Capability]> {
    None
}
```

Mirrored on `DynTool` (tau-runtime-core `builder.rs`) with the same
default, forwarded in the blanket `impl<T: Tool<Session = ()>> DynTool
for T`. Defaulted method ⇒ additive ⇒ tau-ports 0.7.0 → **0.7.1**
(cargo-semver-checks classes defaulted-trait-method addition as minor;
if the ABI job disagrees, bump 0.8.0 + workspace pin instead).

### 12.3 Producer site 1 — compute the per-tool meet (tau-cli)

`setup_mcp_runtime` already holds both sides at registration time. Per
server-tool `st` of a clamped entry, where `envelope` is the entry's
author-declared `[tools.<entry>].capabilities`:

```text
effective_net = tau_domain::meet(envelope ∩ Network, plan.capabilities ∩ Network)
clamped       = effective_net ≠ (envelope ∩ Network)     // lattice inequality
```

**The comparison basis is the author envelope, not `st.caps` (#712).**
This spec originally specified `st.caps`, the server contract's per-tool
declaration. That can never work in production: MCP does not standardize
per-tool capability declaration, and `McpTool` carries no extension field
to hold a tau-specific one, so `ServerContract::from_handshake`'s
`caps_extractor` has nothing to read and every real handshake yields
`caps == []`. Clamping against an always-empty set made
`tool_effective_capabilities` return `None` for every tool of every real
server — the clamp row was unreachable outside tests.

The envelope is also the sounder basis. The server is the untrusted
party, so *"the server said it needs X"* is a weak footing for a
governance display; *"you declared hosts X, governance clamped you to Y"*
is what an operator can act on. Should MCP later standardize per-tool
caps, `st.caps` becomes a second, independent narrowing input — an
intersection with the envelope, not a replacement for it.

If `clamped`, construct the tool's effective set = non-net declared caps
+ `effective_net`, and pass it into the tool:

```rust
McpBackedTool::new(id, client, name, st.caps, effective, schema, desc)
// effective: Option<Vec<Capability>> — None when not narrowed
```

`McpBackedTool` stores it and overrides `effective_capabilities()`.
Notes:
- The meet is **per server-tool**, not per entry: a tool whose declared
  caps contain no `net.http` is never clamped, even on a clamped entry.
- The `[allow.mcp]` ceiling narrows hosts only (any-method ceiling), so
  in practice only hosts narrow; the design still compares full net
  caps so a future method-carrying ceiling needs no rework.
- Empty meet (fail-closed host drop in `mcp_capability_plan`) is still
  a clamp — the tool runs with **no** net authority (§12.5 renders it).

### 12.4 Producer site 2 — verdict mapping at the kernel emit (tau-runtime-core)

The two existing `ToolCall` emit sites in `stream.rs` (success path and
schema-invalid path) currently inline `required.is_empty() → None else
Allow`. Extract one shared helper and use it at both:

```rust
fn capability_verdict(
    tool: &dyn DynTool,
    required: &[Capability],
) -> Option<tau_ports::CapabilityVerdict> {
    if required.is_empty() {
        return None;
    }
    Some(match tool.effective_capabilities() {
        Some(eff) => tau_ports::CapabilityVerdict::Clamp {
            to: render_clamped_to(eff),
        },
        None => tau_ports::CapabilityVerdict::Allow,
    })
}
```

(§13.1 later revises this helper: the clamp branch moves ahead of the
`required.is_empty()` early-return so narrowed-but-ungated IR tools
still surface their clamp. The shape above is what #630 shipped.)

The kernel stays the single emit point — no trace sink in
tau-mcp-tokio, no duplicate rows. The `Drop` path (denial
early-returns) is untouched. The kernel gate itself is unchanged: it
still checks agent-grant vs declared caps, and the OS boundary remains
the enforcer. (Gating against `effective_capabilities()` instead is a
possible later tightening — conservative direction, per the
conservatism note in `capability.rs` — but out of scope here.)

### 12.5 The `to` string

`render_clamped_to(eff)` (kernel-side, next to the helper): the sorted,
comma-joined host list of the effective net caps — e.g.
`"api.weather.com"` or `"a.com,b.com"`; `HostSet::Any` renders `"any"`
(possible when a clamp narrows methods-only in the future); **no** net
cap in `eff` renders `"none"` (the empty-meet fail-closed case).
Rendering lives in the kernel, not the port — the port carries semantic
`Capability` values only. The TUI renderer (`capability_badge` /
`capability_detail`, amber `clamp:<to>`) is already built and tested.

### 12.6 Out of scope / future

- **Reachability wiring**: designed in §13 (issue #631) — no longer
  out of scope.
- **Enforcement observation**: rows record decided authority, not what
  the call actually touched. Surfacing "blocked egress to evil.com"
  needs events from the sandbox proxy (tau-sandbox-darwin) — separate
  feature.
- **Kernel gating on effective caps** (§12.4 note).
- **Non-MCP producers**: any future narrowed tool (wasm-hosted, plugin)
  gets clamp rows by overriding the same defaulted method — no new
  machinery.

### 12.7 Testing

- **tau-cli**: unit test for the per-tool meet/clamp computation —
  clamped entry + net-declaring tool ⇒ `Some(effective)` with narrowed
  hosts; no-net tool ⇒ `None`; empty meet ⇒ `Some` with no net cap
  (mirrors `governed_clamps_net_hosts_to_allow_mcp_ceiling`).
- **tau-runtime-core**: producer test mirroring M1.5's
  `capability_denied_tool_emits_drop_trace_event` — a stub `Tool`
  overriding `effective_capabilities()`, run through the kernel with a
  collecting trace subscriber, assert `ToolCall { capability:
  Some(Clamp { to }) }` with the rendered host list; plus unit tests
  for `render_clamped_to` (hosts / any / none) and
  `capability_verdict` (empty-required ⇒ `None`, default ⇒ `Allow`).
- **Renderer**: the `clamp` badge match arm shipped in M1.5 but —
  contrary to what this section originally claimed — without a render
  test. §13.6 adds it.
- **Docs**: add the `clamp` badge row to `reference/tau-trace.md`
  (omitted from the M1.5 table because it had no producer).

## 13. Clamp-row reachability wiring (issue #631) — design

§12 built the producer; this section makes it reachable from the stock
CLI. Definition of done (from #631): a governed MCP project whose entry
is host-clamped by `[allow.mcp]` renders an amber `clamp:<to>` row in
`tau run --tui` / `tau trace`, with an e2e test covering that path.

Scoping fact discovered during design: stock `tau run` *without*
`--bundle` never calls `setup_mcp_runtime` — MCP tools exist only on
the bundle-v2 path (`run_via_ir`) and `tau dev`. "Stock CLI" for this
design therefore means `tau run --bundle`.

The wiring is two independent gaps plus two latent CLI bugs that block
the DoD end-to-end:

| Gap | Closed by |
|---|---|
| `capability_verdict` never emits for un-gated tools | §13.1 verdict decoupling |
| `DispatcherTool` forwards no effective capabilities | §13.2 dispatcher authority accessor |
| IR runs attach no trace sink | §13.3 dispatcher sink accessor |
| Sink attachment would activate virtual-tool intercepts | §13.4 intercept hardening |
| `tau trace` cannot parse the jsonl the writer produces | §13.5 envelope parse fix |
| `tau run --bundle --tui` silently ignores the flag | §13.5 live TUI join |

### 13.1 Verdict semantics — clamp decoupled from gate participation

The #630 helper (§12.4) couples "has a verdict" to "participates in
the kernel grant gate": `required.is_empty() → None` fires before
`effective_capabilities()` is consulted. IR tools always present
`required = []` (the #581 contract), so no forwarding scheme alone can
ever surface a clamp. Revised helper:

```rust
fn capability_verdict(
    tool: &dyn DynTool,
    required: &[Capability],
) -> Option<tau_ports::CapabilityVerdict> {
    // Narrowed authority is a property of the object (§12.1 ocap
    // framing) and is always visible, whether or not the in-kernel
    // grant gate looked at this tool.
    if let Some(eff) = tool.effective_capabilities() {
        return Some(tau_ports::CapabilityVerdict::Clamp {
            to: render_clamped_to(eff),
        });
    }
    if required.is_empty() {
        None
    } else {
        Some(tau_ports::CapabilityVerdict::Allow)
    }
}
```

Semantics after this change:

- `Clamp { to }` — the call ran under authority narrowed at open time,
  regardless of gate participation.
- `Allow` — gated (non-empty declared caps at the dispatch site) and
  not narrowed. Unchanged.
- `None` — un-gated **and** un-narrowed. Compatible with the
  documented port contract ("`None` for un-gated tools",
  `tau-ports/src/orchestration.rs`).

Documented asymmetry: an **un**-clamped MCP tool renders `allow` on
the multi-agent path (its `McpBackedTool` sits unwrapped in the
registry, declared caps non-empty) but `-` on the IR path (behind
`DispatcherTool`, genuinely un-gated per #581). This is honest — the
badge reflects what the dispatch site actually checked —
and `reference/tau-trace.md` gets a note saying so.

No tau-ports change: `CapabilityVerdict` and the `Tool` trait are
untouched (stays 0.7.1). The helper is kernel-internal.

### 13.2 Capability forwarding — `ToolDispatcher` authority accessor

The interpreter already sources every run ingredient (clock, random,
checkpointing, context pipeline, assets) from the `ToolDispatcher`
trait, and `ForwardingDispatcher` (tau-cli) already holds the
`Arc<dyn DynTool>` for each `McpBackedTool`, which carries the
meet-clamped effective set computed by §12.3. Thread it through the
same seam — one new defaulted trait method
(`tau-runtime-core/src/interpreter/tool_dispatch.rs`):

```rust
/// The meet-clamped authority a tool actually runs under, when
/// narrower than its declared capabilities (§12). `None` (default) =
/// not narrowed, or this dispatcher does not track authority.
fn tool_effective_capabilities(
    &self,
    tool_id: &tau_ir::ids::ToolId,
) -> Option<Vec<tau_domain::Capability>> {
    let _ = tool_id;
    None
}
```

- `ForwardingDispatcher` implements it by consulting its tool map:
  `self.tools.get(tool_id)?.effective_capabilities()` → owned `Vec`.
- `prepare_agent_run` calls it once per tool at `DispatcherTool`
  construction; the wrapper caches `Option<Vec<Capability>>` in a new
  field and overrides **only** `Tool::effective_capabilities()`
  (returning `as_deref()`).
- `Tool::capabilities()` remains un-overridden — `required` stays `[]`
  at the gate, so the #581 pinned contract
  (`tests/ir_dispatch_gate_inert.rs`) holds verbatim. A sibling pin is
  added to that file: a `DispatcherTool` whose dispatcher reports
  effective caps still reaches the dispatcher un-gated (no
  `PolicyDenied`) while its ToolCall row carries `Clamp`.
- All other `ToolDispatcher` impls (wasm guest, `tau dev` callbacks,
  conformance) inherit the `None` default and compile untouched.

### 13.3 Trace sink — `ToolDispatcher` sink accessor + synthetic `RunState`

Same seam, second defaulted method:

```rust
pub struct TraceSinkConfig {
    pub run_id: tau_ports::RunId,
    // TraceSubscriber lives in tau-runtime-core::orchestration::trace —
    // same crate as ToolDispatcher, no tau-ports change.
    pub subscribers: Vec<Arc<dyn crate::orchestration::trace::TraceSubscriber>>,
}

/// Trace sink for this run. `None` (default) = no trace emission
/// (guest, dev, conformance today).
fn trace_sink(&self) -> Option<TraceSinkConfig> {
    None
}
```

`TraceSinkConfig` is `Send + Sync` (subscriber `Arc`s), so it crosses
the `D: Send + Sync` dispatcher bound — which `Arc<RefCell<RunState>>`
cannot. `prepare_agent_run` consumes it:

```text
if let Some(sink) = dispatcher.trace_sink():
    state = RunState::new(sink.run_id, agent_id, RunBudget::default(), clock.now())
    for sub in sink.subscribers: state.trace.add_subscriber(sub)
    options.orchestration_state = Some(Arc::new(RefCell::new(state)))
```

Notes:

- The synthetic `RunState` is inert as orchestration: default budget =
  unlimited (BudgetWatchdog no-ops), empty task list, no
  `orchestration_runtime`. It exists only to carry `run_id` + the
  `TraceStream` fan-out into the three guarded emit sites.
- Pipelines construct one `RunState` per agent step, all sharing the
  same `run_id` and subscribers — one jsonl, one waterfall.
- `Turn` / `Completion` trace events come along for free
  (`run.rs` `trace_ctx` needs state + clock + random, all now
  present), so the IR waterfall gains full rows, not just clamps.
- **`!Send` discipline**: `Arc<RefCell<RunState>>` never crosses a
  spawn boundary. The run future stays on the caller's task; the TUI
  runs in `spawn_blocking`, joined via `tokio::join!` — the exact
  pattern the multi-agent path uses (`cmd/run.rs` `--tui` join). Do
  not `tokio::spawn` the run future.
- `run_ir` / `run_pipeline` signatures are unchanged — no ripple into
  the no_std guest build.

### 13.4 Virtual-tool intercept hardening

The intercept in `stream.rs` (`task.*` / `run.*` / `agent.*.spawn`)
gates on `orchestration_state.is_some()` alone. Attaching a sink to IR
runs would let an MCP tool whose IR id is literally e.g. `task.create`
(entry `task`, server tool `create`) be hijacked in-kernel. Harden the
gate to require `orchestration_runtime.is_some()` **as well**:

- Multi-agent runs set both (`spawn_root_agent_inner`) — no behavior
  change, existing orchestration tests stay green.
- IR runs set state only — the intercept stays dormant, preserving
  "IR runs have no orchestration semantics."

A regression test pins this: an IR run with a trace sink and a tool
named `task.create` dispatches to the dispatcher (not the kernel) and
emits an ordinary ToolCall row.

### 13.5 CLI wiring (`run_via_ir`) + `tau trace` ingestion fix

**Sink construction** (tau-cli `cmd/ir_dispatcher.rs`): `run_via_ir`
mints one ULID run id (reused for the plugin `TraceContext` in place
of today's separate `tau-run-bundle-<ulid>` id), builds the jsonl
writer subscriber via the existing
`tau_runtime_tokio::orchestration::trace_mpsc::channel_with_writer`
targeting `<scope>/.tau/runs/<run_id>.jsonl`, and hands
`TraceSinkConfig` to the dispatcher via a
`ForwardingDispatcher::with_trace(...)` builder method. The writer and
line format are reused as-is from the multi-agent path — same file
namespace, so `tau trace --last` picks up IR runs with zero reader
changes.

**Live `--tui`**: today `run_via_ir` returns before `cmd/run.rs`'s
"--tui requires a multi-agent run" bail, so the flag is silently
ignored on the bundle path. Wire it: when `args.tui`, attach an
`MpscTraceSubscriber` alongside the writer, hand the receiver to
`run_tui(TraceSource::Live(rx))` in `spawn_blocking`, and
`tokio::join!` it with the run future (per the §13.3 `!Send`
discipline). The non-bundle bail and its message are unchanged.

**Ingestion fix** (pre-existing bug, DoD-blocking): the writer emits
envelope lines `{"line_kind":"trace_event","event":{...}}`
(`RunLogLine`, tau-runtime-tokio `persistence.rs`), but
`tau_trace::parse_line` deserializes a **bare** `TraceEvent` — so
`tau trace` file mode renders an empty waterfall for every file ever
written, multi-agent runs included. Fix in `tau-trace` (which must not
depend on tau-runtime-tokio): parse leniently —

1. try the envelope shape; `line_kind == "trace_event"` → yield the
   inner event; any other `line_kind` (e.g. `task_mutation`) → skip
   (`Ok(None)`);
2. fall back to a bare `TraceEvent` for old files and test fixtures.

The envelope shape is mirrored locally in `tau-trace` (a private
struct, serde-tagged the same way) with a round-trip test against a
hand-built literal line, deliberately not a `RunLogLine` value —
`tau-trace` must stay pure and must not depend on `tau-runtime-tokio`,
so there is no code-level tie to the writer's actual type. That means
the unit test pins the reader's own handling (it would still pass if
the writer silently drifted), not a genuine writer↔reader contract.
The real cross-crate tie is the e2e in
`crates/tau-cli/tests/clamp_row_e2e.rs`, which parses a file the
writer actually produced through this same `parse_line`; that tie is
currently dormant because the e2e is `#[ignore]`d (§13.6).

### 13.6 Testing

- **Kernel (tau-runtime-core)**: `capability_verdict` unit tests for
  the new ordering — narrowed + empty required ⇒ `Clamp` (the newly
  reachable case); narrowed + non-empty ⇒ `Clamp`; un-narrowed cases
  unchanged. Producer test: an IR run (via `run_ir` with a stub
  dispatcher reporting effective caps + a collecting sink) yields
  `ToolCall { capability: Some(Clamp { to }) }`.
- **#581 pins** (`ir_dispatch_gate_inert.rs`): existing test untouched
  and green; sibling test per §13.2; intercept-hardening test per
  §13.4.
- **tau-cli**: `ForwardingDispatcher::tool_effective_capabilities`
  unit test (present / absent / non-MCP tool); envelope-parse
  round-trip per §13.5; the missing `Clamp` badge render test in
  `tui/render.rs` (amber `clamp:<to>` cell + detail pane).
- **e2e (DoD anchor)**: `crates/tau-cli/tests/clamp_row_e2e.rs` — a
  governed cassette-MCP project — `[allow]` + `[allow.mcp.<entry>]`
  host ceiling narrower than the tool's declared hosts, `cassette:`
  URL pin (no sandbox needed), scripted LLM — driven through `tau run
  --bundle`; asserts `.tau/runs/<run_id>.jsonl` exists and, parsed
  through `tau_trace::parse_line` (the real reader), contains a
  `ToolCall` with `capability: Some(Clamp { to })` matching the
  ceiling hosts. Builds on `mcp_dispatch.rs`'s `setup_project_with_pin`
  and `cmd_build_mcp.rs` scaffolding. The TUI hop is covered by the
  render unit tests (a real-TTY e2e is not attempted). The test is
  written, complete, and correct, but `#[ignore]`d pending **#712** and
  **#714** (§12.1) — both pre-existing production bugs upstream of
  #631; un-ignoring it is their acceptance test, and it should pass
  unchanged once both land.

  Two findings surfaced while building it, otherwise lost:
  - a pipeline `StepRun::Tool` step can never produce a `ToolCall`
    trace row — `pipeline.rs` calls `dispatcher.invoke` directly and
    has zero trace references anywhere in the file; only the
    agent-turn kernel loop (`stream.rs`, reached via `run_ir`'s
    single-entry-agent path or a pipeline's `StepRun::Agent` arm)
    emits those rows. Any clamp-row e2e must therefore drive a real
    agent whose LLM decides to call the tool, not a bare tool step.
  - `echo-llm` was extended with an additive, defaulted `tool_calls`
    config field, because before that no subprocess-spawnable LLM
    backend plugin could emit a `ToolUse` at all.

### 13.7 Out of scope

- `tau dev` trace sink — its session also calls `setup_mcp_runtime`
  and can adopt `with_trace` later; nothing here blocks it.
- A capability field on `RunEvent` (`--stream` / chat surfaces).
- Everything already listed in §12.6 (enforcement observation, kernel
  gating on effective caps, non-MCP producers).
- **Observability blockers #712 and #714** (§12.1, §13.6): fixing the
  MCP handshake's empty capability extraction and the bundle
  re-lowering's empty MCP cache are both pre-existing production bugs,
  not part of this design — they are what stands between the shipped
  producer chain and an observable clamp row.
