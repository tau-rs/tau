//! macOS sandbox-exec adapter for tau.
//!
//! Wraps plugin commands with `sandbox-exec -f <generated-profile>` so the
//! plugin runs under macOS's kernel-level Sandbox-1 (TrustedBSD MAC framework).
//! Strict tier: filesystem isolation per-capability + outbound network
//! restricted to the host-side `tau-sandbox-proxy` task on `127.0.0.1:8443`.
//!
//! Compared to [`tau_sandbox_native`] (Linux landlock + seccomp + namespaces):
//! - **Pros:** runs natively on macOS dev machines + CI runners; reuses
//!   `tau-sandbox-proxy` for HTTPS allowlist enforcement; same security
//!   envelope as Linux strict from the plugin's perspective.
//! - **Cons:** sandbox-exec is officially-deprecated-but-still-functional
//!   (Apple's note since macOS 10.13; tool ships through current macOS);
//!   no syscall-level filtering (no seccomp equivalent); SBPL profile
//!   parser is finicky (one bad paren rejects the whole profile).

#![deny(missing_docs)]

mod baseline;
mod profile;

pub use profile::build_sbpl_profile;

#[cfg(target_os = "macos")]
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use tokio::sync::OnceCell;

use tau_domain::{Capability, NetCapability};
use tau_ports::{
    CapabilityError, CapabilityGate, CapabilityHandle, CapabilityPlan, CapabilityProbe,
    CapabilityShapeSet, CapabilityTier, ProcessCapabilityGate,
};

/// macOS sandbox-exec adapter.
pub struct DarwinSandbox {
    name: String,
    /// Probe is cached lazily on the first call.
    probe_cache: Arc<OnceCell<CapabilityProbe>>,
}

impl DarwinSandbox {
    /// Construct a darwin adapter.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            probe_cache: Arc::new(OnceCell::new()),
        }
    }
}

impl CapabilityGate for DarwinSandbox {
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
        // Reject HTTP plans whose host allowlist contains forms the proxy
        // can't validate (wildcards, non-loopback IP literals).
        // `HostSet::Any` has no exact strings to validate; only collect the
        // `Exact` union (defense in depth: rejects wildcards + non-loopback
        // IP literals).
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

impl ProcessCapabilityGate for DarwinSandbox {
    #[cfg(target_os = "macos")]
    async fn wrap_spawn(
        &self,
        plan: &CapabilityPlan,
        cmd: &mut Command,
    ) -> Result<CapabilityHandle, CapabilityError> {
        self.validate_plan(plan)?;
        wrap_spawn_macos(plan, cmd).await
    }

    #[cfg(not(target_os = "macos"))]
    async fn wrap_spawn(
        &self,
        _plan: &CapabilityPlan,
        _cmd: &mut Command,
    ) -> Result<CapabilityHandle, CapabilityError> {
        Err(CapabilityError::Unavailable {
            reason: "tau-sandbox-darwin is macOS-only".to_string(),
        })
    }
}

/// Probe for sandbox-exec availability. Cached per `DarwinSandbox`
/// instance via `OnceCell`.
async fn run_probe() -> CapabilityProbe {
    if !cfg!(target_os = "macos") {
        return CapabilityProbe::Unavailable {
            reason: "not running on macOS".to_string(),
        };
    }
    if !std::path::Path::new("/usr/bin/sandbox-exec").exists() {
        return CapabilityProbe::Unavailable {
            reason: "/usr/bin/sandbox-exec missing".to_string(),
        };
    }
    CapabilityProbe::Available {
        tier: CapabilityTier::Strict,
        details: "sandbox-exec; SBPL profile + tau-sandbox-proxy".to_string(),
    }
}

#[cfg(target_os = "macos")]
async fn wrap_spawn_macos(
    plan: &CapabilityPlan,
    cmd: &mut Command,
) -> Result<CapabilityHandle, CapabilityError> {
    use std::os::unix::fs::PermissionsExt;

    // Capture the original program + args; we'll re-emit them after sandbox-exec.
    let original_program = cmd.get_program().to_os_string();
    let original_args: Vec<std::ffi::OsString> = cmd.get_args().map(|a| a.to_os_string()).collect();
    let original_envs: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)> = cmd
        .get_envs()
        .map(|(k, v)| (k.to_os_string(), v.map(|v| v.to_os_string())))
        .collect();

    // Spawn the host-side proxy task if the plan needs HTTP.
    let has_http = plan
        .capabilities
        .iter()
        .any(|c| matches!(c, Capability::Network(NetCapability::Http { .. })));
    let proxy_handle = if has_http {
        let mut any = false;
        let mut exact: Vec<String> = Vec::new();
        for cap in &plan.capabilities {
            if let Capability::Network(NetCapability::Http { hosts, .. }) = cap {
                if hosts.is_any() {
                    any = true;
                } else {
                    exact.extend(hosts.exact_hosts());
                }
            }
        }
        let policy = if any {
            tau_sandbox_proxy::HostAllow::Any
        } else {
            tau_sandbox_proxy::HostAllow::Exact(exact)
        };
        let handle =
            tau_sandbox_proxy::spawn_proxy(policy).map_err(|e| CapabilityError::Proxy {
                message: format!("spawn_proxy: {e}"),
            })?;
        Some(handle)
    } else {
        None
    };

    // Generate the SBPL profile and write it to /tmp.
    let sbpl = build_sbpl_profile(plan);
    let profile_path = write_profile(&sbpl)?;

    // Replace cmd with `sandbox-exec -f <profile> <orig-program> <orig-args>`.
    *cmd = Command::new("/usr/bin/sandbox-exec");
    cmd.arg("-f").arg(&profile_path);
    cmd.arg(&original_program);
    for arg in &original_args {
        cmd.arg(arg);
    }
    // Re-attach the original env vars; sandbox-exec passes these through.
    for (k, v) in original_envs {
        match v {
            Some(val) => {
                cmd.env(k, val);
            }
            None => {
                cmd.env_remove(k);
            }
        }
    }

    // For HTTP plans, set HTTPS_PROXY / HTTP_PROXY so the plugin's reqwest
    // routes through the proxy. Lowercase variants for case-sensitive
    // env-var consumers on Unix.
    if proxy_handle.is_some() {
        let proxy_url = "http://127.0.0.1:8443";
        cmd.env("HTTPS_PROXY", proxy_url);
        cmd.env("HTTP_PROXY", proxy_url);
        cmd.env("https_proxy", proxy_url);
        cmd.env("http_proxy", proxy_url);
    }

    // Build the CapabilityHandle: nest the proxy guard so it drops with the
    // handle, and a closure to remove the temp profile file.
    let cleanup_path = profile_path.clone();
    let mut handle = CapabilityHandle::new(move || {
        let _ = std::fs::remove_file(&cleanup_path);
    });
    if let Some(p) = proxy_handle {
        handle.nest_handle(Box::new(p));
    }

    // Restrict the file's permissions so only the current user can read.
    // sandbox-exec needs to read the profile; the OS user is fine.
    if let Ok(meta) = std::fs::metadata(&profile_path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        let _ = std::fs::set_permissions(&profile_path, perms);
    }

    Ok(handle)
}

/// Write the generated SBPL profile to a unique tempfile in /tmp and
/// return its absolute path. Caller must remove the file when done.
#[cfg(target_os = "macos")]
fn write_profile(sbpl: &str) -> Result<PathBuf, CapabilityError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!("tau-darwin-{}-{}.sb", std::process::id(), n));
    std::fs::write(&path, sbpl).map_err(|e| CapabilityError::WrapFailed {
        message: format!("write SBPL profile: {e}"),
    })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn name_round_trip() {
        let s = DarwinSandbox::new("darwin");
        assert_eq!(s.name(), "darwin");
    }

    #[test]
    fn supported_shapes_includes_all() {
        let s = DarwinSandbox::new("darwin");
        let supported = s.supported_shapes();
        assert!(supported.contains(&tau_domain::CapabilityShape::FilesystemRead));
        assert!(supported.contains(&tau_domain::CapabilityShape::FilesystemWrite));
        assert!(supported.contains(&tau_domain::CapabilityShape::ProcessExec));
        assert!(supported.contains(&tau_domain::CapabilityShape::NetworkHttp));
    }

    #[test]
    fn validate_plan_rejects_unsupported_shape() {
        let s = DarwinSandbox::new("darwin");
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
        let s = DarwinSandbox::new("darwin");
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
