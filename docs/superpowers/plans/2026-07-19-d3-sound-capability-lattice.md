# D3 Sound Capability Lattice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the unsound sampling-based glob-subset with a structurally-decidable, `no_std` capability subset + `meet` in `tau-domain`, as the single source of truth for `tau check` and the future D1-C runtime clamp.

**Architecture:** A restricted glob grammar (G2: literal segments, whole-segment `*`, trailing `**`, brace alternation) is decided analytically; anything outside it fails closed. The primitive lives in `tau-domain` (`no_std` + `alloc`); `tau-pkg` deletes its sampler and delegates. `meet` computes exact language intersection over G2, canonicalized so the lattice law `subset(a,b) ⟺ meet(a,b)==canon(a)` holds structurally.

**Tech Stack:** Rust, `no_std` + `alloc` (tau-domain), proptest (already a tau-domain dev-dependency), nextest.

## Global Constraints

- **Crate `tau-domain` is `#![no_std]` + `alloc`.** No `std`, no filesystem, no `HashMap` (use `alloc::collections::BTreeMap`/`BTreeSet`), no `std::path`. String/Vec/BTree only. Verified by `cargo check -p tau-domain --no-default-features`.
- **Cargo rules (CLAUDE.md):** every cargo command is `timeout <T> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <cmd> -p <crate>`. Test timeout 300, check 180, clippy 240. Use `cargo nextest run` for tests, `cargo test --doc` for doctests.
- **Commit identity:** `git -c user.name="LEBOCQ Titouan" -c user.email="lebocq.tit@gmail.com" commit ...` (lefthook can corrupt worktree identity).
- **Soundness invariant:** any pattern or capability kind outside the decidable grammar is treated as *not a subset* (deny-by-default) and contributes ⊥ (nothing) to `meet`. Never admit-by-default.
- **Module location:** `crates/tau-domain/src/package/capability.rs` gains `pub mod lattice;`; the module lives at `crates/tau-domain/src/package/capability/lattice/{mod.rs,glob.rs,host.rs}`.

---

### Task 1: G2 glob engine — normalize, parse, subset

**Files:**
- Create: `crates/tau-domain/src/package/capability/lattice/glob.rs`
- Modify: `crates/tau-domain/src/package/capability.rs` (add `pub mod lattice;` — create `lattice/mod.rs` declaring `pub mod glob;` in this task)
- Create: `crates/tau-domain/src/package/capability/lattice/mod.rs`
- Test: inline `#[cfg(test)]` in `glob.rs`

**Interfaces:**
- Produces:
  - `enum Segment { Literal(String), Star, StarStar }`
  - `struct Pattern(Vec<Segment>)` — brace-free, normalized, `**` only last
  - `fn expand(pat: &str) -> Option<Vec<Pattern>>` — `None` = outside G2 / invalid (fail-closed). Expands braces, normalizes, validates.
  - `fn pattern_subset(child: &Pattern, parent: &Pattern) -> bool`
  - `fn glob_subset(child: &str, parent: &str) -> bool` — expand both; every child arm ⊆ some parent arm; any `None` → `false`
  - `fn glob_subset_set(children: &[String], parents: &[String]) -> Result<(), String>` — each child ⊆ some parent; `Err(child)` names first offender

- [ ] **Step 1: Write failing tests for expand + normalization + witnesses**

In `crates/tau-domain/src/package/capability/lattice/glob.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn literal_equality_is_subset() {
        assert!(glob_subset("/tmp/foo", "/tmp/foo"));
    }
    #[test]
    fn concrete_path_under_double_star() {
        assert!(glob_subset("/proj/src/main.rs", "/proj/**"));
    }
    #[test]
    fn prefix_only_match_under_double_star() {
        assert!(glob_subset("/proj", "/proj/**"));
    }
    #[test]
    fn root_glob_admits_everything() {
        assert!(glob_subset("/anything/anywhere", "/**"));
    }
    #[test]
    fn disjoint_paths_not_subset() {
        assert!(!glob_subset("/etc/**", "/proj/src/**"));
    }
    #[test]
    fn parent_more_specific_not_subset() {
        assert!(!glob_subset("/proj/**", "/proj/src/**"));
    }
    #[test]
    fn single_star_is_one_component() {
        assert!(glob_subset("/data/*", "/data/**"));
        assert!(!glob_subset("/data/**", "/data/*")); // ** admits deeper than *
    }
    #[test]
    fn brace_child_all_arms_subset() {
        assert!(glob_subset("/proj/{src,docs}/**", "/proj/**"));
    }
    #[test]
    fn brace_child_one_arm_not_subset_rejects() {
        assert!(!glob_subset("/proj/{src,etc}/**", "/proj/src/**"));
    }
    // --- soundness witnesses (the whole point of D3) ---
    #[test]
    fn witness_intra_segment_star_fails_closed() {
        // parent `seed*` is intra-segment → outside G2 → not decidable → false
        assert!(!glob_subset("/proj/*", "/proj/seed*"));
    }
    #[test]
    fn witness_dotdot_normalized_then_rejected() {
        // /proj/../etc/** normalizes to /etc/** ⊄ /proj/**
        assert!(!glob_subset("/proj/../etc/**", "/proj/**"));
    }
    #[test]
    fn question_mark_fails_closed() {
        assert!(!glob_subset("/proj/?", "/proj/**"));
    }
    #[test]
    fn char_class_fails_closed() {
        assert!(!glob_subset("/proj/[abc]", "/proj/**"));
    }
    #[test]
    fn middle_double_star_fails_closed() {
        // middle ** is G3, not G2
        assert!(!glob_subset("/foo/**/bar.txt", "/foo/**"));
    }
    #[test]
    fn relative_path_fails_closed() {
        assert!(!glob_subset("proj/src/**", "/proj/**"));
        assert!(expand("proj/src").is_none());
    }
    #[test]
    fn dotdot_escaping_root_fails_closed() {
        assert!(expand("/../etc").is_none());
    }
    #[test]
    fn subset_set_names_first_offender() {
        let children = vec!["/proj/src/**".into(), "/proj/etc/**".into()];
        let parents = vec!["/proj/{src,docs}/**".into()];
        assert_eq!(glob_subset_set(&children, &parents).unwrap_err(), "/proj/etc/**");
    }
}
```

- [ ] **Step 2: Run to verify it fails (module doesn't exist)**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-domain`
Expected: FAIL — `cannot find module glob` / `expand not found`.

- [ ] **Step 3: Implement glob.rs (normalize + parse + subset)**

At the top of `crates/tau-domain/src/package/capability/lattice/mod.rs`:

```rust
//! Sound capability lattice (D3). Structurally-decidable subset + meet over
//! the restricted G2 glob grammar. See
//! docs/superpowers/specs/2026-07-19-d3-sound-capability-lattice-design.md.
pub mod glob;
```

In `crates/tau-domain/src/package/capability.rs`, add near the other `mod`/`pub use` lines:

```rust
pub mod lattice;
```

In `glob.rs`:

```rust
//! G2 glob grammar: `/`-split segments over {Literal, `*` (one component),
//! `**` (trailing, any suffix)}, plus brace alternation expanded before
//! analysis. Anything outside G2 → `None`/`false` (fail-closed).

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;

/// Max brace-expanded arms before we fail closed (combinatorial guard).
const MAX_ARMS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Literal(String),
    Star,
    StarStar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern(pub Vec<Segment>);

/// Expand braces, normalize, and validate each arm into a `Pattern`.
/// Returns `None` if any arm is outside G2 or the arm count overflows.
pub fn expand(pat: &str) -> Option<Vec<Pattern>> {
    let arms = brace_expand(pat, MAX_ARMS)?;
    let mut out = Vec::with_capacity(arms.len());
    for arm in arms {
        out.push(parse_arm(&arm)?);
    }
    Some(out)
}

/// Expand top-level, possibly-nested brace alternations into concrete strings.
/// `None` on overflow.
fn brace_expand(pat: &str, cap: usize) -> Option<Vec<String>> {
    let mut frontier: Vec<String> = vec![String::new()];
    let mut rest = pat;
    loop {
        let open = match rest.find('{') {
            Some(i) => i,
            None => {
                // no more braces: append remaining literal tail to every arm
                for s in frontier.iter_mut() {
                    s.push_str(rest);
                }
                return Some(frontier);
            }
        };
        let close = open + rest[open..].find('}')?;
        let prefix = &rest[..open];
        let arms_str = &rest[open + 1..close];
        if arms_str.contains('{') {
            return None; // nested inside the alternation: not decidably split at v1
        }
        let arm_options: Vec<&str> = arms_str.split(',').collect();
        let mut next: Vec<String> = Vec::new();
        for s in &frontier {
            for a in &arm_options {
                let mut t = s.clone();
                t.push_str(prefix);
                t.push_str(a);
                next.push(t);
                if next.len() > cap {
                    return None;
                }
            }
        }
        frontier = next;
        rest = &rest[close + 1..];
    }
}

/// Parse one brace-free string into a normalized `Pattern`, or `None` if
/// outside G2.
fn parse_arm(s: &str) -> Option<Pattern> {
    let body = s.strip_prefix('/')?; // must be absolute
    let mut segs: Vec<Segment> = Vec::new();
    for (i, raw) in body.split('/').enumerate() {
        let last = i == raw_count(body) - 1;
        match raw {
            "" | "." => continue, // collapse // and .
            ".." => {
                // fold: pop a literal; escaping root or popping a wildcard → invalid
                match segs.pop() {
                    Some(Segment::Literal(_)) => {}
                    _ => return None,
                }
            }
            "*" => segs.push(Segment::Star),
            "**" => {
                if !last {
                    return None; // middle ** is G3
                }
                segs.push(Segment::StarStar);
            }
            other => {
                // no intra-segment wildcards / classes / ? allowed
                if other.contains(['*', '?', '[', ']', '{', '}']) {
                    return None;
                }
                segs.push(Segment::Literal(other.to_string()));
            }
        }
    }
    Some(Pattern(segs))
}

fn raw_count(body: &str) -> usize {
    body.split('/').count()
}

/// `child ⊆ parent` on normalized, brace-free patterns.
pub fn pattern_subset(child: &Pattern, parent: &Pattern) -> bool {
    subset_segs(&child.0, &parent.0)
}

fn subset_segs(c: &[Segment], p: &[Segment]) -> bool {
    match (c.first(), p.first()) {
        // parent trailing ** is ⊤: admits any child suffix (incl. empty)
        (_, Some(Segment::StarStar)) => true,
        (Some(Segment::Star), Some(Segment::Star)) => subset_segs(&c[1..], &p[1..]),
        (Some(Segment::Literal(_)), Some(Segment::Star)) => subset_segs(&c[1..], &p[1..]),
        (Some(Segment::Literal(a)), Some(Segment::Literal(b))) if a == b => {
            subset_segs(&c[1..], &p[1..])
        }
        (None, None) => true,
        _ => false, // includes child `**`/`*` vs parent literal, mismatched literals, length mismatch
    }
}

/// `child ⊆ parent` on raw pattern strings. Any un-parseable arm → false.
pub fn glob_subset(child: &str, parent: &str) -> bool {
    let (cs, ps) = match (expand(child), expand(parent)) {
        (Some(cs), Some(ps)) => (cs, ps),
        _ => return false,
    };
    cs.iter()
        .all(|c| ps.iter().any(|p| pattern_subset(c, p)))
}

/// Each child pattern ⊆ some parent pattern. `Err(child)` on first offender.
pub fn glob_subset_set(children: &[String], parents: &[String]) -> Result<(), String> {
    for child in children {
        if !parents.iter().any(|p| glob_subset(child, p)) {
            return Err(child.clone());
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-domain glob::`
Expected: PASS (all Task 1 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-domain/src/package/capability.rs \
        crates/tau-domain/src/package/capability/lattice/
git -c user.name="LEBOCQ Titouan" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-domain): sound G2 glob subset (D3 task 1)"
```

---

### Task 2: G2 glob `meet` — intersection + canonicalization

**Files:**
- Modify: `crates/tau-domain/src/package/capability/lattice/glob.rs`
- Test: inline `#[cfg(test)]` in `glob.rs`

**Interfaces:**
- Consumes: `Pattern`, `Segment`, `expand`, `pattern_subset` (Task 1)
- Produces:
  - `fn pattern_intersect(a: &Pattern, b: &Pattern) -> Option<Pattern>` — `None` = ∅
  - `fn render(pat: &Pattern) -> String` — segment list → `/a/b/**`
  - `fn glob_canon(pats: &[String]) -> Vec<String>` — normalize, dedup, absorb (drop any pattern ⊆ another), sorted
  - `fn glob_meet(a: &[String], b: &[String]) -> Vec<String>` — canon(⋃ pairwise intersect)

- [ ] **Step 1: Write failing tests**

Append to `glob.rs` tests module:

```rust
    #[test]
    fn meet_exact_intersection_new_pattern() {
        assert_eq!(glob_meet(&["/a/**".into()], &["/*/b/**".into()]), vec!["/a/b/**".to_string()]);
    }
    #[test]
    fn meet_smaller_operand() {
        assert_eq!(glob_meet(&["/a/*".into()], &["/a/**".into()]), vec!["/a/*".to_string()]);
    }
    #[test]
    fn meet_disjoint_is_empty() {
        assert!(glob_meet(&["/a/**".into()], &["/b/**".into()]).is_empty());
    }
    #[test]
    fn meet_literal_under_star() {
        assert_eq!(glob_meet(&["/a/b".into()], &["/a/*".into()]), vec!["/a/b".to_string()]);
    }
    #[test]
    fn canon_absorbs_redundant() {
        assert_eq!(glob_canon(&["/a/**".into(), "/a/b/**".into()]), vec!["/a/**".to_string()]);
    }
    #[test]
    fn canon_normalizes_dotdot() {
        assert_eq!(glob_canon(&["/proj/../etc/**".into()]), vec!["/etc/**".to_string()]);
    }
    #[test]
    fn meet_is_subset_of_both_operands() {
        let a = ["/a/**".to_string()];
        let b = ["/*/b/**".to_string()];
        let m = glob_meet(&a, &b);
        assert!(glob_subset_set(&m, &a).is_ok());
        assert!(glob_subset_set(&m, &b).is_ok());
    }
```

- [ ] **Step 2: Run to verify fail**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-domain`
Expected: FAIL — `glob_meet`/`glob_canon` not found.

- [ ] **Step 3: Implement intersection, render, canon, meet**

Append to `glob.rs` (before `#[cfg(test)]`):

```rust
/// Exact language intersection of two normalized patterns. `None` = ∅.
pub fn pattern_intersect(a: &Pattern, b: &Pattern) -> Option<Pattern> {
    let mut out = Vec::new();
    if intersect_segs(&a.0, &b.0, &mut out) {
        Some(Pattern(out))
    } else {
        None
    }
}

fn intersect_segs(a: &[Segment], b: &[Segment], out: &mut Vec<Segment>) -> bool {
    match (a.first(), b.first()) {
        // ** is ⊤ for suffixes: intersection is the *other* side's remaining tail
        (Some(Segment::StarStar), _) => {
            out.extend_from_slice(b);
            true
        }
        (_, Some(Segment::StarStar)) => {
            out.extend_from_slice(a);
            true
        }
        (Some(Segment::Star), Some(Segment::Star)) => {
            out.push(Segment::Star);
            intersect_segs(&a[1..], &b[1..], out)
        }
        (Some(Segment::Star), Some(Segment::Literal(l)))
        | (Some(Segment::Literal(l)), Some(Segment::Star)) => {
            out.push(Segment::Literal(l.clone()));
            intersect_segs(&a[1..], &b[1..], out)
        }
        (Some(Segment::Literal(x)), Some(Segment::Literal(y))) if x == y => {
            out.push(Segment::Literal(x.clone()));
            intersect_segs(&a[1..], &b[1..], out)
        }
        (None, None) => true,
        _ => false, // literal≠literal, or one exhausted while other needs more
    }
}

pub fn render(pat: &Pattern) -> String {
    let mut s = String::new();
    for seg in &pat.0 {
        s.push('/');
        match seg {
            Segment::Literal(l) => s.push_str(l),
            Segment::Star => s.push('*'),
            Segment::StarStar => s.push_str("**"),
        }
    }
    if s.is_empty() {
        s.push('/');
    }
    s
}

/// Normalize, dedup, absorb (drop any pattern that is ⊆ another), and sort.
pub fn glob_canon(pats: &[String]) -> Vec<String> {
    // parse+normalize (drop un-parseable — they are ⊥ and carry no grants)
    let mut parsed: Vec<Pattern> = Vec::new();
    for p in pats {
        if let Some(arms) = expand(p) {
            parsed.extend(arms);
        }
    }
    // absorb: keep pattern i only if it is not ⊆ some other distinct pattern j
    let mut kept: Vec<Pattern> = Vec::new();
    'outer: for (i, pi) in parsed.iter().enumerate() {
        for (j, pj) in parsed.iter().enumerate() {
            if i != j && pattern_subset(pi, pj) && !(pattern_subset(pj, pi) && j < i) {
                // pi ⊆ pj and pj is the survivor (or the earlier of an equal pair)
                continue 'outer;
            }
        }
        kept.push(pi.clone());
    }
    let mut rendered: Vec<String> = kept.iter().map(render).collect();
    rendered.sort();
    rendered.dedup();
    rendered
}

/// meet = canon(all pairwise intersections).
pub fn glob_meet(a: &[String], b: &[String]) -> Vec<String> {
    let (ap, bp) = match (join_expand(a), join_expand(b)) {
        (Some(ap), Some(bp)) => (ap, bp),
        // any un-parseable side collapses that side's grants to what parses
        _ => (join_expand(a).unwrap_or_default(), join_expand(b).unwrap_or_default()),
    };
    let mut inter: Vec<String> = Vec::new();
    for x in &ap {
        for y in &bp {
            if let Some(p) = pattern_intersect(x, y) {
                inter.push(render(&p));
            }
        }
    }
    glob_canon(&inter)
}

fn join_expand(pats: &[String]) -> Option<Vec<Pattern>> {
    let mut out = Vec::new();
    for p in pats {
        out.extend(expand(p)?);
    }
    Some(out)
}
```

Note the absorb tie-break: for two equal patterns, only the later index is dropped (`j < i` guard), so exactly one survives.

- [ ] **Step 4: Run tests to verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-domain glob::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-domain/src/package/capability/lattice/glob.rs
git -c user.name="LEBOCQ Titouan" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-domain): G2 glob meet + canonicalization (D3 task 2)"
```

---

### Task 3: Host sub-grammar subset + meet

**Files:**
- Create: `crates/tau-domain/src/package/capability/lattice/host.rs`
- Modify: `crates/tau-domain/src/package/capability/lattice/mod.rs` (add `pub mod host;`)
- Test: inline in `host.rs`

**Interfaces:**
- Produces:
  - `fn host_subset(child: &str, parent: &str) -> bool` — exact, or `*.suffix` containment
  - `fn host_subset_set(children: &[String], parents: &[String]) -> Result<(), String>`
  - `fn host_meet(a: &[String], b: &[String]) -> Vec<String>`

- [ ] **Step 1: Write failing tests**

In `host.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn exact_equal() { assert!(host_subset("api.example.com", "api.example.com")); }
    #[test] fn exact_under_suffix() { assert!(host_subset("api.example.com", "*.example.com")); }
    #[test] fn suffix_under_suffix() { assert!(host_subset("*.a.example.com", "*.example.com")); }
    #[test] fn suffix_not_under_exact() { assert!(!host_subset("*.example.com", "api.example.com")); }
    #[test] fn disjoint() { assert!(!host_subset("api.other.com", "*.example.com")); }
    #[test] fn embedded_star_fails_closed() { assert!(!host_subset("a*b.example.com", "*.example.com")); }
    #[test] fn meet_exact_and_suffix() {
        assert_eq!(host_meet(&["api.example.com".into()], &["*.example.com".into()]),
                   vec!["api.example.com".to_string()]);
    }
    #[test] fn meet_disjoint_empty() {
        assert!(host_meet(&["a.com".into()], &["*.example.com".into()]).is_empty());
    }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-domain`
Expected: FAIL — `host_subset` not found.

- [ ] **Step 3: Implement host.rs**

```rust
//! Host glob sub-grammar: an exact host, or `*.suffix`. Anything else
//! (embedded `*`, multiple `*`) → fail-closed. Deliberately smaller than the
//! path grammar.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

enum Host<'a> { Exact(&'a str), Suffix(&'a str) } // Suffix("example.com") = *.example.com

fn parse(h: &str) -> Option<Host<'_>> {
    if let Some(sfx) = h.strip_prefix("*.") {
        if sfx.is_empty() || sfx.contains('*') { return None; }
        Some(Host::Suffix(sfx))
    } else if h.contains('*') {
        None
    } else {
        Some(Host::Exact(h))
    }
}

pub fn host_subset(child: &str, parent: &str) -> bool {
    match (parse(child), parse(parent)) {
        (Some(c), Some(p)) => match (c, p) {
            (Host::Exact(a), Host::Exact(b)) => a == b,
            (Host::Exact(a), Host::Suffix(s)) => a == s || a.ends_with(&dot(s)),
            (Host::Suffix(a), Host::Suffix(s)) => a == s || a.ends_with(&dot(s)),
            (Host::Suffix(_), Host::Exact(_)) => false, // wildcard can't fit under exact
        },
        _ => false,
    }
}

fn dot(s: &str) -> String {
    let mut d = String::from(".");
    d.push_str(s);
    d
}

pub fn host_subset_set(children: &[String], parents: &[String]) -> Result<(), String> {
    for c in children {
        if !parents.iter().any(|p| host_subset(c, p)) {
            return Err(c.clone());
        }
    }
    Ok(())
}

pub fn host_meet(a: &[String], b: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for x in a {
        for y in b {
            // intersection of two host patterns = the more specific one if one
            // ⊆ the other, else ∅ (host grammar has no cross-produced pattern).
            if host_subset(x, y) {
                push_unique(&mut out, x);
            } else if host_subset(y, x) {
                push_unique(&mut out, y);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn push_unique(v: &mut Vec<String>, s: &str) {
    if !v.iter().any(|e| e == s) { v.push(s.to_string()); }
}
```

- [ ] **Step 4: Run tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-domain host::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-domain/src/package/capability/lattice/host.rs \
        crates/tau-domain/src/package/capability/lattice/mod.rs
git -c user.name="LEBOCQ Titouan" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-domain): host glob sub-grammar subset+meet (D3 task 3)"
```

---

### Task 4: `capability_subset` + `CeilingViolation` (moved to tau-domain)

Move the per-field capability subset logic out of `tau-pkg`'s `subset.rs` into `tau-domain`, re-pointing path checks at Task 1's `glob_subset_set` and host checks at Task 3's `host_subset_set`. Token/mode/max_bytes/custom logic is copied verbatim (it is already sound and `no_std`-safe).

**Files:**
- Modify: `crates/tau-domain/src/package/capability/lattice/mod.rs` (add the types + fns)
- Modify: `crates/tau-domain/src/package/capability.rs` / `crates/tau-domain/src/lib.rs` (re-export `CeilingViolation`, `capability_subset`)
- Test: inline in `mod.rs`

**Interfaces:**
- Consumes: `glob::glob_subset_set` (T1), `host::host_subset_set` (T3), `Capability` and its verb enums (existing tau-domain).
- Produces:
  - `#[non_exhaustive] pub struct CeilingViolation { pub kind: String, pub offender: String, pub reason: String }`
  - `pub fn capability_subset(child: &[Capability], parent: &[Capability]) -> Result<(), CeilingViolation>`

- [ ] **Step 1: Write failing tests** (migrated from `tau-pkg/src/capability_override/subset.rs` tests, now sound)

Append to `lattice/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Capability, FsCapability, NetCapability, ProcessCapability};
    use alloc::vec;

    fn read(paths: &[&str]) -> Capability {
        Capability::Filesystem(FsCapability::Read { paths: paths.iter().map(|s| s.to_string()).collect() })
    }

    #[test]
    fn path_child_within_ceiling_ok() {
        let child = vec![read(&["/proj/src/**"])];
        let parent = vec![read(&["/proj/**"])];
        assert!(capability_subset(&child, &parent).is_ok());
    }
    #[test]
    fn path_child_outside_ceiling_violates() {
        let child = vec![read(&["/etc/**"])];
        let parent = vec![read(&["/proj/**"])];
        let v = capability_subset(&child, &parent).unwrap_err();
        assert_eq!(v.kind, "fs.read");
        assert_eq!(v.offender, "/etc/**");
    }
    #[test]
    fn kind_not_in_ceiling_violates() {
        let child = vec![Capability::Process(ProcessCapability::Spawn { commands: vec!["ls".into()] })];
        let parent = vec![read(&["/proj/**"])];
        let v = capability_subset(&child, &parent).unwrap_err();
        assert_eq!(v.reason, "kind not in ceiling");
    }
    #[test]
    fn host_child_within_ceiling_ok() {
        let child = vec![Capability::Network(NetCapability::Http {
            hosts: vec!["api.example.com".into()], methods: vec!["GET".into()] })];
        let parent = vec![Capability::Network(NetCapability::Http {
            hosts: vec!["*.example.com".into()], methods: vec!["GET".into()] })];
        assert!(capability_subset(&child, &parent).is_ok());
    }
    // The sampling-era admission that is now correctly denied:
    #[test]
    fn intra_segment_ceiling_now_fails_closed() {
        let child = vec![read(&["/proj/x"])];
        let parent = vec![read(&["/proj/seed*"])]; // outside G2 → deny
        assert!(capability_subset(&child, &parent).is_err());
    }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-domain`
Expected: FAIL — `capability_subset`/`CeilingViolation` not found.

- [ ] **Step 3: Move the logic into `lattice/mod.rs`**

Copy these items from `crates/tau-pkg/src/capability_override/subset.rs` verbatim into `lattice/mod.rs`, changing only the two path/host delegations:
`CeilingViolation`, `capability_set_subset` (rename to `capability_subset`), `kind_str`, `same_kind`, `mode_rank`, `cap_subset_against`, `mode_subset`, `gather_paths`, `gather_hosts`, `gather_commands`, `gather_agent_kinds`, `gather_skills`, `most_permissive_max_bytes`, `string_set_subset`, `max_bytes_le`.

The two changed call sites inside `cap_subset_against`:

```rust
// fs.read / fs.exec / fs.write path check:
let pp = gather_paths(parents);
crate::package::capability::lattice::glob::glob_subset_set(paths, &pp)
    .map_err(|o| (o, "not a subset of any allowed path".into()))?;

// net.http host check:
let ph = gather_hosts(parents);
crate::package::capability::lattice::host::host_subset_set(hosts, &ph)
    .map_err(|o| (o, "host not in ceiling".into()))?;
```

Add imports at the top of `mod.rs`:

```rust
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::{
    AgentCapability, Capability, FsCapability, NetCapability, ProcessCapability, SkillCapability,
};
```

Re-export from `crates/tau-domain/src/lib.rs` (next to other capability re-exports):

```rust
pub use package::capability::lattice::{capability_subset, meet, CeilingViolation};
```

(`meet` is added in Task 5; add the name now and it compiles once Task 5 lands, or split the re-export — keep `capability_subset, CeilingViolation` here and add `meet` in Task 5.)

- [ ] **Step 4: Run tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-domain lattice::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-domain/src/
git -c user.name="LEBOCQ Titouan" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-domain): sound capability_subset (moved from tau-pkg) (D3 task 4)"
```

---

### Task 5: `meet` for capability sets + property tests

**Files:**
- Modify: `crates/tau-domain/src/package/capability/lattice/mod.rs`
- Modify: `crates/tau-domain/src/lib.rs` (add `meet` to the re-export)
- Test: inline `#[cfg(test)]` proptest block in `mod.rs`

**Interfaces:**
- Consumes: `glob::glob_meet`, `glob::glob_canon`, `host::host_meet`, `capability_subset` (T4).
- Produces:
  - `pub fn meet(a: &[Capability], b: &[Capability]) -> Vec<Capability>`
  - `pub fn canon_caps(caps: &[Capability]) -> Vec<Capability>` — path/host lists canonicalized, caps sorted deterministically (used by the lattice law)

- [ ] **Step 1: Write failing example + property tests**

Append to `lattice/mod.rs` tests module:

```rust
    #[test]
    fn meet_paths_intersects() {
        let a = vec![read(&["/a/**"])];
        let b = vec![read(&["/*/b/**"])];
        assert_eq!(meet(&a, &b), vec![read(&["/a/b/**"])]);
    }
    #[test]
    fn meet_drops_kind_absent_in_one() {
        let a = vec![read(&["/a/**"])];
        let b = vec![Capability::Process(ProcessCapability::Spawn { commands: vec!["ls".into()] })];
        assert!(meet(&a, &b).is_empty());
    }

    use proptest::prelude::*;

    // small G2 path generator over a fixed alphabet
    fn path_strat() -> impl Strategy<Value = String> {
        let seg = prop_oneof![Just("a"), Just("b"), Just("c"), Just("*")];
        prop::collection::vec(seg, 1..4).prop_flat_map(|mut segs| {
            prop_oneof![Just(false), Just(true)].prop_map(move |dbl| {
                let mut p = String::new();
                for s in &segs { p.push('/'); p.push_str(s); }
                if dbl { p.push_str("/**"); }
                p
            })
        })
    }
    // One cap of a randomly-chosen kind (Read paths or Process.Spawn commands),
    // so property tests exercise cross-kind AND multi-same-kind (0..3 caps can
    // yield two Reads → the merge path in canon_caps).
    fn cap_strat() -> impl Strategy<Value = Capability> {
        prop_oneof![
            prop::collection::vec(path_strat(), 1..3).prop_map(|p| {
                read(&p.iter().map(|s| s.as_str()).collect::<Vec<_>>())
            }),
            prop::collection::vec(prop_oneof![Just("ls"), Just("cat"), Just("rm")], 1..3).prop_map(|c| {
                Capability::Process(ProcessCapability::Spawn {
                    commands: c.iter().map(|s| s.to_string()).collect(),
                })
            }),
        ]
    }
    fn caps_strat() -> impl Strategy<Value = Vec<Capability>> {
        prop::collection::vec(cap_strat(), 0..3)
    }

    proptest! {
        #[test]
        fn prop_meet_subset_of_both(a in caps_strat(), b in caps_strat()) {
            let m = meet(&a, &b);
            prop_assert!(capability_subset(&m, &a).is_ok());
            prop_assert!(capability_subset(&m, &b).is_ok());
        }
        #[test]
        fn prop_meet_idempotent(a in caps_strat()) {
            prop_assert_eq!(meet(&a, &a), canon_caps(&a));
        }
        #[test]
        fn prop_meet_commutative(a in caps_strat(), b in caps_strat()) {
            prop_assert_eq!(meet(&a, &b), meet(&b, &a));
        }
        #[test]
        fn prop_lattice_law(a in caps_strat(), b in caps_strat()) {
            let subset_ok = capability_subset(&a, &b).is_ok();
            let meet_eq = meet(&a, &b) == canon_caps(&a);
            prop_assert_eq!(subset_ok, meet_eq);
        }
    }
```

- [ ] **Step 2: Run to verify fail**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-domain --tests`
Expected: FAIL — `meet`/`canon_caps` not found.

- [ ] **Step 3: Implement `meet` + `canon_caps`**

Append to `lattice/mod.rs` (non-test):

```rust
use crate::package::capability::lattice::glob::{glob_canon, glob_meet};
use crate::package::capability::lattice::host::host_meet;

/// Greatest lower bound of two capability sets. Total on G2.
pub fn meet(a: &[Capability], b: &[Capability]) -> Vec<Capability> {
    let mut out: Vec<Capability> = Vec::new();
    for ca in a {
        for cb in b {
            if same_kind(ca, cb) {
                if let Some(m) = meet_pair(ca, cb) {
                    push_cap(&mut out, m);
                }
            }
        }
    }
    canon_caps(&out)
}

fn meet_pair(a: &Capability, b: &Capability) -> Option<Capability> {
    use Capability::*;
    match (a, b) {
        (Filesystem(FsCapability::Read { paths: pa }), Filesystem(FsCapability::Read { paths: pb })) => {
            let m = glob_meet(pa, pb);
            (!m.is_empty()).then(|| Filesystem(FsCapability::Read { paths: m }))
        }
        (Filesystem(FsCapability::Exec { paths: pa }), Filesystem(FsCapability::Exec { paths: pb })) => {
            let m = glob_meet(pa, pb);
            (!m.is_empty()).then(|| Filesystem(FsCapability::Exec { paths: m }))
        }
        (
            Filesystem(FsCapability::Write { paths: pa, max_bytes: ma }),
            Filesystem(FsCapability::Write { paths: pb, max_bytes: mb }),
        ) => {
            let m = glob_meet(pa, pb);
            if m.is_empty() { return None; }
            let max_bytes = min_max_bytes(*ma, *mb);
            Some(Filesystem(FsCapability::Write { paths: m, max_bytes }))
        }
        (
            Network(NetCapability::Http { hosts: ha, methods: mea }),
            Network(NetCapability::Http { hosts: hb, methods: meb }),
        ) => {
            let hosts = host_meet(ha, hb);
            let methods = str_intersect(mea, meb);
            (!hosts.is_empty() && !methods.is_empty())
                .then(|| Network(NetCapability::Http { hosts, methods }))
        }
        (Process(ProcessCapability::Spawn { commands: a }), Process(ProcessCapability::Spawn { commands: b })) => {
            let c = str_intersect(a, b);
            (!c.is_empty()).then(|| Process(ProcessCapability::Spawn { commands: c }))
        }
        (Agent(AgentCapability::Spawn { allowed_kinds: a }), Agent(AgentCapability::Spawn { allowed_kinds: b })) => {
            let k = str_intersect(a, b);
            (!k.is_empty()).then(|| Agent(AgentCapability::Spawn { allowed_kinds: k }))
        }
        (Skill(SkillCapability::Spawn { allowed_skills: a }), Skill(SkillCapability::Spawn { allowed_skills: b })) => {
            let s = str_intersect(a, b);
            (!s.is_empty()).then(|| Skill(SkillCapability::Spawn { allowed_skills: s }))
        }
        (TaskList { mode: a }, TaskList { mode: b }) => min_mode(a, b, true).map(|mode| TaskList { mode }),
        (Plan { mode: a }, Plan { mode: b }) => min_mode(a, b, false).map(|mode| Plan { mode }),
        (Custom { name: na, params: pa }, Custom { name: nb, params: pb }) if na == nb && pa == pb => {
            Some(Custom { name: na.clone(), params: pa.clone() })
        }
        _ => None,
    }
}

fn str_intersect(a: &[String], b: &[String]) -> Vec<String> {
    let mut out: Vec<String> = a.iter().filter(|x| b.contains(x)).cloned().collect();
    out.sort();
    out.dedup();
    out
}

fn min_max_bytes(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some(x), Some(y)) => Some(x.min(y)),
    }
}

fn min_mode(a: &str, b: &str, allow_manage: bool) -> Option<String> {
    let ra = mode_rank(a, allow_manage)?;
    let rb = mode_rank(b, allow_manage)?;
    let lo = ra.min(rb);
    Some(match lo { 0 => "read", 1 => "write", _ => "manage" }.to_string())
}

fn push_cap(out: &mut Vec<Capability>, cap: Capability) {
    if !out.contains(&cap) { out.push(cap); }
}

/// Canonical form: same-kind caps merged (their pattern/token lists unioned —
/// the ceiling code's `gather_*` already treats a kind's grants as the union
/// across entries), glob/host/token lists canonicalized, empty (⊥) caps
/// dropped, result ordered so the lattice law holds as structural equality.
pub fn canon_caps(caps: &[Capability]) -> Vec<Capability> {
    // 1. merge every capability of the same kind into one
    let mut merged: Vec<Capability> = Vec::new();
    for c in caps {
        if let Some(existing) = merged.iter_mut().find(|m| same_kind(m, c)) {
            union_into(existing, c);
        } else {
            merged.push(c.clone());
        }
    }
    // 2. canonicalize each merged cap's lists; drop empties (⊥)
    let mut out: Vec<Capability> = Vec::new();
    for c in &merged {
        if let Some(cc) = canon_one(c) {
            out.push(cc);
        }
    }
    out.sort_by(|x, y| kind_str(x).cmp(kind_str(y)).then_with(|| render_cap(x).cmp(&render_cap(y))));
    out
}

/// Union `src`'s grant lists into `dst` (same kind guaranteed by the caller).
fn union_into(dst: &mut Capability, src: &Capability) {
    use Capability::*;
    match (dst, src) {
        (Filesystem(FsCapability::Read { paths: d }), Filesystem(FsCapability::Read { paths: s }))
        | (Filesystem(FsCapability::Exec { paths: d }), Filesystem(FsCapability::Exec { paths: s })) => {
            d.extend(s.iter().cloned());
        }
        (
            Filesystem(FsCapability::Write { paths: d, max_bytes: md }),
            Filesystem(FsCapability::Write { paths: s, max_bytes: ms }),
        ) => {
            d.extend(s.iter().cloned());
            *md = match (*md, *ms) { (None, _) | (_, None) => None, (Some(a), Some(b)) => Some(a.max(b)) };
        }
        (
            Network(NetCapability::Http { hosts: dh, methods: dm }),
            Network(NetCapability::Http { hosts: sh, methods: sm }),
        ) => {
            dh.extend(sh.iter().cloned());
            dm.extend(sm.iter().cloned());
        }
        (Process(ProcessCapability::Spawn { commands: d }), Process(ProcessCapability::Spawn { commands: s })) => {
            d.extend(s.iter().cloned());
        }
        (Agent(AgentCapability::Spawn { allowed_kinds: d }), Agent(AgentCapability::Spawn { allowed_kinds: s })) => {
            d.extend(s.iter().cloned());
        }
        (Skill(SkillCapability::Spawn { allowed_skills: d }), Skill(SkillCapability::Spawn { allowed_skills: s })) => {
            d.extend(s.iter().cloned());
        }
        (TaskList { mode: d }, TaskList { mode: s }) => {
            if mode_rank(s, true) > mode_rank(d, true) { *d = s.clone(); }
        }
        (Plan { mode: d }, Plan { mode: s }) => {
            if mode_rank(s, false) > mode_rank(d, false) { *d = s.clone(); }
        }
        _ => {} // custom: same-kind ⟹ same name+params (same_kind contract) — keep dst
    }
}

/// Canonicalize one merged cap's lists; `None` if its grant list is empty (⊥).
fn canon_one(c: &Capability) -> Option<Capability> {
    use Capability::*;
    fn sorted(v: &[String]) -> Vec<String> { let mut o = v.to_vec(); o.sort(); o.dedup(); o }
    let cc = match c {
        Filesystem(FsCapability::Read { paths }) => Filesystem(FsCapability::Read { paths: glob_canon(paths) }),
        Filesystem(FsCapability::Exec { paths }) => Filesystem(FsCapability::Exec { paths: glob_canon(paths) }),
        Filesystem(FsCapability::Write { paths, max_bytes }) =>
            Filesystem(FsCapability::Write { paths: glob_canon(paths), max_bytes: *max_bytes }),
        Network(NetCapability::Http { hosts, methods }) =>
            Network(NetCapability::Http { hosts: sorted(hosts), methods: sorted(methods) }),
        Process(ProcessCapability::Spawn { commands }) =>
            Process(ProcessCapability::Spawn { commands: sorted(commands) }),
        Agent(AgentCapability::Spawn { allowed_kinds }) =>
            Agent(AgentCapability::Spawn { allowed_kinds: sorted(allowed_kinds) }),
        Skill(SkillCapability::Spawn { allowed_skills }) =>
            Skill(SkillCapability::Spawn { allowed_skills: sorted(allowed_skills) }),
        other => other.clone(),
    };
    let empty = match &cc {
        Filesystem(FsCapability::Read { paths } | FsCapability::Exec { paths } | FsCapability::Write { paths, .. }) =>
            paths.is_empty(),
        Network(NetCapability::Http { hosts, methods }) => hosts.is_empty() || methods.is_empty(),
        Process(ProcessCapability::Spawn { commands }) => commands.is_empty(),
        Agent(AgentCapability::Spawn { allowed_kinds }) => allowed_kinds.is_empty(),
        Skill(SkillCapability::Spawn { allowed_skills }) => allowed_skills.is_empty(),
        _ => false,
    };
    if empty { None } else { Some(cc) }
}

fn render_cap(c: &Capability) -> String {
    // stable string key for ordering; not a wire format
    let mut s = String::from(kind_str(c));
    match c {
        Capability::Filesystem(FsCapability::Read { paths })
        | Capability::Filesystem(FsCapability::Exec { paths })
        | Capability::Filesystem(FsCapability::Write { paths, .. }) => {
            for p in paths { s.push('|'); s.push_str(p); }
        }
        Capability::Network(NetCapability::Http { hosts, methods }) => {
            for h in hosts { s.push('|'); s.push_str(h); }
            for m in methods { s.push('#'); s.push_str(m); }
        }
        Capability::Custom { name, .. } => { s.push('|'); s.push_str(name); }
        _ => {}
    }
    s
}
```

Update `crates/tau-domain/src/lib.rs` re-export to include `meet` and `canon_caps`:

```rust
pub use package::capability::lattice::{canon_caps, capability_subset, meet, CeilingViolation};
```

- [ ] **Step 4: Run tests (incl. proptest)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-domain lattice::`
Expected: PASS — all four property tests plus examples. If `prop_lattice_law` fails, the counterexample points at a canon/meet gap; fix `canon_caps`/`glob_canon` (do NOT weaken the property).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-domain/src/
git -c user.name="LEBOCQ Titouan" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-domain): capability-set meet + lattice property tests (D3 task 5)"
```

---

### Task 6: Delete the sampler; delegate tau-pkg; fix flipped tests

**Files:**
- Delete: `crates/tau-pkg/src/capability_override/glob_subset.rs`
- Modify: `crates/tau-pkg/src/capability_override/subset.rs` (delegate)
- Modify: `crates/tau-pkg/src/capability_override/mod.rs` (remove `mod glob_subset`, re-export from domain)

**Interfaces:**
- Consumes: `tau_domain::{capability_subset, CeilingViolation}` (T4), `tau_domain::package::capability::lattice::glob::glob_subset_set` (T1).
- Produces: unchanged public surface of `tau_pkg::capability_override` — `capability_set_subset`, `CeilingViolation`, `paths_subset`, `string_set_subset`, `max_bytes_le`, `compute_effective` keep their signatures so 1.4/1.5 and `compute_effective` callers don't change.

- [ ] **Step 1: Repoint `paths_subset` and re-exports (delegation)**

In `crates/tau-pkg/src/capability_override/subset.rs`, replace the `use super::glob_subset::is_glob_subset_set;` line and `paths_subset`:

```rust
// was: use super::glob_subset::is_glob_subset_set;
use tau_domain::package::capability::lattice::glob::glob_subset_set;

pub(crate) fn paths_subset(child: &[String], parent: &[String]) -> Result<(), String> {
    glob_subset_set(child, parent)
}
```

Replace the local `capability_set_subset` + `CeilingViolation` definitions with a re-export delegating to the domain (keep the `capability_set_subset` name as the alias 1.4/1.5 import):

```rust
pub use tau_domain::{capability_subset as capability_set_subset, CeilingViolation};
```

Remove the now-dead local `kind_str`/`same_kind`/`cap_subset_against`/`gather_*`/`mode_*`/`most_permissive_max_bytes` from `subset.rs` **only if** `compute_effective` does not import them; if `compute_effective` (mod.rs) uses `paths_subset`/`string_set_subset`/`max_bytes_le`, keep exactly those three and delete the rest. (Verify with `grep -n "subset::" crates/tau-pkg/src/capability_override/mod.rs`.)

- [ ] **Step 2: Delete the sampler and its module decl**

```bash
git rm crates/tau-pkg/src/capability_override/glob_subset.rs
```

In `crates/tau-pkg/src/capability_override/mod.rs` remove line `pub(crate) mod glob_subset;` and keep `pub use subset::{capability_set_subset, CeilingViolation};` (now re-exported from domain via subset.rs).

- [ ] **Step 3: Build; fix flipped tests**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-pkg`
Expected: PASS (delegation compiles).

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg capability_override`
Expected: Some `subset.rs`/`mod.rs` tests that asserted sampling-era admissions now fail. For each failing test, confirm the new verdict is the *sound* one and update the assertion. Known flips to expect (update, with a comment `// D3: outside G2 → fail-closed`):
- any `paths_subset` test admitting a `?`/`[...]`/intra-segment child
- `compute_effective` tests whose fixtures used `foo*`-style intra-segment paths (rewrite the fixture to a G2 pattern OR assert the now-correct rejection)

Do NOT change a test to make it green without confirming the sound verdict against §3/§4 of the design.

- [ ] **Step 4: Run tau-pkg + tau-cli test suites**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg`
Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli governance`
Expected: PASS. Any 1.4/1.5 governance test that flips is a real behavior change — update the assertion and note it in the commit body.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/
git -c user.name="LEBOCQ Titouan" -c user.email="lebocq.tit@gmail.com" \
  commit -m "refactor(tau-pkg): delegate capability subset to sound tau-domain primitive; delete sampler (D3 task 6)

Flipped sampling-era admissions to sound fail-closed verdicts:
<list each updated test>"
```

---

### Task 7: no_std gate + workspace verification

**Files:** none (verification only)

- [ ] **Step 1: no_std build of tau-domain with the new module**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-domain --no-default-features`
Expected: PASS. If it fails on `std::`/`HashMap`, replace with `alloc` equivalents — the lattice module must be `no_std`-clean.

- [ ] **Step 2: wasm cross-compile sanity (matches CI std-free gate)**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-domain --no-default-features --target wasm32-unknown-unknown`
Expected: PASS (skip only if the target isn't installed; note it if so).

- [ ] **Step 3: clippy on both crates**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-domain -p tau-pkg -- -D warnings`
Expected: PASS.

- [ ] **Step 4: doctests (re-exports)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-domain --doc`
Expected: PASS.

- [ ] **Step 5: Final commit / push**

```bash
git -c user.name="LEBOCQ Titouan" -c user.email="lebocq.tit@gmail.com" \
  commit --allow-empty -m "test(d3): no_std + wasm + clippy green (D3 task 7)"
git push -u origin feat/d3-sound-capability-lattice
```

Then open a PR (`gh pr create --base main`) noting: (1) the sampler is deleted, (2) the enumerated `tau check` verdict flips, (3) this unblocks D1-C.

---

## Self-Review

**Spec coverage:**
- §1 API (`capability_subset`, `meet`, location, re-exports) → Tasks 4, 5.
- §2 G2 grammar + normalization → Task 1 (`parse_arm`, `brace_expand`).
- §3 subset table → Task 1 (`subset_segs`).
- §4 meet intersection + canon + lattice law → Tasks 2, 5.
- §5 host sub-grammar → Task 3.
- §6 tests (property, witness, migrated, no_std gate, delegation) → Tasks 1–7.
- §7 scope boundary (D1-C/D5/3.4 out) → respected; no clamp/runtime code here.
- §Risk (flipped `tau check`) → Task 6 Step 3–4 enumerate and update.

**Placeholder scan:** the only intentional "fill-in" is Task 6's commit-body `<list each updated test>` and Step 1's conditional dead-code deletion — both require the engineer to read actual compiler output, which cannot be predicted here; instructions say exactly how to decide. No other TBDs.

**Type consistency:** `glob_subset_set(&[String],&[String])->Result<(),String>` (T1) is what `paths_subset` (T6) and `capability_subset` (T4) call. `glob_meet`/`glob_canon`/`host_meet` (T2/T3) are what `meet`/`canon_caps` (T5) call. `CeilingViolation{kind,offender,reason}` is defined in T4 and re-exported in T6. `Segment`/`Pattern` are produced in T1 and consumed in T2. `same_kind`/`mode_rank`/`kind_str` are moved in T4 and reused by `meet` in T5 (same module). Consistent.
