# ADR-0002: The seven syscalls, and the irreducibility test

**Status:** Accepted
**Date:** 2026-09-13
**Deciders:** tau core

## Context

A kernel is defined by what it refuses to grow. Linux has ~400 syscalls, but
the ones that matter — `fork`, `exec`, `exit`, `wait`, `open`, `read`, `write`,
`close` — were settled early and have never broken. Everything since is either
a variant of those or a mistake that had to be kept.

tau v1 had no such boundary. Its capability surface was whatever the union of
its subsystems happened to expose that month, which is why keeping it coherent
took 47 written rules ([ADR-0001](0001-tau-rebooted.md)). A boundary defined by
accretion is not a boundary.

The design question is therefore not "what should agents be able to do" — it is
**"what is the smallest set of operations such that everything else is a program
over them"**.

## Decision

An agent can do exactly seven things. Nothing else is expressible.

| # | Syscall | Signature (abridged) | Group |
|---|---|---|---|
| 1 | `spawn` | `spawn(program, ns: Namespace, budget: Budget) -> AgentId` | tree |
| 2 | `exit` | `exit(result: Bytes) -> !` | tree |
| 3 | `wait` | `wait(Child(id) \| Any) -> ExitResult` | tree |
| 4 | `cancel` | `cancel(id, CancelMode { grace, reason })` | tree |
| 5 | `send` | `send(cap: Capability, payload) -> Corr` | flow |
| 6 | `recv` | `recv(filter: Match) -> Msg` | flow |
| 7 | `attach` | `attach(HookPoint, HookProgram, FailureMode) -> HookId` | meta |

Four manage the tree, two the flow, one the meta. The semantics that are not
negotiable:

- **`spawn`**: the child's namespace must be a subset of the parent's, and it is
  snapshotted at birth. The budget is carved *atomically* from the parent's —
  every dimension or none — and what the child does not spend returns to the
  parent at exit. Authority and resources only ever narrow going down the tree.
- **`exit`**: consumes the kernel handle, so "this is my last act" is a
  type-level guarantee rather than a convention. The result is stored until
  claimed.
- **`wait`**: level-triggered. Results persist until claimed, so a parent that
  waits late does not lose a child's result; `Any` returns in completion order.
- **`cancel`**: two-phase, and the phases are the whole design. An atomic
  subtree freeze, then a `CancelNotice` plus driver abandon, then a
  kernel-enforced grace deadline, then a hard abort. Nothing in userspace can
  extend the deadline.
- **`send`**: the capability *is* the address and the permission at once. Never
  blocks — a full queue is a `WouldBlock` error the caller handles, not a hidden
  await that turns backpressure into deadlock. Appends to the log before it
  delivers.
- **`recv`**: a *closed* filter language — correlation, sender, kind, any, or.
  Closed on purpose: an open predicate language would let userspace push
  arbitrary computation into the kernel's delivery path. Which message matched a
  filter is itself a log entry, because a resolution that is not logged is a
  replay divergence waiting to happen.
- **`attach`**: harness-privileged only. Verdicts are `Allow`, `Deny(reason)`,
  `Emit(Notice)`; every verdict is logged.

**The irreducibility test.** Any proposed eighth syscall must be shown
*inexpressible* as a program over these seven. If it is expressible, it belongs
in `libtau` — the userspace convenience layer that has no special powers. This
test is the structural replacement for v1's 47 guidelines: one question, asked
of every proposal, with an answer that is demonstrable rather than arguable.

## Consequences

- **`libtau` carries the ergonomics.** `infer()`, tool loops, retries, fan-out,
  cascade routing, sub-agent orchestration patterns — all of it is ordinary code
  over the seven, with no privileged access. If a convenience cannot be written
  that way, that is evidence about the kernel, and it goes through this ADR's
  test rather than into the kernel directly.
- **Sub-agent orchestration is not a feature.** Exposing `spawn`/`wait`/`cancel`
  to a model as tools is a harness *choice*, and containers-of-agents emerge
  from namespaces and budgets rather than being implemented.
- **Prompt injection degrades to contained failure.** A hijacked model in a
  search-only child can, at worst, search — because authority is the namespace
  it was born with, and namespaces only narrow.
- **New capability kinds cost nothing.** A new device is a driver behind a
  capability; the syscall surface does not move. This is why the kernel never
  parses payloads: it routes by capability and accounts by driver-reported
  consumption, so model-agnosticism is structural rather than aspirational.
- **The obligation this creates**: proposals that fail the irreducibility test
  must be *recorded* as failing it. A rejected eighth syscall that is not
  written down gets proposed again in six months.

## Alternatives considered

**A larger, more convenient surface** (a `call` that is send-then-recv, a
`broadcast`, a `map` over children). Rejected: each is demonstrably a program
over the seven, which is exactly what the irreducibility test is for. Adding
them would mean freezing three more signatures forever to save `libtau` twenty
lines each.

**An open predicate language for `recv`.** Rejected: it moves arbitrary
userspace computation into the kernel's delivery path, makes delivery cost
unbounded and un-metered, and makes the "which message matched" log entry
dependent on evaluating user code at replay time. The closed language keeps
replay a pure fold.

**A blocking `send` with backpressure.** Rejected: a blocking send inside an
agent that is also being cancelled creates exactly the await-point-holding-state
that the cancel-safety rule forbids. `WouldBlock` makes backpressure a visible,
handleable condition instead of a hang whose cause is three layers away.

**Separate syscalls for capability transfer.** Rejected: transfer rides in a
message, so it is already `send`. Giving it its own syscall would duplicate the
authority check in two places, and the second one is where the bug would live.
