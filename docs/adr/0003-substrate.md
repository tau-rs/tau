# ADR-0003: Substrate — in-process tokio body, event-sourced constitution

**Status:** Accepted
**Date:** 2026-09-13
**Deciders:** tau core

## Context

Four substrates were on the table, and they are not mutually exclusive:

- **A — in-process.** Agents are tokio tasks; the kernel is a library. Fast,
  debuggable, one binary.
- **B — event-sourced.** Every effect is appended to a log before it is applied;
  state is a fold over the log.
- **C — durable execution.** Crash, then resume: refold the log, replay
  completed effects from a journal instead of re-performing them.
- **D — OS isolation.** Subprocesses, cgroups, namespaces, microVMs.

Picking one is the usual mistake. A is a body, B is a constitution, C is a
property, D is a containment boundary; they answer different questions.

The question that actually decides the shape is: **what does the system do that
it cannot retrofit?** Event sourcing is the answer. A journal can be added to a
mutable-state system only by rewriting every mutation site, which in practice
means never. Durable execution, replay, deterministic simulation, time-travel
debugging, and the backward-compatibility oracle all follow from the log
existing on day one, and none of them are reachable without it.

## Decision

**A + B are adopted together. C is a planned upgrade. D is quarantined.**

- **A is the body.** Agents are async tasks; the kernel is a library the harness
  links. No process-per-agent, no network hop in the common path.
- **B is the constitution.** Append-before-apply, always. The reducer is a pure,
  deterministic fold:

  ```text
  fn apply(state, entry) -> state
  ```

  with **no clock, no RNG, and no iteration-order leaks**. Time enters the
  system only as log entries: the clock driver appends ticks and armed
  deadlines, and wall-budget enforcement happens in the reducer when a tick is
  *applied*. The reducer never reads a clock.

  This is enforced mechanically, not by review. `clippy.toml` denies
  `HashMap`/`HashSet` (iteration order leaks into the state hash and surfaces as
  a cross-platform divergence far from the commit that caused it) and denies
  `SystemTime::now`/`Instant::now` outright.

- **C becomes cheap later** precisely because the journal exists from the start:
  crash → refold the log → completed effects replay from the journal rather than
  being re-performed, so a resumed run does not re-bill a model call. Scheduled
  for M4.
- **D lives *only* inside the sandbox driver** — subprocess plus rlimits first,
  cgroups/namespaces after, Firecracker-class if it is ever warranted. Agents
  are trusted loops. The untrusted thing is the code a model asks to run, and it
  is contained where it enters, not everywhere.

Four invariants make the above load-bearing rather than decorative:

1. **No arrow skips the kernel.** Agents never touch drivers; parents never
   touch children directly. Every effect is a log entry.
2. **The kernel never parses payloads.** It routes by capability and accounts by
   driver-reported consumption metadata.
3. **The log is the kernel; everything else is cache.** Any behavior that works
   without a log entry is a bug.
4. **Anything invisible to the log is a replay bug** — no bypass channels, no
   userspace `JoinHandle`s held as hidden state, `recv` resolutions logged.

## Consequences

- **Determinism is a tested property at three intensities.** A fast in-PR check
  (3 seeds, ~10k events, fold twice, compare state hashes), a deep simulation
  soak (10^6+ events, plus a *cross-platform* refold where a macOS runner folds
  a Linux-produced log), and a nightly sentinel that refolds a stored corpus of
  historical logs with today's `main`. One invariant, three budgets — the
  template for every future invariant.
- **The nightly sentinel is "we do not break userspace", executable.** If
  today's reducer produces a different state hash for a log from three releases
  ago, the promise is broken, and it is broken *loudly*, on a schedule, instead
  of on a user's machine.
- **Append-before-apply costs latency.** Every effect pays a log write before it
  takes effect. That is the price of the property and it is accepted; the
  benchmark KPIs (reducer throughput in entries/sec, `send`→deliver latency) are
  tracked so the price stays known rather than merely paid.
- **Cancel-safety becomes a rule with teeth.** Agent futures may hold only plain
  memory and the kernel handle — no guards across an `await`. Enforced by review
  and lint, because a drop-guard that runs during a hard abort is state the log
  never saw.
- **`unsafe` is forbidden workspace-wide**, and panics are denied in kernel code
  (`unwrap_used`, `expect_used`, `panic`, `indexing_slicing`). A panic in the
  reducer is unrecoverable for every agent in the tree, and a partially-applied
  entry is the one state the model has no name for.

## Alternatives considered

**A alone (mutable in-process state, no log).** Rejected: fastest to write,
impossible to upgrade. Replay, resume, and the backward-compat oracle all become
unreachable, and the decision is irreversible in practice.

**B without A (process- or actor-per-agent from the start).** Rejected as
premature: it pays distribution costs — serialization at every hop, supervision
trees, partial-failure semantics — before there is any evidence they are needed.
The log makes the move *possible* later; nothing about A forecloses it.

**C from day one.** Rejected as sequencing, not as direction. Durable execution
needs a journal, a replay story, and driver idempotency keys, all of which are
cheap once B exists and expensive to design against a core that is still moving.
M4.

**D everywhere (every agent in its own sandbox).** Rejected: it taxes every
agent to contain a threat that only exists at one boundary. Agents are code the
operator wrote; model-authored code is the untrusted input, and it arrives
through exactly one driver.

## Amendments

- **2026-09-16** — Invariant 3 gains its one exception
  ([ADR-0012](0012-blob-store-crypto-shredding.md),
  [#121](https://github.com/tau-rs/tau/issues/121)). "Everything else is
  cache" holds for every *derived* thing — state, indexes, snapshots.
  Payload bytes are not derivable from the log; they live in the blob store,
  outside the log on purpose, so that they can be erased while the log
  cannot. The store is the other half of the truth, not a cache to be
  rebuilt, and a shred is not a log entry because it has no effect on the
  fold.
