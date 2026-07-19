//! G2 glob grammar: `/`-split segments over {Literal, `*` (one component),
//! `**` (trailing, any suffix)}, plus brace alternation expanded before
//! analysis. Anything outside G2 → `None`/`false` (fail-closed).

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

/// Max brace-expanded arms before we fail closed (combinatorial guard).
const MAX_ARMS: usize = 256;

/// One `/`-delimited path segment in a normalized G2 pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// A literal path component (no wildcards).
    Literal(String),
    /// `*` — matches exactly one path component.
    Star,
    /// `**` — matches any suffix (zero or more components); only valid trailing.
    StarStar,
}

/// A normalized, brace-free glob pattern: an ordered sequence of segments.
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
    cs.iter().all(|c| ps.iter().any(|p| pattern_subset(c, p)))
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
        assert_eq!(
            glob_subset_set(&children, &parents).unwrap_err(),
            "/proj/etc/**"
        );
    }
}
