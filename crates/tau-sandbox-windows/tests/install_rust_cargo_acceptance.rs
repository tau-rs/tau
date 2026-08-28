//! Windows-only ACCEPTANCE test for #622: a real `kind = "rust-cargo"`
//! install builds under the graduated AppContainer adapter with
//! `allow_unsandboxed_build = false` — the exact scenario ADR-0067
//! documented as failing closed before the egress follow-on.
//!
//! The fixture has zero registry dependencies so `cargo build` needs no
//! real crates.io traffic, but `build_envelope` still carries the
//! registry hosts, so the plan REQUIRES NetworkHttp — before #622 the
//! adapter refused it at wrap_spawn. Slow (~30s: real cargo build).
#![cfg(all(target_os = "windows", feature = "integration-tests"))]

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::str::FromStr;
use std::sync::Arc;

use tau_domain::PackageSource;
use tau_pkg::install_sandbox::{InstallSandbox, InstallSandboxError, InstallSandboxGuard};
use tau_pkg::{install_with_options, InstallOptions, Scope};
use tau_sandbox_windows::WindowsSandbox;
use tempfile::TempDir;

/// Sync→async shim: the same bridging `tau-cli`'s RuntimeInstallSandbox
/// does, minimal for the test.
struct WinGate {
    rt: tokio::runtime::Runtime,
    gate: WindowsSandbox,
}

impl InstallSandbox for WinGate {
    fn is_enforced(&self) -> bool {
        true
    }
    fn wrap(
        &self,
        plan: &tau_ports::capability_gate::CapabilityPlan,
        cmd: &mut std::process::Command,
    ) -> Result<InstallSandboxGuard, InstallSandboxError> {
        use tau_ports::ProcessCapabilityGate;
        dump_envelope(plan);
        let handle = self
            .rt
            .block_on(self.gate.wrap_spawn(plan, cmd))
            .map_err(|e| InstallSandboxError::WrapFailed(e.to_string()))?;
        Ok(InstallSandboxGuard::new(handle))
    }
}

/// Print the envelope this build is actually running under, plus the
/// env it was derived from and whether it covers the real toolchain.
///
/// #622 CI round 2 failed with `rustc.exe ... Access is denied (os error
/// 5)` and two incompatible explanations: either `tau-pkg`'s
/// `build_envelope` resolved `$RUSTUP_HOME` to the wrong place (it falls
/// back to `$HOME`, which Windows does not set by convention), or the
/// paths were right and the ACL grant did not reach the toolchain files.
/// The two are indistinguishable from the failure text, so print the
/// evidence: every granted path with `exists=`, the four env vars the
/// resolution reads, the real `rustc.exe` location, and whether any
/// granted read path is an ancestor of it (`covers-rustc`).
///
/// `covers-rustc=false` ⇒ path-resolution bug (the envelope never named
/// the toolchain). `covers-rustc=true` ⇒ the grant was on the right tree
/// and the denial is an ACL-application problem — which
/// `dir_grant_reaches_preexisting_nested_file` in `egress_integration`
/// then localises to inheritance propagation.
fn dump_envelope(plan: &tau_ports::capability_gate::CapabilityPlan) {
    for var in ["HOME", "USERPROFILE", "CARGO_HOME", "RUSTUP_HOME"] {
        eprintln!(
            "ENVELOPE env {var}={:?}",
            std::env::var(var).unwrap_or_else(|_| "<unset>".into())
        );
    }
    let rustc = resolve_rustc();
    eprintln!("ENVELOPE rustc={rustc:?}");
    let json = serde_json::to_value(&plan.capabilities).unwrap_or(serde_json::Value::Null);
    for capability in json.as_array().into_iter().flatten() {
        let kind = capability
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("?");
        let Some(paths) = capability.get("paths").and_then(|p| p.as_array()) else {
            continue;
        };
        for raw in paths.iter().filter_map(|p| p.as_str()) {
            let cleaned = raw
                .trim_end_matches("/**")
                .trim_end_matches("/*")
                .trim_end_matches('/');
            let path = Path::new(cleaned);
            let covers = rustc.as_ref().map(|r| r.starts_with(path)).unwrap_or(false);
            eprintln!(
                "ENVELOPE {kind} path={cleaned:?} absolute={} exists={} covers-rustc={covers}",
                path.is_absolute(),
                path.exists(),
            );
        }
    }
}

/// The `rustc.exe` cargo will actually spawn, as `rustup which rustc`
/// reports it. `None` if rustup is not on PATH (then `covers-rustc` is
/// simply unknown, not false-negative — the marker says so by printing
/// `rustc=None`).
fn resolve_rustc() -> Option<PathBuf> {
    let out = StdCommand::new("rustup")
        .args(["which", "rustc"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then(|| PathBuf::from(s))
}

// ── fixture helpers (mirrored from tau-pkg/tests/install_builds_rust_cargo_plugin.rs) ──

fn run_git(cwd: &Path, args: &[&str]) {
    let out = StdCommand::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn file_url(p: &Path) -> String {
    format!("file://{}", p.display().to_string().replace('\\', "/"))
}

fn make_plugin_fixture_repo(parent: &Path, name: &str) -> PathBuf {
    let bare = parent.join(format!("{name}.git"));
    std::fs::create_dir_all(&bare).unwrap();
    run_git(&bare, &["init", "-q", "--bare", "-b", "main", "."]);
    let working = parent.join(format!("{name}-working"));
    std::fs::create_dir_all(&working).unwrap();
    run_git(&working, &["init", "-q", "-b", "main"]);
    run_git(&working, &["config", "user.email", "test@example.com"]);
    run_git(&working, &["config", "user.name", "Test User"]);
    std::fs::write(
        working.join("tau.toml"),
        format!(
            r#"name = "{name}"
version = "0.1.0"
description = "acceptance fixture for #622"
authors = ["Test <test@example.com>"]
source = "{src}"
kind = "tool"
dependencies = []
capabilities = []

[plugin]
provides = "tool"
kind     = "rust-cargo"
bin      = "{name}"
"#,
            src = file_url(&bare)
        ),
    )
    .unwrap();
    std::fs::write(
        working.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"{name}\"\npath = \"src/main.rs\"\n\n[dependencies]\n"
        ),
    )
    .unwrap();
    std::fs::create_dir_all(working.join("src")).unwrap();
    std::fs::write(working.join("src/main.rs"), "fn main() {}\n").unwrap();
    run_git(&working, &["add", "."]);
    run_git(&working, &["commit", "-q", "-m", "fixture"]);
    run_git(
        &working,
        &["remote", "add", "origin", &bare.to_string_lossy()],
    );
    run_git(&working, &["push", "-q", "origin", "main"]);
    bare
}

#[test]
fn rust_cargo_install_succeeds_sandboxed_without_unsandboxed_escape() {
    // Point the adapter at the cargo-built launcher + bridge.
    std::env::set_var(
        "TAU_APPCONTAINER_LAUNCHER_PATH",
        env!("CARGO_BIN_EXE_tau-appcontainer-launcher"),
    );
    std::env::set_var(
        "TAU_NET_BRIDGE_WIN_PATH",
        env!("CARGO_BIN_EXE_tau-net-bridge-win"),
    );

    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("tau-home");
    std::fs::create_dir_all(&project_root).unwrap();
    let scope = Scope::new_project(&project_root).unwrap();
    let bare = make_plugin_fixture_repo(tmp.path(), "acceptance-plugin");
    let source = PackageSource::from_str(&file_url(&bare)).unwrap();

    let mut opts = InstallOptions::default();
    opts.skip_cross_check = true; // stub bin has no handshake protocol
    opts.allow_unsandboxed_build = false; // THE acceptance condition
    opts.sandbox = Some(Arc::new(WinGate {
        rt: tokio::runtime::Runtime::new().unwrap(),
        gate: WindowsSandbox::new("windows"),
    }));

    // `.expect` surfaces the `InstallError`'s `Debug` output on failure, so a
    // red CI run tells us *why* (egress refusal, toolchain-read denial,
    // fixture bug, ...) rather than just "called `Result::unwrap()` on an
    // `Err` value".
    let installed = install_with_options(&source, &scope, opts)
        .expect("sandboxed rust-cargo install must succeed without --allow-unsandboxed-build");
    assert_eq!(installed.name.as_str(), "acceptance-plugin");
}
