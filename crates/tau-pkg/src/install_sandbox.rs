//! Install-time sandbox port (audit S2).
//!
//! `tau-pkg` cannot depend on `tau-runtime-tokio` (that crate already depends
//! on `tau-pkg` — a cargo cycle), so it cannot reach the concrete
//! `SandboxAdapter`. Instead it defines this narrow **sync, dyn-safe** port;
//! `tau-cli` implements it with the real adapter and injects it via
//! [`crate::InstallOptions`].
//!
//! The port is sync because `tau-pkg`'s build path is synchronous and because
//! the runtime's `ProcessCapabilityGate::wrap_spawn` is `async fn in trait`
//! (not dyn-safe). The async bridge lives entirely in the `tau-cli` adapter.

use std::path::Path;
use std::process::Command;

use tau_ports::capability_gate::CapabilityPlan;

/// Errors from an [`InstallSandbox`] implementation.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum InstallSandboxError {
    /// The host cannot provide the requested sandbox (no kernel support, etc.).
    #[error("install sandbox unavailable: {0}")]
    Unavailable(String),
    /// Wrapping the command failed.
    #[error("install sandbox failed to wrap command: {0}")]
    WrapFailed(String),
}

/// RAII guard returned by [`InstallSandbox::wrap`]. Holds any ambient
/// resources the adapter created (egress-proxy task, dedicated runtime,
/// namespace fds) and releases them on drop. Must be kept alive across the
/// child process's lifetime.
#[must_use = "the sandbox is released when the guard drops; keep it alive across the spawn"]
pub struct InstallSandboxGuard {
    _cleanup: Box<dyn std::any::Any + Send>,
}

impl InstallSandboxGuard {
    /// Construct a guard that owns `state` (dropped when the guard drops).
    pub fn new<T: Send + 'static>(state: T) -> Self {
        Self {
            _cleanup: Box::new(state),
        }
    }
    /// A guard holding nothing (used by the mock and by no-op adapters).
    pub fn noop() -> Self {
        Self {
            _cleanup: Box::new(()),
        }
    }
}

impl std::fmt::Debug for InstallSandboxGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InstallSandboxGuard(..)")
    }
}

/// Port: lock down a `std::process::Command` before it is spawned.
///
/// Implemented by `tau-cli`'s `RuntimeInstallSandbox`. `tau-pkg` calls
/// [`InstallSandbox::is_enforced`] for the fail-closed decision, then
/// [`InstallSandbox::wrap`] immediately before spawning.
pub trait InstallSandbox: Send + Sync {
    /// `true` iff this gate applies real OS enforcement (tier > None). A
    /// passthrough / no-op gate returns `false`, which `tau-pkg` treats as
    /// "cannot sandbox" for the fail-closed decision.
    fn is_enforced(&self) -> bool;

    /// Apply enforcement to `cmd` in preparation for spawn. The returned
    /// guard must outlive the spawned child.
    fn wrap(
        &self,
        plan: &CapabilityPlan,
        cmd: &mut Command,
    ) -> Result<InstallSandboxGuard, InstallSandboxError>;
}

// ── Capability envelopes ────────────────────────────────────────────────────

/// crates.io sparse-registry hosts a `cargo build` must reach to download
/// dependencies. The old git-index host (`github.com/rust-lang/crates.io-index`)
/// is intentionally omitted; a project configured for the git index fails
/// closed and falls back to `--allow-unsandboxed-build`.
const CRATES_IO_HOSTS: &[&str] = &["index.crates.io", "static.crates.io"];

fn cap(json: serde_json::Value) -> tau_domain::Capability {
    serde_json::from_value(json).expect("internal envelope capability JSON is well-formed")
}

/// Capability envelope for the post-build cross-check spawn: nothing. A
/// well-behaved plugin needs only stdin/stdout to handshake; a malicious one
/// gets no network, no filesystem, no child-exec.
pub fn cross_check_envelope() -> CapabilityPlan {
    CapabilityPlan::new(Vec::new(), None, None)
}

/// Capability envelope for `cargo build --release` in `package_dir`:
/// network to the crates.io registry + any git-dependency hosts the package's
/// `Cargo.toml` declares; write to `target/`, `CARGO_HOME` caches, `TMPDIR`;
/// read of the source tree, `CARGO_HOME`, `RUSTUP_HOME`; child exec allowed
/// (cargo → rustc → cc → build.rs is the whole point).
pub fn build_envelope(package_dir: &Path) -> CapabilityPlan {
    let mut hosts: Vec<String> = CRATES_IO_HOSTS.iter().map(|h| h.to_string()).collect();
    hosts.extend(git_dep_hosts(package_dir));
    hosts.sort();
    hosts.dedup();

    let target = package_dir.join("target");
    let cargo_home = std::env::var("CARGO_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".cargo"));
    let rustup_home = std::env::var("RUSTUP_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".rustup"));
    let tmp = std::env::temp_dir();

    let g = |p: &Path| format!("{}/**", p.display());

    let caps = vec![
        cap(serde_json::json!({"kind": "net.http", "hosts": hosts, "methods": ["GET", "POST"]})),
        cap(serde_json::json!({"kind": "fs.write", "paths": [
            g(&target), g(&cargo_home.join("registry")), g(&cargo_home.join("git")), g(&tmp),
        ]})),
        cap(serde_json::json!({"kind": "fs.read", "paths": [
            g(package_dir), g(&cargo_home), g(&rustup_home),
        ]})),
        cap(serde_json::json!({"kind": "process.spawn", "commands": []})),
    ];
    CapabilityPlan::new(caps, None, None)
}

/// The user's home directory, used only as the fallback base for
/// `$CARGO_HOME` / `$RUSTUP_HOME` when neither is exported.
///
/// `$HOME` first, then `%USERPROFILE%` — the same precedence
/// [`crate::scope`]'s `resolve_global_path_from` already uses (#530).
/// Windows has no `$HOME` by convention; `%USERPROFILE%` is the
/// equivalent. Without the fallback, a Windows host with `RUSTUP_HOME`
/// unset (the GitHub `windows-latest` shape: `dtolnay/rust-toolchain`
/// exports `CARGO_HOME` but not `RUSTUP_HOME`) resolves the toolchain
/// read grant to a bogus **relative** `.rustup`, so the real toolchain
/// never enters the capability envelope.
///
/// Behaviour on Unix is unchanged and the envelope is not widened there:
/// `$HOME` still wins whenever it is set, and `%USERPROFILE%` is not a
/// Unix convention. An empty value counts as unset on both.
fn home_dir() -> std::path::PathBuf {
    resolve_home_dir(std::env::var_os("HOME"), std::env::var_os("USERPROFILE"))
}

/// Pure resolution behind [`home_dir`], separated so the precedence is
/// unit-testable without mutating process-global environment.
fn resolve_home_dir(
    home: Option<std::ffi::OsString>,
    user_profile: Option<std::ffi::OsString>,
) -> std::path::PathBuf {
    [home, user_profile]
        .into_iter()
        .flatten()
        .find(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_default()
}

/// Hosts named by `git = "..."` deps in the top-level `[dependencies]` and
/// `[build-dependencies]` tables of `package_dir/Cargo.toml`. Workspace,
/// target-specific, and dev-dependency tables are intentionally out of scope
/// (they fail closed). Missing/unparsable manifest → empty.
pub fn git_dep_hosts(package_dir: &Path) -> Vec<String> {
    let text = match std::fs::read_to_string(package_dir.join("Cargo.toml")) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let doc: toml::Value = match toml::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for table in ["dependencies", "build-dependencies"] {
        let Some(deps) = doc.get(table).and_then(|v| v.as_table()) else {
            continue;
        };
        for (_name, spec) in deps {
            if let Some(git) = spec
                .as_table()
                .and_then(|t| t.get("git"))
                .and_then(|g| g.as_str())
            {
                if let Some(h) = host_of_git_url(git) {
                    out.push(h);
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Extract the host from a git URL in either URL form
/// (`scheme://[user@]host[:port]/...`) or scp-like form (`user@host:path`).
pub fn host_of_git_url(url: &str) -> Option<String> {
    if let Some((_scheme, rest)) = url.split_once("://") {
        let after_at = rest.rsplit_once('@').map(|(_, h)| h).unwrap_or(rest);
        let host = after_at.split(['/', ':']).next().unwrap_or("");
        return (!host.is_empty()).then(|| host.to_string());
    }
    // scp-like: user@host:path
    if let Some((userhost, _path)) = url.split_once(':') {
        if let Some((_user, host)) = userhost.split_once('@') {
            return (!host.is_empty() && !host.contains('/')).then(|| host.to_string());
        }
    }
    None
}

// ── Fail-closed decision ────────────────────────────────────────────────────

/// Outcome of the fail-closed gate decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxDecision {
    /// An enforcing gate is present — wrap the command and proceed.
    Sandbox,
    /// No enforcing gate, but `--allow-unsandboxed-build` was passed —
    /// proceed without a sandbox (caller must emit a warning).
    Unsandboxed,
    /// No enforcing gate and no override — refuse.
    Refuse,
}

/// Decide whether an install-time spawn may proceed. Fail-closed: an absent
/// or non-enforcing gate refuses unless `allow_unsandboxed` is set.
pub fn sandbox_decision(
    gate: Option<&dyn InstallSandbox>,
    allow_unsandboxed: bool,
) -> SandboxDecision {
    match gate {
        Some(g) if g.is_enforced() => SandboxDecision::Sandbox,
        _ if allow_unsandboxed => SandboxDecision::Unsandboxed,
        _ => SandboxDecision::Refuse,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::{Arc, Mutex};
    use tau_ports::capability_gate::CapabilityPlan;

    /// Test double: records each `wrap` call's plan and reports a
    /// configurable enforcement state.
    #[derive(Clone)]
    pub struct MockInstallSandbox {
        pub enforced: bool,
        pub calls: Arc<Mutex<Vec<CapabilityPlan>>>,
    }

    impl MockInstallSandbox {
        pub fn new(enforced: bool) -> Self {
            Self {
                enforced,
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl InstallSandbox for MockInstallSandbox {
        fn is_enforced(&self) -> bool {
            self.enforced
        }
        fn wrap(
            &self,
            plan: &CapabilityPlan,
            _cmd: &mut Command,
        ) -> Result<InstallSandboxGuard, InstallSandboxError> {
            self.calls.lock().unwrap().push(plan.clone());
            Ok(InstallSandboxGuard::noop())
        }
    }

    #[test]
    fn mock_records_plan_and_reports_enforcement() {
        let mock = MockInstallSandbox::new(true);
        let plan = CapabilityPlan::new(Vec::new(), None, None);
        let mut cmd = Command::new("true");
        assert!(mock.is_enforced());
        let _guard = mock.wrap(&plan, &mut cmd).expect("wrap ok");
        assert_eq!(mock.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn error_display_names_the_flag() {
        let e = InstallSandboxError::Unavailable("no kernel support".into());
        assert!(e.to_string().contains("no kernel support"));
    }

    #[test]
    fn cross_check_envelope_is_empty() {
        let plan = cross_check_envelope();
        assert!(
            plan.capabilities.is_empty(),
            "cross-check needs nothing but stdio"
        );
    }

    #[test]
    fn build_envelope_grants_target_write_and_registry_net() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname=\"x\"\nversion=\"0.1.0\"\n[dependencies]\nserde=\"1\"\n",
        )
        .unwrap();
        let plan = build_envelope(dir.path());
        // Normalize `\` → `/` on both sides so the assertion is path-separator
        // agnostic. In serialized JSON a Windows backslash appears escaped
        // (`\\`), so we collapse both the escaped-pair and any lone backslash
        // to `/` before comparing. See #530.
        let json = serde_json::to_value(&plan.capabilities)
            .unwrap()
            .to_string()
            .replace("\\\\", "/")
            .replace('\\', "/");
        assert!(
            json.contains("index.crates.io"),
            "registry host present: {json}"
        );
        assert!(json.contains("static.crates.io"));
        assert!(json.contains("net.http"));
        assert!(json.contains("fs.write"));
        let target = dir
            .path()
            .join("target")
            .display()
            .to_string()
            .replace('\\', "/");
        assert!(json.contains(&target), "target write path present: {json}");
    }

    #[test]
    fn home_dir_prefers_home_over_userprofile() {
        // Unix behaviour must be untouched: $HOME still wins, so the
        // envelope's CARGO_HOME/RUSTUP_HOME fallbacks are unchanged there.
        let p = resolve_home_dir(
            Some(std::ffi::OsString::from("/x/home")),
            Some(std::ffi::OsString::from("C:\\Users\\x")),
        );
        assert_eq!(p, std::path::Path::new("/x/home"));
    }

    #[test]
    fn home_dir_falls_back_to_userprofile() {
        // The Windows shape: no $HOME, only %USERPROFILE%. Before this
        // fallback the result was an empty PathBuf, which made the
        // envelope grant read on a relative `.rustup` / `.cargo`.
        let p = resolve_home_dir(
            None,
            Some(std::ffi::OsString::from("C:\\Users\\runneradmin")),
        );
        assert_eq!(p, std::path::Path::new("C:\\Users\\runneradmin"));
    }

    #[test]
    fn home_dir_treats_empty_as_unset() {
        let p = resolve_home_dir(
            Some(std::ffi::OsString::new()),
            Some(std::ffi::OsString::from("C:\\Users\\x")),
        );
        assert_eq!(p, std::path::Path::new("C:\\Users\\x"));
    }

    #[test]
    fn home_dir_is_empty_when_both_missing() {
        assert_eq!(resolve_home_dir(None, None), std::path::PathBuf::new());
    }

    #[test]
    fn build_envelope_adds_git_dep_hosts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname=\"x\"\nversion=\"0.1.0\"\n\
[dependencies]\nfoo = { git = \"https://github.com/acme/foo\" }\n\
[build-dependencies]\nbar = { git = \"ssh://git@gitlab.example.com/acme/bar\" }\n",
        )
        .unwrap();
        let hosts = git_dep_hosts(dir.path());
        assert!(hosts.contains(&"github.com".to_string()), "got {hosts:?}");
        assert!(
            hosts.contains(&"gitlab.example.com".to_string()),
            "got {hosts:?}"
        );
    }

    #[test]
    fn git_dep_hosts_ignores_registry_and_path_deps() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname=\"x\"\nversion=\"0.1.0\"\n\
[dependencies]\nserde=\"1\"\nlocal = { path = \"../local\" }\n",
        )
        .unwrap();
        assert!(git_dep_hosts(dir.path()).is_empty());
    }

    #[test]
    fn host_of_url_handles_scp_and_url_forms() {
        assert_eq!(
            host_of_git_url("https://github.com/a/b"),
            Some("github.com".into())
        );
        assert_eq!(
            host_of_git_url("ssh://git@host.example:22/a/b"),
            Some("host.example".into())
        );
        assert_eq!(
            host_of_git_url("git@github.com:a/b.git"),
            Some("github.com".into())
        );
        assert_eq!(host_of_git_url("not a url"), None);
    }

    #[test]
    fn decision_enforced_gate_sandboxes() {
        let g = MockInstallSandbox::new(true);
        assert!(matches!(
            sandbox_decision(Some(&g), false),
            SandboxDecision::Sandbox
        ));
        assert!(matches!(
            sandbox_decision(Some(&g), true),
            SandboxDecision::Sandbox
        ));
    }

    #[test]
    fn decision_unenforced_gate_refuses_without_flag() {
        let g = MockInstallSandbox::new(false);
        assert!(matches!(
            sandbox_decision(Some(&g), false),
            SandboxDecision::Refuse
        ));
        assert!(matches!(
            sandbox_decision(None, false),
            SandboxDecision::Refuse
        ));
    }

    #[test]
    fn decision_allow_flag_permits_unsandboxed() {
        let g = MockInstallSandbox::new(false);
        assert!(matches!(
            sandbox_decision(Some(&g), true),
            SandboxDecision::Unsandboxed
        ));
        assert!(matches!(
            sandbox_decision(None, true),
            SandboxDecision::Unsandboxed
        ));
    }
}
