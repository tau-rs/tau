# Trigger ingress

tau compiles a declarative `[trigger]` table into the workflow artifact as
portable, capability-safe metadata. A **host adapter** reads that metadata and
wires the real trigger. tau emits the *binding*; the host owns the *substrate* —
the scheduler, the socket, the durable queue. This is the same delegation tau
applies to inference and credentials.

> Slice 1 (this page) ships `cron` and `manual` triggers plus a retry policy.
> `webhook` and `queue` are slice 2 and are not yet available.

## The two halves of a trigger

| Half | Example (cron) | Who owns it |
|---|---|---|
| **Substrate** | the timer that fires at 03:00 | the host (systemd, k8s, Lambda, …) |
| **Binding** | "fire `summarizer` on `0 3 * * *`" | tau (declared once, compiled, portable) |

tau adds only the binding — as compiled, content-hashed metadata — and leaves
the substrate where it already lives.

## Declaring triggers in `tau.toml`

```toml
# A named cron binding.
[trigger.nightly]
kind     = "cron"
agent    = "summarizer"        # entrypoint — must be an existing agent id
schedule = "0 3 * * *"         # 5-field cron
timezone = "UTC"               # optional; default UTC

[trigger.nightly.retry]
max_attempts = 3               # total attempts including the first
backoff      = { strategy = "exponential", base = "30s", max = "10m" }
dead_letter  = "dlq-sink"      # sink reference (an MCP contract or granted target)

# The default, made explicit: tau is invoked by an external driver.
[trigger.manual]
kind  = "manual"
agent = "summarizer"
```

`cron` requires a 5-field `schedule`; `manual` takes no schedule. `tau build`
validates the entrypoint agent exists, the cron field count, plain-integer
field ranges, and the retry durations.

## How a trigger compiles

A trigger lowers as **metadata** — it adds no executable node:

- It rides inside the canonical IR (`IrModule.triggers`), so it is
  content-hashed and reproducible. A trigger-less project hashes identically to
  before triggers existed.
- It is mirrored in the bundle manifest as a `[[trigger]]` section, which bumps
  the bundle `schema_version` to `3` so an older tau rejects a trigger-bearing
  bundle rather than silently dropping the binding.

## Emitting host-adapter descriptors

`tau build --emit-trigger=<adapter>` writes scheduler wiring next to the bundle:

```text
tau build --emit-trigger=systemd   # writes tau-<name>.timer + tau-<name>.service
tau build --emit-trigger=k8s       # writes tau-<name>.cronjob.yaml
```

k8s `CronJob` consumes the 5-field cron verbatim. systemd `OnCalendar` is
generated for schedules whose fields are `*` or plain integers; schedules using
ranges, lists, or steps are skipped for systemd (use `k8s` for those). `manual`
triggers emit nothing — the host invokes tau directly.

## Why retry is host-honoured

Retry is a trigger-level re-invocation policy, not a per-node interpreter retry.
The host re-invokes the artifact; tau's interpreter stays deterministic and
stateless across invocations. `dead_letter` names a sink (an MCP contract or an
already-granted capability target) — never a tau-owned queue. This keeps
durable state out of tau's core.

See [ADR-0044](../decisions/0044-trigger-ingress-slice-1.md) for the design
rationale.
