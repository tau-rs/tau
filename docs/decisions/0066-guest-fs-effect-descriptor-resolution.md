# ADR-0066: Guest fs-effect descriptor resolution — preopen plumbing, dual cfg, absence-denial

**Status:** Accepted
**Date:** 2026-08-21
**Deciders:** tau core

## Context

EPIC 3.6-b routes the wasm guest's `fs.read`/`fs.write` tool effects through
`wasi:filesystem`, mirroring 3.6's net mechanism. Unlike net (a single host
call), fs requires a descriptor ceremony — the guest holds no ambient root on
wasip2 and must resolve a requested path against a host-preopened directory
before opening anything. Three sub-decisions fall out of that ceremony.

## Decision

**D2 — Both fs cfgs gate on the single `wasi:filesystem` world import.**
`fs.read` and `fs.write` map to the same two interfaces (`wasi:filesystem/types`
+ `/preopens`); the world text cannot distinguish them. `build.rs` therefore
sets both `tau_cap_fs_read` and `tau_cap_fs_write` whenever `wasi:filesystem` is
present. Read-vs-write is enforced at runtime by the host preopen perms
(`DirPerms::READ` vs `DirPerms::all()` from `PreopenAccess`), not the cfg — a
write to a read-only preopen fails at the host `open-at`. The two-cfg naming is
kept for per-effect symmetry and future interface divergence.

**D3 — Preopen-relative resolution is descriptor plumbing, not a cap gate.**
The guest computes reachability solely from the host's `get-directories()` list.
A matching preopen → strip the guest-path prefix, pass the remainder straight to
the host `open-at` (the guest does NOT reject/normalize `..`; the host rejects
escapes at the descriptor boundary). No matching preopen → the guest holds no
descriptor and cannot fabricate one. This is WASI's capability-security model:
absence of a preopen is absence of capability. The enforcement point is the
host's preopen set (its `WasiCtx`), which the host populates only from granted
caps — preserving 3.4's "no in-guest cap gate" invariant.

*Amended (#604):* selection is the LONGEST-prefix, segment-aware match, not
first-match — with overlapping grants (`/data` RO + `/data/logs` RW) a write
under `/data/logs` binds the RW preopen instead of being host-denied through
the RO one — and a `/` (root) preopen (from a `/**` cap) serves every absolute
path. Selection stays purely lexical: a traversal like `/data/../etc` binds
`/data` with remainder `../etc`, which the host `open-at` rejects (fail-closed
even when a broader preopen also matched). The pure selector lives in
`tau-wasm-guest::preopen` and is table-tested natively.

**D5 — `Write` replaces the file (`CREATE | TRUNCATE`), added by #604.**
The `Write` tool contract is full-content replace: `open-at` passes
`open-flags.create | open-flags.truncate`, so overwriting a longer existing
file leaves no stale tail. Append semantics would be a distinct future tool,
not a flag on this one.

**D4 — Ungranted-path denial is guest-observed absence, not a host error-code.**
Net's denial is a host hook returning a `wasi:http` error-code
(`HttpRequestDenied`), asserted exactly. Fs's no-preopen denial produces no host
error-code by construction — the guest never calls the host for a path it holds
no descriptor for. So the round-trip asserts an exact, stable guest-authored
marker (`FsAccessDenied`). The enforcement stays 100% host (what the host placed
in `get-directories`); only the marker string is guest-emitted.

## Consequences

- A future divergence of the read/write WIT surface would let D2 split the cfgs;
  until then they move in lockstep.
- Tests asserting fs denial key on `FsAccessDenied` (guest constant), whereas
  net keys on the host `HttpRequestDenied` — an intentional, documented
  asymmetry, not an inconsistency.
- The positive fs path is offline-TESTED since #604: `wasi_fs_roundtrip.rs`
  drives the real guest through granted nested-preopen writes (truncate
  verified on the host filesystem), a granted read, and a root-preopen read —
  all against seeded tempdir sandboxes, no network. The host-side
  `wasi_fs_enforcement.rs` fs-probe test still covers `WasiCtx` enforcement
  with a std-fs guest.
