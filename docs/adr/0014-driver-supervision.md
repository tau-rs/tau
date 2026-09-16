# ADR-0014: Driver supervision — health in the log, an unanswered request fails explicitly, restart under the same capability

**Status:** Accepted
**Date:** 2026-09-16
**Deciders:** tau core
**Amends:** [ADR-0004](0004-abi-freeze.md) and [ADR-0010](0010-entry-freeze.md)
(`ABI` 2 → 3: one `Endpoint` variant, three `Entry` kinds, one defaulted
field), [ADR-0006](0006-model-bridge-contract.md) (the "M3 error envelope"
it deferred to is this: a reply from the kernel, not a payload),
[ADR-0013](0013-agent-drivers.md) (§2's "`unavailable` becomes a health
entry" is resolved: it stays a reply kind, and the supervisor acts on it),
[ADR-0009](0009-sandbox-driver.md) §3 and
[#18](https://github.com/tau-rs/tau/issues/18) (what acts on overdraft is
the supervisor, through an event, not the fold)

## Context

### In plain language

A driver is the only code in tau that touches the outside world: it takes a
request the kernel routed to it, does the work — an HTTP call to a model, a
subprocess, a CLI — and hands back a reply and a bill. The kernel never reads
either. Today a driver that *answers* is fully handled, whatever it says:
"the provider returned 500", "over the ceiling", "the child was lost" are all
replies, logged as `Replied`, billed, delivered ([ADR-0006](0006-model-bridge-contract.md),
"driver failures are reply payloads, not exceptions").

What is not handled is a driver that **does not answer**. If its `handle`
panics, the kernel's loop for that driver silently ends; if `handle` never
returns, the loop waits forever. In both cases the request's correlation
stays open, the budget reserved for it stays held, and the agent that sent
it sits in `recv` until its own wall clock runs out — or, for an agent with
no wall grant, for good. The run never drains. HANDOFF §4 item 8 forbids
exactly this: "in-flight corrs across driver restart fail explicitly (error
envelope, never a hang)".

The picture to hold: a driver is a clerk at a counter. Agents hand in
tickets; the clerk hands back an answer and a receipt, and the ledger
records both. If the clerk faints, the tickets on the counter are not lost
in silence: the manager stamps each one *unanswered*, charges the maximum
the ticket could have cost (because nobody knows how far the clerk got),
records in the ledger that the clerk went down, and — if the manager
decides to — seats a new clerk at the same counter, under the same sign.
The tickets still in the queue behind the counter, which no clerk ever
touched, are handed to the new one. And a ticket that has waited past the
time the manager promised is stamped unanswered even if the clerk is only
slow.

This ADR decides: what a driver going down, coming back, or being retired
looks like in the log; what closes an open request and what it is billed;
what the fold confirms and what it ignores; what the kernel itself detects
and what it leaves to the harness's supervisor; and what #18 becomes. The
kernel implementation is a follow-on lane
([#148](https://github.com/tau-rs/tau/issues/148)).

### What is true today, and shapes the decision

- **A driver is an in-process `Driver` impl behind one loop.**
  `Kernel::register_driver` (`kernel/src/kernel.rs`) mints the capability,
  commits `DriverRegistered { driver, cap, ceiling }`, and spawns one task:
  `next_delivery` → `handle` → `reply`, sequentially — one `handle` at a
  time per driver. The loop ends only when `next_delivery` returns `None`,
  which it does at `shutdown` or on a kernel fault. Its abort handle is not
  kept: "nothing cancels them but shutdown". A panic inside `handle`
  unwinds the task; the kernel never learns.
- **`handle` has no error channel, on purpose.** `fn handle(&self, Delivery)
  -> BoxFuture<(Vec<u8>, Consumption)>`. ADR-0006 made every driver-side
  failure a reply so that the tool loop can feed it back to the model. That
  stays. What this ADR adds is the case ADR-0006 named and deferred: the
  driver that raises instead of reporting.
- **The reducer already knows how to close a correlation and settle a
  reservation.** `Replied` removes the corr's reservation, `settle()`s it
  against the report — spent up, refund down, excess to `overdraft` — and
  pushes the envelope into the owner's mailbox. `finish()` releases an
  exiting agent's reservations and drops its corrs, which is why a late
  reply is `UnknownCorr` and dead-lettered by design (HANDOFF §4 item 9,
  the loop comment in `register_driver`).
- **Time is a log entry, and deadlines are the reducer's.** A cancel
  deadline is enforced inside the apply of the `Tick` that reaches it. The
  kernel does the *effects* after the reducer decided (`reap` pulls abort
  handles after `tick` commits). The clock driver is the only reader of a
  clock.
- **The kernel keeps a cache the fold does not have.** `Inner.in_flight:
  BTreeMap<Corr, DriverId>` — "derivable from the `Sent` entries'
  capabilities, kept warm for `cancel`". The fold has never verified that a
  `Replied` comes from the driver the corr was sent to: its check is that
  `msg.from` names *a* registered driver and the corr is open and owned by
  `to`.
- **Unknown consumption is billed up, never down.** ADR-0009 §5 for the
  sandbox, ADR-0013 §5 for agent tasks: a run that ended without a report
  bills the ceiling. The principle exists; this ADR applies it to the case
  where the driver itself is the thing that did not report.
- **`Entry` is frozen and every kind is an ABI event.** ADR-0010: a new kind
  bumps `ABI`, arrives with its snapshot and refusal test, and gets a row in
  ADR-0010 §3 by amendment; a new field on an existing kind is allowed,
  defaulted, and bumped. `State` is not frozen but every canonical field
  change moves every pinned hash and bumps `FOLD` (ADR-0011 §2).
- **The harness is not an agent.** It holds the kernel handle and owns the
  privileges an agent cannot express: `attach`, `cancel_from_harness`,
  `claim`, `tick`, `shred`. "Supervisor" in HANDOFF §4 item 8 is this code.
  There is no harness crate yet; the tests play harness, and the `tau`
  binary is a replay tool.
- **Overdraft is visible and unacted-on.** `Agent.overdraft` is canonical,
  "loud in the state hash"; ADR-0009 §3, ADR-0013 §6 and #18 all point at
  "driver supervision (M3)" as what acts on it.

### Constraints

- **The kernel never parses payloads** (ADR-0003 invariant 2) — and, until
  now, has never *authored* one either. Whatever closes a request must not
  require the kernel to know what a model reply, a sandbox reply or an
  agent reply looks like.
- **The reducer is a pure fold.** Whatever it does with a driver's death, it
  does from entries. It cannot observe a panic, and it cannot append.
- **One position per message.** Two messages sharing a `seq` in one mailbox
  make a `Resolved` ambiguous (ADR-0010 §3, why `Emitted` is its own entry).
  An agent may hold several open requests on one driver, so closing them
  cannot be the side effect of one entry.
- **No hang, with or without a supervisor.** A harness that boots a kernel,
  a clock and drivers and never writes a line of supervision code must
  still drain.
- **Payloads are erasable; the log is not.** Nothing a driver or a panic
  said may land in the log as prose (ADR-0012).

## Decision

### 1. Reporting and raising: the kernel classifies by what came back through `handle`, never by what it says

| What `handle` did | Word | What the kernel does | Supervision? |
|---|---|---|---|
| returned `(payload, consumption)` — any payload, including `error.provider`, `error.lost`, `error.unavailable`, a report above the ceiling | **reported** | `Replied`, as today | no entry; a report above the ceiling raises an `Overdrew` event (§6) |
| unwound (panicked) | **raised** | `DriverDown { cause: crashed }`, then one `Unanswered` per request it had taken | `Crashed` event |
| has not returned by the driver's registered bound | **raised** | one `Unanswered { cause: overdue }` for that request; the driver is *not* declared down | `Overdue` event |
| was never going to be called: the harness retired the driver | — | `DriverDown { cause: retired }`, then one `Unanswered` per open request, taken or queued | (the harness's own act) |

The boundary is mechanical, and that is the point: ADR-0006's rule stands
untouched — a driver that has something to say says it in the reply — and
supervision begins exactly where the reply does not come. A driver never
*asks* to be supervised, and the kernel never reads a reply to decide
whether it should be.

### 2. Three entries and one field

```text
DriverRegistered { seq, driver, cap, ceiling, reply_within: Option<u64> }   -- gains a field
DriverDown       { seq, driver, cause: crashed | retired }                   -- new
DriverUp         { seq, driver }                                             -- new
Unanswered       { msg, to, driver, cause: crashed | overdue | retired }     -- new
```

On the wire, in the shape ADR-0010 §3 pins (payload hashes shortened):

```json
{"entry":"driver_registered","seq":0,"driver":"model","cap":0,"ceiling":{"tokens":8000,"cost_microusd":50000},"reply_within":30000}
{"entry":"driver_down","seq":40,"driver":"model","cause":"crashed"}
{"entry":"unanswered","msg":{"abi":3,"seq":41,"from":{"kind":"kernel"},"corr":7,"kind":"reply","consumed":{"tokens":8000,"cost_microusd":50000},"payload":"…"},"to":3,"driver":"model","cause":"crashed"}
{"entry":"driver_up","seq":45,"driver":"model"}
```

- **`reply_within`** is the most clock units a request to this driver may
  wait for its answer, counted from its `Sent`. Same units as `Tick.now`
  (`wall_ms` by convention). `null` — the default, and what every existing
  log deserializes to — means unbounded: today's behaviour. It is the
  harness's declaration at registration, like `ceiling`, and it is on the
  wire for the same reason the ceiling is: "immutable means no change
  without a log entry" (HANDOFF §4 item 2), and an operator reading
  `unanswered … overdue` at seq 90 must be able to see the bound it
  exceeded without the harness's source.
- **`DriverDown` / `DriverUp`** are the health transitions. `cause` is a
  closed tag, never text: a panic message or a retirement reason is the
  harness's log line, not the kernel's, because the kernel's log cannot be
  shredded and a panic message can carry anything. A driver is up from
  `DriverRegistered`; `DriverUp` is only ever a return.
- **`Unanswered`** closes one correlation and carries the envelope the
  owner receives, exactly as `Replied` does, so the fold copies it into the
  mailbox and synthesizes nothing (the cancel notice's re-stamp problem,
  #109, does not recur). `msg.from` is **`Endpoint::Kernel`**, a new
  variant: the sender is neither the driver (it said nothing) nor the
  harness (on `crashed` and `overdue` it did nothing). `msg.kind` is
  `Reply` — it *is* the terminal answer to the request, so `recv(Corr(c))`
  matches it and nothing in userspace has to learn a new filter.
  `msg.payload` is `BlobRef::EMPTY`: the kernel authors no bytes, and
  `from` alone tells the reader what happened. `driver` and `cause` are the
  log's evidence, like `via` on `Sent`; userspace does not see them and
  §7 says why it does not need to.

### 3. What an unanswered request is billed

`consumed` on the `Unanswered` envelope is chosen by the kernel from one
fact it alone knows — whether the driver had **taken** the delivery from its
inbox — and the fold settles it with the same `settle()` a `Replied` uses:

| The request was | `consumed` | Effect through `settle()` | Why |
|---|---|---|---|
| taken by the driver (`next_delivery` had handed it over) | **the driver's ceiling**, exactly | spent += ceiling, refund 0, overdraft 0 | the driver may have done all of the work — a model call in flight completes provider-side — and nobody can say how much; unknown bills up (ADR-0009 §5, ADR-0013 §5) |
| still queued in the inbox | **`null`** | refund in full, spent 0 | nothing ran, nothing was seen |

`cause` and `consumed` are orthogonal: a `crashed` request is always taken;
an `overdue` one is taken or queued (a request that sat in the queue behind a
hung `handle` goes overdue too, since the bound counts from `Sent`); a
`retired` one is either.

Before and after, for an agent that sent one request to a driver with
ceiling `{tokens: 100}` and was granted `{tokens: 250}`:

| | `budget.tokens` | `reserved[corr]` | `spent.tokens` | `overdraft.tokens` | mailbox |
|---|---|---|---|---|---|
| after `Sent` | 150 | 100 | 0 | 0 | — |
| after `Unanswered`, taken | 150 | — | 100 | 0 | reply from kernel, corr 7 |
| after `Unanswered`, queued | 250 | — | 0 | 0 | reply from kernel, corr 7 |
| (for comparison) after `Replied {tokens: 30}` | 220 | — | 30 | 0 | reply from driver, corr 7 |

The sum-to-root invariant the reducer tests lean on holds through every
row: budget + reservations + spent = grant, and overdraft stays zero,
because the kernel bills exactly the reservation or nothing.

### 4. The fold confirms, settles, delivers — and ignores health

**What `State::check` verifies**, so a log that could not have happened is
refused at the entry:

| Entry | Refused unless | Refusal |
|---|---|---|
| `DriverDown`, `DriverUp` | `driver` is registered | `WrongSender(Driver { id })` |
| `Unanswered` | `driver` is registered; `msg.from == Kernel`; `msg.kind == Reply`; `msg.corr` is `Some` and open; `to` is its owner and live (cancelling included, as for `Replied`); `msg.payload == EMPTY`; `msg.consumed` is `None` **or equal to `driver`'s registered ceiling** | `WrongSender`, `WrongKind`, `UnknownCorr`, `WrongOwner`, `AgentExited`, and a new `Refusal::BadBill { corr }` for a payload or a bill the kernel would never write |

**What `State::apply` does:** `Unanswered` exactly as `Replied` — remove
the reservation, `settle()`, push `msg` to `to`'s mailbox, drop the corr.
`DriverDown` and `DriverUp`: **nothing**, after the check, the way
`Verdicts` applies as nothing (ADR-0008 §3).

**What replay and the fold ignore, and must keep ignoring:**

- `reply_within` on `DriverRegistered`. The fold does not store it and
  does not verify that an `overdue` `Unanswered` came after the bound. The
  moment is the kernel's verdict from its own clock cache, the way a
  `Cancelled` is the canceller's verdict: the fold confirms it is
  well-formed, not that it was timely.
- `cause` on `DriverDown` and `Unanswered`, and `driver` on `Unanswered`,
  beyond confirming the driver exists. The fold has never verified that a
  reply came from the driver the request was routed to; it does not start
  here.
- Health itself. `State` gains **no canonical field**: not a `down` set,
  not a per-corr driver, not a per-corr deadline. A `Sent` through a
  retired driver's capability is accepted by the fold, as a `Replied` from
  any registered driver is accepted today; the live kernel refuses it
  (§5), and a log containing one was written by a kernel that did not.

Why: the fold's job is to reproduce budgets, mailboxes and liveness from
the log, and every one of those is fully determined by the entries above.
Health and the bound are what the *kernel* needs to decide when to write
those entries; they are cache, like `in_flight` and the hook closures. The
consequence this buys is exact: **`FOLD` stays at 1, no fixture, corpus
sidecar or sim pin moves**, because no existing log contains the new kinds
and the canonical state is unchanged. A future ADR that wants the fold to
refuse a `Sent` to a down driver pays the re-pin then, with its reason.

### 5. The kernel: detection, the bound, the inbox, restart

```mermaid
sequenceDiagram
    participant A as agent 3
    participant K as kernel
    participant L as driver loop (model)
    participant S as supervisor (harness)
    A->>K: send(cap) → corr 7
    K->>L: Delivery{corr 7}   (taken)
    Note over L: handle() panics
    L-->>K: loop guard: unwound
    K->>K: DriverDown{model, crashed}
    K->>K: Unanswered{corr 7, to 3, consumed: ceiling, crashed}
    K-->>A: recv(Corr 7) → Reply from kernel
    K-->>S: supervise() → Crashed{model}
    S->>K: replace_driver(model, fresh instance)
    K->>K: DriverUp{model}
    K->>L: new loop; queued deliveries, if any, go to it
```

- **Detection is the loop's.** The loop observes `handle` unwinding — a
  `catch_unwind` around the poll, `AssertUnwindSafe`, std only; the lint
  denies raising a panic, not catching one — and commits `DriverDown
  { crashed }` followed by one `Unanswered { crashed }` for every corr in
  `in_flight` for that driver that had been taken, in corr order, under one
  lock. A `DriverDown` the log cannot take faults the kernel, as any
  unloggable entry does. The loop then ends; the inbox stays. Under a
  `panic = "abort"` profile there is nothing to catch and nothing to log,
  which is that profile's contract, not this design's gap.
- **The bound is enforced at `Kernel::tick`**, after the `Tick` commits and
  before the lock is released: every in-flight corr whose driver has a
  `reply_within` and whose `sent_at + reply_within <= now` gets an
  `Unanswered { overdue }`, `consumed` per §3. `sent_at` joins
  `in_flight` as cache (the `now` at `Sent`); the bound is read from the
  registration. The driver is **not** declared down by an overdue request:
  a slow driver and a dead one look the same from outside, and which it is
  is the supervisor's call (§6). Because the loop is sequential, a hung
  `handle` will make every request behind it overdue in turn; each fails
  explicitly at its own bound, which is the "never a hang" guarantee
  holding without a supervisor.
- **A late reply is dead letter.** A driver that eventually answers an
  overdue or crashed-and-replaced corr hits `UnknownCorr` in `reply`, as a
  reply to an exited owner does today. It costs the requester nothing
  beyond the ceiling already billed.
- **The inbox is the kernel's, not the driver's.** Deliveries still queued
  when a driver crashes stay queued; `send` keeps accepting up to
  `INBOX_CAPACITY` and the supervisor's replacement takes them. Only
  `retire_driver` empties the queue, with one `Unanswered { retired,
  consumed: null }` each, removes the inbox, and makes every later `send`
  through that capability `Refusal::Unroutable(cap)` — the refusal `send`
  already gives a capability with no inbox behind it. `describe` through a
  retired capability is `None`.
- **Restart is `Kernel::replace_driver(id, D)`**, allowed after boot,
  unlike `register_driver`: same `DriverId`, same `Capability` — the
  address every namespace holds must survive — and no new
  `DriverRegistered`. It aborts the old loop if one is still running (the
  loop's abort handle is now kept, which changes the "nothing cancels
  them" rule to "nothing but shutdown and replacement"), drops the old
  instance (an `AgentDriver`'s `Drop` climbs its ladder, ADR-0013 §5),
  commits `DriverDown { retired }` if the driver was up and one
  `Unanswered { retired, consumed: ceiling }` per taken corr, then
  `DriverUp`, installs the new instance and spawns a new loop over the
  same inbox. A replacement of a crashed driver commits only `DriverUp`:
  the `DriverDown` is already there.
- **Not health:** `shutdown` and a kernel fault end every loop through
  `next_delivery → None` and write nothing; the run is over. A `cancel`'s
  `abandon` reaches a driver that is up; a driver that is down has no open
  corrs to abandon, because they were unanswered when it went down.

The kernel surface the follow-on lands, in full:

```rust
impl Kernel {
    /// `register_driver` is this with `reply_within: None` (the `boot`/`boot_with` pattern).
    pub fn register_driver_with<D: Driver>(self: &Arc<Self>, id: DriverId, driver: D,
        ceiling: Budget, reply_within: Option<u64>) -> Result<Capability, KernelError>;
    /// Same id, same capability; `DriverUp` (after `DriverDown { retired }` if it was up).
    pub fn replace_driver<D: Driver>(self: &Arc<Self>, id: &DriverId, driver: D) -> Result<(), KernelError>;
    /// `DriverDown { retired }`, every open request unanswered, the capability unroutable.
    pub fn retire_driver(&self, id: &DriverId) -> Result<(), KernelError>;
    /// The next thing a supervisor would want to know. A future, like `drained`.
    pub fn supervise(self: &Arc<Self>) -> Supervise;
}
```

### 6. The supervisor: harness code, three events, one policy it owns

The supervisor is the harness (HANDOFF §4 item 3), holding the kernel
handle, and it is **optional**: §5 drains without it. What the kernel gives
it is one future and three events, none of which is a log entry:

```rust
#[non_exhaustive]
pub enum DriverEvent {
    /// The loop unwound. `DriverDown { crashed }` and its `Unanswered`s are already logged.
    Crashed { driver: DriverId },
    /// A request passed the bound. Its `Unanswered { overdue }` is already logged.
    Overdue { driver: DriverId, corr: Corr, agent: AgentId },
    /// A `Replied` settled above the ceiling. The overdraft is already on the agent.
    Overdrew { driver: DriverId, corr: Corr, agent: AgentId, excess: Consumption },
}
```

Every event is *after the fact*: the kernel has already done the one thing
that must not wait (close the request, record the transition, bill it), and
the supervisor decides only what happens to the driver next. Its verbs are
`replace_driver` and `retire_driver`, and doing nothing is a valid policy
for every event.

The reference policy, which the kernel's own tests exercise in place of a
harness, and which a real harness copies or replaces:

| Event | Reference policy | Why this default |
|---|---|---|
| `Crashed` | replace with a fresh instance, up to `k` times per driver (`k = 3`); retire on the `k+1`th | a crash is usually a bug in one request's handling; a crash loop is a bug in the driver |
| `Overdue` | first: nothing (the request is already unanswered); second in a row on the same driver: replace | one slow request is the world being slow; two is a hung loop |
| `Overdrew` | nothing; count; retire above a harness-set excess | a misreport is visible in the hash already (#18); what it should *cost* the driver is a harness's call, not the kernel's |

**What #18 becomes.** "Supervision policy for a driver that reports above
its ceiling" is the third row: a harness policy over the `Overdrew` event.
The kernel's part — make the misreport visible (M1b), then make it an
event (this ADR) — is done when the follow-on lands; #18 is re-gated on
that lane and then names a harness policy, not a kernel change. The
alternative in #18's body, a hook verdict, is rejected: hooks judge
*agents'* acts at pinned points (ADR-0008); a driver's misreport is a
breach of the contract the *harness* registered it under.

**What ADR-0013 §2 meant.** "`unavailable` becomes a health entry" cannot
happen inside the kernel — that would be the kernel reading a payload. It
resolves as: `error.unavailable` stays a reply kind; a harness that wants a
logged-out CLI to be *down* watches for it by its own means (the agent
driver's probe verdict is the harness's to read) and calls `retire_driver`
or `replace_driver`, which is what writes the health entry. Nothing in
#131, #127 or #128 changes.

### 7. Userspace: a reply from the kernel

`libtau::infer` waits with `recv(Corr(c) | Kind(Notice))` and returns the
`Reply`. It gains one arm before decoding: a `Reply` whose `from` is
`Endpoint::Kernel` is `InferError::Unanswered { corr }` — not a
`ModelReply`, not a `MissingPayload` (the payload is *empty*, not
*missing*; a shredded reply and an unanswered one are different facts and
stay different errors). The tool loop maps it to a tool result the way it
maps a driver's `error.transport`: `is_error`, `error_kind: failed`, "the
driver did not answer". `should_retry` treats it as `transport`: no answer
arrived and nothing the caller did caused it. If the driver was retired,
the retry's `send` is `Unroutable` and ends the loop the way any refused
`send` does. This is why the envelope carries no cause: the one decision
userspace makes — try again or not — is answered by the next `send`, and
answered correctly.

### 8. ABI impact: `ABI` 2 → 3

| Surface | Before | After | Governed by |
|---|---|---|---|
| `Endpoint` | `Agent`, `Driver`, `Harness`, `Hook` | plus `Kernel` | ADR-0004, additive variant on a `#[non_exhaustive]` enum |
| `Entry` | twelve kinds | plus `DriverDown`, `DriverUp`, `Unanswered` | ADR-0010, a new kind is an ABI event |
| `Entry::DriverRegistered` | `seq, driver, cap, ceiling` | plus `reply_within: Option<u64>`, `#[serde(default)]` | ADR-0010 Consequences, "allowed, defaulted, and bumped" |
| `Entry::seq()` | `msg.seq` for `Sent`, `Replied`, `Emitted` | and for `Unanswered` | ADR-0010 §3, "position lives in two places on purpose" |
| `ABI` | `2` | `3` | ADR-0004 |
| `Refusal` | | plus `BadBill { corr }` | Rust API, `#[non_exhaustive]` |
| `KernelError` | | unchanged | |
| `State`, `FOLD` | | **unchanged** | ADR-0011 §2: no canonical field moves |

**Why `Endpoint::Kernel` and not `Harness`.** ADR-0008 kept `Hook` out of
`Harness` so that two things that are not the same do not share a name on
the envelope. The harness is "the pre-agent code holding the kernel
handle" (HANDOFF §4 item 3) and the kernel is not it; two of the three
causes involve no harness act at all. A reader that sees `from: harness`
on a reply would look for the harness code that wrote it and find none.

**Why bump.** New tag values in frozen enums, a new field on a frozen kind:
the serialized surface grows, and a reader at 2 must refuse a log at 3
rather than hit `MalformedEntry` on line 40. The pull request that lands
§2 carries `abi-change` with this ADR linked, bumps the constant, adds the
three rows and the field to ADR-0010 §3 by amendment, and the row to the
table on `ABI` in `abi/mod.rs`. Per ADR-0010 §6, it re-records nothing and
adds one log recorded at `abi: 3` to the corpus — a sim run whose generator
draws the three kinds — so the nightly sentinel refolds a post-bump log
from the first night.

### 9. Tests the kernel lane must add

Named so they can be checked off; the files are the ones that hold their
neighbours today.

`kernel/tests/abi_snapshot.rs`:

- `entry_driver_registered` regains its pin with `"reply_within":null`, and
  `entry_driver_registered_bounded` pins a bound;
- `entry_driver_down_crashed`, `entry_driver_down_retired`, `entry_driver_up`;
- `entry_unanswered_taken` (`consumed` = a ceiling) and
  `entry_unanswered_queued` (`consumed` = `null`), one of each `cause`
  across the pair;
- `msg_reply_from_kernel_wire_format`;
- `log_file_wire_format` gains one entry of each new kind;
  `every_frozen_type_round_trips` covers them; `abi_version_is_pinned`
  moves to 3 and cites this ADR.

`kernel/tests/abi_invariants.rs`:

- `Entry::seq()` is `msg.seq` for `Unanswered`;
- a `DriverRegistered` line without `reply_within` deserializes to `None`
  (every existing corpus and fixture log is such a line);
- the `corr` an `Unanswered` closes is on the wire, inside its envelope.

`kernel/tests/reducer.rs`:

- `an_unanswered_taken_request_settles_at_the_ceiling`,
  `an_unanswered_queued_request_refunds_its_reservation`: the §3 table,
  including sum-to-root after each;
- `an_unanswered_entry_for_a_closed_corr_is_refused`,
  `an_unanswered_entry_addressed_to_the_wrong_owner_is_refused`,
  `an_unanswered_entry_from_anyone_but_the_kernel_is_refused`,
  `an_unanswered_entry_billing_other_than_none_or_the_ceiling_is_refused`,
  `an_unanswered_entry_with_a_payload_is_refused`;
- `driver_down_and_up_for_an_unregistered_driver_are_refused`;
- `driver_down_and_up_change_nothing_but_the_position`: the state after
  them equals the state before with `next_seq` advanced;
- `a_cancelling_agent_receives_an_unanswered_reply`.

`kernel/tests/m3c_supervision.rs`, with the fixture
`kernel/tests/fixtures/m3c-supervision.log` and its hash snapshot, on the
milestone pattern (`m0`, `m1a`, `m1b`, `m2a`):

- `a_crashed_driver_fails_its_taken_request_and_the_requester_reads_a_reply_from_the_kernel`;
- `a_request_still_queued_at_the_crash_is_delivered_to_the_replacement`;
- `an_overdue_request_fails_at_the_tick_that_passes_its_bound`, and
  `a_request_queued_behind_a_hung_handle_goes_overdue_too`;
- `a_late_reply_to_an_unanswered_request_is_dead_letter` (no `Replied`,
  no second bill);
- `a_retired_driver_fails_its_queue_and_its_capability_is_unroutable`;
- `a_replaced_driver_answers_under_the_same_capability` (the agent's
  namespace is untouched);
- `replacing_a_hung_driver_aborts_its_loop_and_fails_the_taken_request`;
- `shutdown_writes_no_health_entry`;
- `the_supervisor_sees_crashed_overdue_and_overdrew_in_order`;
- `the_reference_policy_retires_after_k_crashes`;
- `the_fixture_refolds_to_the_pinned_state_hash`.

`libtau/tests/infer.rs` and the tool-loop tests:

- `infer_reports_a_reply_from_the_kernel_as_unanswered`;
- `the_tool_loop_renders_an_unanswered_call_as_a_failed_tool_result`;
- `infer_retries_an_unanswered_request_like_a_transport_error`, and the
  retry ends on `Unroutable` when the driver was retired.

`sim`:

- the generator draws `DriverDown`, `DriverUp` and `Unanswered` proposals
  (a taken one bills the ceiling, a queued one bills nothing; a
  malformed one is a refusal the properties test counts), and `PINNED`
  in `sim/tests/determinism.rs` is re-computed on top of `origin/main`
  with this ADR as the reason. The corpus sidecars and the fixture
  snapshots do not move (§4): a sidecar that moves is drift, not this
  lane.

## Consequences

- **One kernel lane, filed:** [#148](https://github.com/tau-rs/tau/issues/148),
  blocked by this ADR's issue (#122) as a native dependency. It lands §2
  under `abi-change` with this ADR linked, §3–§5 in the reducer and the
  kernel, §6's future and events, §7 in `libtau`, the tests of §9, the
  ADR-0010 §3 rows and the `ABI` table row by amendment, and the corpus
  log at `abi: 3`. It re-pins `PINNED` only.
- **The "never a hang" guarantee is the kernel's, given a clock and a
  bound.** A harness that registers every driver with a `reply_within` and
  runs a clock drains every run, supervisor or not. A harness that leaves
  a driver unbounded has chosen today's behaviour for that driver, and the
  log says so on the `DriverRegistered` line.
- **Every unanswered request costs its ceiling.** That is the price of not
  knowing, and it is charged to the requester, whose grant the reservation
  came from. A harness that finds the bill high sets a tighter ceiling or
  a longer bound; the kernel does not guess.
- **The loop's abort handle is kept.** "Nothing cancels a driver loop but
  shutdown" becomes "nothing but shutdown and replacement".
- **`register_driver`'s loop comment comes true and is replaced.** "Driver
  supervision (M3) will surface the error envelope; for now it drops on
  the floor" — a reply to a finished owner still drops on the floor, by
  design; a request whose driver drops on the floor is now `Unanswered`.
- **#18 is re-gated on #148 and re-scoped** to a harness policy over
  `Overdrew`. **#131**'s subprocess ladder is untouched: it is how an
  agent driver *reports* `lost`; this ADR is what happens when it cannot
  report at all. **#144** (store faults) stays open on its own: a store
  fault is the kernel's truth being incomplete, and its shape is
  `KernelError::Faulted`, not a driver's.
- **M4's resume has a name for the requests it cannot resume.** A corr open
  in a restored snapshot has no driver behind it; M4 may close each with an
  `Unanswered` whose cause is a fourth tag, which is why the tag enums are
  `#[non_exhaustive]`. Not decided here.
- **The obligations this creates:**
  - a driver's own bounds — the model driver's HTTP timeout, the sandbox's
    and the agent driver's `wall` plus their ladders — must be shorter than
    its `reply_within`, so a driver that *can* report `lost` does so before
    the kernel stops waiting; the harness sets both and owns the gap;
  - a new `cause` tag, a new `DriverEvent`, is additive and documented here
    by amendment before it ships;
  - a harness that replaces a driver supplies a *fresh* instance; the
    kernel never re-uses one that raised.

## Alternatives considered

**The fold synthesizes the failures, as it synthesizes cancel notices.**
A `DriverDown` whose apply closes every open corr routed to the driver, and
a `Tick` whose apply closes every overdue one. Rejected on two grounds. One
entry closing several corrs gives several mailbox messages one `seq`, which
is the `Resolved` ambiguity ADR-0010 §3 names as the reason `Emitted` is
its own entry — and one agent with two open requests on one driver is
ordinary. And both need `corr → driver` and `corr → deadline` in canonical
state, which moves every pinned hash and bumps `FOLD` to buy a refusal the
fold cannot fully make anyway (it cannot refuse the *absence* of a timeout).
One explicit entry per closed request keeps positions honest and the state
untouched.

**Put the bound in the ceiling, as `wall_ms`.** Tempting: the reducer
already has the ceiling. Rejected: `send` reserves the whole ceiling from
the requester, so a `wall_ms` ceiling makes a timed agent hold that much
wall out of `budget` while the clock keeps charging `budget` on every tick
— the agent is aborted after `grant − bound` instead of `grant`. A bound is
not a cost; it is a promise about when the kernel stops waiting, and it
gets its own field.

**A kernel-authored error payload** — the kernel writes
`{"error":{"kind":"unanswered"}}` as the reply's bytes, so userspace reads
one shape for every failure. Rejected: it makes the kernel the author of a
wire format that has to be frozen, parsed by every requester type (model,
sandbox, agent) whose replies have their own shapes, and sealed under the
requester's key for no content. `from: kernel` on the envelope carries the
same bit with no bytes, and §7 shows userspace needs nothing more.

**`from: Harness` on the unanswered reply**, no new `Endpoint` variant.
Rejected in §8: on `crashed` and `overdue` the harness did nothing, and
HANDOFF §4 item 3 wrote the harness/kernel distinction down precisely so a
later convenience would not erase it.

**Health in canonical state**, so the fold refuses a `Sent` to a down
driver and a double `DriverUp`. Rejected for now: it re-pins everything to
refuse entries a correct kernel never writes, and the fold already accepts a
`Replied` from any registered driver without checking routing. The door is
open — a later ADR pays the re-pin with a reason — and this one keeps
`FOLD` at 1.

**Declare the driver down on the first overdue request.** Rejected: the
kernel cannot tell slow from dead, and a `DriverDown` it writes on a guess
is a health record that lies. The request fails either way; whether the
driver is replaced is the supervisor's, with the reference policy's "second
in a row" as the default.

**Fail the queued requests on a crash too.** Simpler to state, rejected:
nothing saw them, a replacement can take them, and failing them charges the
requester a round trip for a crash it had no part in. Retirement fails them
because nothing will ever take them.

**A `Driver::health()` method the kernel polls.** Rejected: it is the kernel
asking the driver to say something about itself outside the reply, which is
ADR-0006's exception with a new name, and a wedged driver does not answer a
health call either. The kernel classifies by what came back through
`handle`; the harness may probe its drivers however it likes and act
through the two verbs.
