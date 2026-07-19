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

/// `child ⊆ parent` on raw host strings: exact-equal, or child contained
/// under a `*.suffix` parent. Any un-parseable operand → false (fail-closed).
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

/// Each child host ⊆ some parent host. `Err(child)` on first offender.
pub fn host_subset_set(children: &[String], parents: &[String]) -> Result<(), String> {
    for c in children {
        if !parents.iter().any(|p| host_subset(c, p)) {
            return Err(c.clone());
        }
    }
    Ok(())
}

/// meet = for each pair, the more specific host if one ⊆ the other, else
/// nothing contributed (the host grammar has no cross-produced pattern).
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
    host_canon(&out)
}

/// Canonical host list: drop un-parseable hosts, absorb any host that is a
/// subset of another (e.g. `api.example.com` under `*.example.com`), sort,
/// dedup. Shared by `host_meet` and the capability canonicalizer so `meet`
/// and `canon` agree structurally (mirrors `glob::glob_canon`).
pub fn host_canon(hosts: &[String]) -> Vec<String> {
    let parsed: Vec<&String> = hosts.iter().filter(|h| parse(h).is_some()).collect();
    let mut kept: Vec<String> = Vec::new();
    'outer: for (i, hi) in parsed.iter().enumerate() {
        for (j, hj) in parsed.iter().enumerate() {
            // drop hi if it is ⊆ some other hj; for an equal pair, keep the
            // earlier index (the `j < i` guard leaves exactly one survivor).
            if i != j && host_subset(hi, hj) && !(host_subset(hj, hi) && j < i) {
                continue 'outer;
            }
        }
        kept.push((*hi).clone());
    }
    kept.sort();
    kept.dedup();
    kept
}

fn push_unique(v: &mut Vec<String>, s: &str) {
    if !v.iter().any(|e| e == s) { v.push(s.to_string()); }
}

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
    #[test] fn canon_absorbs_redundant_host() {
        // api.example.com ⊆ *.example.com → absorbed
        assert_eq!(host_canon(&["*.example.com".into(), "api.example.com".into()]),
                   vec!["*.example.com".to_string()]);
    }
    #[test] fn meet_absorbs_redundant_result() {
        // a=*.example.com ∩ b=(*.example.com ∪ api.example.com) = *.example.com
        assert_eq!(host_meet(&["*.example.com".into()],
                             &["*.example.com".into(), "api.example.com".into()]),
                   vec!["*.example.com".to_string()]);
    }
}
