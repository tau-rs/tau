//! Host-only translation of allow-bounded capabilities into a wasmtime
//! `WasiCtx` configuration (EPIC 3.3). Pure: no wasmtime types leak in here.

use std::collections::BTreeSet;

/// Network egress policy folded across all of a component's capabilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostAccess {
    /// No network capability present — deny all egress.
    DenyAll,
    /// Some `net.http` cap authorized `hosts = "any"` — unrestricted egress.
    Any,
    /// Union of exact authorized host authorities (`host` or `host:port`).
    Only(BTreeSet<String>),
}

impl HostAccess {
    /// True iff `authority` (a `host` or `host:port` string) may be reached.
    pub fn permits(&self, authority: &str) -> bool {
        match self {
            HostAccess::DenyAll => false,
            HostAccess::Any => true,
            HostAccess::Only(hosts) => hosts.contains(authority),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permits_matches_policy() {
        assert!(!HostAccess::DenyAll.permits("a.com"));
        assert!(HostAccess::Any.permits("a.com"));
        let only = HostAccess::Only(["a.com".into(), "b.com:8443".into()].into());
        assert!(only.permits("a.com"));
        assert!(only.permits("b.com:8443"));
        assert!(!only.permits("b.com")); // port is part of the authority
        assert!(!only.permits("c.com"));
    }
}
