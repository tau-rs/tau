//! Host dimension of the `net.http` capability lattice.
//!
//! Since ADR-0064 (HostSet), a host grant is `HostSet::Any` (the top element)
//! or `HostSet::Exact(set)` of validated exact hostnames — there is no glob
//! grammar (suffix wildcards are deferred). The host lattice operations are
//! therefore plain set operations with a top:
//!
//! - subset: `parent.subsumes(child)` (defined on [`HostSet`]).
//! - join (union): [`host_union`] — `Any` absorbs; else set union.
//! - meet (glb): [`host_meet`] — `Any ∩ x = x`; else set intersection.
//!
//! `canon` is the identity (a `BTreeSet` is already sorted+deduped and there
//! is no glob absorption), kept for symmetry with the path dimension's
//! `glob_canon`.

use crate::package::host::{HostName, HostSet};
use alloc::collections::BTreeSet;

/// Join (union) of a set of host grants. `HostSet::Any` is the top element
/// (absorbs everything); otherwise the union of the exact host sets.
pub fn host_union<'a>(sets: impl IntoIterator<Item = &'a HostSet>) -> HostSet {
    let mut acc: BTreeSet<HostName> = BTreeSet::new();
    for s in sets {
        match s {
            HostSet::Any => return HostSet::Any,
            HostSet::Exact(hs) => acc.extend(hs.iter().cloned()),
        }
    }
    HostSet::Exact(acc)
}

/// Meet (greatest lower bound / intersection). `Any ∩ x = x`; otherwise the
/// intersection of the two exact host sets.
pub fn host_meet(a: &HostSet, b: &HostSet) -> HostSet {
    match (a, b) {
        (HostSet::Any, x) | (x, HostSet::Any) => x.clone(),
        (HostSet::Exact(sa), HostSet::Exact(sb)) => {
            HostSet::Exact(sa.intersection(sb).cloned().collect())
        }
    }
}

/// `HostSet` is already canonical (the `Exact` `BTreeSet` is sorted+deduped
/// and there is no glob absorption), so this is the identity — kept for
/// symmetry with `glob_canon`.
pub fn host_canon(hosts: &HostSet) -> HostSet {
    hosts.clone()
}

/// A host grant denotes ⊥ (grants nothing) iff it is `Exact(∅)`. `Any` is
/// never empty.
pub fn host_is_empty(hosts: &HostSet) -> bool {
    matches!(hosts, HostSet::Exact(s) if s.is_empty())
}

/// Name a host in `child` that the `union` grant does not cover, for error
/// reporting. An uncovered `Any` child reports `"any"`.
pub fn host_offender(child: &HostSet, union: &HostSet) -> alloc::string::String {
    use alloc::string::ToString;
    match child {
        HostSet::Any => "any".to_string(),
        HostSet::Exact(hs) => hs
            .iter()
            .find(|h| !union.subsumes(&HostSet::Exact(core::iter::once((*h).clone()).collect())))
            .map(|h| h.as_str().to_string())
            .unwrap_or_else(|| "host".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact(hs: &[&str]) -> HostSet {
        HostSet::Exact(hs.iter().map(|h| HostName::parse(h).unwrap()).collect())
    }

    #[test]
    fn union_any_absorbs() {
        assert_eq!(
            host_union([&exact(&["a.com"]), &HostSet::Any]),
            HostSet::Any
        );
    }

    #[test]
    fn union_of_exacts() {
        assert_eq!(
            host_union([&exact(&["a.com"]), &exact(&["b.com"])]),
            exact(&["a.com", "b.com"])
        );
    }

    #[test]
    fn meet_any_is_identity() {
        assert_eq!(
            host_meet(&HostSet::Any, &exact(&["a.com"])),
            exact(&["a.com"])
        );
        assert_eq!(host_meet(&HostSet::Any, &HostSet::Any), HostSet::Any);
    }

    #[test]
    fn meet_intersects_exacts() {
        assert_eq!(
            host_meet(&exact(&["a.com", "b.com"]), &exact(&["b.com", "c.com"])),
            exact(&["b.com"])
        );
    }

    #[test]
    fn empty_only_for_exact_empty() {
        assert!(host_is_empty(&exact(&[])));
        assert!(!host_is_empty(&exact(&["a.com"])));
        assert!(!host_is_empty(&HostSet::Any));
    }

    #[test]
    fn offender_names_uncovered_exact_host() {
        assert_eq!(
            host_offender(&exact(&["a.com", "b.com"]), &exact(&["a.com"])),
            "b.com"
        );
        assert_eq!(host_offender(&HostSet::Any, &exact(&["a.com"])), "any");
    }
}
