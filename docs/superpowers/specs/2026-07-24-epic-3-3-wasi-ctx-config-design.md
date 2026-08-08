# EPIC 3.3 — Host `WasiCtx` configuration from capabilities

**Status:** approved (design)
**Date:** 2026-07-24
**Roadmap:** `docs/superpowers/plans/vision-roadmap.md` EPIC 3, story 3.3
**Crate surface:** `tau-ports::target` (extends the 3.1 `wasi_map` module).
Read-only consumption of `tau-domain` capability types + the existing
`host_union` lattice op. No `tau-pkg` touch. No new dependency.

## Goal

Fold a capability *set* into the single host WASI configuration a wasm host
will feed its `WasiCtx`: the unioned egress allow-list (hosts + methods) and
the deduplicated, glob-resolved preopen directories. This is the **pure
resolver** the downstream host embedder consumes when it builds a real
`wasmtime_wasi::WasiCtx`.

3.1 mapped **one** `Capability` → **one** `WasiMapping`. 3.3 maps a
**capability set** → **one** `WasiConfiguration`, performing the three folds
(host union, method union, preopen dedup) and the glob→directory resolution
that 3.1 explicitly deferred ("Glob → directory resolution is deferred to
story 3.3").

## Scope decision: pure resolver, no wasmtime wiring (option A)

3.2 (WIT-world generation) has **not** landed, so the current guest world
`tau:host/runner` (`wit/tau-host.wit`) imports only `tau:host/host`
(`complete` / `now-millis` / `next-u64`). It imports **no** `wasi:http` or
`wasi:filesystem`. Any `WasiCtx` built today therefore has **no consumer** —
the same producer-without-consumer shape 3.1 shipped as (a pure, dormant,
fully-tested table).

3.3 mirrors that slice exactly:

- **In scope:** a pure, total `resolve_wasi_config` in `tau-ports` that turns
  a capability set into a `WasiConfiguration` value object, fully unit-tested,
  `no_std`.
- **Out of scope (deferred):**
  - Building a real `wasmtime_wasi::WasiCtx` / calling `.preopened_dir` /
    `.socket_addr_check` — deferred to the host embedder work that pairs with
    3.2, when a WASI-importing guest exists to exercise it. Adding
    `wasmtime-wasi` (+`wasmtime-wasi-http`) and a `WasiView`/`WasiHttpView`
    impl now would ship dead, un-exercisable surface (no guest imports WASI).
  - Deciding whether a widened preopen is a build error or a warning — that
    **policy** belongs to the build gate in 3.2/3.4. 3.3 makes the widening
    *machine-visible* (see "fs granularity") so those stories can enforce it;
    it does not itself reject.
  - The in-guest gate drop (3.4) and WIT-world text (3.2).

## Non-goals

- No `wasmtime` / `wasmtime-wasi` dependency, no `WasiCtx` construction, no
  `run_component` change.
- No new fallible boundary / `thiserror`. `resolve_wasi_config` is **total**:
  every capability set yields a `WasiConfiguration` (ruling 1 below).
- No narrowing or re-validation of the caps — they arrive already
  `[allow]`-bounded. The resolver folds; it does not gate.
- No guest↔host path remapping policy. `ResolvedPreopen` records the host
  directory; the guest-path mapping (identity, by default) is the host
  embedder's concern in the 3.2-paired work.

## Foundations consumed (all merged, read-only)

- `tau_ports::target::wasi_map::{map_capability, WasiConfig, WasiMapping,
  Preopen, PreopenAccess, Disposition}` — the 3.1 table (PR #511). The
  resolver calls `map_capability` per cap and folds the `WasiConfig`
  fragments.
- `tau_domain::package::capability::lattice::host::host_union` — `Any`
  absorbs; else set union. Reused verbatim for the host fold. (`host.rs:21`.)
- `tau_domain::{Capability, HostSet, HttpMethod}`.
- `HostSet::{Any, Exact(BTreeSet<HostName>)}` — the deny-all bottom is
  `Exact({})` (empty set = no host permitted).

## The fs-granularity divergence (ruling 1: record as data)

WASI preopens are **directory-tree** granular; WASI has no way to grant a
single file. A tau fs capability can be narrower than any directory (a whole-segment `*`
glob, or a literal single file). On native the OS gate (landlock /
sandbox-exec) enforces the glob/file exactly; on wasm it physically cannot.
This is a real wasm-vs-native divergence tau's build-enforcement philosophy
forbids hiding.

**Two design constraints force the rule:** the resolver is pure `no_std` — it
**cannot** stat the filesystem, so it cannot distinguish an all-literal
*directory* path (`/srv`) from an all-literal *file* path (`/etc/app.conf`);
both parse to `Pattern([Literal, …])`. And tau forbids silent widening.

**Reuse tau's G2 glob parser** (`tau_domain::…::lattice::glob::{expand,
Pattern, Segment}`). Each fs-cap path string is fed to `expand`, which
brace-expands and parses into normalized `Pattern`s (`Vec<Segment>` over
`Literal(String)` | `Star` (`*`, exactly one whole component) | `StarStar`
(`**`, trailing-only, any suffix)). Note G2 allows `*` **only as a whole
segment** — an intra-segment wildcard like `/data/*.txt` is **not** G2;
`expand` returns `None` for it. `expand` returning `None` (non-G2 / malformed)
→ that path contributes **no** preopen (fail-closed drop); the resolver stays
total.

**Rule X (chosen): never widen a literal.** From a `Pattern`'s segments:

- **host_dir** = the leading `Literal` components joined under `/` (the
  literal prefix, stopping at the first `Star`/`StarStar`); no leading
  literals (pattern starts with a wildcard, e.g. `/**`, `/*`) → host_dir
  `"/"`.
- **granularity** = `WidenedToDir` iff the pattern contains **any**
  `Segment::Star`; otherwise `Exact`. (`StarStar` is trailing-only and its
  preopened prefix equals the cap subtree, so it is `Exact`; an all-literal
  pattern is preopened as-is, also `Exact`.)

An **all-literal** pattern is preopened as the path **itself**. If that path
is really a file, `.preopened_dir` fails at apply-time ("not a directory") —
a single-file fs cap is thus **unrealizable on wasm**, an honest fail-closed
limitation, never a silent widen to the parent.

```
 "/data/**"        → host_dir "/data"           granularity Exact         (** = whole subtree == preopen)
 "/data/sub/**"    → host_dir "/data/sub"        granularity Exact
 "/data/*"         → host_dir "/data"            granularity WidenedToDir  (whole-segment *)
 "/var/log/*"      → host_dir "/var/log"         granularity WidenedToDir
 "/data/*/logs/**" → host_dir "/data"            granularity WidenedToDir  (mid-* widens to /data)
 "/srv"            → host_dir "/srv"             granularity Exact         (all-literal → preopen self)
 "/etc/app.conf"   → host_dir "/etc/app.conf"    granularity Exact         (all-literal; if a FILE, apply-time preopen fails — unrealizable, not widened)
 "/x.txt"          → host_dir "/x.txt"           granularity Exact         (all-literal; NOT a root "/" preopen)
 "/data/*.txt"     → (dropped)                   non-G2 → expand None → no preopen (fail-closed)
```

So `WidenedToDir` means exactly one thing: a whole-segment `*` glob whose
preopened prefix directory is broader than the star-matched set. `**` and
all-literal are `Exact`. There is no silent file→parent widening and no
`/x.txt → "/"` catastrophe — those only arose from the rejected "widen
literals to parent" rule (rule Y), which additionally required the resolver
to touch the filesystem and so was not implementable purely.

**Ruling on what to do with a widening: record it as data.** The resolver
stays total and pure; each `ResolvedPreopen` carries a `PreopenGranularity`
flag. The *policy* — turn a `WidenedToDir` preopen into a build error by
default, or a `--allow-wasm-fs-widening`-gated warning — belongs to the build
gate in 3.2/3.4. 3.3 hands them the fact; it does not decide. Rejected: (2)
make the resolver fallible and reject widening here — drags build policy into
a pure lowering fn, reintroduces the `thiserror` boundary 3.1 deliberately
avoided, and blocks 3.2 from choosing to *warn*; (3) widen silently —
violates the philosophy outright.

## API

Extends `crates/tau-ports/src/target/wasi_map.rs` (`no_std` + `alloc`,
`#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`). `map_capability` and all
3.1 types are **unchanged**. New public items, re-exported from
`target/mod.rs`:

```rust
/// Whether a resolved preopen equals its capability, or is broader because
/// WASI preopens are directory-granular. See "fs-granularity divergence".
#[non_exhaustive]
pub enum PreopenGranularity {
    /// Preopen == cap: an all-literal path (preopened as-is), or a literal
    /// prefix + trailing `**` (the preopen equals the cap subtree).
    Exact,
    /// Preopen is broader than the cap: a single-`*` glob whose preopened
    /// literal-prefix directory admits more than the `*`-matched set. The
    /// build gate (3.2/3.4) rejects this by default. (A literal single file
    /// is NOT widened — it is preopened as-is and fails at apply-time if it
    /// is really a file; see "fs-granularity divergence".)
    WidenedToDir,
}

/// One preopen after glob→directory resolution: a real host directory to
/// hand the guest, its access mode, and whether resolution widened the cap.
pub struct ResolvedPreopen {
    /// The host directory the embedder will preopen (absolute).
    pub host_dir: String,
    /// Read-only (fs.read) or read-write (fs.write).
    pub access: PreopenAccess,
    /// Whether this preopen exactly matches the cap or widened it.
    pub granularity: PreopenGranularity,
    /// Original capability glob(s) that produced this preopen (diagnostics).
    pub from: Vec<String>,
}

/// The whole host WASI configuration folded from a capability set. Consumed
/// by the wasm host embedder (3.2-paired work) to build a `wasmtime_wasi::
/// WasiCtx`. All non-`Disposition::Wasi` caps contribute nothing.
pub struct WasiConfiguration {
    /// Egress allow-list: `host_union` of every `net.http` `hosts`. Absent
    /// net.http caps yield `HostSet::Exact({})` (deny-all egress).
    pub allowed_hosts: HostSet,
    /// Allowed HTTP methods across all net.http caps. `None` = all methods
    /// (absorbing); else the union of the per-cap method sets.
    pub methods: Option<BTreeSet<HttpMethod>>,
    /// Deduplicated, glob-resolved preopens. Empty absent fs caps.
    pub preopens: Vec<ResolvedPreopen>,
}

/// Fold a capability set into one host WASI configuration.
///
/// Total and pure: every set yields a `WasiConfiguration`. Calls 3.1's
/// `map_capability` per cap and folds the `Disposition::Wasi` fragments
/// (host union, method union, preopen dedup + glob resolution). Fail-closed:
/// no net.http cap → deny-all egress; no fs cap → no preopens.
pub fn resolve_wasi_config<'a>(
    caps: impl IntoIterator<Item = &'a Capability>,
) -> WasiConfiguration;
```

## Folding semantics

The resolver iterates the set, calls `map_capability`, and folds only the
`Disposition::Wasi` fragments:

1. **`allowed_hosts`** = `host_union` over every `AllowedHosts.hosts`. `Any`
   absorbs. No net.http cap → the union of an empty iterator; the resolver
   guarantees this is `HostSet::Exact({})` (deny-all), special-casing the
   empty case if `host_union([])` does not already return it.

2. **`methods`** = `None` if **any** net.http cap has `methods == None`
   (`None` means "all", absorbing); otherwise `Some(⋃ method sets)`.

3. **`preopens`** = dedup by `host_dir`:
   - same `host_dir` seen read-only and read-write → **read-write** wins
     (`access = ⊔`, `ReadWrite ⊒ ReadOnly`);
   - `granularity` on a merged dir → `WidenedToDir` if **any** contributor
     widened, else `Exact`;
   - `from` = concatenation of the contributing globs;
   - nested directories (`/data` and `/data/out`) are kept as **separate**
     preopens — no attempt to collapse a child into its parent.

### Fail-closed defaults

```
 no net.http cap   → allowed_hosts = HostSet::Exact({})   (deny-all, NOT Any)
 no fs cap         → preopens = []
 empty cap set     → WasiConfiguration { Exact({}), None, [] }
```

Non-`Wasi` dispositions (`InGuest`, `HostMediated`, `Unsupported`) contribute
nothing — their gating is 3.4's, not the resolver's.

## Data flow

```
 capability set ─┐
                 │  for each cap
                 ▼
          map_capability (3.1) ─► WasiMapping{ config, disposition }
                 │                        │
                 │           keep Disposition::Wasi configs
                 ▼                        ▼
          resolve_wasi_config  ── folds ──►  WasiConfiguration
             (pure, total, no_std)          ├── allowed_hosts: HostSet         ──┐
                                            ├── methods: Option<Set<HttpMethod>> │─► host embedder
                                            └── preopens: Vec<ResolvedPreopen>   ─┘  (3.2-paired):
                                                     └─ granularity ─────────────► build gate (3.2/3.4)
```

## Testing (part of done, TDD)

Pure unit tests, inline `#[cfg(test)] mod tests`, `tau_domain::fixtures::{
cap_fs_read, cap_fs_write, cap_net_http}` for construction (the same
external-construction path 3.1 used). No filesystem, no wasmtime.

- **Host fold:** two net.http caps `{a.com}` + `{b.com}` → `Exact({a,b})`;
  any `+ Any` → `Any` (absorb).
- **Method fold:** `Some({POST})` + `Some({GET})` → `Some({GET,POST})`;
  `Some({POST})` + `None` → `None` (absorb).
- **Deny-all default:** no net.http cap → `allowed_hosts == Exact({})`; also
  pin `host_union([])` behavior so the special-case is justified.
- **Glob→dir table:** each row of the rule table above (`/data/**`→`/data`
  Exact; `/data/sub/**`→`/data/sub` Exact; `/data/*`→`/data` WidenedToDir;
  `/var/log/*`→`/var/log` WidenedToDir; `/data/*/logs/**`→`/data`
  WidenedToDir; `/srv`→`/srv` Exact).
- **All-literal never widens:** `/etc/app.conf` → `host_dir "/etc/app.conf"`,
  `Exact` (preopened as-is — NOT `/etc`); `/x.txt` → `host_dir "/x.txt"`,
  `Exact` (NOT a `"/"` root preopen). Pins that literals are never widened to
  a parent.
- **Non-G2 drop:** an fs cap path `/data/*.txt` (intra-segment `*`, non-G2) →
  `expand` returns `None` → contributes no preopen; resolver stays total.
- **Preopen dedup:** `fs.read "/data/**"` + `fs.write "/data/other"` →
  `/data` RO (Exact) and `/data/other` RW (Exact), separate; `fs.read
  "/data/**"` + `fs.write "/data/**"` on the **same** dir → single `/data`
  RW; `fs.read "/data/*"` + `fs.read "/data/**"` → single `/data` RO,
  granularity `WidenedToDir` (any contributor widened).
- **Disposition filter:** an `agent.spawn` / `Custom` / `fs.exec` cap in the
  set contributes nothing (`InGuest`/`HostMediated`/`Unsupported`).
- **Totality:** empty set → `{ Exact({}), None, [] }`; large mixed set
  returns without panic.

## Open questions / decisions

- **Widening policy (fatal vs. warn).** Deferred to 3.2/3.4 by design
  (ruling 1). 3.3 only records `PreopenGranularity`. The likely policy —
  `WidenedToDir` is a build error unless `--allow-wasm-fs-widening` — is
  noted here for the downstream story, not implemented.
- **`host_union([])` return value.** If it already returns `Exact({})` the
  resolver relies on it directly; a test pins this. If it returns `Any`
  (unlikely, would be unsound), the resolver special-cases "no net.http cap →
  `Exact({})`". Either way the observable default is deny-all.
- **Guest path = host path.** The resolver records only `host_dir`. Identity
  guest-path mapping is the embedder's default (3.2-paired), out of scope
  here.

## Downstream contract (informative)

The host embedder (paired with 3.2) will, for each `WasiConfiguration`:
`.preopened_dir(host_dir, host_dir, dir_perms, file_perms)` per preopen
(`ReadOnly` → `DirPerms::READ`/`FilePerms::READ`; `ReadWrite` → all), and
enforce `allowed_hosts`/`methods` at the `wasi:http` `WasiHttpView::
send_request` layer (the outgoing-handler filter 3.1 chose over raw sockets).
The build gate (3.2/3.4) reads `granularity` to reject `WidenedToDir`
preopens. This resolver is the single source both consume.
