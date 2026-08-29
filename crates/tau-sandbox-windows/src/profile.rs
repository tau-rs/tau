//! AppContainer profile generation from a [`CapabilityPlan`].
//!
//! Pure functions — no I/O, no Win32 calls. Tested on any platform.

use tau_domain::{Capability, FsCapability, NetCapability};
use tau_ports::CapabilityPlan;

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
    /// Whether the plan requests outbound HTTP. When true, the spawn layer
    /// spawns a per-container named-pipe proxy and routes the command
    /// through `tau-net-bridge-win` (`launcher -- bridge --pipe <name> --
    /// <orig> <args...>`); the bridge dials the pipe and relays the
    /// plugin's traffic to the host-side `tau_sandbox_proxy` allowlist
    /// enforcement. **No capability SIDs are added** (spike #626:
    /// same-package loopback and SID-ACL'd pipes need none, and
    /// `internetClient` would allow bypassing the allowlist).
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

/// The write paths of `plan` whose *raw* capability spelling was a
/// directory subtree (`<dir>/**`, `<dir>/*`, or a trailing `/`),
/// normalised exactly like [`build_appcontainer_caps`] normalises them.
///
/// The Windows spawn layer creates these directories when they are
/// missing, before granting the AppContainer SID write access on them.
/// Two facts make that necessary rather than optional:
///
/// - `SetNamedSecurityInfoW` fails with `ERROR_FILE_NOT_FOUND`
///   (`WIN32_ERROR(2)`) on a path that does not exist, so the grant —
///   and with it the whole `wrap_spawn` — fails closed. `tau-pkg`'s
///   build envelope grants write on `<package>/target/**`, which cargo
///   has not created yet at wrap time; #622's CI round 1 failed exactly
///   there.
/// - The container cannot create the directory itself: the *parent* is
///   not write-granted, by design.
///
/// Only glob-shaped entries qualify. A bare path (`C:\out\report.txt`)
/// may well name a file the plan wants created, and silently turning it
/// into a directory would be worse than the current loud failure — so
/// those still fail closed.
pub fn dir_shaped_write_paths(plan: &CapabilityPlan) -> Vec<String> {
    let mut out = Vec::new();
    for cap in &plan.capabilities {
        if let Capability::Filesystem(FsCapability::Write { paths, .. }) = cap {
            for p in paths {
                if p.ends_with("/**") || p.ends_with("/*") || p.ends_with('/') {
                    out.push(clean_glob_suffix(p));
                }
            }
        }
    }
    out
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
    fn dir_shaped_write_paths_only_matches_globs() {
        let plan = plan_from(json!([
            { "kind": "fs.write", "paths": [
                "C:\\pkg\\target/**", "/var/cache/*", "/srv/out/", "C:\\out\\report.txt"
            ] },
            // Read globs must NOT be included: only write grants are
            // allowed to materialise a directory.
            { "kind": "fs.read", "paths": ["/etc/cfg/**"] }
        ]));
        assert_eq!(
            dir_shaped_write_paths(&plan),
            vec![
                "C:\\pkg\\target".to_string(),
                "/var/cache".to_string(),
                "/srv/out".to_string(),
            ]
        );
    }

    #[test]
    fn dir_shaped_write_paths_normalises_like_build_caps() {
        // Whatever this returns must be comparable against
        // `AppContainerCaps::fs_write_paths` entry-for-entry — the spawn
        // layer looks paths up in a set built from it.
        let plan = plan_from(json!([
            { "kind": "fs.write", "paths": ["/a/b/**", "/c/d"] }
        ]));
        let caps = build_appcontainer_caps(&plan);
        let dirs = dir_shaped_write_paths(&plan);
        assert_eq!(
            caps.fs_write_paths,
            vec!["/a/b".to_string(), "/c/d".to_string()]
        );
        assert_eq!(dirs, vec!["/a/b".to_string()]);
        assert!(caps.fs_write_paths.contains(&dirs[0]));
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
}
