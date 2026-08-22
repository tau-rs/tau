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
//! parts (`acl`, the launcher command rebuild in `lib`) are
//! `cfg(target_os = "windows")`-gated. Windows CI runners are the only
//! place runtime behavior is verified.

#![deny(missing_docs)]

#[cfg(target_os = "windows")]
mod acl;
#[cfg(target_os = "windows")]
mod pipe_proxy;
mod profile;

pub use profile::{build_appcontainer_caps, AppContainerCaps};

pub mod bridge_args;
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
/// **Phase 2:** the real Win32 AppContainer implementation has landed
/// (`acl.rs` grant/revoke, `wrap_spawn` via the launcher exec-wrapper), so
/// this returns `Available { tier: Strict }` on Windows — filesystem and
/// process isolation are enforced. Network egress remains deferred and
/// fail-closed (`NetworkHttp` is not in `supported_shapes`) until the
/// egress follow-on lands.
async fn run_probe() -> CapabilityProbe {
    if !cfg!(target_os = "windows") {
        return CapabilityProbe::Unavailable {
            reason: "not running on Windows".to_string(),
        };
    }
    CapabilityProbe::Available {
        tier: tau_ports::CapabilityTier::Strict,
        details: "AppContainer (FS + process isolation); network egress deferred (fail-closed)"
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

    // Fail closed on network — egress is a deferred follow-on EPIC.
    // (Phase 2 spawns no host-side proxy task; `tau_sandbox_proxy::spawn_proxy`
    // is currently `cfg(unix)`-gated because it builds on
    // `tokio::net::UnixListener`. Windows egress requires either TCP-loopback
    // IPC or named pipes — deferred.)
    if caps.has_http {
        return Err(CapabilityError::Unsupported {
            what: "Network(Http) on Windows: egress not yet supported (deferred follow-on)"
                .to_string(),
        });
    }

    // Rebuild the command to run the target THROUGH the launcher, which
    // does CreateProcessAsUserW inside the AppContainer. Mirrors the
    // darwin sandbox-exec rebuild (`*cmd = Command::new(...)`).
    let launcher = std::env::var_os("TAU_APPCONTAINER_LAUNCHER_PATH")
        .unwrap_or_else(|| std::ffi::OsString::from("tau-appcontainer-launcher"));
    let orig_program = cmd.get_program().to_os_string();
    let orig_args: Vec<std::ffi::OsString> = cmd.get_args().map(|a| a.to_os_string()).collect();
    let orig_envs: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)> = cmd
        .get_envs()
        .map(|(k, v)| (k.to_os_string(), v.map(|x| x.to_os_string())))
        .collect();
    let orig_cwd = cmd.get_current_dir().map(|p| p.to_path_buf());

    *cmd = Command::new(launcher);
    cmd.arg("--profile").arg(&profile_name);
    // caps.capability_sids would be added here once net lands; empty in Phase 2.
    cmd.arg("--").arg(orig_program).args(orig_args);
    for (k, v) in orig_envs {
        match v {
            Some(val) => {
                cmd.env(k, val);
            }
            None => {
                cmd.env_remove(k);
            }
        }
    }
    if let Some(dir) = orig_cwd {
        cmd.current_dir(dir);
    }

    // Build the CapabilityHandle. On drop:
    // 1. revoke ACL grants in reverse order
    // 2. delete the AppContainer profile
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
    fn supported_shapes_is_fs_and_exec() {
        let s = WindowsSandbox::new("windows");
        let supported = s.supported_shapes();
        assert!(supported.contains(&tau_domain::CapabilityShape::FilesystemRead));
        assert!(supported.contains(&tau_domain::CapabilityShape::FilesystemWrite));
        assert!(supported.contains(&tau_domain::CapabilityShape::ProcessExec));
        assert!(
            !supported.contains(&tau_domain::CapabilityShape::NetworkHttp),
            "network is deferred (fail-closed) in Phase 2"
        );
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
                { "kind": "fs.write", "paths": ["/tmp"] }
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
