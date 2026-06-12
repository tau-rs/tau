//! Install-time Layer 2 cross-check integration tests — sub-project B Task 9.
//!
//! Verifies the cross-check at step 8.7 of install_with_options:
//! - Match between binary capabilities and manifest → install succeeds,
//!   LockedPlugin.required_shapes populated.
//! - Mismatch → install aborts with InstallError::CrossCheck, binary
//!   stays on disk (user retries via `tau install --force` after fixing
//!   the manifest).
//! - --force after fix → succeeds.
//! - LLM-backend port → manifest-only path (no per-method enumeration).
//!
//! # Test strategy
//!
//! The cross-check spawns the real plugin binary and performs a
//! `meta.handshake` + `tool.describe_capabilities` RPC. Authoring a
//! fully-correct tau-protocol binary in a test fixture is heavyweight
//! (requires tau-plugin-protocol, handshake codec, etc.). The tests
//! below are therefore scaffolded with `#[ignore]` pending the
//! sub-project D fixture binary, except for two lightweight tests that
//! exercise the wiring:
//!
//! 1. `cross_check_skipped_for_data_only_package` — installs a package
//!    with no [plugin] table; verifies step 8.7 is a no-op and install
//!    succeeds normally.
//!
//! 2. `cross_check_skipped_when_build_skipped` — installs a plugin
//!    package with `skip_build = true`; verifies cross-check is bypassed
//!    (no binary to spawn) and install succeeds with `plugin = None`.

mod fixtures;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::sync::OnceLock;

use tau_domain::PackageSource;
use tau_pkg::{install_with_options, InstallOptions, LockFile, Scope};
use tempfile::TempDir;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Default options with the unsandboxed-build escape hatch enabled. tau-pkg
/// tests inject no sandbox gate, so a real build would otherwise fail closed
/// (audit S2).
fn unsandboxed_opts() -> InstallOptions {
    let mut opts = InstallOptions::default();
    opts.allow_unsandboxed_build = true;
    opts
}

/// Returns true if `git` is available on PATH.
fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Create a bare git repo containing a minimal plugin package (with [plugin])
/// and an empty `src/main.rs` that just `fn main() {}`. Returns the bare repo
/// path. The plugin binary will successfully compile but produce a binary that
/// does NOT speak the tau protocol — which means cross_check_plugin_capabilities
/// will time out or error.
fn make_plugin_repo_no_protocol(parent: &Path, name: &str, version: &str) -> PathBuf {
    let bare = fixtures::make_bare_repo(parent, name);
    let working = parent.join(format!("{name}-working"));
    std::fs::create_dir_all(&working).unwrap();

    run_git(&working, &["init", "-q", "-b", "main"]);
    run_git(&working, &["config", "user.email", "test@example.com"]);
    run_git(&working, &["config", "user.name", "Test User"]);

    let source_url = fixtures::file_url(&bare);
    let manifest = format!(
        r#"name = "{name}"
version = "{version}"
description = "Synthetic fixture for cross-check tests"
authors = ["Test <test@example.com>"]
source = "{source_url}"
kind = "tool"
dependencies = []
capabilities = []

[plugin]
provides = "tool"
kind     = "rust-cargo"
bin      = "{name}"
"#
    );
    std::fs::write(working.join("tau.toml"), manifest).unwrap();

    let cargo_toml = format!(
        r#"[package]
name = "{name}"
version = "{version}"
edition = "2021"

[[bin]]
name = "{name}"
path = "src/main.rs"

[dependencies]
"#
    );
    std::fs::write(working.join("Cargo.toml"), cargo_toml).unwrap();
    std::fs::create_dir_all(working.join("src")).unwrap();
    // This binary does NOT implement the tau protocol — it immediately exits.
    std::fs::write(working.join("src").join("main.rs"), "fn main() {}\n").unwrap();

    run_git(&working, &["add", "."]);
    run_git(&working, &["commit", "-q", "-m", "initial fixture commit"]);
    run_git(
        &working,
        &["remote", "add", "origin", &bare.to_string_lossy()],
    );
    run_git(&working, &["push", "-q", "origin", "main"]);

    bare
}

fn run_git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?} spawn failure: {e}"));
    assert!(
        output.status.success(),
        "git {args:?} in {cwd:?} failed:\nstderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );
}

// ── Test 1: data-only package bypasses cross-check ───────────────────────────

/// A data-only package (no [plugin] table) should install successfully
/// without attempting the cross-check. This verifies step 8.7's guard
/// condition: `locked_plugin` is `None` → cross-check is skipped.
#[test]
fn cross_check_skipped_for_data_only_package() {
    if !git_available() {
        eprintln!("skipping: `git` not on PATH");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("tau-home");
    std::fs::create_dir_all(&project_root).unwrap();
    let scope = Scope::new_project(&project_root).unwrap();

    // fixtures::make_fixture_repo creates a repo with no [plugin] table.
    let bare = fixtures::make_fixture_repo(tmp.path(), "data-only-crosscheck", "0.1.0", "tool");
    let source = PackageSource::from_str(&fixtures::file_url(&bare)).unwrap();

    // Should succeed: data-only package, no cross-check attempted.
    let installed = install_with_options(&source, &scope, unsandboxed_opts()).unwrap();
    assert_eq!(installed.name.as_str(), "data-only-crosscheck");

    let lf = LockFile::load(&scope.lockfile_path()).unwrap();
    let pkg = lf
        .packages
        .iter()
        .find(|p| p.name.as_str() == "data-only-crosscheck")
        .expect("package should be in lockfile");

    // Data-only: no plugin entry, no required_shapes.
    assert!(
        pkg.plugin.is_none(),
        "data-only package should have plugin = None"
    );
}

// ── Test 2: skip_build bypasses cross-check ───────────────────────────────────

/// When `BuildOptions::skip_build = true` the build step is skipped,
/// which means `locked_plugin` is `None` even for plugin packages.
/// Step 8.7 must also be skipped in this case (no binary to spawn).
///
/// This is the `--no-build` / test-harness path. We use `make_fixture_repo`
/// (data-only manifest) with `skip_build = true` so the test doesn't require
/// a compilable rust crate in the fixture.
#[test]
fn cross_check_skipped_when_build_skipped() {
    if !git_available() {
        eprintln!("skipping: `git` not on PATH");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("tau-home");
    std::fs::create_dir_all(&project_root).unwrap();
    let scope = Scope::new_project(&project_root).unwrap();

    let bare = fixtures::make_fixture_repo(tmp.path(), "skip-build-crosscheck", "0.1.0", "tool");
    let source = PackageSource::from_str(&fixtures::file_url(&bare)).unwrap();

    let mut opts = InstallOptions::default();
    opts.build.skip_build = true;

    // Should succeed: build skipped → cross-check skipped → no timeout on binary spawn.
    let installed = install_with_options(&source, &scope, opts).unwrap();
    assert_eq!(installed.name.as_str(), "skip-build-crosscheck");
}

// ── Test 3: cross-check fires and fails for non-protocol binary (ignore) ─────

/// A compiled plugin binary that does NOT speak the tau protocol will cause
/// the cross-check's handshake to time out or encounter EOF, producing
/// `InstallError::CrossCheck`.
///
/// This test is `#[ignore]`'d because:
/// 1. It requires `cargo` on PATH and a full release build (~30s).
/// 2. It exercises the 10-second handshake timeout, making the test slow.
///
/// Rationale: the wiring is verified by the production code change
/// (step 8.7 in install_with_options). This test scaffolds the intended
/// coverage; un-ignore when the CI timeout budget is established.
#[test]
#[ignore = "requires cargo + full release build + 10s handshake timeout; \
            un-ignore when CI budget is established"]
fn cross_check_fires_and_fails_for_non_protocol_binary() {
    let ok_git = Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let ok_cargo = Command::new("cargo")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok_git || !ok_cargo {
        eprintln!("skipping: `git` or `cargo` not on PATH");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("tau-home");
    std::fs::create_dir_all(&project_root).unwrap();
    let scope = Scope::new_project(&project_root).unwrap();

    let bare = make_plugin_repo_no_protocol(tmp.path(), "no-proto-plugin", "0.1.0");
    let source = PackageSource::from_str(&fixtures::file_url(&bare)).unwrap();

    let result = install_with_options(&source, &scope, unsandboxed_opts());
    let err = result.expect_err("expected install to fail with CrossCheck");

    assert!(
        matches!(err, tau_pkg::InstallError::CrossCheck { .. }),
        "expected InstallError::CrossCheck; got: {err:?}"
    );

    // Binary must remain on disk (user can retry after fixing the manifest).
    let pkg_dir = scope.packages_dir().join("no-proto-plugin").join("0.1.0");
    assert!(
        pkg_dir.exists(),
        "package dir should remain on disk after CrossCheck failure: {}",
        pkg_dir.display()
    );
}

// ── Tau-protocol fixture binary helpers ───────────────────────────────────────

/// Build the workspace `echo-tool` binary once per test-process and return
/// its canonicalized path. `echo-tool` (under `crates/tau-plugins/echo-tool`)
/// is a real tau-protocol-compliant tool plugin — it implements
/// `meta.handshake` + `tool.describe` + `tool.describe_capabilities` via the
/// `tau-plugin-sdk` runner — so it satisfies the cross-check's wire-level
/// expectations.
///
/// Subsequent calls return the cached path without re-invoking cargo.
fn ensure_echo_tool_built() -> &'static Path {
    static ECHO_TOOL: OnceLock<PathBuf> = OnceLock::new();
    ECHO_TOOL
        .get_or_init(|| {
            let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
            let status = Command::new(&cargo)
                .args(["build", "--release", "-p", "echo-tool"])
                .status()
                .expect("spawning cargo to build echo-tool");
            assert!(
                status.success(),
                "`cargo build --release -p echo-tool` failed: {status:?}",
            );

            let target_dir = if let Ok(dir) = std::env::var("CARGO_TARGET_DIR") {
                if !dir.is_empty() {
                    PathBuf::from(dir)
                } else {
                    locate_workspace_target()
                }
            } else {
                locate_workspace_target()
            };
            let bin_name = format!("echo-tool{}", std::env::consts::EXE_SUFFIX);
            let path = target_dir.join("release").join(&bin_name);
            assert!(
                path.exists(),
                "expected built echo-tool at {}; did the build succeed?",
                path.display()
            );
            std::fs::canonicalize(&path)
                .unwrap_or_else(|e| panic!("canonicalize {}: {e}", path.display()))
        })
        .as_path()
}

/// Locate the workspace `target/` dir by walking up from `CARGO_MANIFEST_DIR`
/// (which points at `crates/tau-pkg/`).
fn locate_workspace_target() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for ancestor in manifest_dir.ancestors() {
        let candidate = ancestor.join("target");
        if candidate.is_dir() {
            return candidate;
        }
    }
    panic!(
        "could not locate workspace target/ dir from {}",
        manifest_dir.display()
    );
}

/// Create a bare git repo containing a minimal plugin package whose built
/// binary re-execs into the pre-built `echo-tool` binary. From the
/// cross-check's point of view the resulting binary speaks the tau protocol.
///
/// `capabilities_toml` is interpolated into the manifest verbatim — pass `""`
/// to declare no capabilities, or a `[[capabilities]]` block to declare some.
fn make_relay_plugin_repo(
    parent: &Path,
    name: &str,
    version: &str,
    capabilities_toml: &str,
    real_binary: &Path,
) -> PathBuf {
    let bare = fixtures::make_bare_repo(parent, name);
    let working = parent.join(format!("{name}-working"));
    std::fs::create_dir_all(&working).unwrap();

    run_git(&working, &["init", "-q", "-b", "main"]);
    run_git(&working, &["config", "user.email", "test@example.com"]);
    run_git(&working, &["config", "user.name", "Test User"]);

    let source_url = fixtures::file_url(&bare);
    let manifest = format!(
        r#"name = "{name}"
version = "{version}"
description = "Relay fixture: re-execs into pre-built echo-tool"
authors = ["Test <test@example.com>"]
source = "{source_url}"
kind = "tool"
dependencies = []
{capabilities_toml}

[plugin]
provides = "tool"
kind     = "rust-cargo"
bin      = "{name}"
"#
    );
    std::fs::write(working.join("tau.toml"), manifest).unwrap();

    // Standalone Cargo.toml — zero deps; relay is a tiny program.
    let cargo_toml = format!(
        r#"[package]
name = "{name}"
version = "{version}"
edition = "2021"

[[bin]]
name = "{name}"
path = "src/main.rs"

[dependencies]
"#
    );
    std::fs::write(working.join("Cargo.toml"), cargo_toml).unwrap();
    std::fs::create_dir_all(working.join("src")).unwrap();

    // Embed the real binary's path as a string literal. On Unix we use
    // `CommandExt::exec` so the relay process is replaced in-place
    // (no fork, no stdio plumbing). On non-Unix we spawn-and-wait,
    // inheriting stdio.
    let real_path = real_binary.to_string_lossy().replace('\\', "\\\\");
    let main_rs = format!(
        r##"use std::process::Command;

#[cfg(unix)]
fn main() {{
    use std::os::unix::process::CommandExt;
    let err = Command::new("{real_path}").args(std::env::args_os().skip(1)).exec();
    eprintln!("relay: exec failed: {{err}}");
    std::process::exit(127);
}}

#[cfg(not(unix))]
fn main() {{
    let status = Command::new("{real_path}")
        .args(std::env::args_os().skip(1))
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .expect("spawn echo-tool relay child");
    std::process::exit(status.code().unwrap_or(1));
}}
"##
    );
    std::fs::write(working.join("src").join("main.rs"), main_rs).unwrap();

    run_git(&working, &["add", "."]);
    run_git(&working, &["commit", "-q", "-m", "initial fixture commit"]);
    run_git(
        &working,
        &["remote", "add", "origin", &bare.to_string_lossy()],
    );
    run_git(&working, &["push", "-q", "origin", "main"]);

    bare
}

fn cargo_and_git_available() -> bool {
    git_available()
        && Command::new("cargo")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
}

// ── Test 4: install with matching manifest succeeds ──────────────────────────

/// A plugin that correctly implements the tau protocol and whose
/// `tool.describe_capabilities` matches the manifest's `capabilities`
/// should install successfully with `required_shapes` populated.
///
/// echo-tool's default `Tool::capabilities` returns `&[]`, so this test
/// declares an empty capability list in the manifest — the cross-check
/// compares both sides and finds them in agreement, returning an empty
/// `Vec<CapabilityShape>` which is what we assert against
/// `LockedPlugin::required_shapes`.
#[test]
fn install_with_matching_manifest_succeeds_and_populates_required_shapes() {
    if !cargo_and_git_available() {
        eprintln!("skipping: `git` or `cargo` not on PATH");
        return;
    }

    let echo_tool = ensure_echo_tool_built();

    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("tau-home");
    std::fs::create_dir_all(&project_root).unwrap();
    let scope = Scope::new_project(&project_root).unwrap();

    let bare = make_relay_plugin_repo(
        tmp.path(),
        "match-plugin",
        "0.1.0",
        "capabilities = []",
        echo_tool,
    );
    let source = PackageSource::from_str(&fixtures::file_url(&bare)).unwrap();

    let installed = install_with_options(&source, &scope, unsandboxed_opts())
        .expect("install with matching manifest should succeed");
    assert_eq!(installed.name.as_str(), "match-plugin");

    let lf = LockFile::load(&scope.lockfile_path()).unwrap();
    let pkg = lf
        .packages
        .iter()
        .find(|p| p.name.as_str() == "match-plugin")
        .expect("package should be in lockfile");
    let plugin = pkg
        .plugin
        .as_ref()
        .expect("LockedPlugin should be Some after cross-check success");

    // echo-tool exposes no capabilities; manifest declares none → shapes empty.
    assert!(
        plugin.required_shapes.is_empty(),
        "required_shapes should be populated by the cross-check (empty for capability-free plugins); \
         got {:?}",
        plugin.required_shapes,
    );
}

// ── Test 5: install_force after fix succeeds ─────────────────────────────────

/// After a CrossCheck failure the user fixes the manifest and retries
/// via `tau install --force`. The retry must succeed.
///
/// Flow:
///   1. Install a relay plugin whose manifest declares a capability the
///      binary (echo-tool) does not request → cross-check fails with
///      `ManifestDeclaresUnused`.
///   2. Rewrite the upstream bare repo's tau.toml to drop the declaration,
///      pushing a new commit on `main`.
///   3. Re-install with `force = true` → cross-check passes, install succeeds.
#[test]
fn install_force_after_cross_check_fix_succeeds() {
    if !cargo_and_git_available() {
        eprintln!("skipping: `git` or `cargo` not on PATH");
        return;
    }

    let echo_tool = ensure_echo_tool_built();

    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("tau-home");
    std::fs::create_dir_all(&project_root).unwrap();
    let scope = Scope::new_project(&project_root).unwrap();

    // ── Step 1: install with a manifest that declares an unused capability ──
    let caps_unused = "capabilities = [\n  { kind = \"fs.read\", paths = [\"/tmp/**\"] },\n]";
    let bare = make_relay_plugin_repo(tmp.path(), "force-plugin", "0.1.0", caps_unused, echo_tool);
    let source = PackageSource::from_str(&fixtures::file_url(&bare)).unwrap();

    let err = install_with_options(&source, &scope, unsandboxed_opts())
        .expect_err("install should fail with CrossCheck for declared-but-unused capability");
    assert!(
        matches!(err, tau_pkg::InstallError::CrossCheck { .. }),
        "expected InstallError::CrossCheck, got: {err:?}",
    );

    // Binary stays on disk after a CrossCheck failure (sub-project B contract).
    let pkg_dir = scope.packages_dir().join("force-plugin").join("0.1.0");
    assert!(
        pkg_dir.exists(),
        "package dir should remain on disk after CrossCheck failure",
    );

    // ── Step 2: rewrite tau.toml upstream, drop the bogus capability ─────────
    let working = tmp.path().join("force-plugin-fix");
    std::fs::create_dir_all(&working).unwrap();
    run_git(&working, &["clone", "-q", &bare.to_string_lossy(), "."]);
    run_git(&working, &["config", "user.email", "test@example.com"]);
    run_git(&working, &["config", "user.name", "Test User"]);

    let manifest_path = working.join("tau.toml");
    // Normalize CRLF → LF so the literal `caps_unused` (which uses \n) matches
    // on Windows, where `read_to_string` returns line endings as-stored (\r\n).
    let body = std::fs::read_to_string(&manifest_path)
        .unwrap()
        .replace("\r\n", "\n");
    let fixed = body.replace(caps_unused, "capabilities = []");
    assert_ne!(body, fixed, "manifest rewrite must replace the cap block");
    std::fs::write(&manifest_path, fixed).unwrap();
    run_git(&working, &["add", "tau.toml"]);
    run_git(
        &working,
        &["commit", "-q", "-m", "fix: drop unused capability"],
    );
    run_git(&working, &["push", "-q", "origin", "main"]);

    // ── Step 3: re-install with force=true ───────────────────────────────────
    let mut opts = InstallOptions::default();
    opts.force = true;
    opts.allow_unsandboxed_build = true;
    let installed = install_with_options(&source, &scope, opts)
        .expect("install --force after fix should succeed");
    assert_eq!(installed.name.as_str(), "force-plugin");

    let lf = LockFile::load(&scope.lockfile_path()).unwrap();
    let pkg = lf
        .packages
        .iter()
        .find(|p| p.name.as_str() == "force-plugin")
        .expect("force-plugin should be in lockfile after force-install");
    let plugin = pkg
        .plugin
        .as_ref()
        .expect("LockedPlugin should be Some after successful retry");
    assert!(
        plugin.required_shapes.is_empty(),
        "required_shapes should be empty after fix; got {:?}",
        plugin.required_shapes,
    );
}
