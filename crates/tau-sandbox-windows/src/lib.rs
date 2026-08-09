//! Windows AppContainer sandbox adapter for tau.
//!
//! Wraps plugin commands with Windows AppContainer (Windows 8+) so the
//! plugin runs inside a kernel-level isolated container. Strict tier:
//! filesystem isolation per-capability via per-AppContainer-SID ACL
//! grants + outbound network restricted to the host-side
//! `tau-sandbox-proxy` task on `127.0.0.1:8443`.
//!
//! Compared to [`tau_sandbox_native`] (Linux landlock + seccomp + namespaces)
//! and [`tau_sandbox_darwin`] (macOS sandbox-exec):
//! - **Pros:** native Windows kernel sandboxing; same security envelope
//!   as Linux/macOS strict from the plugin's perspective; reuses
//!   `tau-sandbox-proxy` for HTTPS allowlist enforcement.
//! - **Cons:** AppContainer programming is verbose (Win32 API);
//!   development requires a Windows host (not testable on macOS dev);
//!   no per-syscall filtering (no Windows equivalent of seccomp).
//!
//! ## Development constraint
//!
//! This crate cannot be exercised on macOS or Linux. The pure-logic
//! parts (`profile`) compile and unit-test on any platform; the Win32
//! parts (`acl`, `spawn`, the runtime path of `lib`) are
//! `cfg(target_os = "windows")`-gated. Windows CI runners are the only
//! place runtime behavior is verified.

#![deny(missing_docs)]

#[cfg(target_os = "windows")]
mod acl;
mod profile;
#[cfg(target_os = "windows")]
mod spawn;

pub use profile::{build_appcontainer_caps, AppContainerCaps};

pub mod launcher_args;

/// Windows-only test helpers, gated behind the `integration-tests`
/// feature so they never ship in a release build. Exposes just enough
/// of the (otherwise private) `acl` module for
/// `tests/launcher_integration.rs` to create and delete a real
/// AppContainer profile without exposing the whole `acl` module.
#[cfg(all(target_os = "windows", feature = "integration-tests"))]
pub mod test_support {
    /// Create a real AppContainer profile named `name` (idempotent: an
    /// already-existing profile with the same name is treated as
    /// success). See [`crate::acl::create_appcontainer_profile`].
    pub fn create_profile(name: &str) -> std::io::Result<()> {
        crate::acl::create_appcontainer_profile(name).map(|_| ())
    }

    /// Delete the AppContainer profile named `name`. See
    /// [`crate::acl::delete_appcontainer_profile`].
    pub fn delete_profile(name: &str) -> std::io::Result<()> {
        crate::acl::delete_appcontainer_profile(name)
    }
}

use std::process::Command;
use std::sync::Arc;

use tokio::sync::OnceCell;

use tau_domain::{Capability, NetCapability};
use tau_ports::{
    CapabilityError, CapabilityGate, CapabilityHandle, CapabilityPlan, CapabilityProbe,
    CapabilityShapeSet, ProcessCapabilityGate,
};

/// Windows AppContainer adapter.
pub struct WindowsSandbox {
    name: String,
    /// Probe is cached lazily on the first call.
    probe_cache: Arc<OnceCell<CapabilityProbe>>,
}

impl WindowsSandbox {
    /// Construct a Windows adapter.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            probe_cache: Arc::new(OnceCell::new()),
        }
    }
}

impl CapabilityGate for WindowsSandbox {
    fn name(&self) -> &str {
        &self.name
    }

    async fn probe(&self) -> CapabilityProbe {
        self.probe_cache
            .get_or_init(|| async { run_probe().await })
            .await
            .clone()
    }

    fn supported_shapes(&self) -> CapabilityShapeSet {
        let mut set = CapabilityShapeSet::new();
        set.insert(tau_domain::CapabilityShape::FilesystemRead);
        set.insert(tau_domain::CapabilityShape::FilesystemWrite);
        set.insert(tau_domain::CapabilityShape::ProcessExec);
        set.insert(tau_domain::CapabilityShape::NetworkHttp);
        set
    }

    fn validate_plan(&self, plan: &CapabilityPlan) -> Result<(), CapabilityError> {
        let supported = self.supported_shapes();
        for cap in &plan.capabilities {
            let shape = cap.required_shape();
            if !supported.contains(&shape) {
                return Err(CapabilityError::ShapeUnsupported { shape });
            }
        }
        let mut exact: Vec<String> = Vec::new();
        for cap in &plan.capabilities {
            if let Capability::Network(NetCapability::Http { hosts, .. }) = cap {
                if !hosts.is_any() {
                    exact.extend(hosts.exact_hosts());
                }
            }
        }
        if !exact.is_empty() {
            tau_sandbox_proxy::validate_hosts(&exact).map_err(|e| CapabilityError::Proxy {
                message: format!("host validation: {e}"),
            })?;
        }
        Ok(())
    }
}

impl ProcessCapabilityGate for WindowsSandbox {
    #[cfg(target_os = "windows")]
    async fn wrap_spawn(
        &self,
        plan: &CapabilityPlan,
        cmd: &mut Command,
    ) -> Result<CapabilityHandle, CapabilityError> {
        self.validate_plan(plan)?;
        wrap_spawn_windows(plan, cmd).await
    }

    #[cfg(not(target_os = "windows"))]
    async fn wrap_spawn(
        &self,
        _plan: &CapabilityPlan,
        _cmd: &mut Command,
    ) -> Result<CapabilityHandle, CapabilityError> {
        Err(CapabilityError::Unavailable {
            reason: "tau-sandbox-windows is Windows-only".to_string(),
        })
    }
}

/// Probe for AppContainer availability. Cached per `WindowsSandbox`
/// instance via `OnceCell`.
///
/// **Phase 1:** returns `Unavailable` everywhere — the Win32 ACL calls
/// in `acl.rs` are stubs (no-ops). Returning `Unavailable` keeps the
/// resolver from picking this adapter for real work and instead falling
/// back to the next candidate (typically `PassthroughSandbox`). When
/// Phase 2 lands the real Win32 implementation, this returns
/// `Available { tier: Strict }` on Windows.
async fn run_probe() -> CapabilityProbe {
    if !cfg!(target_os = "windows") {
        return CapabilityProbe::Unavailable {
            reason: "not running on Windows".to_string(),
        };
    }
    CapabilityProbe::Unavailable {
        reason: "tau-sandbox-windows Phase 1 ships scaffold only; \
                 Win32 AppContainer calls land in Phase 2"
            .to_string(),
    }
}

#[cfg(target_os = "windows")]
async fn wrap_spawn_windows(
    plan: &CapabilityPlan,
    cmd: &mut Command,
) -> Result<CapabilityHandle, CapabilityError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let caps = build_appcontainer_caps(plan);

    // Generate a unique AppContainer profile name + SID per spawn.
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let profile_name = format!("tau-sandbox-{}-{}", std::process::id(), counter);
    let app_sid = acl::create_appcontainer_profile(&profile_name).map_err(|e| {
        CapabilityError::WrapFailed {
            message: format!("create_appcontainer_profile: {e}"),
        }
    })?;

    // Grant ACLs on plan-specified paths to the AppContainer SID.
    let mut granted_paths: Vec<(String, acl::AccessKind)> = Vec::new();
    for path in &caps.fs_read_paths {
        acl::grant_access(&app_sid, path, acl::AccessKind::Read).map_err(|e| {
            CapabilityError::WrapFailed {
                message: format!("grant read on {path}: {e}"),
            }
        })?;
        granted_paths.push((path.clone(), acl::AccessKind::Read));
    }
    for path in &caps.fs_write_paths {
        acl::grant_access(&app_sid, path, acl::AccessKind::Write).map_err(|e| {
            CapabilityError::WrapFailed {
                message: format!("grant write on {path}: {e}"),
            }
        })?;
        granted_paths.push((path.clone(), acl::AccessKind::Write));
    }

    // Phase 2: spawn the host-side proxy task if the plan needs HTTP.
    //
    // `tau_sandbox_proxy::spawn_proxy` is currently `cfg(unix)`-gated
    // because it builds on `tokio::net::UnixListener`. Making it work on
    // Windows requires either:
    //   - switching the proxy's IPC from Unix-domain sockets to TCP
    //     loopback (simpler; reuses Linux's existing port 8443
    //     convention; works on Windows 10+ since UDS support is
    //     incomplete), or
    //   - using Windows named pipes via `tokio::net::windows::named_pipe`.
    //
    // Phase 1 (this PR) skips the proxy spawn entirely. HTTP plans are
    // refused at probe time (probe returns Unavailable). The proxy
    // wiring lands in Phase 2 alongside the actual `CreateProcessAsUserW`
    // integration.
    if caps.has_http {
        return Err(CapabilityError::Unavailable {
            reason: "tau-sandbox-windows Phase 1 does not support Network(Http) plans; \
                     proxy support requires Phase 2 (UDS->TCP conversion in tau-sandbox-proxy)"
                .to_string(),
        });
    }
    let proxy_handle: Option<()> = None;

    // Configure the command for AppContainer-wrapped spawn. The actual
    // CreateProcessAsUserW call happens when the caller spawns `cmd`;
    // we attach the AppContainer security attributes via env vars that
    // `spawn::pre_exec_appcontainer` reads at spawn time. Because
    // std::process::Command on Windows doesn't expose pre_exec hooks, we
    // record the SID + caps via a thread-local and override the spawn
    // path through a `pre_exec`-style wrapper at spawn time. See spawn.rs
    // for the implementation.
    spawn::register_appcontainer_for_command(cmd, &app_sid, &caps);

    // Build the CapabilityHandle. On drop:
    // 1. revoke ACL grants in reverse order
    // 2. delete the AppContainer profile
    // 3. (Phase 2) drop the proxy guard via nest_handle
    let _ = proxy_handle; // currently always None; placeholder for Phase 2
    let cleanup_sid = app_sid.clone();
    let cleanup_profile = profile_name.clone();
    let cleanup_paths = granted_paths;
    let handle = CapabilityHandle::new(move || {
        for (path, kind) in cleanup_paths.iter().rev() {
            let _ = acl::revoke_access(&cleanup_sid, path, *kind);
        }
        let _ = acl::delete_appcontainer_profile(&cleanup_profile);
    });

    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn name_round_trip() {
        let s = WindowsSandbox::new("windows");
        assert_eq!(s.name(), "windows");
    }

    #[test]
    fn supported_shapes_includes_all() {
        let s = WindowsSandbox::new("windows");
        let supported = s.supported_shapes();
        assert!(supported.contains(&tau_domain::CapabilityShape::FilesystemRead));
        assert!(supported.contains(&tau_domain::CapabilityShape::FilesystemWrite));
        assert!(supported.contains(&tau_domain::CapabilityShape::ProcessExec));
        assert!(supported.contains(&tau_domain::CapabilityShape::NetworkHttp));
    }

    #[test]
    fn validate_plan_rejects_unsupported_shape() {
        let s = WindowsSandbox::new("windows");
        let plan_json = json!({
            "capabilities": [{ "kind": "custom.weird" }],
            "context": null,
            "limits": null,
        });
        let plan: CapabilityPlan = serde_json::from_value(plan_json).expect("decode");
        let err = s
            .validate_plan(&plan)
            .expect_err("must reject unknown shape");
        assert!(
            matches!(err, CapabilityError::ShapeUnsupported { .. }),
            "expected ShapeUnsupported, got {err:?}"
        );
    }

    #[test]
    fn validate_plan_accepts_known_shapes() {
        let s = WindowsSandbox::new("windows");
        let plan_json = json!({
            "capabilities": [
                { "kind": "fs.read",  "paths": ["/etc"] },
                { "kind": "fs.write", "paths": ["/tmp"] },
                { "kind": "net.http", "hosts": ["example.com"], "methods": ["GET"] }
            ],
            "context": null,
            "limits": null,
        });
        let plan: CapabilityPlan = serde_json::from_value(plan_json).expect("decode");
        s.validate_plan(&plan)
            .expect("known shapes must be accepted");
    }

    #[test]
    fn validate_plan_rejects_wildcard_host() {
        // `HostSet`'s deserializer now rejects "*" as a non-hostname at
        // decode time (before `validate_plan` even runs) — callers must
        // spell pass-all as `hosts = "any"`, not a wildcard string.
        let plan_json = json!({
            "capabilities": [
                { "kind": "net.http", "hosts": ["*"], "methods": ["GET"] }
            ],
            "context": null,
            "limits": null,
        });
        let err = serde_json::from_value::<CapabilityPlan>(plan_json)
            .expect_err("wildcard host must be rejected at decode");
        assert!(
            err.to_string().contains("wildcard"),
            "expected wildcard-rejection error, got {err}"
        );
    }
}
