# ADR-0043: Trigger ingress — compile the trigger, delegate the substrate

**Status:** Accepted
**Date:** 2026-06-13
**Supersedes:** none

## Context

tau today is a stdio-driven automation step with no trigger ingress.
`tau serve` speaks JSON-RPC 2.0 over NDJSON on stdin/stdout
([ADR-0033](0033-tau-serve-mode.md)); a parent process initiates every
`runtime.run`. Nothing inside tau ever *initiates* work, listens on a
socket, schedules a future invocation, or persists a failed message.

The recurring ask is whether tau should grow (1) triggers — cron schedules,
webhook receivers, queue consumers; (2) an HTTP serve transport; and (3)
declarative retry / DLQ — or stay "host drives it", with the scheduler, the
listener, and the durable queue all living in the host (systemd, k8s,
Lambda, an API gateway, Temporal, n8n).

The framing doc (PR #330) establishes a load-bearing finding: **"host drives
it" is not a posture tau happens to take — it is wired into three layers of
the engine.**

- **The IR has no trigger and no retry concept.** A lowered module is
  `IrModule { ir_format, tau_version, target, workflow }`; failures
  propagate, but there is no durable timer, retry counter, or persisted
  message anywhere in the interpreter ([ADR-0037](0037-workflow-ir.md)).
- **The capability vocabulary is egress-only.** `net.http` is
  `{ hosts, methods }` — *outbound* allow-listing. There is no `net.listen`,
  no inbound verb, no bind primitive anywhere in `Capability`.
- **The sandbox model has no inbound shape.** `CapabilityShape` enumerates
  `FilesystemRead/Write`, `ProcessExec`, `NetworkHttp`, `AgentSpawn`, … —
  every shape an adapter can enforce. None is "accept a socket"
  ([ADR-0014](0014-sandboxing.md)).

So an inbound trigger is **un-grantable by tau by construction** — NG3
enforced at the type level, not by policy. This ADR records the stance taken
on top of that finding. It cites ROADMAP non-goals NG3 (not a hosted
service), NG5 (not a general workflow engine), NG6 (no persistent store in
core), NG9 (no identity/credentials), and NG11/NG12 (developer tool;
compiler, not a framework).

## Decision

**Compile the trigger; delegate the substrate.**

tau adds a declarative `[trigger]` table that compiles into the IR and the
bundle as portable, capability-safe metadata. A **host adapter** reads that
metadata and wires the real trigger. tau emits the *binding*; the host owns
the *substrate* — the scheduler, the socket, the durable queue. This is the
same delegation pattern tau already applies to inference and credentials:
*tau declares the trigger; operators choose the scheduler.*

Per-axis verdicts:

| Ask | Verdict | The line |
|---|---|---|
| `[trigger]` *declaration* → IR → bundle | ✅ **ADD** | Portable metadata; a host adapter wires the real trigger |
| Retry / DLQ *policy* (`[trigger.*.retry]`) | ✅ **ADD** | Declarative attempts / backoff is portable metadata |
| tau-owned in-process **cron daemon** | ❌ **REJECT** | Long-lived scheduler = NG3; substrate is systemd / k8s / Lambda |
| Public **webhook listener** | ❌ **REJECT** | Inbound ingress = NG3; no inbound capability shape exists, by design |
| Built-in **durable DLQ store** | ❌ **REJECT** | Persistent store in core = NG5 + NG6-adjacent; dead-letter is a *sink contract* |
| HTTP serve transport — **host-driven RPC** (LSP-over-TCP) | ◐ **DEFER (defensible)** | Host still initiates each request; identical request/response shape to stdio. Not a non-goal violation — just not needed yet |
| HTTP serve transport — **internet-facing ingress** | ❌ **REJECT** | That is the webhook listener wearing a transport hat |

A trigger has two halves. The **substrate** (the timer that fires at 03:00,
the TCP socket + TLS + public URL, the queue connection) stays with the
host. The **binding** ("fire `summarizer` on `0 3 * * *`"; "POST /ingest →
`classifier`, retry 3×, DLQ to `sink`") is declared once in `tau.toml`,
compiled, and made portable. The proposal adds **only the binding**.

Concrete decisions:

- A project declares zero or more named triggers. Each binds an external
  event (`kind = cron | webhook | queue | manual`) to a workflow
  `agent` entrypoint validated at build against the project. Substrate
  fields — bind address, port, TLS material, public hostname, auth scheme,
  listener concurrency, queue connection strings — are **deliberately absent**
  from `tau.toml`; including them would make tau the service.
- A `[trigger.*.retry]` sub-table declares `max_attempts`, `backoff`
  (`fixed | exponential`, `base`, `max`), and `dead_letter`. **Retry is a
  trigger-level re-invocation policy honored by the host, not a per-node
  interpreter retry** — putting retry inside the engine would require durable
  timers and persisted attempt counters (durable state in core), which is
  NG3 + NG6-adjacent. Retry is durable exactly where the host is durable, and
  nowhere else.
- `dead_letter` names a **sink contract** — an MCP contract (`mcp:<name>`) or
  an already-granted capability target (an `fs.write` path, a `net.http`
  host). tau never stands up a queue, table, or spool of its own.
- Triggers lower as **metadata** parallel to the `tau.caps` custom section,
  adding no executable IR nodes. Because the binding lives in the canonical
  IR, it is content-hashed and reproducible, and `tau verify --bundle` covers
  it for free — "the trigger is part of the program" becomes literally true.
- `tau build --emit-trigger=<adapter>` optionally emits a host-adapter
  descriptor (systemd `.timer`+`.service`, k8s `CronJob`/`Ingress`,
  cf-workers cron stanza, Lambda EventBridge rule) the operator applies. The
  cargo → systemd-unit analogy made literal: tau produces the wiring; the
  operator owns the substrate. The descriptor set is open and additive.

### Egress-only capability = NG3 enforced at the type level

A webhook receiver needs to accept an inbound connection. tau has no
capability that grants inbound — and **that is the resolution, not a problem
to patch.** The egress-only vocabulary means tau cannot grant "receive an
inbound connection" because tau is not a service. An inbound trigger is
therefore un-grantable by tau by construction: NG3 is structural, not a
policy that could be relaxed.

### Inbound resolves to a host-adapter contract, never a runtime capability

`cron` and `manual` triggers need no inbound primitive — the host invokes
tau as an ordinary child process (an egress-shaped, already-expressible
relationship). They lower to IR metadata only and are the cleanest fit, hence
the natural v1 priority.

`webhook` and `queue` triggers require a primitive tau has no capability
shape for. Their binding lowers to IR metadata **plus a "host-adapter
required" marker**. The host adapter owns the socket and is the only thing
that can satisfy it. This gives `tau check` a real, enforceable rule: a
`webhook` or `queue` trigger with no corresponding host adapter is a
**deploy-time hole tau can name statically** — Rust-class build-time
enforcement of what would otherwise be a silent runtime surprise. The line
stays bright: ingress is always a host adapter that invokes the artifact,
never a mode of `tau serve`.

The HTTP serve transport is a **separate axis, gated on who initiates**, not
on the wire protocol. An HTTP-as-RPC transport where the host POSTs
`runtime.run` and gets a response (LSP-over-TCP) keeps the host driving and
is defensible but deferred (no demand). The moment an HTTP endpoint starts
accepting events and initiating runs on its own, it has become the rejected
webhook listener regardless of label.

## Consequences

**Positive:**

- The trigger lives *inside* the content-hashed artifact: "when and how am I
  invoked" becomes versioned, portable across every target, and reproducible
  — killing the dev/prod drift the compiler philosophy exists to eliminate.
- Adds the one thing only tau can add (a portable, capability-checked,
  content-hashed trigger binding) and delegates everything that would make
  tau a service. Triggers join inference and credentials as a third
  delegated substrate.
- Every non-goal "no" is structural, not promissory — the egress-only
  vocabulary and the "retry = host re-invocation" boundary make the
  violations *un-expressible*, not merely discouraged (NG3/NG5/NG6/NG9/
  NG11/NG12 all cross-check clean).
- `tau check` gains a statically enforceable deploy-time rule for
  webhook/queue triggers.

**Negative:**

- Operators of `webhook`/`queue` triggers must supply a host adapter; tau
  names the gap but does not fill the socket. (Intentional — that is NG3.)
- An HTTP serve transport that some users will want is deferred, not shipped.

**Neutral / obligations:**

- This ADR records a **framing stance, not a roadmap commitment.** No code,
  no plan, no phase pull. An implementation spec must still be written; the
  framing fixes the boundary, the spec fixes the surface.
- If pursued, the cleanest first slice is `cron` + `manual` + retry policy +
  `--emit-trigger=systemd|k8s` — the self-contained kinds that need no
  host-adapter contract — with `webhook`/`queue` following once the
  host-adapter contract in `tau check` is specified.
- Open questions deferred to the impl spec: IR placement of `triggers`
  (sibling of `workflow` vs. field inside it); the additive `ir_format` bump
  + forward-compat read; dead-letter envelope shape; `tau check` rule
  severity (warning at build vs. error under `--require-deployable`); which
  `--emit-trigger` targets ship first; cron dialect + timezone handling;
  one-agent-per-trigger vs. fan-out.

## Alternatives considered

- **Road A — pure "host drives it", add nothing.** tau ships no `[trigger]`
  table; operators hand-write systemd units, k8s CronJobs, and gateway routes
  that call `tau run`/`tau serve`. *Rejected because:* the trigger then lives
  outside the content-hashed artifact — the program's "when and how am I
  invoked" is unversioned, non-portable, and re-authored per target, exactly
  the dev/prod drift the compiler philosophy kills. The cost of Road A is
  precisely the value tau adds elsewhere.
- **Road B — full orchestrator (the n8n / Temporal trap).** tau grows a cron
  daemon, a webhook server, a durable queue, a DLQ store, visual retry
  policies, and a run-history database. *Rejected because:* this is NG3 + NG5
  in their purest form, forces durable state into core (NG6), and turns tau
  from a developer-facing compiler (NG11/NG12) into an operations platform.
  The market already has Temporal, n8n, Airflow, and every cloud scheduler;
  tau's differentiator is the portable capability-safe artifact, which those
  tools do not produce. Road B trades a unique position for a crowded one.

## References

- Framing spec: `docs/superpowers/specs/2026-06-13-trigger-ingress-and-serve-transport-framing.md` (PR #330)
- Philosophy: `docs/explanation/tau-philosophy.md` (the compiler convictions)
- Related ADRs: [0014](0014-sandboxing.md) (sandbox capability shapes),
  [0033](0033-tau-serve-mode.md) (serve mode v1), [0037](0037-workflow-ir.md)
  (workflow IR), [0035](0035-bundle-format.md) (bundle format)
- ROADMAP non-goals: NG3, NG5, NG6, NG9, NG11, NG12
