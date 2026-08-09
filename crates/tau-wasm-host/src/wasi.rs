//! Host-only translation of an allow-bounded `WasiConfiguration` (EPIC 3.3,
//! from tau-ports' `resolve_wasi_config`) into the egress gate and preopen set
//! the wasmtime embedder installs. Pure: no wasmtime types appear here, so the
//! enforcement decisions are unit-testable in isolation — yet these are the
//! exact objects `lib.rs` hands to the linker.

use std::collections::BTreeSet;

use tau_domain::{HostSet, HttpMethod};
use tau_ports::target::{PreopenAccess, WasiConfiguration};

/// The network egress gate. `lib.rs`'s `WasiHttpHooks::send_request` consults
/// it before wasmtime opens any socket for a `wasi:http` outgoing request.
#[derive(Debug, Clone)]
pub struct HttpHostGate {
    allowed: HostSet,
    methods: Option<BTreeSet<HttpMethod>>,
}

impl HttpHostGate {
    /// Build from the folded config.
    pub fn new(cfg: &WasiConfiguration) -> Self {
        Self {
            allowed: cfg.allowed_hosts.clone(),
            methods: cfg.methods.clone(),
        }
    }

    /// True iff a `wasi:http` request to `authority` (`host` or `host:port`)
    /// with `method` is authorized. `HostSet::Exact(∅)` (deny-all) rejects
    /// every host; `HostSet::Any` permits every host; `methods == None`
    /// permits every method, else only members of the set.
    pub fn allows(&self, authority: &str, method: &HttpMethod) -> bool {
        let host_ok =
            self.allowed.is_any() || self.allowed.exact_hosts().iter().any(|h| h == authority);
        let method_ok = match &self.methods {
            None => true,
            Some(set) => set.contains(method),
        };
        host_ok && method_ok
    }
}

/// The exact preopen set the embedder grants: `(host_dir, access)` per
/// `ResolvedPreopen`, identity-mapped (guest path == host_dir; #533's
/// `host_dir` is already absolute and glob-resolved). `lib.rs`'s
/// `build_wasi_ctx` consumes this same list.
pub fn preopen_dirs(cfg: &WasiConfiguration) -> Vec<(&str, PreopenAccess)> {
    cfg.preopens
        .iter()
        .map(|p| (p.host_dir.as_str(), p.access.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_domain::fixtures::{cap_fs_write, cap_net_http};
    use tau_ports::target::resolve_wasi_config;

    fn gate(caps: &[tau_domain::Capability]) -> HttpHostGate {
        HttpHostGate::new(&resolve_wasi_config(caps))
    }

    // THE enforcement test — written first. An authority the caps did not
    // grant is not permitted; the granted one is.
    #[test]
    fn ungranted_host_denied() {
        let g = gate(&[cap_net_http(&["api.example.com"], &[])]);
        assert!(
            g.allows("api.example.com", &HttpMethod::Get),
            "granted host permitted"
        );
        assert!(
            !g.allows("evil.example.com", &HttpMethod::Get),
            "un-granted host denied"
        );
    }

    #[test]
    fn deny_all_denies_every_host() {
        let g = HttpHostGate::new(&WasiConfiguration::deny_all());
        assert!(!g.allows("api.example.com", &HttpMethod::Get));
        assert!(!g.allows("anything", &HttpMethod::Post));
    }

    #[test]
    fn any_host_permits_all() {
        let g = gate(&[cap_net_http(&["any"], &[])]);
        assert!(g.allows("whatever.example:443", &HttpMethod::Get));
    }

    #[test]
    fn method_outside_set_denied() {
        // net.http restricted to GET on api.example.com.
        let g = gate(&[cap_net_http(&["api.example.com"], &["GET"])]);
        assert!(g.allows("api.example.com", &HttpMethod::Get));
        assert!(
            !g.allows("api.example.com", &HttpMethod::Post),
            "un-granted method denied"
        );
    }

    #[test]
    fn preopens_exactly_granted() {
        // fs.write "/work/**" resolves to a single RW preopen at /work.
        let cfg = resolve_wasi_config(&[cap_fs_write(&["/work/**"], None)]);
        assert_eq!(
            preopen_dirs(&cfg),
            vec![("/work", PreopenAccess::ReadWrite)]
        );
    }

    #[test]
    fn deny_all_config_grants_nothing() {
        let cfg = WasiConfiguration::deny_all();
        assert!(preopen_dirs(&cfg).is_empty(), "no preopens");
        assert!(
            !HttpHostGate::new(&cfg).allows("h", &HttpMethod::Get),
            "no egress"
        );
    }
}
