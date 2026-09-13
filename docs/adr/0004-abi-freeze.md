# ADR-0004: The ABI freeze — one directory, three gates, one number

**Status:** Accepted
**Date:** 2026-09-13
**Deciders:** tau core

## Context

"We do not break userspace" is a promise about a *surface*, and a promise about
an unbounded surface is not a promise. Linux can keep it because the surface is
small, enumerated, and separate from the internals that churn beneath it.

tau v1 had the opposite arrangement: its stability surface was described by
prose across three documents and 47 lettered guidelines, and no part of it was
mechanically enforced. The result was that nothing was really frozen and
everything was slightly constrained — the worst of both.

Two further facts force the shape of this decision:

- **A version field cannot be retrofitted.** A log written before the field
  existed has no way to say what it is. Whatever else is undecided on day one,
  the envelope carries its ABI version.
- **A frozen surface needs a mechanical gate.** Review alone does not hold a
  boundary for five years; the reviewer who cares is not always the reviewer who
  is available.

## Decision

**`kernel/src/abi/` is the constitution.** Everything in it is a "do not break
userspace" surface: the message envelope [`Msg`], the grant types
([`Capability`], [`Namespace`], [`Budget`], [`Consumption`]), the kernel-allocated
ids, and the log header. Everything else in this repository may churn freely.

The surface evolves **additively or not at all**. New fields arrive with a
defaulted deserialization; new variants arrive in `#[non_exhaustive]` enums.
Nothing is ever removed, renamed, reordered into a different wire position, or
given a new meaning. `ABI: u16` is bumped when the surface changes, and `1.0`
means that number is frozen for good.

Three gates, deliberately redundant, because each fails differently:

1. **`CODEOWNERS`** requires an explicit owner review for `kernel/src/abi/`.
   Catches a change nobody meant to make. Fails if the owner rubber-stamps.

   > **Status: not yet binding.** Branch protection is unavailable on a private
   > free-plan repository, and even once it is available GitHub does not let an
   > author approve their own pull request — so with one maintainer, requiring
   > code-owner review would block every ABI change behind an admin bypass and
   > train exactly the reflex this gate exists to prevent. Gates 2 and 3 bind
   > today; gate 1 activates when a second maintainer exists. Tracked in
   > [#3](https://github.com/tau-rs/tau/issues/3), because a gate that is
   > described but inert is the failure this project was rebooted to avoid.
2. **The CI diff gate** fails any pull request that touches `kernel/src/abi/`
   unless it also bumps `ABI` or carries the `abi-change` label with a linked
   ADR. Catches a deliberate change with no paper trail. Fails if someone adds
   the label reflexively.
3. **`insta` snapshots** pin the serialized form of every frozen type, so a wire
   change shows up as a diff a human has to accept on purpose. Catches a change
   whose *source* looks innocent — a field reorder, a `rename_all`, a newtype
   swap. Fails only if someone accepts snapshots without reading them.

Plus `cargo-semver-checks` on the `tau-kernel` crate for the Rust-API half of
the surface, which the wire snapshots do not cover.

Concrete commitments in the initial ABI, each of which is a decision rather than
a default:

- **`abi: u16` on `Msg` and on the log header**, from the first commit. The
  header lets a reader refuse a log it cannot understand without parsing a
  single entry; the comparison is `<=`, so a newer reader accepts an older log.
- **Payloads are not in the log.** The envelope carries a 32-byte content hash
  ([`BlobRef`]); bytes live in a content-addressed blob store. This buys dedupe
  for free and, decisively, makes erasure possible: encrypt payloads per
  subtree, drop the key, and the payloads are gone while the structural log —
  and therefore replay of everything except content — still works.
- **The ABI names no hash function.** `BlobRef` is 32 bytes and nothing more, so
  changing the digest algorithm is a blob-store concern rather than a wire break.
- **`Budget` is a map of dimensions, not a struct of fields**, with reserved keys
  `tokens`, `cost_microusd`, `wall_ms`, `calls`, `depth`, `compute_ms`. A fixed
  struct would make every new device kind an ABI break; an image model reports
  pixels and a classifier reports `compute_ms`. A reserved key always parses to
  its own variant, so a driver cannot shadow `tokens` with a custom dimension
  and detach its spending from the dimension the kernel enforces.
- **An absent budget dimension means no grant, not unlimited.** Unlimited is not
  expressible.
- **Names are validated once, at the ABI.** `Name` is `[a-z][a-z0-9_-]{0,62}` and
  cannot be deserialized into existence in an invalid state. In v1 the same
  logical id had a validated grammar in one crate and an unvalidated one in two
  others; the disagreement produced a build failure, a panic on user input, and
  a silent mis-attribution in which a rejected id collapsed into a phantom
  entity instead of failing.
- **Unforgeability lives in the kernel, not in the type.** `Capability` has no
  public constructor, which stops accidental forgery. The *enforcement* is that
  the kernel checks the sender's holder set on every `send`: authority is the
  birth namespace ∪ transferred capabilities, both kernel state derived from the
  log. A value conjured by deserialization buys nothing. This matters because
  replay requires capability values to round-trip, so they must be
  deserializable — security cannot depend on their not being.

## Consequences

- **Every change here is loud by construction.** The cheapest path for a
  contributor who did not mean to touch the ABI is to not touch it.
- **Snapshot churn is a feature.** A pull request that rewrites six `.snap`
  files is showing its author, its reviewer, and the CI gate the same thing at
  the same time.
- **Additive-only has a cost**: mistakes in the initial shape are permanent
  until the next ABI bump, and after 1.0 permanent outright. That is the correct
  trade — an interface that can be corrected is not frozen.
- **The `abi-change` label is a paper trail, not permission.** It requires a
  linked ADR, so "why did the envelope change in March" always has an answer.
- **Obligation**: every type added to `kernel/src/abi/` must arrive with an
  `insta` snapshot in the same pull request. A frozen type with no pinned wire
  form is frozen in name only.

## Alternatives considered

**Version the whole crate with semver and rely on `cargo-semver-checks` alone.**
Rejected: semver checks the Rust API, not the wire format. Renaming a serde
field is a silent, wire-breaking, semver-compatible change — precisely the class
of mistake that most needs catching.

**A hand-written schema (protobuf/flatbuffers) as the source of truth.**
Rejected for now, not on merit. It would give a language-neutral wire contract,
which matters the day a non-Rust harness exists. Today it adds a build step, a
generated-code review problem, and a second place for the types to live. The
snapshots pin the wire format either way, so the door stays open.

**Freeze later, once the shape is known.** Rejected: this is exactly what v1
did. A boundary that is frozen "once things settle" never freezes, because
something is always unsettled. Freezing at `ABI = 0` with an additive policy
means the cost of being early is a bump, not a break.

**Trust review, skip the CI gate.** Rejected: review catches the change someone
thought about. The gate catches the change nobody did.

[`Msg`]: ../../kernel/src/abi/msg.rs
[`Capability`]: ../../kernel/src/abi/cap.rs
[`Namespace`]: ../../kernel/src/abi/cap.rs
[`Budget`]: ../../kernel/src/abi/budget.rs
[`Consumption`]: ../../kernel/src/abi/budget.rs
[`BlobRef`]: ../../kernel/src/abi/msg.rs
