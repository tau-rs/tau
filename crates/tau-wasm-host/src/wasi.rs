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

use std::path::{Path, PathBuf};

use tau_domain::Capability;
use tau_ports::target::wasi_map::{map_capability, Preopen, PreopenAccess, WasiConfig};

/// One filesystem preopen the host will grant the guest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreopenGrant {
    /// Real host directory to open (sandbox_root joined with the guest dir).
    pub host_path: PathBuf,
    /// Path as the guest sees it (the glob's static prefix directory).
    pub guest_path: String,
    /// Read-only or read-write.
    pub access: PreopenAccess,
}

/// The full WASI grant set derived from a component's allow-bounded caps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasiGrants {
    pub hosts: HostAccess,
    pub preopens: Vec<PreopenGrant>,
}

/// The longest leading directory prefix of a glob pattern containing no glob
/// metacharacter (`*`, `?`, `[`). Segments up to the first glob segment are
/// kept verbatim; a pattern with no glob metacharacter is returned whole — its
/// named path IS the preopen (tighter than preopening a parent directory, so
/// the guest never gains authority over siblings the capability didn't name).
/// `/data/**` -> `/data`; `/data/*.txt` -> `/data`; `/out` -> `/out`;
/// `/data/logs` -> `/data/logs`; `/a/b/c.txt` -> `/a/b/c.txt`.
fn glob_prefix_dir(pattern: &str) -> String {
    let mut dir = String::from("/");
    for seg in pattern.trim_start_matches('/').split('/') {
        if seg.is_empty() || seg.contains(['*', '?', '[']) {
            break;
        }
        if dir.len() > 1 {
            dir.push('/');
        }
        dir.push_str(seg);
    }
    dir
}

/// Fold the caps' [`WasiConfig`]s into a [`WasiGrants`]. Reuses E3.1's
/// [`map_capability`]; hardware / in-guest / host-mediated caps contribute
/// nothing (they carry `WasiConfig::None`).
pub fn wasi_grants_from_caps(
    caps: &[Capability],
    sandbox_root: &Path,
) -> Result<WasiGrants, crate::WasmHostError> {
    use tau_ports::target::wasi_map::Disposition;

    let mut any = false;
    let mut exact: BTreeSet<String> = BTreeSet::new();
    let mut has_net = false;
    // guest_path -> access, RW wins over RO for the same dir.
    let mut preopen_map: std::collections::BTreeMap<String, PreopenAccess> =
        std::collections::BTreeMap::new();

    for cap in caps {
        let mapping = map_capability(cap);
        if let Disposition::Unsupported { reason } = &mapping.disposition {
            return Err(crate::WasmHostError::UnsupportedCap {
                reason: reason.to_string(),
            });
        }
        match mapping.config {
            WasiConfig::None => {}
            WasiConfig::AllowedHosts { hosts, .. } => {
                has_net = true;
                if hosts.is_any() {
                    any = true;
                } else {
                    exact.extend(hosts.exact_hosts());
                }
            }
            WasiConfig::Preopens(preopens) => {
                for Preopen { paths, access } in preopens {
                    for pat in paths {
                        let guest_path = glob_prefix_dir(&pat);
                        let entry = preopen_map
                            .entry(guest_path)
                            .or_insert(PreopenAccess::ReadOnly);
                        if access == PreopenAccess::ReadWrite {
                            *entry = PreopenAccess::ReadWrite;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let hosts = if any {
        HostAccess::Any
    } else if has_net {
        HostAccess::Only(exact)
    } else {
        HostAccess::DenyAll
    };

    let preopens = preopen_map
        .into_iter()
        .map(|(guest_path, access)| PreopenGrant {
            host_path: sandbox_root.join(guest_path.trim_start_matches('/')),
            guest_path,
            access,
        })
        .collect();

    Ok(WasiGrants { hosts, preopens })
}

#[cfg(test)]
mod grant_tests {
    use super::*;
    use tau_domain::fixtures::{cap_fs_read, cap_fs_write, cap_net_http};

    #[test]
    fn glob_prefix_rule() {
        assert_eq!(glob_prefix_dir("/data/**"), "/data");
        assert_eq!(glob_prefix_dir("/out"), "/out");
        assert_eq!(glob_prefix_dir("/data/*.txt"), "/data");
        assert_eq!(glob_prefix_dir("/data/logs"), "/data/logs");
        assert_eq!(glob_prefix_dir("/a/b/c.txt"), "/a/b/c.txt");
    }

    #[test]
    fn no_net_cap_is_deny_all() {
        let g = wasi_grants_from_caps(&[cap_fs_read(&["/data/**"])], Path::new("/tmp/root")).unwrap();
        assert_eq!(g.hosts, HostAccess::DenyAll);
    }

    #[test]
    fn exact_hosts_become_only() {
        let g = wasi_grants_from_caps(
            &[cap_net_http(&["a.com", "b.com"], &[])],
            Path::new("/tmp/root"),
        )
        .unwrap();
        assert_eq!(
            g.hosts,
            HostAccess::Only(["a.com".into(), "b.com".into()].into())
        );
    }

    #[test]
    fn fs_read_maps_to_readonly_preopen_under_root() {
        let g = wasi_grants_from_caps(&[cap_fs_read(&["/data/**"])], Path::new("/tmp/root")).unwrap();
        assert_eq!(g.preopens.len(), 1);
        let p = &g.preopens[0];
        assert_eq!(p.guest_path, "/data");
        assert_eq!(p.host_path, PathBuf::from("/tmp/root/data"));
        assert_eq!(p.access, PreopenAccess::ReadOnly);
    }

    #[test]
    fn any_host_cap_yields_any_policy() {
        // `cap_net_http(&["any"], &[])` yields `HostSet::Any` → permit any host.
        let g = wasi_grants_from_caps(&[cap_net_http(&["any"], &[])], Path::new("/tmp/root"))
            .unwrap();
        assert_eq!(g.hosts, HostAccess::Any);
        assert!(g.hosts.permits("anything.example:443"));
    }

    #[test]
    fn fs_write_wins_over_read_for_same_dir() {
        // Both caps name the `/data` glob dir; the two preopens merge and RW wins.
        let g = wasi_grants_from_caps(
            &[cap_fs_read(&["/data/**"]), cap_fs_write(&["/data/**"], None)],
            Path::new("/tmp/root"),
        )
        .unwrap();
        assert_eq!(g.preopens.len(), 1);
        assert_eq!(g.preopens[0].guest_path, "/data");
        assert_eq!(g.preopens[0].access, PreopenAccess::ReadWrite);
    }
}
