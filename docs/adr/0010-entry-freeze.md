# ADR-0010: The `Entry` freeze — twelve kinds join the constitution, `ABI` 1 → 2

**Status:** Accepted
**Date:** 2026-09-15
**Deciders:** tau core
**Amends:** [ADR-0004](0004-abi-freeze.md) (`Entry` and the hook wire types
join `kernel/src/abi/`; `ABI` 1 → 2)

## Context

The log is the kernel and everything else is cache (ADR-0003). A log is a
[`LogHeader`] line followed by one [`Entry`] per line, and the header and the
[`Msg`] envelopes *inside* entries have been frozen since ADR-0004. The
`Entry` enum around them has not: `log.rs` said it "may churn until the
replay CLI lands (M2), at which point it joins the frozen surface". This is
that point. M2 is "hooks, sandbox driver v0, `tau replay <log>`"
(HANDOFF §6), and a replay CLI that reads logs written by other builds
needs the line format it reads to be a promise, not a snapshot of whatever
the reducer happened to emit.

ADR-0008 §6 sequenced this: the three hook entry kinds — `Attached`,
`Verdicts`, `Emitted` — had to exist before the freeze, or the replay lane
would absorb them as its first ABI event. They landed in #84, `Entry` has
twelve kinds, and `corpus/m2a-hooks.log` carries all three.

Four facts shape the decision:

- **The header number has not identified the entry format.** `Claimed`
  gained `by` in M1a (#14) and `DriverRegistered` gained `ceiling` in M1b
  (#19), both as required fields, while every log still said `abi: 0`. A
  log written by the M0 build and a log written by the M1b build carry the
  same header and cannot be read by the same reader. The milestone fixture
  was silently re-recorded twice. Nothing in the wild was hurt, because the
  only reader of any log was the build that wrote it; that stops being true
  at the first `tau replay`.
- **`Entry` does not close over itself.** Three of its kinds carry
  `HookPoint`, `FailureMode`, `HookSource` and `Roll` (a `Vec<(HookId,
  Ruling)>`) from `crate::hook`. Freezing `Entry` without them freezes a
  wrapper around a churnable core: a `rename_all` on `Ruling` would change
  every `Verdicts` line without touching a frozen file.
- **The gates are per directory.** ADR-0004's gate 1 (`CODEOWNERS`) and
  gate 2 (`scripts/abi-guard.sh`) bind on `kernel/src/abi/` and nowhere
  else. A type frozen "in place" in `log.rs` is held by one gate of three.
  ADR-0004 rejected exactly that arrangement — a stability surface described
  in prose rather than enumerated by a directory — as the v1 failure.
- **The Rule grammar is a string.** `HookSource::Rule(String)` records a
  rule by its source text (#85). The *field* is on the wire; the *grammar*
  is ADR-0008 §5's, amended there. This freeze covers the former only.

## Decision

### 1. `Entry` moves into `kernel/src/abi/`, and the hook wire types move with it

`Entry` moves to `kernel/src/abi/entry.rs`. `HookPoint`, `FailureMode`,
`HookSource`, `Ruling` and the `Roll` alias move to `kernel/src/abi/hook.rs`,
because they are the wire form of three entry kinds and a frozen type must
be frozen through everything it serializes. `abi/mod.rs` re-exports all of
them; `log` and `hook` keep `pub use` re-exports so `tau_kernel::log::Entry`
and `tau_kernel::hook::HookPoint` still resolve.

What stays where it is, and why:

| Type | Stays in | Why it is not ABI |
|---|---|---|
| `Log`, `LogError`, the reader and writer | `log.rs` | Behaviour, not shape. The *framing* is frozen below; the struct that implements it may change. |
| `HookEvent`, `Verdict`, `HookFailure`, `HookProgram` | `hook.rs` | What a hook *sees* and *answers* at runtime. Never written to a log; `Ruling` is the recorded form of `Verdict`. |
| `HookRecord`, `State` | `hook.rs`, `reducer.rs` | Canonical state. Its serialized form feeds the state hash the corpus pins, but state is cache (ADR-0003); its persistence is the M3 snapshot ADR's question. |
| `Refusal`, `KernelError` | `reducer.rs`, `kernel.rs` | Rust API, `#[non_exhaustive]`, covered by `cargo-semver-checks`. |
| The Rule grammar | ADR-0008 §5 | A string on the wire. Grammar changes are ADR-0008 amendments with fuzz seeds, and change no bytes here. |

`Entry` becomes `#[non_exhaustive]`, like `Endpoint` and `MsgKind`: a
thirteenth kind must not be a source break for a reader outside the crate.
The reducer's `apply` stays exhaustive inside the crate, which is the
property that matters — a kind with no `apply` arm is a compile error, not
a silent no-op. Variants are constructible from outside (the `sim` crate
builds `Spawned` and `Sent` by hand), so a new *field* on an existing kind
is wire-additive but costs each such constructor a line. A new kind is the
preferred evolution; a new field is allowed and defaulted.

### 2. `ABI` 1 → 2, and what the number means from here

`ABI` bumps to 2 in the pull request that lands §1. Not because bytes
change — no line any current build writes changes by one byte — but
because **2 is the first number that identifies the entry format.** Up to
1, the header said which `Msg` envelope was inside; from 2, it says what
every line is. A reader at 2 accepts `abi ≤ 2`, as ADR-0004 prescribes, and
makes two different promises about what it accepted:

| Header says | What a reader at 2 may assume | Why |
|---|---|---|
| `abi: 2` or later-and-accepted | every line is one of the kinds in §3, in the shape pinned by snapshot | the freeze was in force when it was written |
| `abi: 0` or `abi: 1` | the header and envelopes are frozen; the entry lines are *probably* today's shape and a parse failure is possible and honest | `Entry` churned under 0 and was unfrozen under 1 |

ADR-0005 declined a bump "that changes no bytes" because it "teaches readers
to ignore bumps". The number's job is to let a reader know, from the header
alone, whether it can trust what follows. This bump changes what the number
lets a reader assume; that is the number doing its job, not ceremony. The
cost is one false refusal: a reader at 1 — today's build — refuses a log at
2 it could have read. There is no such reader in the wild, and the
alternative (landing at 1 with the label, §Alternatives) leaves *every*
future reader unable to tell a pre-freeze `abi: 1` log from a post-freeze
one without a commit hash.

The reader rule is unchanged: `header.abi <= ABI`, refuse otherwise. A log
whose header is accepted but whose entry line does not parse is
`LogError::MalformedEntry` with the line number, as today. A kind this build
does not know is a malformed entry, not a skipped one: the header promised
the reader could understand every line, and it cannot.

### 3. The twelve kinds, frozen as they are

Every kind, its position, and its wire form. The wire form is unchanged by
this ADR; what changes is where the type lives and what pins it. Payload
hashes are shortened to `…` here and are 64 hex characters on the wire
(`BlobRef`, ADR-0004).

| Kind | Position | Wire form | Before | After |
|---|---|---|---|---|
| `DriverRegistered` | `seq` | `{"entry":"driver_registered","seq":0,"driver":"tool","cap":0,"ceiling":{"tokens":30}}` | `log.rs`, hash-pinned | `abi/entry.rs`, `entry_driver_registered` snapshot |
| `Spawned` | `seq` | `{"entry":"spawned","seq":1,"parent":null,"agent":0,"ns":{"caps":[0]},"budget":{"tokens":70,"calls":10,"depth":2}}` | same | `entry_spawned_root`, `entry_spawned_child` |
| `Sent` | `msg.seq` | `{"entry":"sent","msg":{"abi":2,"seq":12,"from":{"kind":"agent","id":0},"corr":0,"kind":"request","consumed":null,"payload":"…"},"via":0}` | same | `entry_sent` |
| `Replied` | `msg.seq` | `{"entry":"replied","msg":{"abi":2,"seq":17,"from":{"kind":"driver","id":"tool"},"corr":0,"kind":"reply","consumed":{"tokens":12},"payload":"…"},"to":0}` | same | `entry_replied` |
| `Resolved` | `seq` | `{"entry":"resolved","seq":5,"agent":1,"matched":4}` | same | `entry_resolved` |
| `Exited` | `seq` | `{"entry":"exited","seq":7,"agent":0,"result":"…"}` | same | `entry_exited` |
| `Claimed` | `seq` | `{"entry":"claimed","seq":47,"agent":1,"by":0}` | same | `entry_claimed_by_parent`, `entry_claimed_by_harness` (`"by":null`) |
| `Cancelled` | `seq` | `{"entry":"cancelled","seq":4,"by":0,"agent":1,"grace":10,"reason":"…"}` | same | `entry_cancelled_by_agent`, `entry_cancelled_by_harness` |
| `Tick` | `seq` | `{"entry":"tick","seq":4,"now":50}` | same | `entry_tick` |
| `Attached` | `seq` | `{"entry":"attached","seq":1,"hook":0,"point":"pre_send","failure":"closed","program":{"native":"tattle-shell"}}` | same | `entry_attached_native`, `entry_attached_rule` (`"program":{"rule":"when pre_send then allow"}`), `entry_attached_on_budget` (`"point":{"on_budget":{"dim":"tokens","below":50}}`) |
| `Verdicts` | `seq` | `{"entry":"verdicts","seq":32,"point":"pre_send","subject":1,"roll":[[0,{"emit":{"to":0,"payload":"…"}}],[1,{"deny":"…"}]]}` | same | `entry_verdicts` with all four rulings: `"allow"`, `{"deny":"…"}`, `{"emit":{…}}`, `{"failed":{"mode":"closed","error":"…"}}` |
| `Emitted` | `msg.seq` | `{"entry":"emitted","hook":5,"to":0,"msg":{"abi":2,"seq":14,"from":{"kind":"hook","id":5},"corr":null,"kind":"notice","consumed":null,"payload":"…"}}` | same | `entry_emitted` |

Three shapes deserve a sentence each:

- **Position lives in two places on purpose.** Nine kinds carry a top-level
  `seq`; `Sent`, `Replied` and `Emitted` carry a `Msg`, and a message's
  position *is* its envelope's `seq` — duplicating it would be a second
  source of truth for `Refusal::OutOfOrder` to disagree with. `Entry::seq()`
  is the accessor and its rule is frozen with the shape.
- **`Roll` is an array of pairs**, `[[hook, ruling], …]`, in `HookId`
  order, stopping at the first ruling that stops it (ADR-0008 §3). A map
  keyed by hook would lose the stop and re-sort the roll on some future
  reader; the pair list is the record of what happened in the order it
  happened.
- **`HookSource::Rule` has never been on the wire.** No corpus log carries
  it: `sim` attaches natives only (#86 changes that). Its snapshot is the
  first pin of the shape, which is exactly why the snapshot exists.

The `entry` tag strings — `driver_registered`, `spawned`, `sent`, `replied`,
`resolved`, `exited`, `claimed`, `cancelled`, `tick`, `attached`,
`verdicts`, `emitted` — are part of the freeze. So are the field names
above, the `snake_case` casing of `HookPoint`, `FailureMode`, `HookSource`
and `Ruling`, and the externally-tagged enum form serde gives them
(`"pre_send"`, `{"on_budget":{…}}`).

### 4. The file framing is frozen with the lines

A log is: line 1, a `LogHeader` as JSON; every following line, one `Entry`
as JSON; lines separated by `\n`; a line that is empty or whitespace is
skipped; nothing else. No trailer, no length prefix, no comment syntax, no
multi-line entry. A truncated file reads up to the truncation and the
reducer refuses at the first missing position. This is what `Log::write_to`
writes and `Log::read_from` reads today; it is stated here so that a reader
in another language can be written from this document and the snapshots.

### 5. What the snapshot suite must gain

ADR-0004's obligation is that a frozen type arrives with its snapshot. The
pull request that lands §1 adds to `kernel/tests/abi_snapshot.rs`:

- **One snapshot per kind**, named as in the §3 table, with every optional
  or variant-bearing field exercised across the set: `parent` and `by` as
  `null` and as an id; all five `HookPoint`s (`on_budget` is the only
  struct-shaped one); both `FailureMode`s; both `HookSource`s; all four
  `Ruling`s in one `Verdicts` roll.
- **`log_file_wire_format`**: one string snapshot of a whole log as
  `Log::write_to` writes it — header, then one entry of every kind — so the
  framing of §4 is pinned as bytes, not as prose.
- **`every_frozen_type_round_trips`** extends to every kind: write, read,
  equal. This is the property the replay CLI depends on.
- **`abi_version_is_pinned`** moves to 2 and cites this ADR.

And to `kernel/tests/abi_invariants.rs`, the shapes the fold's refusals key
on:

| Invariant | Refusal it protects |
|---|---|
| `Entry::seq()` is the top-level `seq` for nine kinds and `msg.seq` for `Sent`, `Replied`, `Emitted` | `OutOfOrder` |
| the envelope inside `Sent`/`Replied`/`Emitted` carries `abi`, and a reader at 2 accepts an envelope at 1 inside a log at 1 | `Envelope` |
| the ids a kind introduces are on the wire: `agent` in `Spawned`, `cap` in `DriverRegistered`, `corr` in `Sent`'s envelope, `hook` in `Attached` | `BadAllocation` (ADR-0005: confirmed, never re-derived) |
| a `Roll` deserializes in written order and is not re-sorted | `BadRoll`, `UnknownHook` |
| an unknown `entry` tag is `LogError::MalformedEntry` naming its line, never a skipped line or a panic | the header's promise in §2 |

Every one of these is a test that fails today only by someone changing the
shape; that is the point. A frozen type with no pinned wire form is frozen
in name only.

### 6. Fixtures and the corpus after the freeze

- **`TAU_UPDATE_FIXTURES=1` stays, and narrows to one cause.** Before this
  ADR it was the mechanism for absorbing an `Entry` churn. After it, an
  `Entry` shape change is impossible without a bump and an ADR, so
  re-recording a fixture has exactly one legitimate cause: a reducer
  *semantic* change — the fold of the same log produces a different state —
  which needs its own ADR, per `corpus/README.md`. A bump is not a cause. The
  four `kernel/tests/fixtures/*.log` are frozen files whose header says
  `abi: 1`; a reader at 2 accepts them, and their pinned hashes do not move.
  The pull request that bumps to 2 re-records nothing.
- **The corpus does not move, and grows by one.** `corpus/*.log` are frozen
  files; their entries are already in the §3 shape and their envelopes say
  `abi: 0` or `abi: 1`, which a reader at 2 accepts. `State` gains no
  canonical field here. If a sidecar hash moves in the bump pull request,
  that is drift the Tier 3 sentinel exists to catch, not a consequence of
  this ADR. The bump pull request adds one log recorded at `abi: 2`, so the
  sentinel refolds a post-freeze log nightly from the first night.
- **The corpus is the freeze's regression suite.** Every log in it is a log
  "written by another build". `tau replay` folding all of them to their
  sidecars is the acceptance test for the CLI.

### 7. What the replay CLI may assume

The reader this freeze exists for — `tau replay <log>` — is specified by
[#91](https://github.com/tau-rs/tau/issues/91). What this ADR guarantees it:

- A header it accepts is followed by lines it can parse, or a
  `MalformedEntry` with a line number. Nothing in between.
- Every line is one of the kinds in §3, with `Entry::seq()` dense from 0.
- Folding the lines through the reducer yields the same state hash on every
  build at `ABI ≥ 2`, or a `Refusal` naming the entry. It never needs a hook
  program, a driver, or a clock (ADR-0008: the fold confirms verdicts and
  never runs them).
- A log at `abi: 0` or `1` is best effort: the corpus's seven read today,
  and the CLI should say which promise applies when it prints the header.

## Consequences

- **Two implementation lanes, sequenced.** M2c-kernel
  ([#90](https://github.com/tau-rs/tau/issues/90)) lands §1, §2, §5 and §6 in
  one pull request carrying `abi-change` with this ADR linked; the snapshot
  diff shows the new cases and the `ABI` line. M2c-cli
  ([#91](https://github.com/tau-rs/tau/issues/91)), gated on it, ships
  `tau replay <log>` against §7.
- **`kernel/src/abi/` grows by two files and the gates cover them from the
  first commit.** `CODEOWNERS` already names the directory; the diff gate
  already diffs it; the snapshot job already runs `abi_snapshot`. No gate
  changes.
- **A thirteenth kind is an ABI event.** It bumps the number, arrives with
  its snapshot and its refusal test (ADR-0008's standing obligation), and
  gets a row in §3 by amendment. A kind that exists in code and not in this
  table is the v1 pattern.
- **A new field on an existing kind is allowed, defaulted, and bumped.** It
  deserializes from a log that lacks it (`#[serde(default)]`), so the
  corpus keeps reading, and the bump records that logs from then on carry
  it. Removing, renaming, retyping or moving a field is not allowed, ever;
  the correction is a new kind.
- **The state hash is not frozen by this ADR.** `State`'s serialized form
  is what the corpus sidecars pin, and it moves when the reducer's canonical
  state changes — a reducer-behaviour change with its own ADR, as
  `corpus/README.md` says. The M3 snapshot ADR decides whether `State` joins
  the directory.
- **`log.rs` stops apologising.** The "may churn" paragraph is replaced by
  a pointer here in the pull request that lands this ADR, ahead of the move.

## Alternatives considered

**Freeze `Entry` in place, in `log.rs`, with snapshots only.** Rejected. One
gate of three would hold it: `CODEOWNERS` and the diff gate are per
directory, and widening them to a second path list is the "rulebook"
arrangement ADR-0004 was written to end. One directory is the design.

**Land the freeze at `ABI = 1` with the `abi-change` label.** The bytes do
not change, and ADR-0005 set a precedent for the label in that case.
Rejected because the number would then be ambiguous for good: `abi: 1` logs
exist from before the freeze (the m2a fixture at `ae8fcad`) and from after
it, and no header distinguishes them. ADR-0005's constructor changed neither
bytes nor what a reader may assume; this changes the latter, and the number
is the only channel a reader has for it.

**Per-version readers in the CLI** — `tau replay` carrying a decoder per
historical `Entry` shape. Rejected: it makes the replay CLI the place where
the entry format is *defined*, by code, per version, which is the surface-
in-prose failure with extra steps. The freeze makes one reader sufficient;
the corpus proves it stays sufficient.

**A separate `entry_v` field in the header, so the entry format and the
envelope format version independently.** Rejected. ADR-0004 chose one
number on purpose, and ADR-0006 already showed where a second version
belongs — on a payload the kernel carries but does not read. `Entry` is
the thing the kernel reads. Two numbers on one line that always bump
together is one number with a chance to disagree.

**Freeze the hook wire types by re-exporting them from `abi/` without moving
the source.** Rejected: the diff gate diffs paths, not symbols. A type is
frozen where its source lives.

**Freeze the Rule grammar with it.** Rejected, per the M2c brief: the
grammar is ADR-0008 §5's, amended there, with fuzz seeds that grow with it.
A log is self-describing because it carries the rule's text; whether a
future build can *evaluate* that text is a question for `attach`, not for
the reader, which never evaluates anything.


## Amendments

- **2026-09-16** — `Msg.abi` leaves the canonical state
  ([#109](https://github.com/tau-rs/tau/issues/109)). §7 promised the same
  hash on every build at `ABI ≥ 2`, but the notice a `Cancelled` entry makes
  the fold synthesize was stamped with `Msg::new`'s `ABI` — the build's — and
  `Agent.mailbox` was hashed with the stamp. A log ending with that notice
  undrained folded to one hash per build. The fold never decides on an
  envelope's `abi`, so the mailbox now serializes without it (ADR-0003's
  "everything else is cache", as #62 did for the indexes) and a restored
  state re-stamps with the restoring build's `ABI`. The hash's definition
  moved; no sidecar, fixture or `PINNED` value did, because every pinned log
  ends drained and a finished agent's mailbox is empty. A future log that
  ends mid-cancel pins to a hash that already holds on every build.
- **2026-09-16** — The question this ADR deferred is answered
  ([ADR-0011](0011-snapshots.md), [#110](https://github.com/tau-rs/tau/issues/110)):
  `State` stays out of `kernel/src/abi/`. Its canonical form is the reducer's
  to correct and is versioned by a fold number of its own, `FOLD`, which the
  re-pin rule in `corpus/README.md` bumps; a snapshot from another fold is
  refused, not reinterpreted. Only the snapshot's header joins the
  directory. The re-stamp the 2026-09-16 row above introduced is named as a
  non-promise there (§3): the `abi` on a delivered envelope is the handing-
  over build's, and replay does not preserve it.

[`LogHeader`]: ../../kernel/src/abi/msg.rs
[`Msg`]: ../../kernel/src/abi/msg.rs
[`Entry`]: ../../kernel/src/abi/entry.rs
