# ADR-0015: Wall-exhaustion grace is clock-source policy over `cancel`, not a kernel rule

**Status:** Accepted
**Date:** 2026-09-26
**Deciders:** tau core
**Amends:** [ADR-0002](0002-seven-syscalls.md) (one more proposal recorded as
failing the irreducibility test) and [ADR-0014](0014-driver-supervision.md)
(the sender of a wall-grace notice is `Harness`, and §8's objection to
`from: harness` does not reach it)
**Decides:** [#197](https://github.com/tau-rs/tau/issues/197); re-scopes
[#17](https://github.com/tau-rs/tau/issues/17)

## Context

### In plain language

An agent's budget has several dimensions: tokens, calls, cost, and wall time.
Three of them run out in the agent's own hands: a `send` that the budget
cannot cover is *refused*, the program sees the refusal, and it can exit with
whatever it has. Wall time is different. Nobody spends it on purpose; a clock
source outside the kernel writes readings into the log as `Tick` entries, and
the tick that empties an agent's grant aborts it inside the same apply
(ADR-0002, "wall time is spent by the clock"). The agent gets no warning and
no last word. A parent that fanned work out to children and is waiting on
them loses every result it had not yet claimed.

The kernel already owns a gentle ending. `cancel` (ADR-0002) freezes a
subtree, puts a `Notice` in every frozen mailbox, sets a deadline the kernel
enforces, and hard-aborts whatever is still running when the deadline arrives.
A frozen agent may still `recv`, `wait` and `exit`; it may not `spawn`, `send`
or `cancel`. That is exactly the shape a wall-exhaustion warning needs: a
notice, a bounded window to wrap up, an abort that nothing in userspace can
postpone.

So the only open question in #197 was **where the trigger lives**: inside the
kernel, as a rule the reducer applies on every tick, or outside it, as a
policy of the thing that already owns time.

The analogy: a library closes at nine. It can either wire the lights to a
clock that cuts them at nine sharp, or have the front desk walk the floor at
a quarter to and say so. Both close at nine. Only one of them lets a reader
finish the sentence.

### What is true today, and shapes the decision

- **The reducer never reads a clock** (ADR-0003). Time is a `Tick` entry;
  `State::now` is the last one applied. A cancel deadline is an absolute
  reading on the agent's record, set by the `Cancelled` entry as
  `now + grace`, and only ever brought earlier.
- **The harness owns the clock.** `kernel/src/driver/clock.rs` holds the only
  two things that append ticks: `VirtualClock`, on demand, and `WallClock`,
  on an interval. The private `read` in that module is the one permitted
  `Instant::now` in the workspace. A clock source knows the next reading
  before it publishes it.
- **The harness can already cancel any agent.** `Kernel::cancel_from_harness`
  writes a `Cancelled` entry with `by: None`; the reducer stamps the notice
  `from: Endpoint::Harness`.
- **The kernel's state is readable.** `Kernel::state()` returns the fold's
  `State`; an agent's remaining wall is `budget.get(&DimKey::WallMs)`, its
  status is `Live`, `Cancelling`, or finished.
- **Every timed subtree is timed all the way down.** A child of a timed
  parent may not be untimed (ADR-0002, M1b). A child's remaining wall may
  nonetheless exceed its parent's, because the parent carved the child's
  grant out of its own and both are charged by every tick.
- **A parent whose wall empties is aborted alone today.** Its live children
  carry on as orphans; their unspent grant, when they finish, walks up to
  the nearest live ancestor or the root's record (`heir`). The only party
  that can ever claim an orphan's result is the harness, via
  `Kernel::claim`.
- **`Endpoint::Kernel` exists since `ABI` 3** (ADR-0014 §2). It is the sender
  of an `Unanswered` reply, chosen because on `crashed` and `overdue` no
  harness code acted. #17 was filed before that variant existed and framed
  the whole question as needing it.
- **ADR-0014 already rejected a tick-apply rule over per-record deadlines**
  for `reply_within`: it would put `corr → deadline` in canonical state and
  move every pinned hash. The same shape was on the table here.
- **Three pin surfaces move on a reducer change and not otherwise**
  (ADR-0011): the fixture hash snapshots in `kernel/tests/`, the corpus
  sidecars under `corpus/`, and `PINNED` in the sim.

### Constraints

- ADR-0002's irreducibility test: what is expressible over the existing
  surface does not go into the kernel, and a proposal that fails the test is
  recorded as failing it.
- ADR-0004 and ADR-0010: `kernel/src/abi/` moves additively with a bump, or
  not at all. A grace the reducer enforces must be readable from the log, so
  it must ride the wire.
- The M1b statement "wall exhaustion is the deadline" is pinned by
  `kernel/tests/fixtures/m1b-wall.log` and stays true.

## Decision

### 1. The rule: a policy of the clock source, issued through `cancel`

The soft path is wanted, and it is **not a kernel rule**. It is a policy the
clock source runs before it publishes each tick, using the harness's
existing `cancel`:

> Before publishing reading `t`, for every agent that is `Live` (not yet
> cancelling), timed, and whose remaining wall after `t` would be at most
> the grace `g`: `cancel_from_harness(agent, CancelMode { grace: remaining,
> reason })`, where `remaining` is the agent's wall grant as of the last
> published reading.

Because the reducer sets the deadline as `now + grace` and `now` is the last
published reading, the deadline is exactly the reading at which the agent's
grant would have emptied anyway. **The deadline never moves; the notice
moves earlier.** The tick that would have aborted the agent still aborts it,
in the same apply, if it has not exited. "Wall exhaustion is the deadline"
remains literally true.

```mermaid
sequenceDiagram
    participant C as clock source (harness)
    participant K as kernel
    participant A as agent
    Note over C: before publishing tick t
    C->>K: cancel_from_harness(agent, grace = remaining)<br/>for each Live, timed agent whose remaining ≤ g after t
    K->>A: Cancelled entry: subtree freeze + Notice(from: harness, reason)
    Note over A: may recv / wait / exit; may not spawn / send / cancel;<br/>a reply already in flight still lands
    C->>K: tick t, t+1, …
    K->>A: the tick that empties the grant: Aborted (same tick as today)
```

One agent granted `wall_ms: 100`, clock period 10, grace 30:

| clock reading | today | with the policy |
|---|---|---|
| 60 | `Live`, 40 left | `Live`, 40 left |
| 70 | `Live`, 30 left | **`Cancelling`**: notice in mailbox, deadline 100 |
| 70–100 | `Live`, may still `send` | frozen: `recv`/`wait`/`exit` only; a reply already in flight is delivered |
| 100 | `Aborted` | `Aborted` if it has not exited, in the same tick |

In the log the `Cancelled` entry precedes the `Tick` that opens the window,
which is what makes the guarantee statable: the notice is in the mailbox at
least `g` clock units before the deadline, at one-tick granularity.

Why the clock source and not the kernel, in three sentences:

1. **It passes the irreducibility test only one way.** The harness owns the
   clock and can already cancel; "one tick before an agent enters its last
   `g` units, cancel it with grace equal to its remaining wall" is a program
   over what exists. What is expressible over the existing surface does not
   go into the kernel (ADR-0002).
2. **The kernel version is the design ADR-0014 already rejected.** A tick
   apply acting on per-record lead times held in canonical state moves every
   pinned hash and bumps `FOLD`, to enforce a courtesy.
3. **The kernel version is an ABI bump after all.** The reducer cannot
   enforce a grace it cannot read from the log, so a per-spawn or
   harness-declared grace lands as a new defaulted field on `Spawned`
   (ADR-0010: "allowed, defaulted, and bumped"). `Endpoint::Kernel`
   existing made the *sender* free, not the grace.

### 2. Who sets it, and the default

- **Default: zero. A harness that does not opt in sees today's behaviour
  exactly.** No single lead time is right for both a 100 ms child and a
  one-hour root, so any non-zero default is wrong for one of them.
- **The harness sets it, on the clock source.** `VirtualClock` and
  `WallClock` in `kernel/src/driver/clock.rs` gain a grace: a flat number of
  clock units, or a policy over the agent record when the harness wants a
  per-agent lead time. Why the harness and not the parent: the program that
  needs the lead time is one the harness spawned and knows, and a parent has
  no clock and no wire field to say "this much" with. Why not a kernel
  constant: the default.
- **The reason is the harness's.** `CancelMode.reason` is a payload the
  kernel does not read; the clock policy supplies one so a program can tell
  a wall warning from another cancel if it cares to. Its shape is not this
  ADR's to freeze.

### 3. The sender is `Harness`

The notice's `from` is `Endpoint::Harness`, because that is what the
reducer stamps on a `Cancelled` entry with `by: None`, and it is accurate:
harness code, the clock policy, wrote the entry. ADR-0014 §8 kept the
`Unanswered` reply off `from: harness` because "a reader that sees
`from: harness` would look for the harness code that wrote it and find
none". Here the reader finds it, in the clock source. `Endpoint::Kernel` is
not the sender, and #17's premise that a new endpoint was the gate is
withdrawn.

### 4. Subtrees freeze with the parent

`cancel` is a subtree freeze, so under this policy a parent that enters its
last `g` units takes its live children into `Cancelling` with it, each with
the parent's deadline or an earlier one it already had. They are aborted at
the parent's exhaustion if they have not exited. Today they would have run
on as orphans.

This is the one behaviour change beyond the notice, and it is decided
deliberately: a parent that is out of time can never `wait` on those
children, so their results could only ever have been claimed by the
harness, and a frozen child still gets to `exit` with what it has. A
harness that wants orphans on today's path writes its policy to skip agents
with live children; the kernel does not decide this for it.

### 5. What the kernel does not do, recorded per ADR-0002

ADR-0002 obliges a proposal that fails the irreducibility test to be
written down as failing it. The rejected rule, as #17 stated it:

> On every tick, for each timed agent whose remaining wall is at most a
> grace `g` carried on its record, the reducer freezes it and its subtree
> and stamps a notice `from: Endpoint::Kernel`.

Rejected because it is expressible over the seven plus the harness's
existing `cancel_from_harness` (§1 point 1); because it repeats the
tick-apply-over-canonical-deadlines design ADR-0014 rejected (point 2); and
because `g` must reach the reducer through the log, which makes it a
`Spawned` field and an `ABI` 3 → 4 bump (point 3). Its one genuine edge,
that a reducer-side freeze could skip phase two and not abandon the request
in flight, is advisory here: a reply that arrives during the grace is still
delivered to the frozen agent, and the abandon only tells the driver it may
stop.

### 6. Tests the implementing lane must add

- A kernel integration test over `VirtualClock` with a non-zero grace: one
  timed root, one timed child whose grant is smaller, a driver that never
  answers. The child receives a `Notice` `from: Harness` one tick before its
  last `g` units, is `Cancelling` with `deadline` equal to its exhaustion
  reading, exits cleanly inside the window, and the parent's `wait` sees
  `Exited`. A second child that does not exit is `Aborted` on the same tick
  it would have been today.
- A subtree case: the parent enters its window first, its child is frozen
  with it and aborted at the parent's exhaustion.
- A grace of zero over the same tree produces byte-for-byte the log
  `m1b-wall.log` pins.
- Recorded as a new fixture under `kernel/tests/fixtures/` with its own hash
  snapshot, and a new corpus entry if the nightly should hold it.

## Consequences

- **No code in `kernel/src/abi/`, no `ABI` bump, no `FOLD` bump.** The
  change is in `driver::clock` and a test. `Endpoint`, `Entry`, `Spawned`
  and `State` are untouched.
- **#17 is re-scoped, not implemented as written.** It becomes "clock:
  wall-exhaustion grace policy over cancel", loses `abi-change`, and points
  here.
- **One-tick granularity.** The notice can arrive up to one clock period
  earlier than `g` units before the deadline, never later. A harness that
  wants a tighter window ticks faster.
- **Phase two abandons the in-flight request.** As with every cancel, the
  driver holding an open request is told the requester is gone; a reply that
  arrives anyway during the grace is still delivered. A program that wants
  the answer it is waiting for should set a grace longer than its driver's
  `reply_within`.
- **Orphans are gone under a non-zero grace.** §4. A harness that prefers
  today's orphaning writes the skip into its policy.
- **The reference policy lives where CI keeps it honest.** As ADR-0014's
  amendments settled for the supervisor, the worked policy is the test named
  in §6, not an unexercised example.

## Alternatives considered

| Option | Cost | Failure it allows |
|---|---|---|
| **A. Kernel rule, grace on `Spawned`** (the #17 design) | `ABI` 3 → 4; a reducer change; grace becomes canonical `Agent` state so sidecars and `FOLD` move unless the field is skip-if-none; a new fixture and corpus entry | Repeats the tick-apply design ADR-0014 rejected; the wire carries a field forever for a courtesy. Its one edge: it could skip phase two so an in-flight call is not abandoned. |
| **B. Clock-source policy over `cancel`** (chosen) | A change in `driver::clock` only; no `ABI`, no reducer, no pin moves | One-tick granularity; phase two abandons the in-flight request (advisory: a reply that arrives during grace is still delivered) |
| **C. Kernel constant** | Smallest code | Freezes a 100 ms child at birth, or gives an hour-long root a useless second |
| **D. `DimKey::Custom("wall_grace")` in the budget** | No ABI text change | Gives an existing wire form a new meaning (ADR-0004 forbids it) and drags a non-resource through carve and return |
| **E. No soft path** | Nothing | Fan-out parents lose every unclaimed child result; no agent can ever say "here is what I had" |

## Pins

**Nothing frozen moves.** This ADR is not a reducer change: no fixture hash
snapshot in `kernel/tests/`, no corpus sidecar under `corpus/`, and no
`PINNED` seed in `sim/tests/determinism.rs` changes. `m1b-wall.log` keeps
pinning the hard path, which is what a grace of zero still produces. The
implementing lane adds a new fixture (§6) and touches no existing one.
