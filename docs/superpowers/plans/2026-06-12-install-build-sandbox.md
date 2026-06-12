# Install-time Build Sandbox (audit S2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run `tau install`'s `cargo build` and the post-build capability cross-check under the same OS sandbox tier the plugin would later run under, failing closed unless `--allow-unsandboxed-build` is passed.

**Architecture:** `tau-pkg` defines a small **sync, dyn-safe port** `InstallSandbox` plus two fixed **capability envelopes** (build / cross-check) and a **fail-closed decision**. The build and cross-check spawn sites wrap their `std::process::Command` through the injected port before spawning. `tau-cli` implements the port with a `RuntimeInstallSandbox` adapter that bridges to the async `SandboxAdapter::wrap_spawn`, driving it on a **dedicated long-lived tokio runtime** held inside the returned guard so the strict-tier egress proxy survives the build. The dependency arrow points downward (`tau-cli → tau-pkg`); the reverse (`tau-pkg → tau-runtime-tokio`) is a hard cargo cycle and is *not* used.

**Tech Stack:** Rust, `tau-ports` (`CapabilityPlan`/`Capability`), `tau-runtime-tokio` (`SandboxAdapter`, `resolve_adapter`), `tokio`, `serde_json` (capability construction is serde-only — the variants are `#[non_exhaustive]`), `toml` (Cargo.toml git-dep parsing), `clap`.

**CARGO RULES:** every cargo command in this plan uses
`timeout <n> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-s2 cargo ...`
and is scoped with `-p tau-pkg` or `-p tau-cli`. Tests prefer `cargo nextest run`; doctests use `cargo test --doc`.

**Git identity:** this worktree's `[user]` config is corrupted to `Test User` by a lefthook test. Commit with explicit overrides:
`git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit ...`

---

## File Structure

- `crates/tau-pkg/src/install_sandbox.rs` *(new)* — the `InstallSandbox` port trait, `InstallSandboxGuard`, `InstallSandboxError`, the fail-closed `SandboxDecision` helper, the `MockInstallSandbox` test double, and the two envelope builders (`build_envelope`, `cross_check_envelope`) + Cargo.toml git-host parser. One cohesive responsibility: "everything the installer needs to lock down a spawn." (~300 lines; split the git-host parser into its own `mod git_hosts` inside the file if it grows.)
- `crates/tau-pkg/src/lib.rs` *(modify)* — `pub mod install_sandbox;` + re-exports (`InstallSandbox`, `InstallSandboxError`, `InstallSandboxGuard`).
- `crates/tau-pkg/src/install.rs` *(modify)* — new `InstallOptions` fields; thread `gate: Option<&dyn InstallSandbox>` + `allow_unsandboxed_build` into `build_rust_cargo_plugin` and the cross-check call; apply the fail-closed decision + `wrap` at both spawn sites.
- `crates/tau-pkg/src/error.rs` *(modify)* — `InstallError::UnsandboxedBuildRefused` variant.
- `crates/tau-pkg/src/sandbox_check.rs` *(modify)* — `cross_check_plugin_capabilities` takes the gate, applies the cross-check envelope to `command.as_std_mut()` before spawn.
- `crates/tau-cli/src/cmd/install_sandbox.rs` *(new)* — `RuntimeInstallSandbox` adapter (the sync→async + dedicated-runtime bridge) + its `RuntimeGuard`.
- `crates/tau-cli/src/cmd/install.rs` *(modify)* — resolve adapter, build `RuntimeInstallSandbox`, set `InstallOptions { sandbox, allow_unsandboxed_build }`.
- `crates/tau-cli/src/cli.rs` *(modify)* — `--allow-unsandboxed-build` flag on `InstallArgs`.

---

## Task 1: `InstallSandbox` port, guard, error, and mock

**Files:**
- Create: `crates/tau-pkg/src/install_sandbox.rs`
- Modify: `crates/tau-pkg/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Add to the bottom of the new file `crates/tau-pkg/src/install_sandbox.rs`:

```rust
#[cfg(test)]
mod tests {
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
            Self { enforced, calls: Arc::new(Mutex::new(Vec::new())) }
        }
    }

    impl InstallSandbox for MockInstallSandbox {
        fn is_enforced(&self) -> bool { self.enforced }
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
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-s2 cargo build -p tau-pkg`
Expected: FAIL — `cannot find type InstallSandbox / InstallSandboxGuard / InstallSandboxError`.

- [ ] **Step 3: Write minimal implementation**

At the top of `crates/tau-pkg/src/install_sandbox.rs`:

```rust
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
    /// Construct a guard that owns `state` (dropped LIFO when the guard drops).
    pub fn new<T: Send + 'static>(state: T) -> Self {
        Self { _cleanup: Box::new(state) }
    }
    /// A guard holding nothing (used by the mock and by no-op adapters).
    pub fn noop() -> Self {
        Self { _cleanup: Box::new(()) }
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
```

Add to `crates/tau-pkg/src/lib.rs` (near the other `pub mod` lines):

```rust
pub mod install_sandbox;
pub use install_sandbox::{InstallSandbox, InstallSandboxError, InstallSandboxGuard};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-s2 cargo nextest run -p tau-pkg install_sandbox`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/install_sandbox.rs crates/tau-pkg/src/lib.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-pkg): InstallSandbox port + guard + error (audit S2)"
```

---

## Task 2: Capability envelopes + Cargo.toml git-host parser

**Files:**
- Modify: `crates/tau-pkg/src/install_sandbox.rs`
- Test: same file, `mod tests`

Background: `Capability` variants are `#[non_exhaustive]` with no public constructor, so they are built via serde (`serde_json::from_value`). The build envelope's network allowlist is the crates.io sparse-registry hosts plus hosts parsed from `git = "..."` deps in the package's top-level `Cargo.toml` `[dependencies]` + `[build-dependencies]`. The cross-check envelope is empty.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests`:

```rust
use std::path::Path;

#[test]
fn cross_check_envelope_is_empty() {
    let plan = cross_check_envelope();
    assert!(plan.capabilities.is_empty(), "cross-check needs nothing but stdio");
}

#[test]
fn build_envelope_grants_target_write_and_registry_net() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"),
        "[package]\nname=\"x\"\nversion=\"0.1.0\"\n[dependencies]\nserde=\"1\"\n").unwrap();
    let plan = build_envelope(dir.path());
    let json = serde_json::to_value(&plan.capabilities).unwrap().to_string();
    assert!(json.contains("index.crates.io"), "registry host present: {json}");
    assert!(json.contains("static.crates.io"));
    assert!(json.contains("net.http"));
    assert!(json.contains("fs.write"));
    // the package's own target/ dir must be writable
    assert!(json.contains(&dir.path().join("target").display().to_string()));
}

#[test]
fn build_envelope_adds_git_dep_hosts() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "\
[package]\nname=\"x\"\nversion=\"0.1.0\"\n\
[dependencies]\nfoo = { git = \"https://github.com/acme/foo\" }\n\
[build-dependencies]\nbar = { git = \"ssh://git@gitlab.example.com/acme/bar\" }\n").unwrap();
    let hosts = git_dep_hosts(dir.path());
    assert!(hosts.contains(&"github.com".to_string()), "got {hosts:?}");
    assert!(hosts.contains(&"gitlab.example.com".to_string()), "got {hosts:?}");
}

#[test]
fn git_dep_hosts_ignores_registry_and_path_deps() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "\
[package]\nname=\"x\"\nversion=\"0.1.0\"\n\
[dependencies]\nserde=\"1\"\nlocal = { path = \"../local\" }\n").unwrap();
    assert!(git_dep_hosts(dir.path()).is_empty());
}

#[test]
fn host_of_url_handles_scp_and_url_forms() {
    assert_eq!(host_of_git_url("https://github.com/a/b"), Some("github.com".into()));
    assert_eq!(host_of_git_url("ssh://git@host.example:22/a/b"), Some("host.example".into()));
    assert_eq!(host_of_git_url("git@github.com:a/b.git"), Some("github.com".into()));
    assert_eq!(host_of_git_url("not a url"), None);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-s2 cargo build -p tau-pkg --tests`
Expected: FAIL — `cannot find function build_envelope / cross_check_envelope / git_dep_hosts / host_of_git_url`.

- [ ] **Step 3: Write minimal implementation**

Add to `crates/tau-pkg/src/install_sandbox.rs` (module body, above `mod tests`):

```rust
use std::path::Path;

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

fn home_dir() -> std::path::PathBuf {
    std::env::var("HOME").map(std::path::PathBuf::from).unwrap_or_default()
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
        let Some(deps) = doc.get(table).and_then(|v| v.as_table()) else { continue };
        for (_name, spec) in deps {
            if let Some(git) = spec.as_table().and_then(|t| t.get("git")).and_then(|g| g.as_str()) {
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
```

- [ ] **Step 4: Run to verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-s2 cargo nextest run -p tau-pkg install_sandbox`
Expected: PASS (7 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/install_sandbox.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-pkg): build + cross-check capability envelopes (audit S2)"
```

---

## Task 3: Fail-closed decision helper

**Files:**
- Modify: `crates/tau-pkg/src/install_sandbox.rs`

- [ ] **Step 1: Write the failing tests**

Add to `mod tests`:

```rust
#[test]
fn decision_enforced_gate_sandboxes() {
    let g = MockInstallSandbox::new(true);
    assert!(matches!(sandbox_decision(Some(&g), false), SandboxDecision::Sandbox));
    // an enforced gate wins regardless of the allow flag
    assert!(matches!(sandbox_decision(Some(&g), true), SandboxDecision::Sandbox));
}

#[test]
fn decision_unenforced_gate_refuses_without_flag() {
    let g = MockInstallSandbox::new(false);
    assert!(matches!(sandbox_decision(Some(&g), false), SandboxDecision::Refuse));
    assert!(matches!(sandbox_decision(None, false), SandboxDecision::Refuse));
}

#[test]
fn decision_allow_flag_permits_unsandboxed() {
    let g = MockInstallSandbox::new(false);
    assert!(matches!(sandbox_decision(Some(&g), true), SandboxDecision::Unsandboxed));
    assert!(matches!(sandbox_decision(None, true), SandboxDecision::Unsandboxed));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-s2 cargo build -p tau-pkg --tests`
Expected: FAIL — `cannot find SandboxDecision / sandbox_decision`.

- [ ] **Step 3: Write minimal implementation**

Add to module body:

```rust
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
```

Note: the test passes `Some(&g)` where `g: MockInstallSandbox`; `&g` coerces to `&dyn InstallSandbox`.

- [ ] **Step 4: Run to verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-s2 cargo nextest run -p tau-pkg install_sandbox`
Expected: PASS (10 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/install_sandbox.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-pkg): fail-closed sandbox decision (audit S2)"
```

---

## Task 4: `InstallError::UnsandboxedBuildRefused` + `InstallOptions` fields

**Files:**
- Modify: `crates/tau-pkg/src/error.rs`
- Modify: `crates/tau-pkg/src/install.rs:119-192` (the `BuildOptions` / `InstallOptions` region)

- [ ] **Step 1: Write the failing tests**

Add to `crates/tau-pkg/src/install.rs` `mod tests`:

```rust
#[test]
fn unsandboxed_refused_error_names_the_flag() {
    let e = crate::error::InstallError::UnsandboxedBuildRefused;
    assert!(e.to_string().contains("--allow-unsandboxed-build"), "{e}");
}

#[test]
fn install_options_default_is_fail_closed() {
    let o = InstallOptions::default();
    assert!(o.sandbox.is_none());
    assert!(!o.allow_unsandboxed_build);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-s2 cargo build -p tau-pkg --tests`
Expected: FAIL — unknown variant `UnsandboxedBuildRefused`; no field `sandbox` / `allow_unsandboxed_build`.

- [ ] **Step 3: Write minimal implementation**

In `crates/tau-pkg/src/error.rs`, add to the `InstallError` enum (it is `#[non_exhaustive]`):

```rust
    /// The install-time `cargo build` could not be sandboxed and
    /// `--allow-unsandboxed-build` was not passed.
    #[error(
        "refusing to run an unsandboxed build of untrusted package code; \
         re-run with --allow-unsandboxed-build to override (audit S2)"
    )]
    UnsandboxedBuildRefused,
```

In `crates/tau-pkg/src/install.rs`, add fields to `InstallOptions` (struct at line ~159) and its `Default` impl (line ~183). The new `sandbox` field forces a manual `Debug` impl, so **remove `Debug` from the derive** and add a hand-written one:

```rust
// at top of file, ensure these are imported:
use std::sync::Arc;
use crate::install_sandbox::InstallSandbox;

// change:  #[derive(Debug, Clone)]   ->   #[derive(Clone)]   on InstallOptions
#[non_exhaustive]
#[derive(Clone)]
pub struct InstallOptions {
    // ... existing fields unchanged ...
    /// Install-time sandbox gate, injected by the caller (`tau-cli`). When an
    /// enforcing gate is present, the build + cross-check run sandboxed. When
    /// absent, the install fails closed unless `allow_unsandboxed_build` is set.
    pub sandbox: Option<Arc<dyn InstallSandbox>>,
    /// Explicit escape hatch: permit an unsandboxed build when no enforcing
    /// gate is available. Maps to `tau install --allow-unsandboxed-build`.
    pub allow_unsandboxed_build: bool,
}

impl std::fmt::Debug for InstallOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstallOptions")
            .field("block_on_lock", &self.block_on_lock)
            .field("force", &self.force)
            .field("build", &self.build)
            .field("skip_cross_check", &self.skip_cross_check)
            .field("sandbox", &self.sandbox.as_ref().map(|_| "<gate>"))
            .field("allow_unsandboxed_build", &self.allow_unsandboxed_build)
            .finish()
    }
}
```

In the `Default` impl add:

```rust
            sandbox: None,
            allow_unsandboxed_build: false,
```

- [ ] **Step 4: Run to verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-s2 cargo nextest run -p tau-pkg`
Expected: PASS (existing tests still green + 2 new).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/error.rs crates/tau-pkg/src/install.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-pkg): InstallOptions sandbox fields + refused error (audit S2)"
```

---

## Task 5: Sandbox the build spawn

**Files:**
- Modify: `crates/tau-pkg/src/install.rs` — `build_plugin_if_needed` (line ~622), `build_rust_cargo_plugin` (line ~654), and the call site (line ~445).

- [ ] **Step 1: Write the failing test**

Add to `crates/tau-pkg/src/install.rs` `mod tests` (a focused unit test on the build wrapper using a minimal manifest + a non-existent cargo so the build never actually runs — we only assert the *decision* fires before spawn):

```rust
#[test]
fn build_refuses_when_unsandboxed_and_not_allowed() {
    use crate::install_sandbox::tests::MockInstallSandbox; // re-export for tests; see note
    // A non-enforcing gate + no allow flag must refuse BEFORE spawning cargo.
    let gate = MockInstallSandbox::new(false);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"),
        "[package]\nname=\"p\"\nversion=\"0.1.0\"\n").unwrap();
    let err = run_cargo_build_gated(
        dir.path(),
        "p-bin",
        Some(&gate),
        false,
        &BuildOptions::default(),
    ).unwrap_err();
    assert!(matches!(err, crate::error::InstallError::UnsandboxedBuildRefused));
}
```

Note: to share `MockInstallSandbox` between `install_sandbox`'s tests and `install`'s tests, move `MockInstallSandbox` out of `#[cfg(test)] mod tests` into a `#[cfg(any(test, feature = "test-fixtures"))]` block in `install_sandbox.rs`, or simplest: define it under `pub(crate) mod test_support` gated on `#[cfg(test)]` and `pub(crate) use`. Pick the in-crate `#[cfg(test)] pub(crate)` path (no new feature).

- [ ] **Step 2: Run to verify failure**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-s2 cargo build -p tau-pkg --tests`
Expected: FAIL — `cannot find function run_cargo_build_gated`.

- [ ] **Step 3: Write minimal implementation**

Refactor `build_rust_cargo_plugin` to (a) thread the gate + allow flag, (b) make the fail-closed decision, (c) wrap the command before `cmd.output()`. Extract the post-`Command`-construction "decide + wrap + spawn" into `run_cargo_build_gated` so it is unit-testable:

```rust
use crate::install_sandbox::{build_envelope, sandbox_decision, InstallSandbox, SandboxDecision};

/// Decide-then-wrap-then-spawn the configured `cargo build` command.
/// Returns the captured `std::process::Output`.
fn run_cargo_build_gated(
    package_dir: &Path,
    bin: &str,
    gate: Option<&dyn InstallSandbox>,
    allow_unsandboxed_build: bool,
    options: &BuildOptions,
) -> Result<std::process::Output, InstallError> {
    let cargo = options.cargo_path.clone().unwrap_or_else(|| PathBuf::from("cargo"));
    let target_dir = package_dir.join("target");
    let mut cmd = Command::new(&cargo);
    cmd.arg("build").arg("--release").arg("--bin").arg(bin)
        .current_dir(package_dir)
        .env("CARGO_TARGET_DIR", &target_dir);
    for arg in &options.extra_args {
        cmd.arg(arg);
    }

    let _guard = match sandbox_decision(gate, allow_unsandboxed_build) {
        SandboxDecision::Sandbox => {
            let plan = build_envelope(package_dir);
            let g = gate.expect("Sandbox decision implies a gate is present");
            Some(g.wrap(&plan, &mut cmd).map_err(|e| InstallError::Internal {
                message: format!("wrapping cargo build in sandbox: {e}"),
            })?)
        }
        SandboxDecision::Unsandboxed => {
            tracing::warn!(
                target: "tau_pkg::install",
                "building untrusted package code WITHOUT a sandbox \
                 (--allow-unsandboxed-build); build.rs runs with full host access",
            );
            None
        }
        SandboxDecision::Refuse => return Err(InstallError::UnsandboxedBuildRefused),
    };

    cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            InstallError::CargoNotFound
        } else {
            InstallError::Internal { message: format!("spawning cargo at {}: {e}", cargo.display()) }
        }
    })
}
```

Then in `build_rust_cargo_plugin`, replace the inline `Command` construction + `cmd.output()` (lines ~687-710) with a call to `run_cargo_build_gated(package_dir, &plugin_manifest.bin, gate, allow_unsandboxed_build, options)`, keeping the existing stderr-streaming + status-check + binary-path logic on the returned `output`. Add `gate: Option<&dyn InstallSandbox>` and `allow_unsandboxed_build: bool` params to both `build_rust_cargo_plugin` and `build_plugin_if_needed`, and pass them from the call site at line ~445:

```rust
let mut locked_plugin = build_plugin_if_needed(
    &manifest, &target, &options.build,
    options.sandbox.as_deref(), options.allow_unsandboxed_build,
)?;
```

(`options.sandbox.as_deref()` turns `&Option<Arc<dyn InstallSandbox>>` into `Option<&dyn InstallSandbox>`.)

- [ ] **Step 4: Run to verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-s2 cargo nextest run -p tau-pkg`
Expected: PASS — new refuse test green; existing build tests still green (they use `skip_build` or pass a gate / allow flag — see Step 5 note).

- [ ] **Step 4b: Fix existing tests that now hit fail-closed**

Any existing test that drives a real build path with default `InstallOptions` will now get `UnsandboxedBuildRefused`. Grep them:
`git grep -n "skip_build\|build:.*BuildOptions\|install_with_options" crates/tau-pkg`.
For tests that *do* build, set `options.allow_unsandboxed_build = true` (they run in CI without a real sandbox). Tests that set `skip_build = true` are unaffected (no build → no decision).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/install.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-pkg): sandbox or fail-closed the install build (audit S2)"
```

---

## Task 6: Sandbox the cross-check spawn

**Files:**
- Modify: `crates/tau-pkg/src/sandbox_check.rs` — `cross_check_plugin_capabilities` (line ~104, the spawn at ~109).
- Modify: `crates/tau-pkg/src/install.rs` — the cross-check call site (line ~466).

- [ ] **Step 1: Write the failing test**

Add to `crates/tau-pkg/src/sandbox_check.rs` `mod tests`:

```rust
#[tokio::test]
async fn cross_check_refuses_unsandboxed_without_flag() {
    use crate::install_sandbox::tests::MockInstallSandbox;
    let gate = MockInstallSandbox::new(false);
    let manifest = crate::install::tests::minimal_plugin_manifest(); // tiny helper, see note
    // binary path need not exist: the fail-closed decision precedes spawn.
    let err = cross_check_plugin_capabilities_gated(
        std::path::Path::new("/nonexistent/bin"),
        &manifest,
        Some(&gate),
        false,
    ).await.unwrap_err();
    assert!(matches!(err, CrossCheckError::SandboxRefused));
}
```

Note: add a small `pub(crate) fn minimal_plugin_manifest() -> PackageManifest` test helper in `install.rs` (deserialize a minimal tau.toml with a `[plugin]` table) if one does not already exist.

- [ ] **Step 2: Run to verify failure**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-s2 cargo build -p tau-pkg --tests`
Expected: FAIL — `cross_check_plugin_capabilities_gated` / `CrossCheckError::SandboxRefused` not found.

- [ ] **Step 3: Write minimal implementation**

In `crates/tau-pkg/src/sandbox_check.rs`:

1. Add a `SandboxRefused` variant to `CrossCheckError` (it is `#[non_exhaustive]`):

```rust
    /// The cross-check spawn could not be sandboxed and the unsandboxed
    /// escape hatch was not enabled.
    #[error(
        "refusing to spawn an unsandboxed cross-check of untrusted plugin code; \
         re-run with --allow-unsandboxed-build to override (audit S2)"
    )]
    SandboxRefused,
```

2. Rename the existing public fn to `cross_check_plugin_capabilities_gated` with two extra params, and keep a thin back-compat wrapper if any other crate calls the old name (grep: `git grep -n cross_check_plugin_capabilities crates`). Apply the decision + wrap before spawn:

```rust
use crate::install_sandbox::{cross_check_envelope, sandbox_decision, InstallSandbox, SandboxDecision};

pub async fn cross_check_plugin_capabilities_gated(
    binary_path: &Path,
    manifest: &PackageManifest,
    gate: Option<&dyn InstallSandbox>,
    allow_unsandboxed_build: bool,
) -> Result<Vec<CapabilityShape>, CrossCheckError> {
    let mut command = Command::new(binary_path);
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);

    let _guard = match sandbox_decision(gate, allow_unsandboxed_build) {
        SandboxDecision::Sandbox => {
            let plan = cross_check_envelope();
            let g = gate.expect("Sandbox decision implies a gate");
            Some(g.wrap(&plan, command.as_std_mut())
                .map_err(|e| CrossCheckError::SpawnFailed(format!("sandbox wrap: {e}")))?)
        }
        SandboxDecision::Unsandboxed => {
            tracing::warn!(target: "tau_pkg::install",
                "spawning cross-check WITHOUT a sandbox (--allow-unsandboxed-build)");
            None
        }
        SandboxDecision::Refuse => return Err(CrossCheckError::SandboxRefused),
    };

    let mut child = command.spawn()
        .map_err(|e| CrossCheckError::SpawnFailed(format!("{e}")))?;
    // ... rest of the existing function body unchanged (stdin/stdout take, handshake, diff) ...
}
```

Keep the old name as a wrapper used by nothing in production once the call site is updated; if no external caller exists, just rename in place.

3. Update the call site in `install.rs` (~466) to pass the gate + flag:

```rust
let shapes = block_on_in_fresh_thread(move || async move {
    crate::sandbox_check::cross_check_plugin_capabilities_gated(
        &binary_path, &manifest_for_check, gate_for_check, allow_for_check,
    ).await
})?
```

Capture `let gate_for_check = options.sandbox.clone();` (an `Option<Arc<dyn InstallSandbox>>`, which is `Send + 'static`) and `let allow_for_check = options.allow_unsandboxed_build;` before the closure, then inside pass `gate_for_check.as_deref()`.

- [ ] **Step 4: Run to verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-s2 cargo nextest run -p tau-pkg`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/sandbox_check.rs crates/tau-pkg/src/install.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-pkg): sandbox or fail-closed the cross-check spawn (audit S2)"
```

---

## Task 7: `RuntimeInstallSandbox` adapter in tau-cli

**Files:**
- Create: `crates/tau-cli/src/cmd/install_sandbox.rs`
- Modify: `crates/tau-cli/src/cmd/mod.rs` (add `pub mod install_sandbox;`)

The bridge: the runtime's `SandboxAdapter::wrap_spawn` is async and, on strict Linux, spawns an egress-proxy task that must outlive `cmd.output()`. The proxy task lives on whatever runtime ran `wrap_spawn`. To avoid tokio nesting panics (`Runtime::block_on` / `Handle::block_on` from inside a runtime thread both panic) and to keep the proxy alive, the adapter owns a **dedicated multi-thread runtime** and drives `wrap_spawn` on a **fresh OS thread** (scoped, so it can borrow `&mut Command`). The dedicated runtime is moved into the guard so the proxy survives until the guard drops.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tau_pkg::InstallSandbox;

    #[test]
    fn passthrough_adapter_reports_unenforced() {
        // Passthrough is tier=None → not enforcing.
        let rt = std::sync::Arc::new(tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1).enable_all().build().unwrap());
        let adapter = SandboxAdapter::Passthrough(Default::default());
        let g = RuntimeInstallSandbox::new(adapter, rt);
        assert!(!g.is_enforced());
    }
}
```

(If `PassthroughSandbox` has no `Default`, construct it via its public constructor — check `tau_runtime_tokio::process_gate::passthrough`; adjust the test accordingly.)

- [ ] **Step 2: Run to verify failure**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-s2 cargo build -p tau-cli --tests`
Expected: FAIL — `RuntimeInstallSandbox` not found.

- [ ] **Step 3: Write minimal implementation**

```rust
//! `tau-cli`'s adapter implementing `tau_pkg::InstallSandbox` with the real
//! runtime `SandboxAdapter` (audit S2). Bridges the sync port to the async
//! `ProcessCapabilityGate::wrap_spawn` and keeps the strict-tier egress proxy
//! alive for the duration of the gated spawn.

use std::process::Command;
use std::sync::Arc;

use tau_pkg::{InstallSandbox, InstallSandboxError, InstallSandboxGuard};
use tau_ports::capability_gate::{CapabilityHandle, CapabilityPlan, CapabilityProbe, CapabilityTier};
use tau_runtime_tokio::process_gate::resolver::SandboxAdapter;
use tau_ports::CapabilityGate; // for `probe()` / `name()`

pub struct RuntimeInstallSandbox {
    adapter: Arc<SandboxAdapter>,
    rt: Arc<tokio::runtime::Runtime>,
    enforced: bool,
}

impl RuntimeInstallSandbox {
    pub fn new(adapter: SandboxAdapter, rt: Arc<tokio::runtime::Runtime>) -> Self {
        // Probe once to learn the delivered tier.
        let enforced = matches!(
            rt.block_on(adapter.probe()),
            CapabilityProbe::Available { tier, .. } if tier > CapabilityTier::None
        );
        Self { adapter: Arc::new(adapter), rt, enforced }
    }
}

/// Holds the runtime + the live `CapabilityHandle` so the proxy task and any
/// namespace resources survive until the gated child exits.
struct RuntimeGuard {
    _handle: CapabilityHandle,
    _rt: Arc<tokio::runtime::Runtime>,
}

impl InstallSandbox for RuntimeInstallSandbox {
    fn is_enforced(&self) -> bool {
        self.enforced
    }

    fn wrap(
        &self,
        plan: &CapabilityPlan,
        cmd: &mut Command,
    ) -> Result<InstallSandboxGuard, InstallSandboxError> {
        let adapter = self.adapter.clone();
        let rt = self.rt.clone();
        // Fresh thread: no ambient runtime here, so `rt.block_on` is legal and
        // the proxy task spawned inside `wrap_spawn` lands on `rt` (kept alive
        // by the returned guard).
        let handle = std::thread::scope(|s| {
            s.spawn(|| rt.block_on(adapter.wrap_spawn(plan, cmd)))
                .join()
                .map_err(|_| InstallSandboxError::WrapFailed("wrap thread panicked".into()))?
                .map_err(|e| InstallSandboxError::WrapFailed(e.to_string()))
        })?;
        Ok(InstallSandboxGuard::new(RuntimeGuard { _handle: handle, _rt: self.rt.clone() }))
    }
}
```

If `SandboxAdapter::probe` / `wrap_spawn` are inherent methods (enum-dispatched) rather than trait methods in scope, call them directly (they are `pub async fn` on the enum per `resolver.rs:246-281`); drop the `CapabilityGate` import if unused.

- [ ] **Step 4: Run to verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-s2 cargo nextest run -p tau-cli install_sandbox`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-cli/src/cmd/install_sandbox.rs crates/tau-cli/src/cmd/mod.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-cli): RuntimeInstallSandbox bridge adapter (audit S2)"
```

---

## Task 8: Wire the flag + adapter into `tau install`

**Files:**
- Modify: `crates/tau-cli/src/cli.rs:388-399` (`InstallArgs`)
- Modify: `crates/tau-cli/src/cmd/install.rs:31-74` (handler) and `:53` (the `install_with_options` call)

- [ ] **Step 1: Write the failing test**

In `crates/tau-cli/src/cli.rs` tests (or wherever clap parse tests live — grep `try_parse_from`):

```rust
#[test]
fn install_accepts_allow_unsandboxed_build() {
    use clap::Parser;
    let cli = Cli::try_parse_from([
        "tau", "install", "https://example.com/p.git", "--allow-unsandboxed-build",
    ]).unwrap();
    // navigate to the InstallArgs and assert the flag is true
    match cli.command {
        Command::Install(a) => assert!(a.allow_unsandboxed_build),
        _ => panic!("expected install"),
    }
}
```

(Adjust `Cli` / `Command` names to the actual types in `cli.rs`.)

- [ ] **Step 2: Run to verify failure**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-s2 cargo build -p tau-cli --tests`
Expected: FAIL — no field `allow_unsandboxed_build`.

- [ ] **Step 3: Write minimal implementation**

In `cli.rs`, add to `InstallArgs`:

```rust
    /// Allow `cargo build` of the package to run WITHOUT an OS sandbox when no
    /// enforcing sandbox is available on this host. Install-time build code
    /// (build.rs, proc macros) then runs with full host access. (audit S2)
    #[arg(long)]
    pub allow_unsandboxed_build: bool,
```

In `crates/tau-cli/src/cmd/install.rs`, replace the line-53 call. Resolve an adapter (reuse the same default `SandboxRequirements` the plugin path uses), wrap it, and pass options. The handler is already `async`:

```rust
use crate::cmd::install_sandbox::RuntimeInstallSandbox;
use tau_runtime_tokio::process_gate::resolver::resolve_adapter;
use std::sync::Arc;

// build a dedicated runtime for the sandbox bridge (the proxy task lives here)
let sandbox_rt = Arc::new(
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?,
);
// resolve the adapter using default requirements (no plugin floors at install time)
let requirements = tau_pkg::scope::SandboxRequirements::default();
let adapter = resolve_adapter(&requirements, &[]).await
    .map_err(|e| anyhow::anyhow!("resolving install sandbox adapter: {e}"))?;
let gate = RuntimeInstallSandbox::new(adapter, sandbox_rt);

let mut options = InstallOptions::default();
options.sandbox = Some(Arc::new(gate));
options.allow_unsandboxed_build = args.allow_unsandboxed_build;

let installed = install_with_options(&source, &scope, options)?;
```

Confirm `SandboxRequirements::default()` exists and yields the host's best tier (grep `impl Default for SandboxRequirements` / `SandboxRequirements::` in `crates/tau-pkg/src/scope`). If there is no `Default`, construct it the same way `plugin_loader.rs:146-154` does.

- [ ] **Step 4: Run to verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-s2 cargo nextest run -p tau-cli`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-cli/src/cli.rs crates/tau-cli/src/cmd/install.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-cli): wire install build sandbox + --allow-unsandboxed-build (audit S2)"
```

---

## Task 9: Workspace clippy + fmt + full gate

**Files:** none (verification only)

- [ ] **Step 1: fmt**

Run: `timeout 30 env CARGO_TARGET_DIR=target/agent-s2 cargo fmt -p tau-pkg -p tau-cli -- --check`
Expected: clean (run without `--check` to fix, then re-check).

- [ ] **Step 2: clippy (both crates)**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-s2 cargo clippy -p tau-pkg -p tau-cli --all-targets`
Expected: no warnings (the repo denies warnings in CI).

- [ ] **Step 3: full test sweep + doctests**

Run:
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-s2 cargo nextest run -p tau-pkg -p tau-cli
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-s2 cargo test --doc -p tau-pkg
```
Expected: all PASS.

- [ ] **Step 4: Open the PR**

```bash
git push -u origin install-build-sandbox
gh pr create --base main --title "fix(security): sandbox install-time build + cross-check (audit S2)" \
  --body "Closes audit S2. tau-pkg defines an InstallSandbox port + build/cross-check capability envelopes + fail-closed decision; tau-cli injects the real SandboxAdapter via a RuntimeInstallSandbox bridge. Escape hatch: --allow-unsandboxed-build.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

---

## Self-Review notes

- **Spec coverage:** port (T1), envelopes incl. registry+git-host net (T2), fail-closed (T3/T4), build sandbox (T5), cross-check sandbox (T6), tau-cli adapter w/ proxy-lifetime bridge (T7), flag + wiring (T8). All spec sections map to a task.
- **Type consistency:** `InstallSandbox::{is_enforced, wrap}`, `InstallSandboxGuard::{new, noop}`, `SandboxDecision::{Sandbox, Unsandboxed, Refuse}`, `sandbox_decision(gate, allow)`, `build_envelope(dir)`, `cross_check_envelope()`, `git_dep_hosts(dir)`, `host_of_git_url(url)`, `InstallError::UnsandboxedBuildRefused`, `CrossCheckError::SandboxRefused`, `RuntimeInstallSandbox::new(adapter, rt)` — names used identically across tasks.
- **Known risk to validate during T7/T8:** confirm `SandboxAdapter::probe`/`wrap_spawn` are reachable as `pub` from tau-cli and that `cmd: &mut std::process::Command` matches (`process.rs:9` imports `std::process::Command` — confirmed). Confirm `SandboxRequirements::default()` exists; fall back to the `plugin_loader.rs` construction if not.
- **Deferred (per spec):** workspace/target/dev-dep git hosts; offline build mode; sandboxing `Git::clone`.
```
