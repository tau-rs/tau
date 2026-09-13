# ADR-0005: Kernel-allocated ids are reducer counters, confirmed on replay

**Status:** Accepted
**Date:** 2026-09-13
**Deciders:** tau core

## Context

Four id kinds are allocated by the kernel: `AgentId` at `spawn`, `Corr` at
`send`, `Capability` at driver registration, and `Seq` at every append. The
ABI says only that they are never reused within a run
([ADR-0004](0004-abi-freeze.md), `kernel/src/abi/ids.rs`). It does not say
where the values come from, and M0 — the first kernel logic — has to decide.

Two constraints shape the answer:

- **Replay must reproduce every id exactly.** A log that says `agent:7` sent
  `corr:12` is only replayable if the refold assigns the same numbers; a
  refold that re-derives ids from its own counters and *hopes* they match is a
  divergence waiting for the first bug.
- **`Capability` has no public constructor**, by design: unforgeability in
  ordinary code. The kernel is not ordinary code, but the type lives in the
  frozen directory, and the freeze applies to the *wire format* — not to which
  module inside the crate is allowed to mint a value.

## Decision

1. **Every kernel-allocated id is a monotonic counter in reducer state**
   (`next_seq`, `next_agent`, `next_corr`, `next_cap`). Values start at zero
   and only ever increase. Because the counters are state, they are a pure
   function of the log, like everything else in the reducer.

2. **A log entry carries the id it was allocated**, and the reducer's check
   *confirms* it equals the counter rather than re-deriving it. A mismatch is
   a `Refusal::BadAllocation`: from a live kernel it means the kernel is
   inconsistent with its own state (a fault); from a fold it means the log
   was not produced by this reducer. Either way it is loud, at the exact
   entry, instead of a silent renumbering.

3. **`Capability` gains a crate-private constructor**, `Capability::alloc`,
   which only the kernel's allocator calls. The `testing`-gated `mint` stays
   for test targets. Neither changes the serialized form: the wire snapshots
   in `kernel/tests/snapshots/` are byte-identical before and after this
   decision, and `ABI` stays at `0`.

## Consequences

- **This ADR is the paper trail for a diff under `kernel/src/abi/`.** The PR
  that lands M0 carries the `abi-change` label because the gate is mechanical
  and cannot tell a constructor from a field rename. That is the gate working
  as designed: the change is loud, and this is the answer to "why".
- **Replay confirms, never re-derives.** Every future allocator follows the
  same shape: counter in state, value in the entry, equality in the check.
  A future `HookId` at `attach` (M2) is the next instance.
- **Ids are dense and predictable.** That is a feature for logs a human reads
  and a non-feature for security, which is fine: authority never lived in the
  value ([ADR-0004](0004-abi-freeze.md), "Unforgeability lives in the kernel,
  not in the type"). Guessing `cap:3` grants nothing; holding it in a
  namespace the kernel recorded does.
- **Drivers are named by the harness at registration**, not by themselves —
  the way a filesystem is named by its mount point. `DriverId` is the one
  kernel-side identifier that is a name rather than a counter, because it must
  resolve the same endpoint across restarts with a different driver set, and
  that resolution belongs to the operator.

## Alternatives considered

**Re-derive ids on replay and assert at the end.** Rejected: a divergence
surfaces as "state hash mismatch" with no pointer to the entry that caused it.
Confirming per entry costs one integer comparison and names the culprit.

**Random or hashed ids.** Rejected: the reducer draws no randomness
([ADR-0003](0003-substrate.md)), and a content-derived id would make the
allocation depend on payload bytes, which the kernel does not read.

**Enable the `testing` feature for the kernel's own build and use `mint`.**
Rejected: a feature that is always on is not a feature, and it would hand
every downstream crate a public constructor. A `pub(crate)` function is the
precise scope.

**Bump `ABI` to 1 for the constructor.** Rejected: the wire format did not
change, and the number must mean exactly that. A bump that changes no bytes
teaches readers to ignore bumps.
