//! Per-host network filtering helpers for the Strict sandbox tier.
//!
//! # Sub-project F — per-host egress filtering
//!
//! Two decisions are made at plan-evaluation time (parent process, before fork):
//!
//! 1. **Unshare flags** (`unshare_flags_for_plan`):
//!    - Always `CLONE_NEWUSER | CLONE_NEWNET` (F task 6.5).
//!    - No `Capability::Network(Http)` → netns is empty; seccomp blocks all
//!      socket syscalls, giving full network isolation.
//!    - `Capability::Network(Http)` present → `validate_plan` has confirmed
//!      F prerequisites are available; `apply_post_spawn` configures the netns
//!      with per-host nftables filtering before the child is released.
//!
//! 2. **Seccomp socket syscalls** (`extend_with_network_rules`):
//!    - No `Capability::Network(Http)` → `SYS_socket`, `SYS_connect`,
//!      `SYS_getpeername`, `SYS_getsockname` are **absent** from the baseline
//!      allow-list, so seccomp `KillProcess` fires on any socket attempt.
//!    - `Capability::Network(Http)` present → adds the 4 client-side syscalls
//!      so HTTP plugins can open TCP connections, plus the 4 server-side
//!      syscalls (`SYS_bind`, `SYS_listen`, `SYS_accept`, `SYS_accept4`)
//!      that `tau-net-bridge` needs. Per ADR-0020, the strict-tier
//!      `wrap_spawn` rebuilds the plugin Command to `execve` the bridge,
//!      which listens on `127.0.0.1:8443` inside the netns and proxies
//!      CONNECT/HTTP traffic via a host-side Unix socket. The bridge
//!      inherits the seccomp filter via `execve`, so its server-side
//!      syscalls must be allowed.

use std::collections::BTreeMap;

use nix::sched::CloneFlags;
use seccompiler::SeccompRule;
use tau_domain::{Capability, NetCapability};
use tau_ports::CapabilityPlan;

/// Returns the flags to pass to `unshare(2)` for the given plan.
///
/// Always returns `CLONE_NEWUSER | CLONE_NEWNET`.
///
/// F task 6.5: always isolate the child into a fresh empty netns.
/// - Plans without `Network(Http)`: the netns is empty; seccomp blocks all
///   socket syscalls, so the child has no network access.
/// - Plans with `Network(Http)`: `validate_plan` has already confirmed that
///   F prerequisites are available; `apply_post_spawn` will configure the
///   netns with per-host nftables filtering before the child is released
///   from the sync-pipe.
pub(crate) fn unshare_flags_for_plan(plan: &CapabilityPlan) -> CloneFlags {
    let _ = plan;
    // F task 6.5: always include CLONE_NEWUSER | CLONE_NEWNET.
    // validate_plan rejects Network(Http) plans on F-unavailable hosts;
    // if we reach here with Network(Http), F is available and
    // apply_post_spawn will configure the netns.
    // For plans without Network(Http), the netns is empty and seccomp
    // blocks all socket syscalls.
    CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNET
}

/// Extend the seccomp rules map with network-related syscalls for the given plan.
///
/// If the plan contains **any** `Capability::Network(Http)` capability, the
/// following syscalls are added to the allow-list:
/// - `SYS_socket`, `SYS_connect` — open TCP/UDP sockets and connect to peers.
/// - `SYS_getpeername`, `SYS_getsockname` — query socket addresses.
/// - `SYS_bind`, `SYS_listen`, `SYS_accept`, `SYS_accept4` — server-side syscalls
///   for the bridge.
///
/// Per ADR-0020, the strict-tier `wrap_spawn` rebuilds the plugin Command
/// to `execve` `tau-net-bridge`, which listens on `127.0.0.1:8443` inside
/// the empty netns and proxies CONNECT/HTTP traffic through a host-side
/// Unix socket. The bridge inherits the seccomp filter via `execve`, so
/// its server-side syscalls need to be allowed when `Network(Http)` is
/// in the plan. Plugins themselves remain HTTP clients; the server-side
/// syscalls are dead code from the plugin's perspective unless the plugin
/// invokes them directly (which no current real plugin does).
///
/// If the plan has no network capability, these syscalls are **not** added; the
/// baseline already excludes them, so any socket attempt triggers seccomp
/// `KillProcess`.
///
/// Note: `sendto`/`recvfrom`/`sendmsg`/`recvmsg` are in the baseline (needed for
/// plugin IPC) and therefore available to `Network(Http)` plans, covering UDP-
/// based transports (DNS resolution, HTTP/3).
///
/// # Arguments
///
/// - `rules` — mutable reference to the rules map built by `baseline_syscall_map`.
/// - `plan` — the sandbox plan to inspect for network capabilities.
pub(crate) fn extend_with_network_rules(
    rules: &mut BTreeMap<i64, Vec<SeccompRule>>,
    plan: &CapabilityPlan,
) {
    let has_http = plan
        .capabilities
        .iter()
        .any(|c| matches!(c, Capability::Network(NetCapability::Http { .. })));

    if !has_http {
        return;
    }

    // Allow socket-family syscalls needed by HTTP clients, plus server-side
    // syscalls for the bridge.
    let net_syscalls: &[i64] = &[
        // Client-side (HTTP plugin egress + bridge proxy dial)
        libc::SYS_socket,
        libc::SYS_connect,
        libc::SYS_getpeername,
        libc::SYS_getsockname,
        // Bridge server-side: per ADR-0020, the strict-tier wrap_spawn
        // rebuilds the plugin Command to execve tau-net-bridge, which
        // listens on 127.0.0.1:8443 inside the netns. Bridge inherits
        // the seccomp filter via execve, so its TcpListener::bind +
        // accept loop needs these syscalls.
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_accept,
        libc::SYS_accept4,
        // Bridge runtime syscalls discovered via strace under T0c e2e
        // testing. The bridge's tokio current-thread runtime (used by
        // bring_lo_up's rtnetlink request) installs a POSIX-timer based
        // sleep wheel and uses ioctl on its accept loop's socket; on
        // child-process teardown rt_sigsuspend is called to wait for
        // SIGCHLD. None of these are in the baseline syscall map (which
        // is tuned for plugin clients, not for the bridge process), so
        // we extend them under the same Network(Http) gate that already
        // pulls in bind/listen/accept.
        libc::SYS_timer_create,
        libc::SYS_timer_settime,
        libc::SYS_ioctl,
        libc::SYS_rt_sigsuspend,
    ];

    for &nr in net_syscalls {
        rules.entry(nr).or_default();
    }
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::*;
    use crate::strict::baseline_syscall_map;

    // ---- unshare_flags_for_plan tests ----

    /// An empty plan (no capabilities) should yield both CLONE_NEWUSER and
    /// CLONE_NEWNET (sub-project F: always isolate into a fresh netns).
    #[test]
    fn unshare_flags_default_no_network() {
        let plan_json = serde_json::json!({
            "capabilities": [],
            "context": null,
            "limits": null,
        });
        let plan: CapabilityPlan = serde_json::from_value(plan_json).expect("valid plan");
        let flags = unshare_flags_for_plan(&plan);
        assert!(
            flags.contains(CloneFlags::CLONE_NEWUSER),
            "must include CLONE_NEWUSER"
        );
        assert!(
            flags.contains(CloneFlags::CLONE_NEWNET),
            "must include CLONE_NEWNET for empty plan"
        );
    }

    /// A plan with Network(Http) must still include CLONE_NEWNET (F task 6.5:
    /// always isolate into a fresh netns; apply_post_spawn configures it with
    /// per-host nftables filtering before the child is released from the
    /// sync-pipe).
    #[test]
    fn unshare_flags_with_http_includes_newnet() {
        let plan_json = serde_json::json!({
            "capabilities": [{
                "kind": "net.http",
                "hosts": ["api.example.com"],
                "methods": ["GET"],
            }],
            "context": null,
            "limits": null,
        });
        let plan: CapabilityPlan = serde_json::from_value(plan_json).expect("valid plan");
        let flags = unshare_flags_for_plan(&plan);
        assert!(
            flags.contains(CloneFlags::CLONE_NEWUSER),
            "must include CLONE_NEWUSER"
        );
        assert!(
            flags.contains(CloneFlags::CLONE_NEWNET),
            "CLONE_NEWNET must be present (F task 6.5: always isolate into a fresh netns; \
             apply_post_spawn configures it with per-host nftables filtering)"
        );
    }

    /// A plan with only fs.read (no network capability) should yield both
    /// CLONE_NEWUSER and CLONE_NEWNET.
    #[test]
    fn unshare_flags_no_network_capability() {
        let plan_json = serde_json::json!({
            "capabilities": [{ "kind": "fs.read", "paths": ["/tmp"] }],
            "context": null,
            "limits": null,
        });
        let plan: CapabilityPlan = serde_json::from_value(plan_json).expect("valid plan");
        let flags = unshare_flags_for_plan(&plan);
        assert!(
            flags.contains(CloneFlags::CLONE_NEWUSER),
            "must include CLONE_NEWUSER"
        );
        assert!(
            flags.contains(CloneFlags::CLONE_NEWNET),
            "must include CLONE_NEWNET when no network capability is present"
        );
    }

    // ---- extend_with_network_rules tests ----

    /// When the plan has a Network(Http) capability, socket-family syscalls
    /// must be added to the rules map (both client-side and server-side for
    /// the bridge).
    #[test]
    fn extend_adds_socket_when_http_capability_present() {
        let plan_json = serde_json::json!({
            "capabilities": [{
                "kind": "net.http",
                "hosts": ["api.example.com"],
                "methods": ["GET"],
            }],
            "context": null,
            "limits": null,
        });
        let plan: CapabilityPlan = serde_json::from_value(plan_json).expect("valid plan");

        let mut rules = baseline_syscall_map();
        // Baseline must NOT contain socket before extension (verified by Task 4 tests).
        assert!(
            !rules.contains_key(&libc::SYS_socket),
            "precondition: SYS_socket absent in baseline"
        );

        extend_with_network_rules(&mut rules, &plan);

        // Client-side syscalls
        assert!(
            rules.contains_key(&libc::SYS_socket),
            "SYS_socket must be present after extension"
        );
        assert!(
            rules.contains_key(&libc::SYS_connect),
            "SYS_connect must be present after extension"
        );
        assert!(
            rules.contains_key(&libc::SYS_getpeername),
            "SYS_getpeername must be present after extension"
        );
        assert!(
            rules.contains_key(&libc::SYS_getsockname),
            "SYS_getsockname must be present after extension"
        );

        // Server-side syscalls must also be present (per T0a findings; tau-net-bridge needs them).
        assert!(
            rules.contains_key(&libc::SYS_bind),
            "SYS_bind must be present (bridge server-side per ADR-0020)"
        );
        assert!(
            rules.contains_key(&libc::SYS_listen),
            "SYS_listen must be present (bridge server-side per ADR-0020)"
        );
        assert!(
            rules.contains_key(&libc::SYS_accept),
            "SYS_accept must be present (bridge server-side per ADR-0020)"
        );
        assert!(
            rules.contains_key(&libc::SYS_accept4),
            "SYS_accept4 must be present (bridge server-side per ADR-0020)"
        );
    }

    /// When the plan has no network capability, extend_with_network_rules is a
    /// no-op: socket-family syscalls remain absent from the rules map.
    #[test]
    fn extend_no_op_when_no_http_capability() {
        let plan_json = serde_json::json!({
            "capabilities": [{ "kind": "fs.read", "paths": ["/tmp"] }],
            "context": null,
            "limits": null,
        });
        let plan: CapabilityPlan = serde_json::from_value(plan_json).expect("valid plan");

        let mut rules = baseline_syscall_map();
        let snapshot_before: Vec<i64> = rules.keys().copied().collect();

        extend_with_network_rules(&mut rules, &plan);

        let snapshot_after: Vec<i64> = rules.keys().copied().collect();
        assert_eq!(
            snapshot_before, snapshot_after,
            "extend_with_network_rules must be a no-op when no Network(Http) capability is present"
        );
        assert!(
            !rules.contains_key(&libc::SYS_socket),
            "SYS_socket must remain absent when no network capability"
        );
    }

    /// Bridge server-side syscalls must be added when Network(Http) is in plan.
    /// Per ADR-0020 + T0a findings: tau-net-bridge inherits the seccomp filter
    /// via execve and needs to bind+listen+accept on 127.0.0.1:8443.
    #[test]
    fn extend_adds_bridge_server_syscalls_when_http() {
        let plan_json = serde_json::json!({
            "capabilities": [{
                "kind": "net.http",
                "hosts": ["api.example.com"],
                "methods": ["GET"],
            }],
            "context": null,
            "limits": null,
        });
        let plan: CapabilityPlan = serde_json::from_value(plan_json).expect("valid plan");
        let mut rules = baseline_syscall_map();
        super::extend_with_network_rules(&mut rules, &plan);
        for nr in [
            libc::SYS_bind,
            libc::SYS_listen,
            libc::SYS_accept,
            libc::SYS_accept4,
        ] {
            assert!(
                rules.contains_key(&nr),
                "Network(Http) plan must allow bridge server-side syscall {nr} (T0a 2026-05-10)"
            );
        }
    }

    /// Server-side syscalls must NOT be added when Network(Http) is absent.
    #[test]
    fn extend_does_not_add_bridge_syscalls_without_http() {
        let plan_json = serde_json::json!({
            "capabilities": [{ "kind": "fs.read", "paths": ["/tmp"] }],
            "context": null,
            "limits": null,
        });
        let plan: CapabilityPlan = serde_json::from_value(plan_json).expect("valid plan");
        let mut rules = baseline_syscall_map();
        super::extend_with_network_rules(&mut rules, &plan);
        for nr in [
            libc::SYS_bind,
            libc::SYS_listen,
            libc::SYS_accept,
            libc::SYS_accept4,
        ] {
            assert!(
                !rules.contains_key(&nr),
                "Without Network(Http), syscall {nr} must remain absent"
            );
        }
    }

    /// Bridge runtime syscalls (timer_create, timer_settime, ioctl,
    /// rt_sigsuspend) must be added when Network(Http) is in plan.
    /// Discovered via T0c e2e strict_bridge.rs strace investigation:
    /// without these the bridge process SIGSYS-dies during tokio
    /// runtime init / waitpid teardown.
    #[test]
    fn extend_adds_bridge_runtime_syscalls_when_http() {
        let plan_json = serde_json::json!({
            "capabilities": [{
                "kind": "net.http",
                "hosts": ["api.example.com"],
                "methods": ["GET"],
            }],
            "context": null,
            "limits": null,
        });
        let plan: CapabilityPlan = serde_json::from_value(plan_json).expect("valid plan");
        let mut rules = baseline_syscall_map();
        super::extend_with_network_rules(&mut rules, &plan);
        for nr in [
            libc::SYS_timer_create,
            libc::SYS_timer_settime,
            libc::SYS_ioctl,
            libc::SYS_rt_sigsuspend,
        ] {
            assert!(
                rules.contains_key(&nr),
                "Network(Http) plan must allow bridge runtime syscall {nr} (T0c 2026-05-10)"
            );
        }
    }

    /// Bridge syscalls coexist with client-side syscalls (regression: don't
    /// accidentally remove client-side when adding server-side).
    #[test]
    fn extend_adds_both_client_and_server_syscalls_when_http() {
        let plan_json = serde_json::json!({
            "capabilities": [{
                "kind": "net.http",
                "hosts": ["api.example.com"],
                "methods": ["GET"],
            }],
            "context": null,
            "limits": null,
        });
        let plan: CapabilityPlan = serde_json::from_value(plan_json).expect("valid plan");
        let mut rules = baseline_syscall_map();
        super::extend_with_network_rules(&mut rules, &plan);
        // Client-side
        assert!(rules.contains_key(&libc::SYS_socket), "SYS_socket missing");
        assert!(
            rules.contains_key(&libc::SYS_connect),
            "SYS_connect missing"
        );
        // Server-side
        assert!(rules.contains_key(&libc::SYS_bind), "SYS_bind missing");
        assert!(rules.contains_key(&libc::SYS_listen), "SYS_listen missing");
    }
}
