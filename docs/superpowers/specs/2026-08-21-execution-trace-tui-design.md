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
```
