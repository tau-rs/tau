//! Host-only network egress policy derived from the canonical
//! [`tau_ports::target::resolve_wasi_config`] fold (EPIC 3.3). Pure: no
//! wasmtime types leak in here.

use std::collections::BTreeSet;

use tau_domain::package::host::{HostSet, HttpMethod};
use tau_ports::target::WasiConfiguration;

/// Network egress policy folded across a component's allow-bounded caps,
/// sourced from the canonical `resolve_wasi_config` output. Consulted by the
/// `WasiHttpHooks::send_request` override before any outgoing request is
/// sent.
#[derive(Debug, Clone)]
pub struct EgressPolicy {
    /// Allowed authorities. `Exact({})` (empty) denies all egress.
    pub allowed_hosts: HostSet,
    /// Allowed HTTP methods; `None` = all methods.
    pub methods: Option<BTreeSet<HttpMethod>>,
}

impl EgressPolicy {
    /// Build the policy from the canonical fold's output.
    pub fn from_config(cfg: &WasiConfiguration) -> Self {
        Self {
            allowed_hosts: cfg.allowed_hosts.clone(),
            methods: cfg.methods.clone(),
        }
    }

    /// True iff a request to `authority` with HTTP `method` (verb string) is
    /// allowed by this policy.
    pub fn permits(&self, authority: &str, method: &str) -> bool {
        // Cap hosts are lowercased by `HostName::parse`, but the runtime
        // `authority` off `request.uri().authority()` preserves the wire case.
        // Lowercase it before the exact-host compare so `A.com` matches a cap
        // for `a.com` (ports are numeric, so lowercasing the whole authority
        // is safe).
        let authority = authority.to_ascii_lowercase();
        let host_ok = self.allowed_hosts.is_any()
            || self
                .allowed_hosts
                .exact_hosts()
                .iter()
                .any(|h| h == &authority);
        let method_ok = match &self.methods {
            None => true,
            Some(set) => set.iter().any(|m| m.as_str().eq_ignore_ascii_case(method)),
        };
        host_ok && method_ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_domain::fixtures::{cap_fs_read, cap_net_http};
    use tau_ports::target::resolve_wasi_config;

    #[test]
    fn no_net_cap_denies_all_egress() {
        let caps = [cap_fs_read(&["/data/**"])];
        let cfg = resolve_wasi_config(&caps);
        let policy = EgressPolicy::from_config(&cfg);
        assert!(!policy.permits("a.com", "GET"));
    }

    #[test]
    fn exact_host_cap_enforces_host_and_method() {
        let caps = [cap_net_http(&["a.com"], &["GET"])];
        let cfg = resolve_wasi_config(&caps);
        let policy = EgressPolicy::from_config(&cfg);
        assert!(policy.permits("a.com", "GET"));
        assert!(!policy.permits("a.com", "POST"), "method must be enforced");
        assert!(!policy.permits("b.com", "GET"), "host must be enforced");
    }

    #[test]
    fn exact_host_match_is_case_insensitive() {
        // Caps lowercase hosts at parse time; a request whose authority arrives
        // in mixed case must still match. `&[]` methods = all methods allowed.
        let caps = [cap_net_http(&["a.com"], &[])];
        let cfg = resolve_wasi_config(&caps);
        let policy = EgressPolicy::from_config(&cfg);
        assert!(
            policy.permits("A.com", "GET"),
            "host match must be case-insensitive"
        );
    }

    #[test]
    fn any_host_no_methods_permits_everything() {
        let caps = [cap_net_http(&["any"], &[])];
        let cfg = resolve_wasi_config(&caps);
        let policy = EgressPolicy::from_config(&cfg);
        assert!(policy.permits("anything:443", "DELETE"));
    }
}
