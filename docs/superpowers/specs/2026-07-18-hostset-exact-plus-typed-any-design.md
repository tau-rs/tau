# HostSet: one host semantic — exact hostnames + a typed `any`; no derived hosts

**Status:** approved (brainstorm) — ready for implementation plan
**Date:** 2026-07-18
**Branch target:** `main` @ `d678802d`
**ADR:** 0059 (0057 is taken by open PR #423; 0058 is control-flow)
**Origin:** handoff D4-B (`.context/attachments/55UnJ6/…`), verified against the tree 2026-07-18

## Problem

Three divergent host semantics ship today:

1. **Docs** claim hosts are "exact match or glob"
   (`crates/tau-domain/src/package/capability.rs:137`).
2. **The lattice** does exact string equality — `hosts=["*"]` is treated as the
   literal host `"*"` (`crates/tau-pkg/src/capability_override/subset.rs:122-125`
   via `string_set_subset` :269-276).
3. **The runtime proxy** rejects any `*` at spawn
   (`crates/tau-sandbox-proxy/src/validate.rs:26-38`, invoked from
   `crates/tau-sandbox-native/src/strict.rs:404-407` and the darwin / container /
   windows adapters).

Consequence: a manifest with `hosts = ["*"]` passes parse → lattice → `tau check`
→ `tau build`, then **deterministically fails at `tau run`**. Build-accepts but
run-rejects — the exact false-security divergence tau exists to prevent.

Two secondary defects:

- **No case folding.** `API.COM` and `api.com` are distinct strings through the
  whole stack.
- **Hand-rolled `derive_host`** (`crates/tau-pkg/src/project/allow.rs:156-167`)
  parses `https://user@evil.com/` → host `user@evil.com` and IPv6 URLs → `"["`.

## Decision (D4-B)

Hosts are **bare lowercase hostnames** or a **typed any-sentinel** — one semantic
from parse through lattice through proxy. Suffix wildcards (`*.x.com`) are
deliberately deferred (additive later). This spec also completes the long-standing
**methods** hole in the lattice (`subset.rs:122` ignores methods today; test
:339-348 documents it).

Three design questions raised during brainstorm and their resolutions:

- **Q1 — `derive_host` / MCP.** `derive_host` is *not* used by the `net.http`
  capability bridge; it derives the host ceiling for `[allow.mcp.<name>]` from the
  server `url`. **Resolution (A):** delete `derive_host`; make
  `[allow.mcp.<name>].hosts` **required**. No URL→host derivation anywhere in the
  capability stack. Absent `hosts` on an MCP entry is now an error.
- **Q2 — PR split.** Changing the `Http` variant's field type is atomic across the
  Cargo workspace; a "types-only PR 1" cannot compile alone. **Resolution (A):**
  one PR, logically-ordered commits (domain → consumers → docs).
- **Q3 — methods type & absent-vs-empty.** **Resolution (a+b):** a typed
  `HttpMethod` enum (9 standard verbs, parse-error on anything else);
  `methods` absent = `None` = *all methods*; `methods = []` = `Some(∅)` =
  *deny all methods*. Never conflate absent with empty (`unwrap_or_default` is the
  audit-B2 bug pattern).

## New types (tau-domain, `no_std` + `alloc`)

```rust
/// A validated bare hostname with optional port. Never a URL, glob, or scheme.
pub struct HostName(String);   // invariant: lowercase; ASCII labels [a-z0-9-]
                               // separated by '.'; optional ":<port>" (1..=65535);
                               // no scheme, '@', '/', '[', ']', '*', whitespace.

impl HostName {
    /// Parse + case-fold. `A.COM` -> `a.com` (accept-and-fold, never reject on case).
    pub fn parse(s: &str) -> Result<HostName, HostNameError>;
    pub fn as_str(&self) -> &str;
}
// Ord/PartialOrd/Eq derived on the inner String -> deterministic BTreeSet order.

pub enum HostSet {
    Any,                        // authored as  hosts = "any"
    Exact(BTreeSet<HostName>),  // authored as  hosts = ["a.com", "b.io:8080"]
}

impl HostSet {
    /// Any ⊇ everything; Exact(p) ⊇ Exact(c) ⟺ c ⊆ p; Exact ⊉ Any.
    pub fn subsumes(&self, child: &HostSet) -> bool;
}

/// The 9 standard HTTP verbs. Obscure/extension verbs (PROPFIND, …) are a
/// deliberate not-yet — additive later, same as suffix wildcards.
pub enum HttpMethod { Get, Head, Post, Put, Delete, Connect, Options, Trace, Patch }

impl HttpMethod {
    pub fn parse(s: &str) -> Result<HttpMethod, HttpMethodError>; // case-insensitive in, canonical UPPER out
}
```

`Capability::Network(NetCapability::Http { … })` becomes:

```rust
Http {
    hosts: HostSet,
    methods: Option<BTreeSet<HttpMethod>>,  // None = all methods (implicit-today made explicit)
}
```

### `HostName` truth table (accept / fold / reject)

| Input | Result |
|---|---|
| `api.anthropic.com` | accept |
| `localhost:8080` | accept |
| `b.io:8080` | accept |
| `xn--nxasmq6b.com` (punycode) | accept |
| `A.COM` | **accept, fold → `a.com`** |
| `*` | reject → `help: write hosts = "any"` |
| `*.a.com` | reject → `help: suffix wildcards not yet supported; enumerate hosts` |
| `https://a.com` | reject → `help: write the bare host, not a URL` |
| `a.com/path` | reject (contains `/`) |
| `user@a.com` | reject (contains `@`) |
| `[::1]:8080` | reject (contains `[`) — IPv6 literals not yet supported |
| `""` / whitespace | reject |

### `HostSet::subsumes` truth table

| parent | child | subsumes |
|---|---|---|
| `Any` | anything | true |
| `Exact({a})` | `Any` | **false** (Exact ⊉ Any) |
| `Exact({a,b})` | `Exact({a})` | true |
| `Exact({a})` | `Exact({a,b})` | false |

### methods subsumes (child ⊆ parent; `None` = full set)

| parent | child | subsumes |
|---|---|---|
| `None` (all) | `Some({GET})` | true |
| `None` (all) | `None` (all) | true |
| `Some({GET})` | `None` (all) | **false** |
| `Some({GET,POST})` | `Some({GET})` | true |
| `Some({GET})` | `Some({POST})` | false — *the audit's witness now fails* |

## Serde (the three hand-written impls in `capability.rs`)

All three move together: `Deserialize` (:411-488), `Serialize` (:490-562),
`JsonSchema` (:565-694).

**Deserialize — `hosts` field** accepts either the exact string `"any"`
(→ `HostSet::Any`) **or** a list of strings, each parsed via `HostName::parse`
(→ `HostSet::Exact`). Any `"*"` anywhere → error with `help: write hosts = "any"`.
Case-fold at parse. `BTreeSet` guarantees deterministic serialization order.

Implementation shape (an untagged helper over the `hosts` value; works for both
TOML and JSON since the flat form is deserialized via `serde_json` from the
`[allow]` bridge and via `toml` from manifests):

```rust
#[derive(Deserialize)]
#[serde(untagged)]
enum RawHosts { Any(String), List(Vec<String>) }   // Any variant is validated == "any"
```

- `hosts = "any"` → `HostSet::Any`.
- `hosts = "api.com"` (bare string ≠ "any") → error (`help: write hosts = "any"` or a list).
- `hosts = ["a.com","B.com"]` → `Exact({a.com, b.com})`.
- `hosts = ["*"]` / `["*.a.com"]` → `HostName::parse` error (help text per truth table).

**Deserialize — `methods` field:** absent key → `None`; present list → `Some(set)`
with each entry via `HttpMethod::parse`; unknown verb → parse error. `methods = []`
→ `Some(∅)`.

**Serialize:**
- `HostSet::Any` → `"hosts": "any"`.
- `HostSet::Exact(set)` → `"hosts": [sorted host strings]`.
- `methods == None` → **omit the key** (round-trips to absent = all).
- `methods == Some(set)` → `"methods": [sorted UPPER verbs]`.

**JsonSchema — `net.http`** `hosts` becomes `oneOf: [ {const:"any"}, {array of
string} ]`; `methods` becomes optional (drop it from `required`) with
`items.enum` = the 9 verbs. `required` stays `["kind","hosts"]`.

### Hash / golden stability

- Already-lowercase, sorted `Exact` host lists serialize byte-identically → bundle
  hashes unchanged for those manifests. **Golden test** asserts this.
- Manifests that **omitted** `methods` previously round-tripped through
  `unwrap_or_default` to `"methods": []`; now they omit the key → **hash changes**
  for those. None expected in-tree (schema currently requires `methods`), but the
  migration sweep confirms. Manifests with `methods = []` that *meant* "all" must
  drop the key (see Migration).

## Consumers to update

1. **Lattice** (`subset.rs`): the `Http` arm calls `HostSet::subsumes` +
   methods inclusion; delete the `gather_hosts` + `string_set_subset` path for
   hosts. `gather_hosts` (:196-205) and the host branch go away; methods gets a
   real inclusion check. Update tests at :339-348 (`_method_diff_ignored` → now
   *checked*), :350-360, :417-425, :473-483.
2. **`[allow]` bridge** (`allow.rs`): the `net.http` cap bridge (`bridge_cap`
   :127-143) needs no change — it re-emits `{kind,…}` into the domain deserializer,
   inheriting HostSet parse for free. **Delete `derive_host` (:156-167)** and make
   `[allow.mcp.<name>].hosts` **required**: `UncheckedMcpAllow.hosts` stays
   `Vec<String>` but empty → error (`[allow.mcp.<name>]: hosts must be non-empty;
   URL-derived hosts are no longer supported`). `McpAllowEntry.hosts` unchanged
   type. Flip test `mcp_url_derives_host_when_absent` → asserts the error;
   `mcp_explicit_hosts_preserved` stays green.
3. **Proxy** (`tau-sandbox-proxy`): introduce a typed host policy and a pass-all
   mode reachable **only** from `HostSet::Any`:

   ```rust
   pub enum HostAllow { Any, Exact(Vec<String>) }   // strings are pre-validated HostName::as_str
   pub fn spawn_proxy(hosts: HostAllow) -> …;
   pub fn validate_hosts(hosts: &[String]) -> …;     // still rejects '*' + non-loopback IP (defense in depth)
   ```

   `validate_hosts` is called on the `Exact` list only. Runtime CONNECT host
   matching must **case-fold**. `HostAllow::Any` = allow every CONNECT target.
   The proxy keeps its own light enum (no `tau-domain` dep — layering).
4. **Sandbox adapters** (native `strict.rs`, darwin `lib.rs`, container
   `runner.rs`, windows `lib.rs`): where they collect `hosts` from
   `NetCapability::Http`, map `HostSet::Exact(set)` → `HostAllow::Exact(case-folded
   Vec)` and `HostSet::Any` → `HostAllow::Any`; pass to `spawn_proxy`. The
   `matches!(…Http{..})` presence checks are untouched.
5. **EPIC 1.6 hookup:** the coarse-ceiling lint now targets the **typed**
   `HostSet::Any` in `[allow]` (warn: `coarse ceiling: net.http grants any host`),
   not a string match on `"*"`. Note in `feat/epic-1.6-coarse-lint` (commit
   `57fe7c98`); fold 1.6 in here only if trivial, else leave a pointer.
6. **Docs** (`capability.rs:137` doc comment): "exact match or glob" → "exact
   hostname or `\"any\"`; suffix wildcards not yet supported".
7. **Fixture builder** (`fixtures.rs:145` `cap_net_http`): adapt to construct
   `HostSet` + `Option<BTreeSet<HttpMethod>>`. Keep signature
   `(hosts: &[&str], methods: &[&str])`; `hosts` containing the single element
   `"any"` → `HostSet::Any`, else `Exact`; **empty `methods` slice → `None`**
   (all) to match the ergonomic intent of existing call sites, non-empty →
   `Some(parsed)`. This is a test-only convenience; the *wire* semantic
   (`[]`=deny-all) is unchanged and is exercised directly by dedicated serde tests.

## End-to-end divergence test (the point of the whole change)

A single integration test proving **build-accepts ⟺ run-enforces**, both directions:

1. A project with `hosts = "any"` passes `tau check` **and** `tau build` **and**
   `tau run` reaches the proxy in **pass-all** mode (no spawn-time rejection).
2. A project authored with `hosts = ["*"]` fails at **parse** (`tau check`),
   never reaching build.

Plus:
- Proxy: pass-all mode integration test; case-insensitive host-match test.
- `HostName` truth table (unit).
- `HostSet::subsumes` + methods-subsumes truth tables (unit).
- Isolated `cargo check -p tau-domain --no-default-features` (no_std guard).

## Migration

- **`hosts = ["*"]`**: none in any manifest/fixture (verified). Only prose ref is
  `docs/decisions/0019-per-host-network-filter.md:105` (superseded ADR) — leave,
  or add a one-line "superseded by 0059" note.
- **`methods = []`**: `tau-cli/tests/cmd_build_mcp.rs:260`,
  `tau-sandbox-native/src/light.rs:376`, and the `subset.rs` test corpus. Audit
  each: if it meant "all methods", drop the key (→ `None`); if it genuinely meant
  "deny all", keep `[]`. Most are lattice tests where the set is the point — keep
  explicit and update expectations for the now-active methods check.
- **`[allow.mcp.*]` without `hosts`**: only in docs/specs/plans (no real
  fixtures). Update those examples to include explicit `hosts`.

## Deliverables & conventions

- **ADR 0059** — "one host semantic: exact + typed any; no derived hosts" — plus
  `docs/SUMMARY.md` entry.
- One `feat/*` branch → single PR to `main`; ordered commits (domain → consumers →
  docs). Compiles green only at end of the consumers commit — expected, CI gates on
  the merged result.
- CLAUDE.md cargo rules (per-agent `CARGO_TARGET_DIR`, `-p`, `timeout`,
  `CARGO_INCREMENTAL=0`); `cargo nextest` for tests.
- `mdbook build` + linkcheck before the docs land; SUMMARY.md entry mandatory.

## Explicitly out of scope (deferred, additive later)

- Suffix wildcards `*.x.com`.
- IPv6 literal hosts (`[::1]:8080`).
- **Runtime method enforcement.** The proxy filters by host, not method; methods
  are enforced only in the build-time lattice (as today, but now *checked* rather
  than ignored). This leaves methods build-checked-but-not-run-enforced — a
  narrower residual than the host divergence this spec closes, noted here so it is
  not a silent hole. Closing it means teaching the proxy method filtering; tracked
  as follow-up, not part of D4-B.
