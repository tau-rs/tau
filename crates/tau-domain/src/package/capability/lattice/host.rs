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
    out.sort();
    out.dedup();
    out
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
}
