# EPIC 3.3 — Host `WasiCtx` configuration from capabilities Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a pure, total `resolve_wasi_config` function (plus `WasiConfiguration`, `ResolvedPreopen`, `PreopenGranularity`) to `tau-ports`'s existing `wasi_map` module that folds a capability *set* into the single host WASI configuration a wasm embedder will feed its `WasiCtx`: unioned egress allow-list + deduplicated, glob-resolved preopen directories.

**Architecture:** Extend `crates/tau-ports/src/target/wasi_map.rs` (the 3.1 module). The resolver calls 3.1's `map_capability` per capability, keeps only the `Disposition::Wasi` `WasiConfig` fragments, and folds them via three operations: `tau_domain`'s `host_union` for hosts, a `None`-absorbing union for methods, and a dedup-by-directory for preopens. Glob→directory resolution reuses `tau_domain`'s G2 glob parser (`glob::expand`). Pure `no_std`; no wasmtime, no `WasiCtx` construction, no `run_component` touch (deferred to the 3.2-paired embedder). `map_capability` and all 3.1 types are untouched.

**Tech Stack:** Rust `no_std` + `alloc`, `tau-domain` (capability types + `host_union` + `glob`), inline `#[cfg(test)] mod tests`, `cargo nextest`.

## Global Constraints

- **Design doc (source of truth):** `docs/superpowers/specs/2026-07-24-epic-3-3-wasi-ctx-config-design.md`. Its rule table and folding semantics are authoritative; this plan implements them verbatim.
- **Crate:** only `crates/tau-ports`. Do **not** touch `tau-pkg`, `tau-wasm-host`, or any other crate. **No new dependency** (`wasmtime-wasi` is explicitly out of scope — see design "Scope decision").
- **Extend, don't rewrite:** all changes live in `crates/tau-ports/src/target/wasi_map.rs`; `map_capability`, `WasiMapping`, `WasiConfig`, `Preopen`, `PreopenAccess`, `Disposition`, `WitInterface`, `WASI_VERSION` from 3.1 are **unchanged**. Only `target/mod.rs` gets new re-export lines.
- **`no_std`:** the crate is `#![no_std]`; use `alloc::{vec::Vec, string::String, collections::{BTreeSet, BTreeMap}}`. `std` is only available under `#[cfg(test)]`.
- **Lints:** `#![forbid(unsafe_code)]` (workspace) and `#![deny(missing_docs)]` (crate) are in force — **every** `pub` item needs a `///` doc comment or the build fails. Private helpers (`resolve_pattern`, `preopens_for_path`, `dedup_preopens`) need no doc.
- **`thiserror`:** NOT introduced. `resolve_wasi_config` is **total** (returns for every input); there is no fallible boundary. A non-G2 fs path (`glob::expand` → `None`) is dropped fail-closed, not errored.
- **Reuse, don't reimplement:** host union = `tau_domain::package::capability::lattice::host::host_union`; glob parsing = `tau_domain::package::capability::lattice::glob::{expand, Pattern, Segment}`. Do not write a second glob parser or host-union.
- **Fail-closed defaults:** no net.http cap → `allowed_hosts = HostSet::Exact({})` (falls out of `host_union` over an empty iterator — verified: `host.rs:21` returns `Exact(∅)` when the iterator is empty). No fs cap → `preopens = []`.
- **CARGO RULES (CLAUDE.md) — every cargo command uses this exact shape:**
  - Tests: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-ports`
  - Filter one test: append the test-name substring, e.g. `... cargo nextest run -p tau-ports wasi_map::tests::resolve_glob`
  - Doctests: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo test -p tau-ports --doc`
  - Never run bare `cargo`, never omit `-p`, always wrap in `timeout`.

## Type / import reference (verified against the tree)

Consumed from 3.1 (same file — `crates/tau-ports/src/target/wasi_map.rs`):

```rust
pub fn map_capability(cap: &Capability) -> WasiMapping;   // WasiMapping{ imports, config, disposition }
pub enum WasiConfig { None, AllowedHosts { hosts: HostSet, methods: Option<BTreeSet<HttpMethod>> }, Preopens(Vec<Preopen>) }
pub struct Preopen { pub paths: Vec<String>, pub access: PreopenAccess }
pub enum PreopenAccess { ReadOnly, ReadWrite }            // derives Debug, Clone, PartialEq, Eq
```

Consumed from `tau_domain` (all `no_std`):

```rust
use tau_domain::{Capability, HostSet, HttpMethod};        // crate-root re-exports
use tau_domain::package::capability::lattice::host::host_union; // fn(impl IntoIterator<Item=&'a HostSet>) -> HostSet
use tau_domain::package::capability::lattice::glob::{expand, Pattern, Segment};
//   expand(&str) -> Option<Vec<Pattern>>   (brace-expands + parses; None if non-G2)
//   Pattern(pub Vec<Segment>)
//   Segment::{ Literal(String), Star, StarStar }   // Star = one whole component; StarStar = trailing any-suffix
```

Test construction (feature `test-fixtures`, already reachable in tau-ports tests + doctests — 3.1 uses it):

```rust
use tau_domain::fixtures::{cap_fs_read, cap_fs_write, cap_net_http, cap_process_spawn, cap_fs_exec};
//   cap_fs_read(paths: &[&str])                 -> Capability
//   cap_fs_write(paths: &[&str], max: Option<u64>) -> Capability
//   cap_net_http(hosts: &[&str], methods: &[&str]) -> Capability   // hosts == ["any"] => HostSet::Any
```

## File structure

- Modify: `crates/tau-ports/src/target/wasi_map.rs` — append the new public types (`PreopenGranularity`, `ResolvedPreopen`, `WasiConfiguration`), the public `resolve_wasi_config`, and private helpers (`resolve_pattern`, `preopens_for_path`, `dedup_preopens`), plus new inline tests. 3.1 items untouched.
- Modify: `crates/tau-ports/src/target/mod.rs` — add re-export lines for the new public items (3.1 already re-exports the module; extend the `pub use` list).

---

### Task 1: Glob→directory resolution + leaf types (`PreopenGranularity`, `ResolvedPreopen`)

The pure, per-path resolution: one normalized G2 `Pattern` → `(host_dir, granularity)`, and one raw fs-cap path string → the `ResolvedPreopen`s it yields (fail-closed drop on non-G2).

**Files:**
- Modify: `crates/tau-ports/src/target/wasi_map.rs` (append after the 3.1 code, before `#[cfg(test)] mod tests`; add tests inside the existing `mod tests`)

**Interfaces:**
- Consumes: `glob::{expand, Pattern, Segment}`, `PreopenAccess` (from 3.1).
- Produces:
  - `pub enum PreopenGranularity { Exact, WidenedToDir }` (`#[non_exhaustive]`, derives `Debug, Clone, Copy, PartialEq, Eq`)
  - `pub struct ResolvedPreopen { pub host_dir: String, pub access: PreopenAccess, pub granularity: PreopenGranularity, pub from: Vec<String> }` (derives `Debug, Clone, PartialEq, Eq`)
  - `fn resolve_pattern(pat: &Pattern) -> (String, PreopenGranularity)` (private)
  - `fn preopens_for_path(path: &str, access: PreopenAccess) -> Vec<ResolvedPreopen>` (private)

- [ ] **Step 1: Write the failing tests**

Add these imports to the existing `#[cfg(test)] mod tests` `use` block if not already present: `use tau_domain::package::capability::lattice::glob::expand;` and `use alloc::string::ToString;` (already present per 3.1). Then add:

```rust
#[test]
fn resolve_pattern_glob_to_dir_table() {
    // (input glob, expected host_dir, expected granularity)
    let cases = [
        ("/data/**", "/data", PreopenGranularity::Exact),
        ("/data/sub/**", "/data/sub", PreopenGranularity::Exact),
        ("/data/*", "/data", PreopenGranularity::WidenedToDir),
        ("/var/log/*", "/var/log", PreopenGranularity::WidenedToDir),
        ("/data/*/logs/**", "/data", PreopenGranularity::WidenedToDir),
        ("/srv", "/srv", PreopenGranularity::Exact),          // all-literal dir
        ("/etc/app.conf", "/etc/app.conf", PreopenGranularity::Exact), // all-literal; preopen self, NOT /etc
        ("/x.txt", "/x.txt", PreopenGranularity::Exact),      // all-literal; NOT root "/"
        ("/**", "/", PreopenGranularity::Exact),              // whole FS, exact
        ("/*", "/", PreopenGranularity::WidenedToDir),        // whole-segment star at root
    ];
    for (glob, dir, gran) in cases {
        let pats = expand(glob).expect("G2 valid");
        assert_eq!(pats.len(), 1, "{glob} brace-expands to one pattern");
        let (host_dir, granularity) = resolve_pattern(&pats[0]);
        assert_eq!(host_dir, dir, "host_dir for {glob}");
        assert_eq!(granularity, gran, "granularity for {glob}");
    }
}

#[test]
fn preopens_for_path_non_g2_is_dropped_fail_closed() {
    // Intra-segment wildcard is not G2; expand -> None -> no preopen.
    assert!(expand("/data/*.txt").is_none(), "intra-segment * is non-G2");
    assert_eq!(preopens_for_path("/data/*.txt", PreopenAccess::ReadOnly), Vec::new());
}

#[test]
fn preopens_for_path_brace_expands_to_multiple() {
    // A brace glob yields one ResolvedPreopen per arm, carrying the raw path in `from`.
    let got = preopens_for_path("/data/{a,b}/**", PreopenAccess::ReadOnly);
    let dirs: Vec<&str> = got.iter().map(|p| p.host_dir.as_str()).collect();
    assert_eq!(dirs, vec!["/data/a", "/data/b"]);
    assert!(got.iter().all(|p| p.access == PreopenAccess::ReadOnly));
    assert!(got.iter().all(|p| p.granularity == PreopenGranularity::Exact));
    assert!(got.iter().all(|p| p.from == vec!["/data/{a,b}/**".to_string()]));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-ports wasi_map::tests::resolve_pattern wasi_map::tests::preopens_for_path`
Expected: FAIL — `resolve_pattern`, `preopens_for_path`, `PreopenGranularity`, `ResolvedPreopen` not defined.

- [ ] **Step 3: Add the leaf types**

Append to `wasi_map.rs` (after the 3.1 `fs_preopen` helper, before `#[cfg(test)]`):

```rust
/// Whether a resolved preopen equals its capability, or is broader because
/// WASI preopens are directory-granular. See the design doc's
/// "fs-granularity divergence".
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreopenGranularity {
    /// Preopen == cap: an all-literal path (preopened as-is), or a literal
    /// prefix + trailing `**` (the preopen equals the cap subtree).
    Exact,
    /// Preopen is broader than the cap: a whole-segment `*` glob whose
    /// preopened literal-prefix directory admits more than the `*`-matched
    /// set. The build gate (3.2/3.4) rejects this by default. A literal
    /// single file is NOT widened — it is preopened as-is and fails at
    /// apply-time if it is really a file.
    WidenedToDir,
}

/// One preopen after glob→directory resolution: a host directory to hand the
/// guest, its access mode, whether resolution widened the cap, and the
/// originating cap glob(s) for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPreopen {
    /// The absolute host directory the embedder will preopen.
    pub host_dir: String,
    /// Read-only (fs.read) or read-write (fs.write).
    pub access: PreopenAccess,
    /// Whether this preopen exactly matches the cap or widened it.
    pub granularity: PreopenGranularity,
    /// Original capability glob(s) that produced this preopen.
    pub from: Vec<String>,
}
```

- [ ] **Step 4: Add the private helpers**

Append after the types:

```rust
/// The `(host_dir, granularity)` a single normalized G2 pattern resolves to.
///
/// `host_dir` is the leading `Literal` prefix joined under `/` (empty prefix
/// → `"/"`). Granularity is `WidenedToDir` iff the pattern contains any
/// whole-segment `Star`; `StarStar` and all-literal are `Exact`.
fn resolve_pattern(pat: &Pattern) -> (String, PreopenGranularity) {
    let mut dir = String::new();
    let mut widened = false;
    let mut stopped = false; // true once we pass the first wildcard segment
    for seg in &pat.0 {
        match seg {
            Segment::Literal(s) if !stopped => {
                dir.push('/');
                dir.push_str(s);
            }
            Segment::Star => {
                widened = true;
                stopped = true;
            }
            Segment::StarStar => {
                stopped = true;
            }
            // A literal after the first wildcard does not extend host_dir.
            Segment::Literal(_) => {}
        }
    }
    if dir.is_empty() {
        dir.push('/');
    }
    let granularity = if widened {
        PreopenGranularity::WidenedToDir
    } else {
        PreopenGranularity::Exact
    };
    (dir, granularity)
}

/// The `ResolvedPreopen`s one raw fs-cap path yields. Non-G2 / malformed
/// (`expand` → `None`) → empty (fail-closed drop). A brace glob yields one
/// per expanded arm; each records the raw `path` in `from`.
fn preopens_for_path(path: &str, access: PreopenAccess) -> Vec<ResolvedPreopen> {
    match expand(path) {
        None => Vec::new(),
        Some(patterns) => patterns
            .iter()
            .map(|p| {
                let (host_dir, granularity) = resolve_pattern(p);
                ResolvedPreopen {
                    host_dir,
                    access: access.clone(),
                    granularity,
                    from: vec![path.to_string()],
                }
            })
            .collect(),
    }
}
```

Add the import at the top of `wasi_map.rs` (with the other `tau_domain` uses):

```rust
use tau_domain::package::capability::lattice::glob::{expand, Pattern, Segment};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-ports wasi_map::tests::resolve_pattern wasi_map::tests::preopens_for_path`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-ports/src/target/wasi_map.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-ports): glob→preopen-dir resolution + granularity (3.3)"
```

---

### Task 2: `resolve_wasi_config` fold + `WasiConfiguration` + re-exports

Fold a capability set into one `WasiConfiguration`: host union, `None`-absorbing method union, and preopen dedup-by-directory. Wire the new public items through `target/mod.rs`.

**Files:**
- Modify: `crates/tau-ports/src/target/wasi_map.rs` (add `WasiConfiguration`, `resolve_wasi_config`, `dedup_preopens`, tests + a doctest)
- Modify: `crates/tau-ports/src/target/mod.rs` (extend the `pub use wasi_map::{…}` re-export)

**Interfaces:**
- Consumes: Task 1's `ResolvedPreopen`, `PreopenGranularity`, `preopens_for_path`; 3.1's `map_capability`, `WasiConfig`, `Preopen`, `PreopenAccess`; `host_union`; `HostSet`, `HttpMethod`.
- Produces:
  - `pub struct WasiConfiguration { pub allowed_hosts: HostSet, pub methods: Option<BTreeSet<HttpMethod>>, pub preopens: Vec<ResolvedPreopen> }` (derives `Debug, Clone, PartialEq, Eq`)
  - `pub fn resolve_wasi_config<'a>(caps: impl IntoIterator<Item = &'a Capability>) -> WasiConfiguration`

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` (fixtures `cap_fs_read`, `cap_fs_write`, `cap_net_http` are already imported per 3.1; add `cap_process_spawn`, `cap_fs_exec` to the fixtures `use`, and `use alloc::collections::BTreeSet;` if not present):

```rust
#[test]
fn host_fold_unions_exact_and_any_absorbs() {
    let a = resolve_wasi_config(&[cap_net_http(&["a.com"], &[]), cap_net_http(&["b.com"], &[])]);
    assert_eq!(a.allowed_hosts, exact(&["a.com", "b.com"]));
    let b = resolve_wasi_config(&[cap_net_http(&["a.com"], &[]), cap_net_http(&["any"], &[])]);
    assert_eq!(b.allowed_hosts, HostSet::Any);
}

#[test]
fn method_fold_unions_and_none_absorbs() {
    let post = HttpMethod::parse("POST").unwrap();
    let get = HttpMethod::parse("GET").unwrap();
    // Some ∪ Some = union
    let a = resolve_wasi_config(&[cap_net_http(&["a.com"], &["POST"]), cap_net_http(&["a.com"], &["GET"])]);
    assert_eq!(a.methods, Some([get, post].into_iter().collect::<BTreeSet<_>>()));
    // Some ∪ None = None (all methods absorbs)
    let b = resolve_wasi_config(&[cap_net_http(&["a.com"], &["POST"]), cap_net_http(&["a.com"], &[])]);
    assert_eq!(b.methods, None);
}

#[test]
fn no_net_cap_is_deny_all_egress() {
    let cfg = resolve_wasi_config(&[cap_fs_read(&["/data/**"])]);
    assert_eq!(cfg.allowed_hosts, HostSet::Exact(BTreeSet::new())); // deny-all, NOT Any
    assert_eq!(cfg.methods, None);
}

#[test]
fn empty_cap_set_grants_nothing() {
    let cfg = resolve_wasi_config(core::iter::empty());
    assert_eq!(cfg.allowed_hosts, HostSet::Exact(BTreeSet::new()));
    assert_eq!(cfg.methods, None);
    assert!(cfg.preopens.is_empty());
}

#[test]
fn preopen_dedup_rw_wins_and_widen_sticks() {
    // Same dir seen RO (from **, exact) and RW (from **, exact) -> single RW.
    let same = resolve_wasi_config(&[cap_fs_read(&["/data/**"]), cap_fs_write(&["/data/**"], None)]);
    assert_eq!(same.preopens.len(), 1);
    assert_eq!(same.preopens[0].host_dir, "/data");
    assert_eq!(same.preopens[0].access, PreopenAccess::ReadWrite);
    assert_eq!(same.preopens[0].granularity, PreopenGranularity::Exact);

    // Same dir, one contributor widened (/data/*) -> granularity WidenedToDir.
    let widen = resolve_wasi_config(&[cap_fs_read(&["/data/*"]), cap_fs_read(&["/data/**"])]);
    assert_eq!(widen.preopens.len(), 1);
    assert_eq!(widen.preopens[0].host_dir, "/data");
    assert_eq!(widen.preopens[0].access, PreopenAccess::ReadOnly);
    assert_eq!(widen.preopens[0].granularity, PreopenGranularity::WidenedToDir);
}

#[test]
fn preopen_nested_dirs_kept_separate_and_sorted() {
    let cfg = resolve_wasi_config(&[cap_fs_write(&["/data/other"], None), cap_fs_read(&["/data/**"])]);
    let dirs: Vec<&str> = cfg.preopens.iter().map(|p| p.host_dir.as_str()).collect();
    assert_eq!(dirs, vec!["/data", "/data/other"]); // BTreeMap order (sorted, deterministic)
    assert_eq!(cfg.preopens[0].access, PreopenAccess::ReadOnly);  // /data      RO
    assert_eq!(cfg.preopens[1].access, PreopenAccess::ReadWrite); // /data/other RW
}

#[test]
fn non_wasi_dispositions_contribute_nothing() {
    // agent.spawn is InGuest; process.spawn / fs.exec are Unsupported; none add config.
    let cfg = resolve_wasi_config(&[cap_process_spawn(&["ls"]), cap_fs_exec(&["/bin/**"])]);
    assert_eq!(cfg.allowed_hosts, HostSet::Exact(BTreeSet::new()));
    assert!(cfg.preopens.is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-ports wasi_map::tests`
Expected: FAIL — `resolve_wasi_config` / `WasiConfiguration` not defined.

- [ ] **Step 3: Add `WasiConfiguration` + the fold**

Append to `wasi_map.rs` (after Task 1's helpers). Add `use alloc::collections::BTreeMap;` to the top-of-file `use` block (alongside `BTreeSet`):

```rust
/// The whole host WASI configuration folded from a capability set. Consumed
/// by the wasm host embedder (3.2-paired work) to build a
/// `wasmtime_wasi::WasiCtx`. All non-`Disposition::Wasi` caps contribute
/// nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasiConfiguration {
    /// Egress allow-list: `host_union` of every `net.http` `hosts`. Absent
    /// net.http caps yield `HostSet::Exact({})` (deny-all egress).
    pub allowed_hosts: HostSet,
    /// Allowed HTTP methods across all net.http caps. `None` = all methods
    /// (absorbing); else the union of the per-cap sets. No net.http cap →
    /// `None`.
    pub methods: Option<BTreeSet<HttpMethod>>,
    /// Deduplicated, glob-resolved preopens, sorted by `host_dir`. Empty
    /// absent fs caps.
    pub preopens: Vec<ResolvedPreopen>,
}

/// Fold a capability set into one host WASI configuration.
///
/// Total and pure: every set yields a [`WasiConfiguration`]. Calls
/// [`map_capability`] per cap and folds the `Disposition::Wasi` fragments —
/// host union, `None`-absorbing method union, and preopen dedup + glob→dir
/// resolution. Fail-closed: no net.http cap → deny-all egress; no fs cap →
/// no preopens; a non-G2 fs path is dropped.
///
/// # Example
///
/// ```
/// use tau_ports::target::wasi_map::{resolve_wasi_config, PreopenAccess, PreopenGranularity};
/// use tau_domain::fixtures::{cap_fs_read, cap_net_http};
///
/// let caps = [cap_net_http(&["api.example.com"], &["POST"]), cap_fs_read(&["/data/**"])];
/// let cfg = resolve_wasi_config(&caps);
/// assert_eq!(cfg.preopens.len(), 1);
/// assert_eq!(cfg.preopens[0].host_dir, "/data");
/// assert_eq!(cfg.preopens[0].access, PreopenAccess::ReadOnly);
/// assert_eq!(cfg.preopens[0].granularity, PreopenGranularity::Exact);
/// ```
pub fn resolve_wasi_config<'a>(
    caps: impl IntoIterator<Item = &'a Capability>,
) -> WasiConfiguration {
    let mut host_sets: Vec<HostSet> = Vec::new();
    let mut any_net = false;
    let mut methods_all = false;
    let mut method_union: BTreeSet<HttpMethod> = BTreeSet::new();
    let mut raw_preopens: Vec<ResolvedPreopen> = Vec::new();

    for cap in caps {
        match map_capability(cap).config {
            WasiConfig::AllowedHosts { hosts, methods } => {
                any_net = true;
                host_sets.push(hosts);
                match methods {
                    None => methods_all = true,
                    Some(s) => method_union.extend(s),
                }
            }
            WasiConfig::Preopens(preopens) => {
                for p in preopens {
                    for path in &p.paths {
                        raw_preopens.extend(preopens_for_path(path, p.access.clone()));
                    }
                }
            }
            WasiConfig::None => {}
        }
    }

    // host_union over an empty iterator returns Exact(∅) = deny-all (host.rs:21).
    let allowed_hosts = host_union(host_sets.iter());
    let methods = if !any_net || methods_all {
        None
    } else {
        Some(method_union)
    };

    WasiConfiguration {
        allowed_hosts,
        methods,
        preopens: dedup_preopens(raw_preopens),
    }
}

/// Merge preopens by host directory: RW absorbs RO, `WidenedToDir` absorbs
/// `Exact`, `from` concatenated. `BTreeMap` yields a deterministic
/// host_dir-sorted result. Nested directories stay separate (distinct keys).
fn dedup_preopens(raw: Vec<ResolvedPreopen>) -> Vec<ResolvedPreopen> {
    use alloc::collections::btree_map::Entry;
    let mut by_dir: BTreeMap<String, ResolvedPreopen> = BTreeMap::new();
    for p in raw {
        match by_dir.entry(p.host_dir.clone()) {
            Entry::Occupied(mut o) => {
                let e = o.get_mut();
                if p.access == PreopenAccess::ReadWrite {
                    e.access = PreopenAccess::ReadWrite;
                }
                if p.granularity == PreopenGranularity::WidenedToDir {
                    e.granularity = PreopenGranularity::WidenedToDir;
                }
                e.from.extend(p.from);
            }
            Entry::Vacant(v) => {
                v.insert(p);
            }
        }
    }
    by_dir.into_values().collect()
}
```

- [ ] **Step 4: Re-export the new public items**

In `crates/tau-ports/src/target/mod.rs`, extend the existing `pub use wasi_map::{…}` line (added by 3.1) to also export the new items. The result must include:

```rust
pub use wasi_map::{
    map_capability, Disposition, Preopen, PreopenAccess, PreopenGranularity, ResolvedPreopen,
    WasiConfig, WasiConfiguration, WitInterface, WasiMapping, WASI_VERSION, resolve_wasi_config,
};
```

(Preserve whatever 3.1 already listed; just add `PreopenGranularity`, `ResolvedPreopen`, `WasiConfiguration`, `resolve_wasi_config`. Read the current line first and add to it rather than replacing blind.)

- [ ] **Step 5: Run the full crate test suite + doctests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-ports`
Expected: PASS (all 3.1 tests + the new ones).

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo test -p tau-ports --doc`
Expected: PASS (the `resolve_wasi_config` doctest + 3.1's `map_capability` doctest).

- [ ] **Step 6: Clippy (warn == deny in CI)**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo clippy -p tau-ports --all-targets`
Expected: no warnings. (Recall workspace treats warnings as deny in CI.)

- [ ] **Step 7: Commit**

```bash
git add crates/tau-ports/src/target/wasi_map.rs crates/tau-ports/src/target/mod.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-ports): fold cap set → WasiConfiguration (EPIC 3.3)"
```

---

## Self-Review

**1. Spec coverage** — every design section maps to a task:

| Spec section | Task |
|---|---|
| `resolve_wasi_config` API + `WasiConfiguration` | Task 2 |
| `ResolvedPreopen` + `PreopenGranularity` | Task 1 |
| Glob→dir rule X (all-literal Exact, `**` Exact, `*` WidenedToDir, non-G2 drop) | Task 1 (`resolve_pattern`, `preopens_for_path`) + tests |
| Host fold (`host_union`, Any absorbs) | Task 2 `host_fold_unions_*` |
| Method fold (`None` absorbs, else union) | Task 2 `method_fold_*` |
| Preopen dedup (RW ⊒ RO, widen sticks, nested separate) | Task 2 `preopen_dedup_*`, `preopen_nested_*` |
| Fail-closed defaults (no net → deny-all; no fs → []; empty set) | Task 2 `no_net_cap_*`, `empty_cap_set_*` |
| Disposition filter (InGuest/HostMediated/Unsupported contribute nothing) | Task 2 `non_wasi_dispositions_*` |
| Re-export | Task 2 Step 4 |

**2. Placeholder scan** — every step contains real code, exact commands, and expected outcomes. No TBD/TODO. ✅

**3. Type consistency** — `PreopenGranularity`, `ResolvedPreopen`, `WasiConfiguration`, `resolve_wasi_config`, `resolve_pattern`, `preopens_for_path`, `dedup_preopens` are named identically across the Interfaces blocks, code, and tests. `host_dir`/`access`/`granularity`/`from` field names consistent. `allowed_hosts`/`methods`/`preopens` consistent. ✅

**Note on out-of-scope (design-confirmed):** no `wasmtime-wasi` dep, no `WasiCtx` construction, no `run_component` change, no widening *policy* (fatal vs. warn) — those are the 3.2-paired embedder and the 3.2/3.4 build gate. 3.3 is the pure resolver only.
