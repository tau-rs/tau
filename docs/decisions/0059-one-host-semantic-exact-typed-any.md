# ADR-0059: One host semantic — exact hostnames + a typed `any`; no derived hosts

**Status:** Accepted
**Date:** 2026-07-18
**Deciders:** tau core

## Context

The `net.http` capability's `hosts` field had three divergent semantics live at
once:

1. **Docs** claimed hosts are "exact match or glob"
   (`crates/tau-domain/src/package/capability.rs:137`, pre-change).
2. **The lattice** did exact string equality — `hosts=["*"]` was treated as the
   literal host `"*"` rather than a wildcard
   (`crates/tau-pkg/src/capability_override/subset.rs`, `string_set_subset`).
3. **The runtime proxy** rejected any `*` at spawn time
   (`crates/tau-sandbox-proxy/src/validate.rs`, invoked from the native
   `strict.rs` and the darwin / container / windows sandbox adapters).

The practical consequence: a manifest authored with `hosts = ["*"]` parsed
successfully, passed the capability lattice, passed `tau check`, and passed
`tau build` — then **deterministically failed at `tau run`** when the proxy
rejected the wildcard. Build-accepts-but-run-rejects is exactly the
false-security divergence tau's build-time enforcement principle exists to
prevent.

Two secondary defects compounded this:

- **No case folding.** `API.COM` and `api.com` were distinct strings through
  the entire stack (parse, lattice, proxy), so a ceiling authored in one case
  did not subsume a grant authored in another.
- **Hand-rolled `derive_host`** (`crates/tau-pkg/src/project/allow.rs`) parsed
  `[allow.mcp.<name>].url` into a host ceiling using ad hoc string surgery —
  `https://user@evil.com/` derived to host `user@evil.com`, and IPv6 URLs
  derived to `"["`.

Separately, the lattice's `methods` field was ignored entirely
(`subset.rs`): a child capability could declare `methods = ["POST"]` against
a parent that only granted `methods = ["GET"]` and the subset check would
pass, because method inclusion was never checked.

Full analysis: `docs/superpowers/specs/2026-07-18-hostset-exact-plus-typed-any-design.md`.

## Decision

Hosts are **bare lowercase hostnames** or a **typed any-sentinel** — one
semantic enforced identically from parse through the capability lattice
through the runtime proxy.

```rust
/// A validated bare hostname with optional port. Never a URL, glob, or scheme.
pub struct HostName(String);

pub enum HostSet {
    Any,                        // authored as  hosts = "any"
    Exact(BTreeSet<HostName>),  // authored as  hosts = ["a.com", "b.io:8080"]
}

impl HostSet {
    /// Any ⊇ everything; Exact(p) ⊇ Exact(c) ⟺ c ⊆ p; Exact ⊉ Any.
    pub fn subsumes(&self, child: &HostSet) -> bool;
}
```

`HostName::parse` case-folds (`A.COM` → `a.com`, accept-and-fold, never
reject on case) and rejects anything that is not a bare hostname: `*` (with a
`help: write hosts = "any"` hint), suffix globs like `*.a.com`, URLs
(`https://...`), userinfo (`user@a.com`), paths, and IPv6 literals
(`[::1]:8080`). `BTreeSet` gives deterministic, sorted serialization.

`methods` becomes a typed, checked field:

```rust
pub enum HttpMethod { Get, Head, Post, Put, Delete, Connect, Options, Trace, Patch }

Http {
    hosts: HostSet,
    methods: Option<BTreeSet<HttpMethod>>,  // None = all methods
}
```

**Absent vs. empty is load-bearing and never conflated:** `methods` absent →
`None` → *all methods*; `methods = []` → `Some(∅)` → *deny all methods*. The
old lattice code collapsed this distinction via `unwrap_or_default`, which is
the same bug shape as the audit-B2 finding this ADR closes for methods.

Three resolutions from the design brainstorm:

- **`derive_host` is deleted.** It backed only `[allow.mcp.<name>]`'s host
  ceiling (the `net.http` capability bridge never used it). No URL→host
  derivation exists anywhere in the capability stack after this change.
  `[allow.mcp.<name>].hosts` is now **required**; an MCP entry with no
  explicit `hosts` is a validation error.
- **One PR, ordered commits.** The `Http` variant's field type change is
  atomic across the workspace — a types-only first commit does not compile
  standalone — so domain → consumers → docs land as one PR with
  logically-ordered commits, compiling green only at the end.
- **Typed `HttpMethod` + absent-vs-empty**, as above.

### Consumers updated to the one semantic

- **Lattice** (`subset.rs`): the `Http` arm now calls `HostSet::subsumes` and
  a real methods-inclusion check; the old `gather_hosts` / string-set-subset
  path for hosts is gone.
- **`[allow]` bridge** (`allow.rs`): `derive_host` deleted;
  `[allow.mcp.<name>].hosts` required, empty/absent is an error.
- **Proxy** (`tau-sandbox-proxy`): a typed `HostAllow { Any, Exact(Vec<String>) }`
  policy, with pass-all mode reachable only from `HostSet::Any`. Runtime
  CONNECT host matching case-folds. `validate_hosts` still rejects `*` and
  non-loopback IPs as defense in depth on the `Exact` path.
- **Sandbox adapters** (native `strict.rs`, darwin, container, windows): map
  `HostSet::Exact` → `HostAllow::Exact` (case-folded) and `HostSet::Any` →
  `HostAllow::Any` before calling `spawn_proxy`.

## Consequences

**Positive:**

- **Build-accepts ⟺ run-enforces**, both directions. `hosts = "any"` passes
  parse, `tau check`, `tau build`, and reaches the proxy in pass-all mode
  with no spawn-time rejection. `hosts = ["*"]` now fails at **parse** —
  it never reaches build, closing the divergence this ADR exists to fix.
- Case folding is uniform: a ceiling authored `API.COM` and a grant authored
  `api.com` are recognized as the same host everywhere.
- `methods` inclusion is a real, checked lattice rule instead of a silently
  ignored field; the `_method_diff_ignored` test class flips to actually
  checking method subsumption.
- `[allow.mcp.<name>].hosts` being required removes a whole class of
  URL-parsing edge cases (`derive_host`'s userinfo/IPv6 mishandling) by
  deleting the code path rather than hardening it.

**Negative / obligations:**

- `[allow.mcp.<name>]` entries that previously relied on `derive_host` (no
  explicit `hosts`) now fail validation and must be updated with an explicit
  `hosts = [...]` list. No such fixtures existed in-tree at time of writing;
  documentation examples were swept for the same pattern.
- Manifests that previously omitted `methods` and round-tripped through
  `unwrap_or_default` into a serialized `"methods": []` now correctly
  serialize with the key omitted — a byte-stability change for that specific
  (pre-existing bug) shape. Already-lowercase, sorted `Exact` host lists and
  manifests with real `methods` sets are unaffected; a golden test asserts
  this.
- EPIC 1.6's coarse-ceiling lint must target the typed `HostSet::Any`
  variant, not a string match on `"*"` — noted on that branch
  (`feat/epic-1.6-coarse-lint`), not implemented here.

## Explicitly deferred (additive later, not part of this ADR)

- **Suffix wildcards** (`*.x.com`). Would require per-label glob matching in
  both the lattice and the proxy's CONNECT-host check; deferred until there
  is a concrete need.
- **IPv6 literal hosts** (`[::1]:8080`). `HostName::parse` rejects `[` today;
  adding IPv6 support is additive and does not require revisiting the
  `HostSet`/`HostName` shape.
- **Runtime method enforcement.** The proxy filters by host, not method;
  `methods` is enforced only in the build-time lattice (now *checked*,
  previously ignored — see Decision). This leaves methods
  build-checked-but-not-run-enforced, a narrower residual than the host
  divergence this ADR closes. Tracked as a follow-up, not part of this
  change.

## Alternatives considered

**A. Keep hosts as a raw `Vec<String>` and special-case `"*"` at each
consumer.** Rejected. This is exactly the status quo that produced the
three-way divergence — each consumer (lattice, proxy) would need its own
special-case logic and case-folding, with no shared invariant. A typed
`HostSet` makes "what counts as a host" and "what counts as any" a single,
testable definition.

**B. Support suffix wildcards now, alongside exact + any.** Rejected for this
ADR. Wildcards require resolution-time or match-time glob logic in the
proxy's CONNECT-host check, which is materially more code and a larger
attack surface for host-matching bugs. Exact + typed-any closes the
build/run divergence completely without it; wildcards are additive later.

**C. Keep `derive_host` and harden it against the userinfo/IPv6 bugs
instead of deleting it.** Rejected. `derive_host` is not used by the
`net.http` capability bridge at all — it exists solely to backfill
`[allow.mcp.<name>].hosts` from a URL. Requiring explicit `hosts` removes an
entire parsing surface rather than patching it, and matches the
governance principle that ceilings should be explicit, not inferred.
