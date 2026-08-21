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
- The positive/connected fs path is offline-untested by design (as net); the
  host-side `wasi_fs_enforcement.rs` fs-probe test covers the granted read.
