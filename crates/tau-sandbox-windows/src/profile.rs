//! AppContainer profile generation from a [`CapabilityPlan`].
//!
//! Pure functions — no I/O, no Win32 calls. Tested on any platform.

use tau_domain::{Capability, FsCapability, NetCapability};
use tau_ports::CapabilityPlan;

/// TCP port the host-side `tau-sandbox-proxy` task listens on. Plugins
/// reach it via `HTTPS_PROXY=http://127.0.0.1:8443`.
//
// `dead_code` allow: on non-Windows builds the lib.rs's `wrap_spawn_windows`
// is cfg-gated out, so the const has no runtime use, but the unit test
// below still references it as a cross-platform invariant assertion.
#[allow(dead_code)]
pub(crate) const PROXY_PORT: u16 = 8443;

/// Result of [`build_appcontainer_caps`]: the AppContainer-shape inputs that
/// the spawn layer turns into Win32 calls.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AppContainerCaps {
    /// Filesystem paths the plugin needs to read. Spawn layer adds an ACL
    /// grant on each (the AppContainer SID + GENERIC_READ).
    pub fs_read_paths: Vec<String>,
    /// Filesystem paths the plugin needs to write. Spawn layer adds an ACL
    /// grant on each (the AppContainer SID + GENERIC_READ + GENERIC_WRITE).
    pub fs_write_paths: Vec<String>,
    /// Whether the plan requests outbound HTTP. When true, spawn layer:
    ///   - adds the `lpacPnpNotifications` + `privateNetworkClientServer`
    ///     well-known capability SIDs to the AppContainer (loopback only;
    ///     **NOT** `internetClient` so direct internet is blocked);
    ///   - sets `HTTPS_PROXY=http://127.0.0.1:PROXY_PORT` env so reqwest
    ///     routes via the host-side proxy task;
    ///   - spawns the proxy task before `CreateProcessAsUserW`.
    pub has_http: bool,
    /// Whether the plan grants process-spawn. AppContainer children inherit
    /// the same security context, so this doesn't widen the sandbox; we
    /// just don't restrict child spawn explicitly.
    pub has_process_spawn: bool,
}

/// Translate a `CapabilityPlan` into AppContainer-shape inputs for the spawn
/// layer. Pure; no Win32, no I/O.
pub fn build_appcontainer_caps(plan: &CapabilityPlan) -> AppContainerCaps {
    let mut fs_read_paths = Vec::new();
    let mut fs_write_paths = Vec::new();
    let mut has_http = false;
    let mut has_process_spawn = false;

    for cap in &plan.capabilities {
        match cap {
            Capability::Filesystem(FsCapability::Read { paths, .. }) => {
                for p in paths {
                    fs_read_paths.push(clean_glob_suffix(p));
                }
            }
            Capability::Filesystem(FsCapability::Write { paths, .. }) => {
                for p in paths {
                    fs_write_paths.push(clean_glob_suffix(p));
                }
            }
            Capability::Network(NetCapability::Http { .. }) => {
                has_http = true;
            }
            Capability::Process(_) => {
                has_process_spawn = true;
            }
            _ => {}
        }
    }

    AppContainerCaps {
        fs_read_paths,
        fs_write_paths,
        has_http,
        has_process_spawn,
    }
}

/// Strip trailing glob suffixes from a path. AppContainer ACLs are
/// per-directory + inherited; `/srv/data/**` and `/srv/data` both grant
/// the same scope so we normalise to the parent path.
fn clean_glob_suffix(p: &str) -> String {
    p.trim_end_matches("/**")
        .trim_end_matches("/*")
        .trim_end_matches('/')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plan_from(capabilities: serde_json::Value) -> CapabilityPlan {
        let plan_json = json!({
            "capabilities": capabilities,
            "context": null,
            "limits": null,
        });
        serde_json::from_value(plan_json).expect("decode plan")
    }

    #[test]
    fn empty_plan_emits_empty_caps() {
        let plan = plan_from(json!([]));
        let caps = build_appcontainer_caps(&plan);
        assert!(caps.fs_read_paths.is_empty());
        assert!(caps.fs_write_paths.is_empty());
        assert!(!caps.has_http);
        assert!(!caps.has_process_spawn);
    }

    #[test]
    fn fs_read_paths_collected() {
        let plan = plan_from(json!([
            { "kind": "fs.read", "paths": ["/etc/foo", "/data/cache"] }
        ]));
        let caps = build_appcontainer_caps(&plan);
        assert_eq!(
            caps.fs_read_paths,
            vec!["/etc/foo".to_string(), "/data/cache".to_string()]
        );
        assert!(caps.fs_write_paths.is_empty());
    }

    #[test]
    fn fs_write_separate_from_read() {
        let plan = plan_from(json!([
            { "kind": "fs.read",  "paths": ["/etc/cfg"] },
            { "kind": "fs.write", "paths": ["/data/scratch"] }
        ]));
        let caps = build_appcontainer_caps(&plan);
        assert_eq!(caps.fs_read_paths, vec!["/etc/cfg".to_string()]);
        assert_eq!(caps.fs_write_paths, vec!["/data/scratch".to_string()]);
    }

    #[test]
    fn glob_suffix_stripped() {
        let plan = plan_from(json!([
            { "kind": "fs.read", "paths": ["/srv/data/**", "/etc/*", "/tmp/"] }
        ]));
        let caps = build_appcontainer_caps(&plan);
        assert_eq!(
            caps.fs_read_paths,
            vec![
                "/srv/data".to_string(),
                "/etc".to_string(),
                "/tmp".to_string()
            ]
        );
    }

    #[test]
    fn http_capability_sets_flag() {
        let plan = plan_from(json!([
            { "kind": "net.http", "hosts": ["api.example.com"], "methods": ["GET"] }
        ]));
        let caps = build_appcontainer_caps(&plan);
        assert!(caps.has_http);
    }

    #[test]
    fn process_spawn_sets_flag() {
        let plan = plan_from(json!([
            { "kind": "process.spawn", "commands": ["/bin/echo"] }
        ]));
        let caps = build_appcontainer_caps(&plan);
        assert!(caps.has_process_spawn);
    }

    #[test]
    fn proxy_port_is_8443() {
        // Cross-platform invariant: the proxy port matches what the
        // Linux native + macOS darwin adapters use, so the same
        // tau-sandbox-proxy crate works for all three.
        assert_eq!(PROXY_PORT, 8443);
    }
}
