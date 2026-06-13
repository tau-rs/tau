# Framing: triggers, serve transport, and retry/DLQ

**Status:** Framing / design doc. No code. Establishes the position tau takes
on event ingress before any implementation spec is written.

**Date:** 2026-06-13.

**Audience:** contributors weighing whether tau should grow trigger ingress,
an HTTP serve transport, or declarative retry/DLQ; ADR authors; anyone
deciding whether tau replaces an orchestrator (it does not).

**Relates to:** [`tau-philosophy.md`](../../explanation/tau-philosophy.md)
(the compiler convictions), ROADMAP non-goals NG3 / NG5 / NG11,
the workflow IR ([ADR-0037](../../decisions/0037-workflow-ir.md)),
serve mode v1 (PR #143), and the egress-only capability vocabulary in
`tau-domain::package::capability`.

---

## The question

tau today is a **stdio-driven automation step with no trigger ingress.**
`tau serve` speaks JSON-RPC 2.0 over NDJSON on stdin/stdout; a parent
process initiates every `runtime.run`. Nothing inside tau ever *initiates*
work, listens on a socket, schedules a future invocation, or persists a
failed message for later.

Should tau add:

1. **Triggers** — cron schedules, webhook receivers, queue/event consumers;
2. **An HTTP serve transport** — `tau serve` accepting requests over a
   socket instead of (or beside) stdio;
3. **Declarative retry / DLQ** — `tau.toml`-level retry policy and a
   dead-letter destination for runs that exhaust it;

— or stay **"host drives it"**: tau is one well-bounded automation step, and
the scheduler, the listener, and the durable queue all live in the host
(systemd, k8s, Lambda, an API gateway, Temporal, n8n)?

This doc takes a position. It is **A** in the brainstorming sense: a single
recommended stance, with the rejected alternatives documented as roads not
taken.

---

## TL;DR

**Compile the trigger; delegate the substrate.**

tau should add a declarative `[trigger]` table that **compiles into the IR
and the bundle** as portable, capability-safe metadata. A **host adapter**
reads that metadata and wires the real trigger. tau emits the *binding*; the
host owns the *substrate* — the scheduler, the socket, the durable queue.
This is the same delegation pattern tau already applies to inference and
credentials: *tau resolves; operators choose the vault* becomes *tau
declares the trigger; operators choose the scheduler.*

tau should **not** grow a tau-owned cron daemon, a public webhook listener,
or a built-in durable DLQ store. Those make tau a long-lived hosted service
(NG3) competing with general orchestrators on breadth (NG5).

| Ask | Verdict | The line |
|---|---|---|
| `[trigger]` *declaration* → IR → bundle | ✅ **ADD** | Portable metadata; a host adapter wires the real trigger |
| tau-owned in-process **cron daemon** | ❌ **REJECT** | Long-lived scheduler = NG3; substrate is systemd / k8s / Lambda |
| Public **webhook listener** | ❌ **REJECT** | Inbound ingress = NG3; *no inbound capability shape exists, by design* |
| HTTP serve transport — **host-driven RPC** (LSP-over-TCP shape) | ◐ **DEFER (defensible)** | Host still initiates each request; identical request/response shape to stdio. Not a non-goal violation — just not needed yet |
| HTTP serve transport — **internet-facing ingress** | ❌ **REJECT** | That is the webhook listener wearing a transport hat |
| Retry / DLQ **policy declaration** (`[trigger.*.retry]`) | ✅ **ADD** | Declarative attempts / backoff is portable metadata |
| Built-in **durable DLQ store** | ❌ **REJECT** | Persistent store in core = NG5 + NG6-adjacent; dead-letter is a *sink contract*, never a tau-owned queue |

---

## Current state — "host drives it" is the architecture, not a default

Serve mode is already strictly host-driven. The host opens the process and
initiates every request; tau only responds and emits streaming events. There
is no listening socket and nothing durable.

```
                         HOST (cron unit, k8s Job, parent process, CI step)
                                       │
                       spawns `tau serve`, owns stdin/stdout
                                       │
                                       ▼  initiates every request
   ┌───────────────────────────────────────────────────────────────────┐
   │  tau serve  (JSON-RPC 2.0 over NDJSON stdio)                        │
   │                                                                    │
   │   stdin  ──▶ reader ──▶ dispatcher ──▶ Runtime.run ──▶ writer ──▶ stdout
   │                              │                                     │
   │   meta.handshake / meta.ping / runtime.run /                      │
   │   runtime.run_streaming / runtime.cancel        (host → tau)       │
   │   runtime.event notifications                   (tau → host)       │
   │                                                                    │
   │   max_concurrent · idle_timeout · graceful drain · PDEATHSIG       │
   └───────────────────────────────────────────────────────────────────┘

   Direction of initiative:  HOST ──▶ tau   (always; never the reverse)
```

Three facts about the *engine* below serve mode constrain everything that
follows:

- **The IR has no trigger and no retry concept.** A lowered module is
  `IrModule { ir_format, tau_version, target, workflow }` where
  `Workflow { agents, tools, steps, edges, capability_table }`. Failures
  propagate (a failed subflow surfaces `is_error: true`); there is no
  durable timer, no retry counter, no persisted message anywhere in the
  interpreter.
- **The capability vocabulary is egress-only.** `net.http` is
  `{ hosts, methods }` — *outbound* allow-listing. There is **no
  `net.listen`, no inbound verb, no bind primitive** anywhere in
  `Capability`. This is not an oversight to be patched; it is **NG3 enforced
  at the type level.** tau cannot grant "receive an inbound connection"
  because tau is not a service.
- **The sandbox model has no inbound shape either.** `CapabilityShape`
  enumerates `FilesystemRead/Write`, `ProcessExec`, `NetworkHttp`,
  `AgentSpawn`, … — every shape an adapter can enforce. None of them is
  "accept a socket." A webhook listener has nothing to enforce against.

So "host drives it" is not a posture tau happens to take today. It is wired
into the IR, the capability vocabulary, and the sandbox. Any inbound trigger
is, by construction, **un-grantable by tau** and can only be satisfied by a
host that owns the socket.

---

## What's needed — the binding, not the substrate

A trigger has two halves:

| Half | Example (cron) | Example (webhook) | Who should own it |
|---|---|---|---|
| **Substrate** | the timer that fires at 03:00 | the TCP socket + TLS + the public URL | **the host** (systemd, k8s, API gateway, Lambda URL, CF Workers cron) |
| **Binding** | "fire `summarizer` on `0 3 * * *`" | "POST /ingest → `classifier`, retry 3×, DLQ to `sink`" | **tau** (declared once, compiled, portable) |

Today neither half exists in tau. The proposal adds **only the binding** —
as compiled metadata — and leaves the substrate where it already lives.

```
                       ONE DECLARATION  (tau.toml [trigger.*])
                                  │
                          tau build / lower
                                  │
                ┌─────────────────┴──────────────────┐
                ▼                                    ▼
        IR trigger-binding                   host-adapter descriptor
        (in canonical IR + bundle             (emitted, optional:
         `tau.trigger` section)                systemd timer / k8s CronJob /
                │                               CF Workers cron / gateway route)
                │                                          │
                ▼                                          ▼
        portable, content-hashed,                 operator APPLIES it;
        capability-checked                        host now owns the substrate
                │                                          │
                └───────────────────┬──────────────────────┘
                                    ▼
                    host fires the substrate ──▶ invokes the SAME artifact
                            (child process today; transport TBD)

   inference   ─────▶  pluggable endpoint        (existing delegation)
   credentials ─────▶  provider chain            (existing delegation)
   TRIGGERS    ─────▶  host scheduler / socket / queue   (this proposal)
```

The third delegation line is the entire point: triggers join inference and
credentials as a *substrate tau describes but does not own*.

---

## Proposed `tau.toml [trigger]` schema (v1 surface)

This is a **proposed** surface, exhaustive enough that the later
implementation spec inherits a complete shape — not a finalized grammar.

### Shape

A project declares zero or more **named** triggers. Each binds an external
event to a workflow **entrypoint** (an agent id already defined in the
project). A trigger is *metadata about how tau is invoked*, never an
executable node.

```toml
# A named cron binding.
[trigger.nightly]
kind     = "cron"
agent    = "summarizer"        # entrypoint — must be an existing agent id
schedule = "0 3 * * *"         # 5-field cron; cron-only field
timezone = "UTC"               # optional; default UTC (no host-local ambiguity)

[trigger.nightly.retry]
max_attempts = 3               # total attempts including the first
backoff      = { strategy = "exponential", base = "30s", max = "10m" }
dead_letter  = "dlq-sink"      # sink reference (see Dead-letter, below)

# A named webhook binding.
[trigger.ingest]
kind   = "webhook"
agent  = "classifier"
path   = "/ingest"             # path the HOST adapter routes; tau does not bind it
methods = ["POST"]             # echoed into the host-adapter descriptor
# auth/TLS/host/port are deliberately ABSENT — they are substrate (host-owned)

# A named queue/event binding.
[trigger.orders]
kind   = "queue"
agent  = "order-handler"
source = "mcp:orders-queue"    # an MCP contract that delivers messages

# The default, made explicit: tau is invoked by an external driver.
[trigger.manual]
kind  = "manual"
agent = "summarizer"
```

### Field reference

| Field | Kinds | Meaning | Owner |
|---|---|---|---|
| `kind` | all | `cron` \| `webhook` \| `queue` \| `manual` | tau |
| `agent` | all | entrypoint agent id (validated at build against the project) | tau |
| `schedule` | cron | 5-field cron expression | tau (binding) / host (firing) |
| `timezone` | cron | IANA tz name; default `UTC` | tau |
| `path` | webhook | route the host adapter should map to this trigger | host (substrate) |
| `methods` | webhook | HTTP methods echoed into the descriptor | host (substrate) |
| `source` | queue | MCP contract name that delivers messages | host (substrate) |
| `[…].retry` | cron, webhook, queue | re-invocation policy (below) | host honors it |

Deliberately **absent** (because they are substrate, and putting them in
`tau.toml` would make tau the service): bind address, port, TLS material,
public hostname, auth scheme, concurrency of the listener, queue connection
strings/credentials. Those live in the host adapter's own config, resolved
through the existing credential chain where secrets are involved.

### The retry sub-table

```toml
[trigger.<name>.retry]
max_attempts = 3
backoff      = { strategy = "exponential", base = "30s", max = "10m" }
dead_letter  = "<sink-reference>"
```

| Field | Meaning |
|---|---|
| `max_attempts` | total attempts including the first; `1` = no retry |
| `backoff.strategy` | `fixed` \| `exponential` (v1) |
| `backoff.base` | duration string |
| `backoff.max` | cap on the computed delay |
| `dead_letter` | where a run that exhausts `max_attempts` is sent — a **sink reference**, never a tau-owned queue |

**Retry is a trigger-level re-invocation policy, not a per-node interpreter
retry.** The host (or host adapter) re-invokes the artifact; tau's
interpreter stays deterministic and stateless across invocations. This is
the single most important boundary in the retry design: putting retry inside
the engine would require durable timers and persisted attempt counters —
durable state in core — which is NG3 (a service) and NG6-adjacent (a store).
Keeping retry at the trigger level means *retry is durable exactly where the
host is durable*, and nowhere else.

### Dead-letter is a sink contract, not a store

`dead_letter` names one of:

- an **MCP contract** (`mcp:<name>`) the operator has wired — the failed
  run's envelope is handed to that contract;
- a **capability target** already granted to the workflow (e.g. an
  `fs.write` path, or a `net.http` host) — the envelope is written there.

tau **never** stands up a queue, a table, or a spool directory of its own.
A DLQ *store* is exactly the persistent-store-in-core that NG5/NG6 forbid.
The dead-letter envelope shape (run id, trigger name, attempt count, last
error, original input hash) is part of the later spec; it is small,
content-addressable, and carries no tau-managed durability.

---

## Lowering — where the binding lives in the IR and bundle

Triggers lower as **metadata**, parallel to how capabilities already lower
into the `tau.caps` custom section. They add no executable nodes.

```
tau.toml [trigger.*]
        │  lower
        ▼
IrModule
 ├─ workflow            (unchanged: agents / tools / steps / edges / capability_table)
 └─ triggers  ← NEW     (Vec<TriggerBinding>, canonically ordered by name)

TriggerBinding {
    name, kind, agent_entrypoint,
    schedule? / path? / methods? / source?,
    retry?: { max_attempts, backoff, dead_letter },
    // a webhook/queue binding ALSO records that it requires a
    // host-adapter contract — see "the killer design hole" below
}
```

Bundle manifest gains a `trigger` section + a `tau.trigger` custom section
(mirroring `ir_payload` / `tau.caps`). Because the binding is in the
canonical IR, it is **content-hashed and reproducible** — two builds of the
same declaration produce byte-identical trigger metadata, and `tau verify
--bundle` covers it for free. Trigger metadata participating in the hash is
the property that makes "the trigger is part of the program" true rather than
aspirational.

### `tau build --emit-trigger=<adapter>` (proposed)

Alongside the artifact, `tau build` optionally emits a **host-adapter
descriptor** the operator applies:

| `--emit-trigger` | cron kind emits | webhook kind emits |
|---|---|---|
| `systemd` | `.timer` + `.service` unit referencing the artifact | n/a (cron only) |
| `k8s` | a `CronJob` manifest | an `Ingress` + route hint (operator completes) |
| `cf-workers` | `[triggers] crons` config stanza | a Worker route stanza |
| `lambda` | an EventBridge schedule rule | a Function URL + route hint |

This is the cargo → systemd-unit analogy made literal: a Rust binary does
not contain a cron daemon, but you can generate a unit that runs it on a
timer. tau produces the wiring; the operator owns the substrate. The
descriptor set is open — adapters are additive and need not all ship at once.

---

## The killer design hole — and how the schema closes it

> A webhook receiver needs to accept an inbound connection. tau has no
> capability that grants inbound. How can a webhook trigger possibly be
> capability-safe?

It can't be a **tau-runtime** capability — and that is the resolution, not a
problem. The schema makes inbound triggers compile to a **host-adapter
contract**, never to a runtime capability:

```
cron / manual                         webhook / queue
──────────────                        ───────────────
host invokes tau as a child           an INBOUND event must be received
process (an egress-shaped,            ──▶ requires a primitive tau has NO
already-expressible relationship)         capability shape for, BY DESIGN (NG3)
        │                                          │
        ▼                                          ▼
binding lowers to IR metadata         binding lowers to IR metadata
only; needs no inbound grant          PLUS a "host-adapter required" marker;
                                      the host adapter owns the socket and is
                                      the ONLY thing that can satisfy it
        │                                          │
        └──────────────────┬───────────────────────┘
                           ▼
        `tau check` can statically distinguish:
        cron/manual = self-contained;
        webhook/queue = REQUIRES a host adapter to be deployable
```

This gives `tau check` a real, enforceable rule: a `webhook` or `queue`
trigger with no corresponding emitted/declared host adapter is a **deploy-
time hole** tau can name — *Rust-class build-time enforcement* of the thing
that would otherwise be a silent runtime surprise. `cron` and `manual` need
no inbound primitive (the host invokes tau as an ordinary child process), so
they are the cleanest fit and the natural v1 priority.

---

## The HTTP serve transport — a separate axis, gated on *who initiates*

This question is orthogonal to triggers and must not be conflated with them.
The deciding test is **direction of initiative**, not the wire protocol.

| Shape | Who initiates | Verdict | Why |
|---|---|---|---|
| stdio JSON-RPC (today) | host | ✅ shipped | host-driven request/response |
| HTTP-as-RPC: host POSTs `runtime.run`, gets a response (LSP-over-TCP) | host | ◐ **defensible, deferred** | Identical request/response semantics to stdio, different framing. Host still drives. No non-goal violation — but no demand yet, so not now. |
| HTTP-as-ingress: an internet-facing endpoint that *receives events* and starts runs | the network | ❌ **reject** | This is the webhook listener. tau initiating work from an inbound connection is NG3. |

An HTTP **transport** that merely re-frames the existing host-driven RPC is
not a non-goal violation — it is the same `tau serve` contract over a socket,
useful when the host and tau are on different machines. It is deferred only
because nothing needs it yet. The moment an HTTP endpoint starts *accepting
events and initiating runs on its own*, it has become the rejected webhook
listener regardless of how it is labeled. The schema above keeps that line
bright: ingress is always a **host adapter** that invokes the artifact, never
a mode of `tau serve`.

---

## Roads not taken

### Road A — pure "host drives it", add nothing

tau ships no `[trigger]` table at all. Operators hand-write systemd units,
k8s CronJobs, and gateway routes that call `tau run`/`tau serve`.

*Rejected because:* the trigger then lives **outside** the content-hashed
artifact. The program's "when and how am I invoked" is unversioned,
non-portable, and re-authored per target — exactly the dev/prod drift the
compiler philosophy exists to kill. Declaring the binding in the IR makes
the trigger *part of the program*, portable across every target, and
reproducible. The cost of Road A is precisely the value tau adds elsewhere.

### Road B — full orchestrator (the n8n / Temporal trap)

tau grows a cron daemon, a webhook server, a durable queue, a DLQ store,
visual retry policies, a run history database — a long-lived service you
deploy and operate.

*Rejected because:* this is NG3 (a hosted service) and NG5 (competing with
general orchestrators on breadth) in their purest form. It also forces
durable state into core, contradicting NG6, and turns tau from a
developer-facing compiler (NG11/NG12) into an operations platform. The
market already has Temporal, n8n, Airflow, and every cloud's scheduler; tau's
differentiator is the *portable capability-safe artifact*, which those tools
do not produce. Building Road B trades a unique position for a crowded one.

### The chosen middle — *compile the binding, delegate the substrate*

Adds the one thing only tau can add (a portable, capability-checked,
content-hashed trigger binding) and delegates everything that would make tau
a service. It is the inference/credentials delegation pattern applied to a
third substrate.

---

## Non-goal cross-check

| Non-goal | Does this proposal violate it? |
|---|---|
| **NG3** — not a hosted service | **No.** No tau-owned listener/daemon/queue. tau emits bindings; hosts run them. The egress-only capability vocabulary keeps inbound un-grantable. |
| **NG5** — not a general workflow engine | **No.** tau does not orchestrate across systems, hold durable run state, or compete on breadth. It compiles a binding; the orchestrator (if any) stays the host. |
| **NG6** — no persistent memory/store in core | **No.** Retry is host-honored; DLQ is a sink contract, never a tau store. |
| **NG9** — no identity/credentials | **No.** Listener auth/TLS/secrets are substrate, resolved through the existing provider chain by the host adapter. |
| **NG11 / NG12** — developer tool; runtime + compiler, not a framework | **No.** `[trigger]` is a compile-time declaration that lowers to IR — squarely compiler behavior. |

Every "no" is structural, not promissory: the egress-only vocabulary and the
"retry = host re-invocation" boundary make the violations *un-expressible*,
not merely discouraged.

---

## Open questions for the implementation spec

These are deliberately deferred; the framing fixes the boundary, the spec
fixes the surface.

1. **IR placement.** Is `triggers` a sibling of `workflow` in `IrModule`, or
   a field inside `Workflow`? (Leaning sibling: triggers are about invocation,
   not the call graph; keeping them out of `Workflow` preserves the existing
   conformance hashes for trigger-less modules.)
2. **`ir_format` bump.** Adding `triggers` is an additive IR change — confirm
   it is a minor `ir_format` bump with forward-compat read of older modules.
3. **Dead-letter envelope shape.** Exact fields and their canonical encoding.
4. **`tau check` rule severity.** Is "webhook trigger without a host adapter"
   an error or a warning at `tau build` time? (Leaning: warning at build,
   error only under a `--require-deployable` flag — the artifact is still a
   valid program; it just needs a substrate to fire.)
5. **Adapter descriptor formats.** Which `--emit-trigger` targets ship first
   (systemd + k8s are the obvious v1 pair).
6. **Cron expression dialect.** 5-field standard vs. 6-field (seconds);
   timezone handling across host schedulers that disagree.
7. **Multiple triggers per agent / one trigger fanning to multiple agents.**
   v1 proposes one `agent` per trigger; revisit if demand appears.

---

## Sequencing note

This is **not** an active roadmap commitment — it is the framing that lets
one be written. If pursued, it most naturally rides **after** the workflow IR
is fully settled (β.2 family, shipped) and slots beside the bundle/target
machinery (Phase 2 §C, shipped), because it is purely additive IR + bundle +
CLI surface. It depends on nothing in flight. The cleanest first slice is
**`cron` + `manual` + retry policy + `--emit-trigger=systemd|k8s`** — the
self-contained kinds that need no host-adapter contract — with `webhook` and
`queue` following once the host-adapter contract in `tau check` is specified.
