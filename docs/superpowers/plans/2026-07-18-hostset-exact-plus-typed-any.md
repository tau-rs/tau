# HostSet: exact hosts + typed `any` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `net.http`'s `hosts: Vec<String>` / `methods: Vec<String>` with a single validated `HostSet` (exact lowercase hostnames or a typed `Any`) and `Option<BTreeSet<HttpMethod>>`, so one host/method semantic flows unbroken from parse → lattice → proxy — closing the build-accepts-but-run-rejects `hosts=["*"]` divergence.

**Architecture:** New validated value types in `tau-domain` (`no_std`). The `Capability` flat-wire serde (hand-written) parses `hosts = "any"` | `[...]` and rejects `*` at parse. The lattice uses `HostSet::subsumes` + methods inclusion. The proxy gains a typed `HostAllow { Any, Exact(Vec<String>) }` pass-all mode reachable only from `HostSet::Any`, and case-folds runtime host matches. `derive_host` is deleted; `[allow.mcp.*].hosts` becomes required.

**Tech Stack:** Rust (workspace, 8 crates), serde (hand-written + derived), schemars, tokio (proxy), thiserror at boundaries, `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-07-18-hostset-exact-plus-typed-any-design.md`

## Global Constraints

- **CARGO RULES (CLAUDE.md) — every cargo command:** `timeout <secs> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p <crate>`. Scope to one crate with `-p`. Timeouts: test 300, build/check 180, clippy 240, fmt 30. Never bare `cargo`, never `--workspace`. Doctests: `cargo test -p <crate> --doc`.
- **`tau-domain` is `no_std` + `alloc`.** Use `alloc::string::String`, `alloc::collections::{BTreeSet, BTreeMap}`, `alloc::vec::Vec`. New serde code gated behind `#[cfg(feature = "serde")]`; schema behind `#[cfg(feature = "schema")]`. Guard: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-domain --no-default-features`.
- **`forbid(unsafe_code)`** holds in every crate touched.
- **Deferred (do NOT implement):** suffix wildcards `*.x.com`, IPv6 literal hosts `[::1]`, runtime method enforcement in the proxy.
- **Single PR**, `feat/hostset-exact-typed-any` branch, ordered commits. Workspace compiles green only at the end of the consumer commits — expected; CI gates on the merged result.
- **Absent vs empty:** `methods` absent = `None` = all methods; `methods = []` = `Some(∅)` = deny all. Never `unwrap_or_default`.
- **ADR number: 0062.** Conventional commits, imperative, scoped.

---

## File Structure

- **Create** `crates/tau-domain/src/package/host.rs` — `HostName`, `HostSet`, `HttpMethod` + parse/validation + errors + serde impls + unit tests. One responsibility: validated network-host/method value types.
- **Modify** `crates/tau-domain/src/package/mod.rs` — declare `pub mod host;` + re-export.
- **Modify** `crates/tau-domain/src/lib.rs` — re-export `HostName, HostSet, HttpMethod`.
- **Modify** `crates/tau-domain/src/package/capability.rs` — `Http` variant field types + the three hand-written serde impls + `shape_tests`.
- **Modify** `crates/tau-domain/src/fixtures.rs` — `cap_net_http` builder.
- **Modify** `crates/tau-pkg/src/capability_override/subset.rs` — lattice `Http` arm + tests.
- **Modify** `crates/tau-pkg/src/project/allow.rs` — delete `derive_host`, require MCP hosts.
- **Modify** `crates/tau-pkg/src/bundle/build.rs` — `HostSet → Vec<String>` for the bundle summary.
- **Modify** `crates/tau-sandbox-proxy/src/lib.rs` — `HostAllow`, `spawn_proxy` signature, pass-all + case-fold.
- **Modify** adapters: `tau-sandbox-native/src/strict.rs`, `tau-sandbox-darwin/src/lib.rs`, `tau-sandbox-container/src/runner.rs`, `tau-sandbox-windows/src/lib.rs`.
- **Modify** `crates/tau-runtime-core/src/orchestration/skill_resolve.rs` — test helper + assertion (test-only).
- **Create** `docs/decisions/0062-one-host-semantic-exact-typed-any.md`; **Modify** `docs/SUMMARY.md`, `capability.rs:137` doc comment.
- **Create/Modify** end-to-end divergence test in `crates/tau-cli/tests/`.

---

## Task 1: `HostName` value type

**Files:**
- Create: `crates/tau-domain/src/package/host.rs`
- Modify: `crates/tau-domain/src/package/mod.rs`, `crates/tau-domain/src/lib.rs`

**Interfaces:**
- Produces: `HostName` (`pub fn parse(&str) -> Result<HostName, HostNameError>`, `pub fn as_str(&self) -> &str`), `HostNameError` (Display). `Ord`/`Eq`/`Clone`/`Debug` derived on inner `String`.

- [ ] **Step 1: Create `host.rs` with `HostName` + tests (failing — module not wired yet)**

```rust
//! Validated network host/method value types shared by `net.http`
//! capabilities, the capability lattice, and the sandbox proxy.
//!
//! One semantic end-to-end: hosts are bare lowercase hostnames (optional
//! port) or the typed [`HostSet::Any`] sentinel — never a URL, scheme, glob,
//! or IP-with-brackets. Suffix wildcards and IPv6 literals are deliberately
//! deferred (additive later).

use alloc::collections::BTreeSet;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// A validated bare hostname with an optional `:port`.
///
/// Invariant (guaranteed by [`HostName::parse`]): ASCII, lowercase, labels of
/// `[a-z0-9-]` separated by `.`, optional trailing `:<port>` (1..=65535); no
/// scheme, `@`, `/`, `[`, `]`, `*`, or whitespace.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HostName(String);

/// Why a string is not a valid [`HostName`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostNameError {
    /// Contained a `*` (globs/`any` are not hostnames).
    Wildcard,
    /// Looked like a URL (scheme `://`, `@`, or `/`).
    UrlShaped,
    /// Contained `[` or `]` (IPv6 literal — not yet supported).
    BracketedIp,
    /// Empty host, empty label, or whitespace.
    Empty,
    /// A label held a character outside `[a-z0-9-]`.
    BadChar(char),
    /// The `:port` suffix was absent-digits or out of 1..=65535.
    BadPort,
}

impl fmt::Display for HostNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostNameError::Wildcard => {
                write!(f, "wildcards are not hostnames; write hosts = \"any\", or (suffix wildcards) enumerate the hosts")
            }
            HostNameError::UrlShaped => write!(f, "write the bare host, not a URL"),
            HostNameError::BracketedIp => write!(f, "IPv6 literal hosts are not yet supported"),
            HostNameError::Empty => write!(f, "empty host or label"),
            HostNameError::BadChar(c) => write!(f, "invalid character {c:?} in host label"),
            HostNameError::BadPort => write!(f, "port must be an integer in 1..=65535"),
        }
    }
}

impl HostName {
    /// Parse and case-fold. `A.COM` → `a.com` (accept-and-fold, never reject
    /// on case). See the module docs for the full accept/reject contract.
    pub fn parse(s: &str) -> Result<HostName, HostNameError> {
        if s.contains('*') {
            return Err(HostNameError::Wildcard);
        }
        if s.contains("://") || s.contains('@') || s.contains('/') {
            return Err(HostNameError::UrlShaped);
        }
        if s.contains('[') || s.contains(']') {
            return Err(HostNameError::BracketedIp);
        }
        if s.is_empty() || s.chars().any(|c| c.is_whitespace()) {
            return Err(HostNameError::Empty);
        }
        let folded = s.to_ascii_lowercase();
        // Split optional :port (at most one ':').
        let (host, port) = match folded.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (folded.as_str(), None),
        };
        if let Some(p) = port {
            match p.parse::<u32>() {
                Ok(n) if (1..=65535).contains(&n) => {}
                _ => return Err(HostNameError::BadPort),
            }
        }
        if host.is_empty() {
            return Err(HostNameError::Empty);
        }
        for label in host.split('.') {
            if label.is_empty() {
                return Err(HostNameError::Empty);
            }
            for c in label.chars() {
                if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
                    return Err(HostNameError::BadChar(c));
                }
            }
        }
        Ok(HostName(folded))
    }

    /// The canonical (lowercase) host string, including any `:port`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod host_name_tests {
    use super::*;

    #[test]
    fn accepts_plain_and_port_and_punycode() {
        for ok in ["api.anthropic.com", "localhost:8080", "b.io:8080", "xn--nxasmq6b.com"] {
            assert!(HostName::parse(ok).is_ok(), "should accept {ok}");
        }
    }

    #[test]
    fn folds_case() {
        assert_eq!(HostName::parse("A.COM").unwrap().as_str(), "a.com");
    }

    #[test]
    fn rejects_wildcards_urls_ipv6_paths_at_users() {
        assert_eq!(HostName::parse("*"), Err(HostNameError::Wildcard));
        assert_eq!(HostName::parse("*.a.com"), Err(HostNameError::Wildcard));
        assert_eq!(HostName::parse("https://a.com"), Err(HostNameError::UrlShaped));
        assert_eq!(HostName::parse("a.com/path"), Err(HostNameError::UrlShaped));
        assert_eq!(HostName::parse("user@a.com"), Err(HostNameError::UrlShaped));
        assert_eq!(HostName::parse("[::1]:8080"), Err(HostNameError::BracketedIp));
        assert_eq!(HostName::parse(""), Err(HostNameError::Empty));
        assert!(matches!(HostName::parse("bad_host"), Err(HostNameError::BadChar('_'))));
        assert_eq!(HostName::parse("a.com:0"), Err(HostNameError::BadPort));
        assert_eq!(HostName::parse("a.com:99999"), Err(HostNameError::BadPort));
    }
}
```

- [ ] **Step 2: Wire the module + re-exports**

In `crates/tau-domain/src/package/mod.rs` add after `pub mod capability;`:

```rust
pub mod host;
```

and add to the `pub use` block near `pub use capability::{…};`:

```rust
pub use host::{HostName, HostNameError, HostSet, HttpMethod, HttpMethodError};
```

In `crates/tau-domain/src/lib.rs`, add `HostName, HostSet, HttpMethod` to the existing `pub use package::{ … };` list (alphabetically, near `GitLocation`):

```rust
    GitLocation, HostName, HostSet, HttpMethod, NetCapability, PackageDep, PackageId, PackageKind,
```

(`HostSet`/`HttpMethod` don't exist until Tasks 2–3 — this block stays broken until Step of Task 3; that's fine, we compile at the end of Task 3.)

- [ ] **Step 3: Compile-check just `HostName` in isolation first**

Temporarily narrow the re-exports to only `HostName, HostNameError` (drop `HostSet, HttpMethod, HttpMethodError`) so the crate compiles now:

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-domain host_name_tests`
Expected: PASS (all `host_name_tests`).

- [ ] **Step 4: Commit**

```bash
git add crates/tau-domain/src/package/host.rs crates/tau-domain/src/package/mod.rs
git commit -m "feat(tau-domain): add validated HostName value type"
```

---

## Task 2: `HttpMethod` value type

**Files:** Modify `crates/tau-domain/src/package/host.rs`

**Interfaces:**
- Produces: `enum HttpMethod { Get, Head, Post, Put, Delete, Connect, Options, Trace, Patch }` (`Ord`/`Eq`/`Copy`/`Clone`/`Debug`), `pub fn parse(&str) -> Result<HttpMethod, HttpMethodError>`, `pub fn as_str(&self) -> &'static str` (UPPER), `HttpMethodError` (Display).

- [ ] **Step 1: Append `HttpMethod` + tests to `host.rs`**

```rust
/// One of the 9 standard HTTP verbs. Obscure/extension verbs (PROPFIND, …)
/// are a deliberate not-yet — additive later, like suffix wildcards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HttpMethod {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Connect,
    Options,
    Trace,
    Patch,
}

/// An unrecognized HTTP method token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpMethodError(pub String);

impl fmt::Display for HttpMethodError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown HTTP method {:?} (expected one of GET, HEAD, POST, PUT, DELETE, CONNECT, OPTIONS, TRACE, PATCH)",
            self.0
        )
    }
}

impl HttpMethod {
    /// Parse case-insensitively; canonical output is uppercase.
    pub fn parse(s: &str) -> Result<HttpMethod, HttpMethodError> {
        Ok(match s.to_ascii_uppercase().as_str() {
            "GET" => HttpMethod::Get,
            "HEAD" => HttpMethod::Head,
            "POST" => HttpMethod::Post,
            "PUT" => HttpMethod::Put,
            "DELETE" => HttpMethod::Delete,
            "CONNECT" => HttpMethod::Connect,
            "OPTIONS" => HttpMethod::Options,
            "TRACE" => HttpMethod::Trace,
            "PATCH" => HttpMethod::Patch,
            _ => return Err(HttpMethodError(s.to_string())),
        })
    }

    /// The canonical uppercase verb.
    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Head => "HEAD",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Connect => "CONNECT",
            HttpMethod::Options => "OPTIONS",
            HttpMethod::Trace => "TRACE",
            HttpMethod::Patch => "PATCH",
        }
    }
}

#[cfg(test)]
mod http_method_tests {
    use super::*;

    #[test]
    fn parses_case_insensitively_and_canonicalizes() {
        assert_eq!(HttpMethod::parse("get").unwrap(), HttpMethod::Get);
        assert_eq!(HttpMethod::parse("PoSt").unwrap().as_str(), "POST");
    }

    #[test]
    fn rejects_unknown_verb() {
        assert_eq!(HttpMethod::parse("GTE"), Err(HttpMethodError("GTE".into())));
    }
}
```

- [ ] **Step 2: Run**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-domain http_method_tests`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-domain/src/package/host.rs
git commit -m "feat(tau-domain): add typed HttpMethod (9 standard verbs)"
```

---

## Task 3: `HostSet` + `subsumes`

**Files:** Modify `crates/tau-domain/src/package/host.rs`, `crates/tau-domain/src/package/mod.rs` (restore full re-export)

**Interfaces:**
- Produces: `enum HostSet { Any, Exact(BTreeSet<HostName>) }` (`Clone`/`Debug`/`PartialEq`/`Eq`), `pub fn subsumes(&self, child: &HostSet) -> bool`, `pub fn is_any(&self) -> bool`, `pub fn exact_hosts(&self) -> Vec<String>` (sorted `as_str`s; empty for `Any`).

- [ ] **Step 1: Append `HostSet` + tests to `host.rs`**

```rust
/// The host ceiling of a `net.http` capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostSet {
    /// Any host (authored `hosts = "any"`). Widest ceiling.
    Any,
    /// Exactly these hosts (authored `hosts = ["a.com", …]`).
    Exact(BTreeSet<HostName>),
}

impl HostSet {
    /// Ceiling subsumption: `self ⊇ child`.
    /// `Any` ⊇ everything; `Exact(p)` ⊇ `Exact(c)` ⟺ `c ⊆ p`; `Exact` ⊉ `Any`.
    pub fn subsumes(&self, child: &HostSet) -> bool {
        match (self, child) {
            (HostSet::Any, _) => true,
            (HostSet::Exact(_), HostSet::Any) => false,
            (HostSet::Exact(p), HostSet::Exact(c)) => c.is_subset(p),
        }
    }

    /// True iff this is the `Any` sentinel.
    pub fn is_any(&self) -> bool {
        matches!(self, HostSet::Any)
    }

    /// Sorted canonical host strings; empty for `Any`.
    pub fn exact_hosts(&self) -> Vec<String> {
        match self {
            HostSet::Any => Vec::new(),
            HostSet::Exact(set) => set.iter().map(|h| h.as_str().to_string()).collect(),
        }
    }
}

#[cfg(test)]
mod host_set_tests {
    use super::*;

    fn exact(hosts: &[&str]) -> HostSet {
        HostSet::Exact(hosts.iter().map(|h| HostName::parse(h).unwrap()).collect())
    }

    #[test]
    fn subsumes_truth_table() {
        assert!(HostSet::Any.subsumes(&exact(&["a.com"])));
        assert!(HostSet::Any.subsumes(&HostSet::Any));
        assert!(!exact(&["a.com"]).subsumes(&HostSet::Any)); // Exact ⊉ Any
        assert!(exact(&["a.com", "b.com"]).subsumes(&exact(&["a.com"])));
        assert!(!exact(&["a.com"]).subsumes(&exact(&["a.com", "b.com"])));
    }

    #[test]
    fn exact_hosts_are_sorted_and_folded() {
        assert_eq!(exact(&["B.com", "a.com"]).exact_hosts(), vec!["a.com", "b.com"]);
        assert!(HostSet::Any.exact_hosts().is_empty());
    }
}
```

- [ ] **Step 2: Restore full re-export in `mod.rs`** (it already lists all five names from Task 1 Step 2). Confirm `lib.rs` compiles with `HostSet`/`HttpMethod` now present.

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-domain -E 'test(host_name_tests) + test(http_method_tests) + test(host_set_tests)'`
Expected: PASS. Then no_std guard:
Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-domain --no-default-features`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-domain/src/package/host.rs crates/tau-domain/src/package/mod.rs crates/tau-domain/src/lib.rs
git commit -m "feat(tau-domain): add HostSet with Any/Exact subsumption"
```

---

## Task 4: swap `NetCapability::Http` field types + serde

This task is atomic: the field-type change breaks `tau-domain`'s own serde + tests, so the serde impls, `fixtures.rs`, and `shape_tests` move together. `tau-domain` must be green at the end.

**Files:** Modify `crates/tau-domain/src/package/capability.rs`, `crates/tau-domain/src/fixtures.rs`, `crates/tau-domain/src/package/host.rs` (add serde impls for the new types).

**Interfaces:**
- Consumes: `HostName`, `HostSet`, `HttpMethod` from Tasks 1–3.
- Produces: `Http { hosts: HostSet, methods: Option<BTreeSet<HttpMethod>> }`; `fixtures::cap_net_http(hosts: &[&str], methods: &[&str]) -> Capability` (unchanged signature; `["any"]` → `HostSet::Any`, empty `methods` → `None`).

- [ ] **Step 1: serde impls for the new types (in `host.rs`, `#[cfg(feature = "serde")]`)**

`NetCapability` still `#[derive(Serialize, Deserialize)]` (capability.rs:132), so the new field types need serde. `HostName`/`HttpMethod` deserialize through their validating `parse` (single parse path, no bypass); `HostSet` derives. Append to `host.rs`:

```rust
#[cfg(feature = "serde")]
mod host_serde {
    use super::*;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    impl Serialize for HostName {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            s.serialize_str(self.as_str())
        }
    }
    impl<'de> Deserialize<'de> for HostName {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let s = String::deserialize(d)?;
            HostName::parse(&s).map_err(serde::de::Error::custom)
        }
    }

    impl Serialize for HttpMethod {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            s.serialize_str(self.as_str())
        }
    }
    impl<'de> Deserialize<'de> for HttpMethod {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let s = String::deserialize(d)?;
            HttpMethod::parse(&s).map_err(serde::de::Error::custom)
        }
    }

    // HostSet derives via HostName's impls; a small manual impl keeps the
    // `Any`/`Exact` shape explicit for the vestigial NetCapability derive path.
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum HostSetRepr {
        Any,
        Exact(BTreeSet<HostName>),
    }
    impl Serialize for HostSet {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            match self {
                HostSet::Any => HostSetRepr::Any.serialize(s),
                HostSet::Exact(set) => HostSetRepr::Exact(set.clone()).serialize(s),
            }
        }
    }
    impl<'de> Deserialize<'de> for HostSet {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            Ok(match HostSetRepr::deserialize(d)? {
                HostSetRepr::Any => HostSet::Any,
                HostSetRepr::Exact(set) => HostSet::Exact(set),
            })
        }
    }
}
```

- [ ] **Step 2: change the `Http` variant (capability.rs:136-141)**

Replace:

```rust
    Http {
        /// Allowed hosts (exact match or glob).
        hosts: Vec<String>,
        /// Allowed HTTP methods (uppercase by convention, e.g. `["GET", "POST"]`).
        methods: Vec<String>,
    },
```

with:

```rust
    Http {
        /// Allowed hosts: exact lowercase hostnames or the typed `Any`
        /// (authored `hosts = "any"`). Suffix wildcards are not yet supported.
        hosts: HostSet,
        /// Allowed HTTP methods. `None` = all methods; `Some(set)` = only those.
        methods: Option<BTreeSet<HttpMethod>>,
    },
```

Add imports at the top of `capability.rs` (near line 12):

```rust
use alloc::collections::BTreeSet;
use crate::package::host::{HostName, HostSet, HttpMethod};
```

- [ ] **Step 3: `RawCapability` fields + the flat `hosts`/`methods` parse (capability.rs:396-398, 425-427)**

In `RawCapability`, replace the `hosts`/`methods` fields with a wildcard-aware helper:

```rust
        #[serde(default)]
        hosts: Option<RawHosts>,
        #[serde(default)]
        methods: Option<Vec<String>>,
```

Add the helper enum inside `mod capability_de` (above `RawCapability`):

```rust
    /// `hosts` is authored as the exact string `"any"` OR a list of host
    /// strings. Untagged so it works in both TOML (manifests) and JSON (the
    /// `[allow]` bridge).
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum RawHosts {
        Str(String),
        List(Vec<String>),
    }
```

In the `"net.http"` deserialize arm (currently 425-428), replace:

```rust
                "net.http" => Capability::Network(NetCapability::Http {
                    hosts: raw.hosts.unwrap_or_default(),
                    methods: raw.methods.unwrap_or_default(),
                }),
```

with (using `serde::de::Error` — `D::Error` is in scope as the fn's error type):

```rust
                "net.http" => {
                    let hosts = parse_hosts_field(raw.hosts)?;
                    let methods = parse_methods_field(raw.methods)?;
                    Capability::Network(NetCapability::Http { hosts, methods })
                }
```

Add these two free fns inside `mod capability_de`:

```rust
    fn parse_hosts_field<E: serde::de::Error>(raw: Option<RawHosts>) -> Result<HostSet, E> {
        match raw {
            None => Ok(HostSet::Exact(alloc::collections::BTreeSet::new())),
            Some(RawHosts::Str(s)) if s == "any" => Ok(HostSet::Any),
            Some(RawHosts::Str(s)) => Err(E::custom(alloc::format!(
                "net.http hosts: bare string {s:?} is not valid; write hosts = \"any\" or a list of hosts"
            ))),
            Some(RawHosts::List(list)) => {
                let mut set = alloc::collections::BTreeSet::new();
                for h in list {
                    set.insert(HostName::parse(&h).map_err(|e| {
                        E::custom(alloc::format!("net.http host {h:?}: {e}"))
                    })?);
                }
                Ok(HostSet::Exact(set))
            }
        }
    }

    fn parse_methods_field<E: serde::de::Error>(
        raw: Option<Vec<String>>,
    ) -> Result<Option<alloc::collections::BTreeSet<HttpMethod>>, E> {
        match raw {
            None => Ok(None),
            Some(list) => {
                let mut set = alloc::collections::BTreeSet::new();
                for m in list {
                    set.insert(HttpMethod::parse(&m).map_err(E::custom)?);
                }
                Ok(Some(set))
            }
        }
    }
```

> Note the intentional absent-vs-empty semantics: absent `hosts` → `Exact(∅)` (grant nothing, matches today's `unwrap_or_default` empty-vec); absent `methods` → `None` (all); `methods = []` → `Some(∅)` (deny all).

- [ ] **Step 4: serialize arm (capability.rs:515-521)**

Replace:

```rust
                Capability::Network(NetCapability::Http { hosts, methods }) => {
                    let mut m = s.serialize_map(Some(3))?;
                    m.serialize_entry("kind", "net.http")?;
                    m.serialize_entry("hosts", hosts)?;
                    m.serialize_entry("methods", methods)?;
                    m.end()
                }
```

with (Any → `"any"`; Exact → sorted list; `methods == None` omits the key):

```rust
                Capability::Network(NetCapability::Http { hosts, methods }) => {
                    let len = 2 + usize::from(methods.is_some());
                    let mut m = s.serialize_map(Some(len))?;
                    m.serialize_entry("kind", "net.http")?;
                    match hosts {
                        HostSet::Any => m.serialize_entry("hosts", "any")?,
                        HostSet::Exact(set) => {
                            let list: Vec<&str> = set.iter().map(|h| h.as_str()).collect();
                            m.serialize_entry("hosts", &list)?;
                        }
                    }
                    if let Some(set) = methods {
                        let list: Vec<&str> = set.iter().map(|v| v.as_str()).collect();
                        m.serialize_entry("methods", &list)?;
                    }
                    m.end()
                }
```

- [ ] **Step 5: JSON schema (capability.rs:606-616)**

Replace the `net.http` schema object with `hosts` = `oneOf[const "any", array]`, `methods` optional enum, `required` drops `methods`:

```rust
                // net.http
                {
                    "type": "object",
                    "required": ["kind", "hosts"],
                    "additionalProperties": false,
                    "properties": {
                        "kind":  { "const": "net.http" },
                        "hosts": {
                            "oneOf": [
                                { "const": "any" },
                                { "type": "array", "items": { "type": "string" } }
                            ]
                        },
                        "methods": {
                            "type": "array",
                            "items": {
                                "type": "string",
                                "enum": ["GET","HEAD","POST","PUT","DELETE","CONNECT","OPTIONS","TRACE","PATCH"]
                            }
                        }
                    }
                },
```

- [ ] **Step 6: `fixtures::cap_net_http` (fixtures.rs:145-149)**

Replace:

```rust
pub fn cap_net_http(hosts: &[&str], methods: &[&str]) -> Capability {
    Capability::Network(NetCapability::Http {
        hosts: hosts.iter().map(|s| s.to_string()).collect(),
        methods: methods.iter().map(|s| s.to_string()).collect(),
    })
}
```

with (test-ergonomic: `["any"]` → `Any`; empty `methods` slice → `None`; wire `[]`=deny-all is exercised directly by serde tests, not here):

```rust
pub fn cap_net_http(hosts: &[&str], methods: &[&str]) -> Capability {
    use tau_domain::{HostName, HostSet, HttpMethod};
    let hosts = if hosts == ["any"] {
        HostSet::Any
    } else {
        HostSet::Exact(hosts.iter().map(|h| HostName::parse(h).expect("valid host")).collect())
    };
    let methods = if methods.is_empty() {
        None
    } else {
        Some(methods.iter().map(|m| HttpMethod::parse(m).expect("valid method")).collect())
    };
    Capability::Network(NetCapability::Http { hosts, methods })
}
```

> `fixtures.rs` imports: check the top of the file — it already `use`s `tau_domain::{…}` or `crate::…`. Match the existing style (the module uses `NetCapability` already, so add `HostName, HostSet, HttpMethod` to that import instead of the inline `use` if the file imports at module scope).

- [ ] **Step 7: fix `shape_tests` (capability.rs:727-729)**

Replace the `Http { hosts: vec![...], methods: vec![...] }` construction:

```rust
        let cap = Capability::Network(NetCapability::Http {
            hosts: HostSet::Exact([HostName::parse("api.example.com").unwrap()].into_iter().collect()),
            methods: Some([HttpMethod::Get].into_iter().collect()),
        });
```

- [ ] **Step 8: Add focused serde tests to `capability.rs` (or `host.rs`)**

```rust
    #[test]
    fn hosts_any_round_trips() {
        let c: Capability = serde_json::from_str(r#"{"kind":"net.http","hosts":"any"}"#).unwrap();
        assert!(matches!(&c, Capability::Network(NetCapability::Http { hosts, .. }) if hosts.is_any()));
        assert_eq!(serde_json::to_value(&c).unwrap()["hosts"], serde_json::json!("any"));
    }

    #[test]
    fn hosts_star_rejected_at_parse() {
        let e = serde_json::from_str::<Capability>(r#"{"kind":"net.http","hosts":["*"]}"#).unwrap_err();
        assert!(e.to_string().contains("any") || e.to_string().to_lowercase().contains("wildcard"), "got: {e}");
    }

    #[test]
    fn methods_absent_is_none_empty_is_some_empty() {
        let all: Capability = serde_json::from_str(r#"{"kind":"net.http","hosts":["a.com"]}"#).unwrap();
        let none: Capability = serde_json::from_str(r#"{"kind":"net.http","hosts":["a.com"],"methods":[]}"#).unwrap();
        let m = |c: &Capability| match c { Capability::Network(NetCapability::Http { methods, .. }) => methods.clone(), _ => unreachable!() };
        assert_eq!(m(&all), None);
        assert_eq!(m(&none), Some(alloc::collections::BTreeSet::new()));
    }

    #[test]
    fn unknown_method_rejected() {
        assert!(serde_json::from_str::<Capability>(r#"{"kind":"net.http","hosts":["a.com"],"methods":["GTE"]}"#).is_err());
    }

    #[test]
    fn exact_hosts_serialize_byte_stable_sorted() {
        // hash-stability: already-lowercase input serializes identically regardless of input order.
        let a: Capability = serde_json::from_str(r#"{"kind":"net.http","hosts":["b.com","a.com"]}"#).unwrap();
        assert_eq!(serde_json::to_value(&a).unwrap()["hosts"], serde_json::json!(["a.com","b.com"]));
    }
```

- [ ] **Step 9: Run tau-domain green (incl. doctests) + no_std guard**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-domain`
Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-domain --doc`
Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-domain --no-default-features`
Expected: all PASS/clean.

- [ ] **Step 10: Commit**

```bash
git add crates/tau-domain/
git commit -m "feat(tau-domain): net.http hosts=HostSet, methods=Option<BTreeSet<HttpMethod>>"
```

---

## Task 5: lattice `Http` arm (`subset.rs`)

**Files:** Modify `crates/tau-pkg/src/capability_override/subset.rs`

**Interfaces:**
- Consumes: `HostSet::subsumes`, `HttpMethod`, the new `Http` fields.
- Produces: (internal) `Http` arm now checks hosts via `HostSet::subsumes` and methods via set inclusion (`None` = full set).

- [ ] **Step 1: Update failing tests first (TDD — encode the new semantics)**

In `subset.rs` tests, update the four `net.http` tests to the new fixture shape and add the methods-now-checked witness. Replace `net_http_host_in_ceiling_ok_method_diff_ignored` (:339-348) with:

```rust
    #[test]
    fn net_http_host_and_method_subset_ok() {
        let child = vec![cap(r#"{"kind":"net.http","hosts":["api.x.com"],"methods":["GET"]}"#)];
        let parent = vec![cap(r#"{"kind":"net.http","hosts":["api.x.com"],"methods":["GET","POST"]}"#)];
        assert!(capability_set_subset(&child, &parent).is_ok());
    }

    #[test]
    fn net_http_method_exceeds_ceiling_rejected() {
        // The audit witness: GET-only parent ⊉ POST child.
        let child = vec![cap(r#"{"kind":"net.http","hosts":["api.x.com"],"methods":["POST"]}"#)];
        let parent = vec![cap(r#"{"kind":"net.http","hosts":["api.x.com"],"methods":["GET"]}"#)];
        assert!(capability_set_subset(&child, &parent).is_err());
    }

    #[test]
    fn net_http_child_all_methods_under_capped_parent_rejected() {
        // child methods absent (=all) but parent restricts → violation.
        let child = vec![cap(r#"{"kind":"net.http","hosts":["api.x.com"]}"#)];
        let parent = vec![cap(r#"{"kind":"net.http","hosts":["api.x.com"],"methods":["GET"]}"#)];
        assert!(capability_set_subset(&child, &parent).is_err());
    }

    #[test]
    fn net_http_any_child_under_exact_parent_rejected() {
        let child = vec![cap(r#"{"kind":"net.http","hosts":"any"}"#)];
        let parent = vec![cap(r#"{"kind":"net.http","hosts":["api.x.com"]}"#)];
        assert!(capability_set_subset(&child, &parent).is_err());
    }
```

Keep `net_http_host_outside_ceiling_rejected` (:350) and `multi_parent_net_http_union_admits_host_from_second_parent` (:475) but change their `methods` to be equal/absent so only the host axis is under test — set both to no `methods` key.

- [ ] **Step 2: Run — verify they fail to compile/assert**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg -E 'test(net_http)'`
Expected: FAIL (old host-only arm still uses `gather_hosts`/`string_set_subset`; the type won't even compile until Step 3).

- [ ] **Step 3: Rewrite the `Http` arm (:122-125) + delete `gather_hosts` (:196-205)**

Replace the `Http` arm in `cap_subset_against`:

```rust
        Capability::Network(NetCapability::Http { hosts, methods }) => {
            // Host axis: child HostSet must be subsumed by the union of parent
            // HostSets. Any parent that is `Any` subsumes everything.
            let host_ok = parents.iter().any(|p| matches!(
                p,
                Capability::Network(NetCapability::Http { hosts: ph, .. }) if ph.subsumes(hosts)
            ));
            if !host_ok {
                let offender = match hosts {
                    tau_domain::HostSet::Any => "any".to_string(),
                    tau_domain::HostSet::Exact(_) => hosts
                        .exact_hosts()
                        .into_iter()
                        .find(|h| !parents.iter().any(|p| matches!(
                            p,
                            Capability::Network(NetCapability::Http { hosts: ph, .. })
                                if ph.subsumes(&tau_domain::HostSet::Exact(
                                    core::iter::once(tau_domain::HostName::parse(h).unwrap()).collect()
                                ))
                        )))
                        .unwrap_or_else(|| "host".to_string()),
                };
                return Err((offender, "host not in ceiling".into()));
            }
            // Method axis: child ⊆ some parent; None = the full set.
            let method_ok = parents.iter().any(|p| matches!(
                p,
                Capability::Network(NetCapability::Http { methods: pm, .. })
                    if methods_subset(methods.as_ref(), pm.as_ref())
            ));
            if !method_ok {
                return Err(("methods".to_string(), "methods exceed ceiling".into()));
            }
            Ok(())
        }
```

Delete `gather_hosts` (:196-205). Add a small helper near the other free fns:

```rust
/// `child ⊆ parent` over HTTP methods, where `None` denotes the full method
/// set. `None` child is admitted only by a `None` parent.
fn methods_subset(
    child: Option<&std::collections::BTreeSet<tau_domain::HttpMethod>>,
    parent: Option<&std::collections::BTreeSet<tau_domain::HttpMethod>>,
) -> bool {
    match (child, parent) {
        (_, None) => true,      // parent = all methods
        (None, Some(_)) => false, // child = all, parent restricts
        (Some(c), Some(p)) => c.is_subset(p),
    }
}
```

Import `HostName` where needed: add `HostName, HostSet, HttpMethod` to the `use tau_domain::{…}` at the top of `subset.rs` (currently imports `AgentCapability, Capability, FsCapability, NetCapability, ProcessCapability, SkillCapability`).

> The multi-parent host union is preserved: `parents.iter().any(... subsumes ...)`. An `Exact` child host is admitted if *any* parent's `HostSet` subsumes it (matching the old union-of-hosts behavior), and any `Any` parent subsumes all.

- [ ] **Step 4: Run tau-pkg subset tests green**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg -E 'package(tau-pkg) and test(subset)'`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/capability_override/subset.rs
git commit -m "feat(tau-pkg): lattice checks HostSet subsumption + method inclusion"
```

---

## Task 6: `[allow]` — delete `derive_host`, require MCP hosts

**Files:** Modify `crates/tau-pkg/src/project/allow.rs`

**Interfaces:**
- Produces: `[allow.mcp.<name>].hosts` required (empty → error); `derive_host` removed. `net.http` cap bridge unchanged (inherits HostSet parse via the domain deserializer).

- [ ] **Step 1: Update tests first**

Replace `mcp_url_derives_host_when_absent` (:340-353) with:

```rust
    #[test]
    fn mcp_absent_hosts_now_rejected() {
        let raw = allow_from(
            r#"
[mcp.weather]
url = "https://api.weather.com/mcp"
"#,
        );
        let err = validate_allow(raw).unwrap_err();
        assert!(format!("{err}").contains("hosts"), "got: {err}");
    }
```

Update `raw_caps_bridge_into_capability_vec` (:255-262) host assertion to the new type:

```rust
        assert!(cfg.ceiling.iter().any(|c| matches!(
            c,
            Capability::Network(NetCapability::Http { hosts, .. })
                if hosts.exact_hosts() == vec!["api.weather.com".to_string()]
        )));
```

Add a bridge test for `hosts = "any"`:

```rust
    #[test]
    fn allow_net_http_any_bridges_to_hostset_any() {
        let raw = allow_from(r#""net.http" = { hosts = "any" }"#);
        let cfg = validate_allow(raw).expect("validate");
        assert!(cfg.ceiling.iter().any(|c| matches!(
            c, Capability::Network(NetCapability::Http { hosts, .. }) if hosts.is_any()
        )));
    }
```

Import `NetCapability` is already in the test `use` (line 238).

- [ ] **Step 2: Run — fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg -E 'test(mcp_absent_hosts_now_rejected) + test(allow_net_http_any_bridges_to_hostset_any)'`
Expected: FAIL (`derive_host` still derives; new test names missing).

- [ ] **Step 3: Delete `derive_host` (:153-167) and require MCP hosts (:197-208)**

Remove the entire `derive_host` fn. Replace the MCP host block:

```rust
        let hosts = if m.hosts.is_empty() {
            let host = derive_host(&m.url).ok_or_else(|| {
                err(format!(
                    "[allow.mcp.{name}]: cannot derive host from url {:?}",
                    m.url
                ))
            })?;
            vec![host]
        } else {
            m.hosts
        };
```

with:

```rust
        if m.hosts.is_empty() {
            return Err(err(format!(
                "[allow.mcp.{name}]: hosts must be non-empty \
                 (URL-derived hosts are no longer supported; author them explicitly)"
            )));
        }
        let hosts = m.hosts;
```

Also delete the now-invalid `mcp_unparseable_url_rejected` test (:383-393) — an unparseable URL is no longer host-derived, so it's no longer rejected on that basis; replace it with an assertion that a URL + explicit hosts is accepted (that's already `mcp_explicit_hosts_preserved`, so just delete `mcp_unparseable_url_rejected`). Update the `UncheckedMcpAllow.hosts` doc comment (:46-48) to drop "empty = derive from `url`".

- [ ] **Step 4: Run tau-pkg allow tests green**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg -E 'test(allow) or test(mcp)'`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/project/allow.rs
git commit -m "feat(tau-pkg): require explicit [allow.mcp.*] hosts; delete derive_host"
```

---

## Task 7: proxy `HostAllow` + pass-all + case-fold

**Files:** Modify `crates/tau-sandbox-proxy/src/lib.rs` (keep `validate.rs` as-is — still rejects `*`, defense in depth).

**Interfaces:**
- Produces: `pub enum HostAllow { Any, Exact(Vec<String>) }`; `pub fn spawn_proxy(hosts: HostAllow) -> std::io::Result<ProxyHandle>`. Runtime host match case-folds; `Any` allows every CONNECT/HTTP target.

- [ ] **Step 1: Add the `HostAllow` type + host-match helper (top of `lib.rs`, platform-agnostic)**

```rust
/// Host egress policy for the proxy. `Any` = pass-all (reachable only from a
/// `HostSet::Any` capability); `Exact` = only these (pre-validated, lowercase)
/// hosts. Case-insensitive matching at runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostAllow {
    /// Allow every host (pass-all).
    Any,
    /// Allow exactly these hosts (case-insensitive).
    Exact(Vec<String>),
}

impl HostAllow {
    /// True iff `host` is permitted. Case-folds both sides.
    pub fn permits(&self, host: &str) -> bool {
        match self {
            HostAllow::Any => true,
            HostAllow::Exact(list) => list.iter().any(|h| h.eq_ignore_ascii_case(host)),
        }
    }
}
```

Export it: add `HostAllow` to the `pub use` at the top, or make it `pub` in-module (it's defined `pub` here, so just ensure it's reachable — no `pub use` needed for an item defined `pub` in `lib.rs`).

- [ ] **Step 2: Thread `HostAllow` through `spawn_proxy` → `accept_loop` → `handle_connection` → `handle_connect`/`handle_http`**

Change the signatures (all `#[cfg(unix)]`):

```rust
pub fn spawn_proxy(hosts: HostAllow) -> std::io::Result<ProxyHandle> {
    let (sock_dir, sock_path) = make_run_dir_and_sock_path()?;
    let listener = UnixListener::bind(&sock_path)?;
    let task = tokio::spawn(accept_loop(listener, hosts));
    Ok(ProxyHandle { sock_path, sock_dir, task })
}

async fn accept_loop(listener: UnixListener, hosts: HostAllow) {
    loop {
        match listener.accept().await {
            Ok((mut conn, _)) => {
                let hosts = hosts.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(&mut conn, &hosts).await {
                        tracing::warn!(error = %e, "proxy connection failed");
                    }
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "proxy accept failed");
                return;
            }
        }
    }
}
```

Change `handle_connection`, `handle_connect`, `handle_http` params from `allowed_hosts: &[String]` to `hosts: &HostAllow`, and replace both occurrences of:

```rust
    if !allowed_hosts.iter().any(|h| h == &req.host) {
```

with:

```rust
    if !hosts.permits(&req.host) {
```

- [ ] **Step 3: Update the proxy's own tests to the new signature + add pass-all/case-fold tests**

Every `spawn_proxy(vec!["…".to_string()])` becomes `spawn_proxy(HostAllow::Exact(vec!["…".to_string()]))`. Add:

```rust
    #[tokio::test]
    async fn pass_all_permits_any_host() {
        let h = spawn_proxy(HostAllow::Any).expect("spawn");
        let mut conn = UnixStream::connect(h.sock_path()).await.expect("connect");
        // Non-443 port still 400s, but the host is NOT 403'd under Any:
        conn.write_all(b"CONNECT anything.example.com:443 HTTP/1.1\r\n\r\n").await.expect("write");
        let mut resp = [0u8; 256];
        let n = conn.read(&mut resp).await.expect("read");
        let s = std::str::from_utf8(&resp[..n]).expect("utf8");
        assert!(!s.starts_with("HTTP/1.1 403"), "Any must not 403, got: {s}");
    }

    #[tokio::test]
    async fn exact_match_is_case_insensitive() {
        let h = spawn_proxy(HostAllow::Exact(vec!["allowed.example.com".to_string()])).expect("spawn");
        let mut conn = UnixStream::connect(h.sock_path()).await.expect("connect");
        conn.write_all(b"CONNECT ALLOWED.EXAMPLE.COM:443 HTTP/1.1\r\n\r\n").await.expect("write");
        let mut resp = [0u8; 256];
        let n = conn.read(&mut resp).await.expect("read");
        let s = std::str::from_utf8(&resp[..n]).expect("utf8");
        assert!(!s.starts_with("HTTP/1.1 403"), "case-folded host must not 403, got: {s}");
    }
```

- [ ] **Step 4: Run proxy crate green**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-sandbox-proxy`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-sandbox-proxy/src/lib.rs
git commit -m "feat(tau-sandbox-proxy): typed HostAllow with pass-all + case-insensitive match"
```

---

## Task 8: wire adapters + bundle summary + skill_resolve tests

**Files:** Modify `crates/tau-sandbox-native/src/strict.rs`, `crates/tau-sandbox-darwin/src/lib.rs`, `crates/tau-sandbox-container/src/runner.rs`, `crates/tau-sandbox-windows/src/lib.rs`, `crates/tau-pkg/src/bundle/build.rs`, `crates/tau-runtime-core/src/orchestration/skill_resolve.rs`.

**Interfaces:**
- Consumes: `HostSet` (`is_any`/`exact_hosts`), `tau_sandbox_proxy::{HostAllow, spawn_proxy, validate_hosts}`.

Each adapter uses the same map: collect a `HostAllow` across all `Http` caps (any `Any` ⇒ pass-all), validate only the `Exact` strings.

- [ ] **Step 1: native `strict.rs` (:393-407)**

Replace the collect-and-validate block with:

```rust
        // Collect a host policy across all Http capabilities. Any `HostSet::Any`
        // ⇒ pass-all; otherwise the union of exact hosts.
        let mut any = false;
        let mut exact: Vec<String> = Vec::new();
        for cap in &plan.capabilities {
            if let tau_domain::Capability::Network(tau_domain::NetCapability::Http { hosts, .. }) = cap {
                if hosts.is_any() { any = true; } else { exact.extend(hosts.exact_hosts()); }
            }
        }
        let policy = if any { tau_sandbox_proxy::HostAllow::Any } else { tau_sandbox_proxy::HostAllow::Exact(exact.clone()) };
        // Validate the exact list (defense in depth: rejects '*', non-loopback IPs).
        tau_sandbox_proxy::validate_hosts(&exact).map_err(|e| CapabilityError::Proxy {
            message: format!("host validation: {e}"),
        })?;
```

and change the `spawn_proxy(allowed_hosts)` call (:411) to `spawn_proxy(policy)`.

- [ ] **Step 2: darwin `lib.rs`** — the validate-only site (:85-96) and the spawn site (:165-173). For the validate site, use the `any`/`exact` collect above but only call `validate_hosts(&exact)` (skip when `exact` empty, preserving the existing `is_empty` guard). For the spawn site, build `policy` as in Step 1 and call `spawn_proxy(policy)`.

- [ ] **Step 3: container `runner.rs` (:99-111)** — same collect; `validate_hosts(&exact)` then `spawn_proxy(policy)`.

- [ ] **Step 4: windows `lib.rs` (:93-105)** — validate-only (spawn is `cfg(unix)`-gated and unreachable here): build `exact`, `validate_hosts(&exact)` guarded by `!exact.is_empty()`.

- [ ] **Step 5: bundle `build.rs` (:493-497)** — map `HostSet` into the string summary:

```rust
            Capability::Network(NetCapability::Http { hosts, .. }) => {
                let host_strs: Vec<String> = if hosts.is_any() {
                    vec!["any".to_string()]
                } else {
                    hosts.exact_hosts()
                };
                out.allow_net_http
                    .extend(e.allow_override.clone().unwrap_or(host_strs));
                out.deny_net_http.extend(e.deny.clone());
            }
```

> Bundle-hash note: for `Exact`, the emitted strings are identical to before (same hosts) → hashes stable. `Any` emits `["any"]` (new; no prior in-tree manifest used `*`).

- [ ] **Step 6: skill_resolve.rs test helper (:381-390) + assertion (:412)** — replace the `net_http` test helper body's `"hosts": hosts_json` (unchanged — it authors a list, still valid) but change the assertion `assert_eq!(hosts[0], "api.example.com")` (:412) to:

```rust
                assert_eq!(hosts.exact_hosts(), vec!["api.example.com".to_string()]);
```

- [ ] **Step 7: Build + test every touched crate**

```
for C in tau-sandbox-native tau-sandbox-darwin tau-sandbox-container tau-sandbox-windows tau-pkg tau-runtime-core; do
  timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p $C || break
done
```

> On macOS, `tau-sandbox-native` (Linux-only landlock) may only `cargo check`; run `cargo check -p tau-sandbox-native` there and rely on CI for its Linux tests. `tau-sandbox-windows` similarly checks on non-Windows.
Expected: PASS/clean per platform.

- [ ] **Step 8: Commit**

```bash
git add crates/tau-sandbox-native crates/tau-sandbox-darwin crates/tau-sandbox-container crates/tau-sandbox-windows crates/tau-pkg/src/bundle/build.rs crates/tau-runtime-core/src/orchestration/skill_resolve.rs
git commit -m "feat(sandbox): map HostSet -> HostAllow across adapters + bundle summary"
```

---

## Task 9: docs, ADR, migration sweep

**Files:** Create `docs/decisions/0062-one-host-semantic-exact-typed-any.md`; Modify `docs/SUMMARY.md`, `crates/tau-domain/src/package/capability.rs` (doc comment), migration targets.

- [ ] **Step 1: Fix the doc comment (capability.rs — the `hosts` field, formerly :137)**

Already changed in Task 4 Step 2 to "exact lowercase hostnames or the typed `Any`… Suffix wildcards are not yet supported." Verify it reads correctly.

- [ ] **Step 2: Write ADR 0062**

Create `docs/decisions/0062-one-host-semantic-exact-typed-any.md` following the repo's ADR template (status Accepted; context = the three-way divergence; decision = `HostSet` exact + typed `Any`, no derived hosts, typed `HttpMethod`, absent-vs-empty; consequences = build-accepts ⟺ run-enforces; deferred = suffix wildcards, IPv6 literals, runtime method enforcement). Reference the spec.

- [ ] **Step 3: SUMMARY.md entry** — add the ADR line under the decisions section (mirror the `0058` line's format).

- [ ] **Step 4: Migration sweep — `methods = []` and MCP docs**

- `crates/tau-cli/tests/cmd_build_mcp.rs:260` — `methods = []`: this asserts a bundle build; decide whether it meant "all" (drop key → `None`) or "deny all" (keep `[]`). Inspect the assertion around it; if it just needs *a* net.http cap, drop `methods` to mean all. Update expected bundle bytes/hash if asserted.
- `crates/tau-sandbox-native/src/light.rs:376` — `{ "kind": "net.http", "hosts": [], "methods": [] }`: `hosts: []` → `Exact(∅)`, `methods: []` → `Some(∅)`. If this fixture just exercises "an http cap exists", set `hosts` to `["example.com"]` and drop `methods`. Verify the test's intent.
- `[allow.mcp.*]` without `hosts` in `docs/superpowers/{plans,specs}/*.md` and `docs/superpowers/plans/vision-roadmap.md` — add explicit `hosts = [...]` to each example (they're prose examples; keep them valid).
- `docs/decisions/0019-per-host-network-filter.md:105` — add a one-line "Superseded in part by ADR-0062 (hosts are now `HostSet`; `\"*\"` is a parse error, `\"any\"` is the sentinel)."

- [ ] **Step 5: Build the book**

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build`
Expected: only `[INFO]` lines. Then `rm -rf docs/book`.

- [ ] **Step 6: Commit**

```bash
git add docs/ crates/tau-cli/tests/cmd_build_mcp.rs crates/tau-sandbox-native/src/light.rs
git commit -m "docs(adr-0062): one host semantic; migrate methods=[] fixtures + mcp examples"
```

> EPIC 1.6 hookup: the coarse-ceiling lint (branch `feat/epic-1.6-coarse-lint`, commit `57fe7c98`) should target `HostSet::Any` in `[allow]`, not a `"*"` string. Leave a note in that branch's design doc; do NOT implement it here unless 1.6 is trivial to fold in — out of this PR's scope.

---

## Task 10: end-to-end divergence test (the capstone)

**Files:** Create/Modify a test in `crates/tau-cli/tests/` (e.g. extend `cmd_build_mcp.rs` or add `hostset_divergence.rs`).

**Interfaces:** Consumes the whole stack (`tau check`/`tau build`/`tau run` or their library entrypoints).

- [ ] **Step 1: Write the divergence test**

Assert both directions of build-accepts ⟺ run-enforces:

```rust
// 1. hosts = "any" passes check AND build AND reaches the proxy in pass-all mode.
#[test]
fn hosts_any_passes_build_and_reaches_pass_all() {
    // Build a project/manifest with a net.http cap `hosts = "any"`.
    // Assert `tau check` (validate_allow / capability_set_subset) is Ok,
    // `tau build` produces a bundle, and the lowered CapabilityPlan carries
    // HostSet::Any (which the adapter maps to HostAllow::Any). Use the
    // library entrypoints the other tests in this file use.
}

// 2. hosts = ["*"] fails at PARSE (tau check), never reaching build.
#[test]
fn hosts_star_fails_at_parse() {
    let e = serde_json::from_str::<tau_domain::Capability>(
        r#"{"kind":"net.http","hosts":["*"]}"#,
    ).unwrap_err();
    assert!(e.to_string().to_lowercase().contains("any") || e.to_string().to_lowercase().contains("wildcard"));
}
```

> Follow the existing `cmd_build_mcp.rs` harness for constructing a project + invoking build; mirror its fixture setup. The first test proves build-accepts ⇒ run-enforces (pass-all reachable); the second proves the old escape hatch is now a parse error.

- [ ] **Step 2: Run**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli -E 'test(hosts_any) + test(hosts_star)'`
Expected: PASS.

- [ ] **Step 3: Full sweep of the two most-affected crates + fmt + clippy**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-domain
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg
timeout 30  env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo fmt --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-domain -p tau-pkg -p tau-sandbox-proxy
```
Expected: PASS/clean.

- [ ] **Step 4: Commit + open PR**

```bash
git add crates/tau-cli/tests/
git commit -m "test(tau-cli): end-to-end hosts=any build-accepts <=> run-enforces; '*' fails at parse"
git push -u origin feat/hostset-exact-typed-any
gh pr create --base main --title "feat: one host semantic — HostSet exact + typed any (ADR-0062)" --body "Implements docs/superpowers/specs/2026-07-18-hostset-exact-plus-typed-any-design.md. Closes the hosts=['*'] build-accepts-but-run-rejects divergence."
```

---

## Self-Review (completed)

- **Spec coverage:** HostName/HostSet/HttpMethod (T1–3), serde+schema+field swap (T4), lattice hosts+methods (T5), derive_host delete + MCP hosts required (T6), proxy pass-all+case-fold (T7), adapters+bundle+skill_resolve (T8), docs/ADR/migration (T9), end-to-end divergence + proxy pass-all/case-fold tests (T7/T10). All spec sections mapped.
- **Placeholder scan:** Task 2/9 ADR body and Task 10 project-construction reference existing repo templates/harnesses rather than inlining unknowable fixture scaffolding — the surrounding assertions are concrete. No `TBD`/`add error handling`/`similar to Task N`.
- **Type consistency:** `HostSet` methods (`subsumes`, `is_any`, `exact_hosts`), `HttpMethod::{parse,as_str}`, `HostAllow::{Any,Exact,permits}`, `methods_subset` used consistently across T4–T8.
