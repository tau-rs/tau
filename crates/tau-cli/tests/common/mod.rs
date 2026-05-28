//! Shared test helpers for tau-cli integration tests.
//!
//! Three families of fixtures live here:
//!
//! - [`temp_project`] / [`temp_project_with_tau_toml`] / [`read_tau_toml`]:
//!   minimal cwd helpers that landed with sub-project 9 Task 10.
//! - [`install_fixture`] / [`setup_project_with_installed_agent`] /
//!   [`setup_project`]: hand-author a project `tau.toml` plus a matching
//!   `.tau/` lockfile + on-disk package tree, mirroring the unit-test
//!   `install_fixture` from `crates/tau-cli/src/config/agent.rs`. The
//!   lockfile entries here carry NO `[plugin]` table, so they're only
//!   safe for tests that don't drive `tau run` / `tau chat` past plugin
//!   loading.
//! - [`setup_echo_project`] (+ the [`echo_plugins`] sub-module): build
//!   `echo-llm` and `echo-tool` once per session via
//!   [`echo_plugins::ensure_echo_plugins_built`] and synthesize a
//!   project tau.toml + lockfile whose `[package.plugin]` entries point
//!   at the resulting binaries. Used by every integration test that
//!   needs a real plugin spawn (Task 21).
//! - [`run_git`] / [`file_url`] / [`setup_local_package_fixture`]: the
//!   bare-repo + working-repo `file://` git fixture used by `tau install`
//!   and `tau list` integration tests. Honours the `init.defaultBranch`
//!   override (`refs/heads/main`) and tau-pkg's
//!   `protocol.file.allow=always` plumbing so the suite is portable across
//!   CI runners.
//!
//! All helpers are `#[allow(dead_code)]` so that no-features and partial
//! `--test <foo>` builds compile without warnings — different `cmd_*.rs`
//! files use different subsets.

#![allow(dead_code, unused_imports)]

pub mod echo_plugins;
pub mod mock_llm;
pub use mock_llm::MockLlmBackend;

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::str::FromStr;

use tempfile::TempDir;

// ---- orchestration test helpers (T9: pattern tests) -------------------------

/// Build a minimal `AgentDefinition` for orchestration pattern tests.
pub fn agent_def(
    id: &str,
    display_name: &str,
    package_id: &str,
    llm_backend_name: &str,
) -> tau_domain::AgentDefinition {
    let (name, version) = package_id
        .split_once('@')
        .expect("package id must use <name>@<version> form");
    let package = tau_domain::PackageId::new(
        tau_domain::PackageName::from_str(name).expect("valid package name"),
        tau_domain::Version::parse(version).expect("valid version"),
    );
    tau_domain::AgentDefinition::new(
        tau_domain::AgentId::from_str(id).expect("valid agent id"),
        display_name.to_string(),
        package,
        tau_domain::PackageName::from_str(llm_backend_name).expect("valid llm backend name"),
    )
}

/// Parse a TOML manifest body into a validated `PackageManifest`.
/// Panics on failure — test fixtures are author-controlled.
pub fn manifest_from_toml(toml_body: &str) -> tau_domain::PackageManifest {
    let unchecked: tau_domain::UncheckedManifest =
        toml::from_str(toml_body).expect("test manifest TOML must parse");
    unchecked
        .validate()
        .expect("test manifest must satisfy validation")
}

/// Build a fresh user-authored `Message` with the given text payload.
pub fn user_message(content: &str) -> tau_domain::Message {
    tau_domain::Message::new(
        tau_domain::Address::User,
        tau_domain::Address::User, // overwritten by runtime
        tau_domain::MessagePayload::Text {
            content: content.to_string(),
        },
    )
}

// ---- minimal cwd helpers ----------------------------------------------------

/// Create a tempdir to use as the project cwd.
pub fn temp_project() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// Create a tempdir with a tau.toml of the given contents.
pub fn temp_project_with_tau_toml(contents: &str) -> TempDir {
    let dir = temp_project();
    std::fs::write(dir.path().join("tau.toml"), contents).expect("write tau.toml");
    dir
}

/// Read the tau.toml from a tempdir; panic if missing.
pub fn read_tau_toml(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("tau.toml")).expect("read tau.toml")
}

/// Build a [`tau_pkg::RequiredTool`] from string fixtures.  For tests that
/// construct in-memory fixtures (vs. emitting TOML through
/// [`setup_echo_project`]).
pub fn make_required_tool(name: &str, source_url: &str, version: &str) -> tau_pkg::RequiredTool {
    use std::str::FromStr;
    tau_pkg::RequiredTool::new(
        tau_domain::PackageName::from_str(name).expect("valid package name"),
        tau_domain::PackageSource::from_str(source_url).expect("valid PackageSource"),
        semver::VersionReq::parse(version).expect("valid VersionReq"),
    )
}

// ---- hand-authored lockfile + project fixtures (run / chat) -----------------

/// Hand-author a lockfile + on-disk package tree under `<root>/.tau/`.
///
/// Uses raw TOML I/O because `LockedPackage` / `LockedVersion` are
/// `#[non_exhaustive]` (E0639). Schema is stable per Task 6.
///
/// Each call appends one `[[package]]` entry to `<root>/tau-lock.toml`,
/// upserting if the lockfile already exists, so a single project can
/// stack a tool package + an LLM-backend package side-by-side.
pub fn install_fixture(root: &Path, name: &str, version: &str, kind: &str, source_url: &str) {
    let dot_tau = root.join(".tau");
    std::fs::create_dir_all(dot_tau.join("packages").join(name).join(version)).unwrap();

    // Manifest (package's tau.toml).
    let manifest = format!(
        r#"name = "{name}"
version = "{version}"
description = "fixture"
authors = ["tester <test@example.com>"]
source = "{source_url}"
kind = "{kind}"
dependencies = []
capabilities = []
"#
    );
    std::fs::write(
        dot_tau
            .join("packages")
            .join(name)
            .join(version)
            .join("tau.toml"),
        manifest,
    )
    .unwrap();

    // Append/upsert lockfile entry. `tau-pkg` reads `<root>/tau-lock.toml`
    // (project-scope lockfile lives at the scope root, not inside .tau/).
    let lockfile_path = root.join("tau-lock.toml");
    let existing = if lockfile_path.exists() {
        std::fs::read_to_string(&lockfile_path).unwrap()
    } else {
        String::new()
    };

    let now_rfc3339 = "2026-04-28T00:00:00Z";
    let resolved_commit = "0".repeat(40);
    let new_entry = format!(
        r#"
[[package]]
name = "{name}"
active_version = "{version}"
source = "{source_url}"

[[package.versions]]
version = "{version}"
resolved_commit = "{resolved_commit}"
sha256 = ""
installed_at = "{now_rfc3339}"
"#
    );

    let new_lockfile = if existing.is_empty() {
        format!(
            r#"schema_version = 4
generated_by_tau_version = "0.0.0"
generated_at = "{now_rfc3339}"
{new_entry}"#
        )
    } else {
        format!("{existing}\n{new_entry}")
    };
    std::fs::write(&lockfile_path, new_lockfile).unwrap();
}

/// Stand up a project-scope tempdir with a `tau.toml` declaring a
/// single agent and the matching package + LLM backend pre-installed
/// in the project's `.tau/` lockfile.
///
/// NOTE: the lockfile entries written here carry no `[plugin]` table,
/// so any test that drives `tau run` / `tau chat` past plugin loading
/// will surface an error from `cmd::plugin_loader`. For real-spawn
/// fixtures, use [`setup_echo_project`] instead.
pub fn setup_project_with_installed_agent(
    agent_id: &str,
    pkg_name: &str,
    pkg_version: &str,
    llm_backend: &str,
) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    install_fixture(
        root,
        pkg_name,
        pkg_version,
        "tool",
        "https://example.com/pkg.git",
    );
    install_fixture(
        root,
        llm_backend,
        "0.1.0",
        "llm-backend",
        "https://example.com/llm.git",
    );

    let project_toml = format!(
        r#"[project]
name = "demo"

[agents.{agent_id}]
display_name = "Test Agent"
package      = "{pkg_name}@^0.1"
llm_backend  = "{llm_backend}"
"#
    );
    std::fs::write(root.join("tau.toml"), project_toml).unwrap();

    dir
}

/// Convenience wrapper: `setup_project_with_installed_agent("reviewer",
/// "code-reviewer", "0.1.0", "mock-llm")`.
///
/// Used by `cmd_chat.rs` and the cross-cutting suites for the canonical
/// "happy-path agent" fixture.
pub fn setup_project() -> TempDir {
    setup_project_with_installed_agent("reviewer", "code-reviewer", "0.1.0", "mock-llm")
}

// ---- bare-repo `file://` git fixtures (install / list) ----------------------

/// Run `git` with `args` in `cwd`, panicking with stderr/stdout on failure.
///
/// Strips inherited `GIT_*` discovery variables. Git sets `GIT_DIR` (and
/// friends) for the processes it spawns to run hooks; when the test suite
/// runs under such a hook (e.g. `git commit` triggering a pre-commit
/// `cargo nextest run`), those vars override `current_dir` and make every
/// fixture `git` invocation operate on the HOST repo instead of `cwd` —
/// `symbolic-ref HEAD` and `commit` below then rewrite the developer's
/// real branch. Clearing the vars keeps the fixture hermetic regardless
/// of how the test binary was launched.
pub fn run_git(cwd: &Path, args: &[&str]) {
    let mut cmd = StdCommand::new("git");
    cmd.args(args).current_dir(cwd);
    for (key, _) in std::env::vars() {
        if key.starts_with("GIT_") {
            cmd.env_remove(key);
        }
    }
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("git {args:?} spawn failure: {e}"));
    assert!(
        output.status.success(),
        "git {args:?} in {cwd:?} failed:\nstderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );
}

/// Build a `file://` URL from a path, with forward slashes for portability.
pub fn file_url(path: &Path) -> String {
    let forward = path
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    if forward.starts_with('/') {
        format!("file://{forward}")
    } else {
        format!("file:///{forward}")
    }
}

/// Set up a bare git repository containing a minimal package `tau.toml`.
///
/// Returns `(tempdir, file_url, bare_path)`. The tempdir owns both the
/// bare repo and the working repo; both go away when it drops.
///
/// The manifest's declared `source` matches the bare repo's `file://`
/// URL so tau-pkg's source/manifest match check passes. The bare HEAD
/// is forced to `refs/heads/main` to defeat host `init.defaultBranch`
/// drift across CI runners.
pub fn setup_local_package_fixture(
    name: &str,
    version: &str,
) -> (tempfile::TempDir, String, PathBuf) {
    setup_local_package_fixture_with_kind(name, version, "tool")
}

/// Same as [`setup_local_package_fixture`] but with an explicit `kind`.
pub fn setup_local_package_fixture_with_kind(
    name: &str,
    version: &str,
    kind: &str,
) -> (tempfile::TempDir, String, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");

    // Bare repo (clone target).
    let bare = dir.path().join(format!("{name}.git"));
    std::fs::create_dir_all(&bare).unwrap();
    run_git(&bare, &["init", "--bare", "-q"]);
    run_git(&bare, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    let url = file_url(&bare);

    // Working repo where we author the initial commit.
    let work = dir.path().join(format!("{name}-work"));
    std::fs::create_dir_all(&work).unwrap();
    run_git(&work, &["init", "-q", "-b", "main"]);
    run_git(&work, &["config", "user.email", "test@example.com"]);
    run_git(&work, &["config", "user.name", "Test User"]);

    let manifest = format!(
        r#"name = "{name}"
version = "{version}"
description = "test fixture"
authors = ["Test <test@example.com>"]
source = "{url}"
kind = "{kind}"
dependencies = []
capabilities = []
"#
    );
    std::fs::write(work.join("tau.toml"), manifest).unwrap();

    run_git(&work, &["add", "tau.toml"]);
    run_git(&work, &["commit", "-q", "-m", "initial"]);
    run_git(&work, &["remote", "add", "origin", &bare.to_string_lossy()]);
    run_git(&work, &["push", "-q", "origin", "main"]);

    (dir, url, bare)
}

// ---- echo-plugin project fixtures (Task 21 real-spawn integration tests) ----

/// Project the path to a forward-slashed string suitable for embedding
/// in a TOML field (`binary_path = "..."`). Backslashes inside Windows
/// paths would otherwise get interpreted as TOML escape sequences.
fn toml_path_string(p: &Path) -> String {
    p.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/")
}

/// Synthesize a project tau.toml + lockfile + on-disk package tree
/// keyed against pre-built echo-llm / echo-tool binaries.
///
/// This is the Task-21 counterpart to [`install_fixture`] /
/// [`setup_project_with_installed_agent`]: the lockfile entries it
/// writes carry a `[package.plugin]` table that matches the schema
/// `tau-pkg::install` writes during a real install, so
/// `tau_runtime::plugin_host::load_*` can spawn the binaries
/// straight from the recorded paths.
///
/// The project tau.toml is written to `<root>/tau.toml` with the
/// supplied `agent_id`, `agent_config_toml`, and `tools_in_requires`
/// stitched into `[agents.<id>]`. The first install — `echo-llm` — is
/// always recorded as the LLM backend; `echo-tool` is recorded
/// alongside if `tools_in_requires` is non-empty.
///
/// Returns the [`TempDir`] owning the entire layout. Drop it to clean
/// up.
pub fn setup_echo_project(
    agent_id: &str,
    agent_config_toml: &str,
    tools_in_requires: &[(&str, &str, Option<&str>)],
) -> TempDir {
    let (echo_llm, echo_tool) = echo_plugins::ensure_echo_plugins_built();
    let dir = tempfile::tempdir().expect("tempdir for echo project");
    let root = dir.path();

    // Materialize the .tau/ state dir so `Scope::resolve` walks up to
    // a Project scope, and a packages/ tree so `build_agent_definition`
    // can read each manifest.
    std::fs::create_dir_all(root.join(".tau")).unwrap();

    // Write a scope config.toml with required_tier = "none" so the
    // sandbox resolver does not require strict or light isolation.
    std::fs::write(
        root.join(".tau").join("config.toml"),
        r#"schema_version = 2
kind = "project"
created_at = "2026-05-01T00:00:00Z"
created_by_tau_version = "0.0.0"

[sandbox]
required_tier = "none"
"#,
    )
    .unwrap();

    // Ensure TAU_TESTING_ALLOW_MOCK_SANDBOX=1 is set in the process
    // environment so spawned tau subprocesses bypass the adapter registry
    // and use the mock sandbox. This is needed because on machines where
    // `docker --version` succeeds (CLI installed but daemon not running),
    // the probe returns Available, the Container adapter wins on priority,
    // and docker run fails — crashing the plugin before the handshake.
    //
    // Set via OnceLock to avoid races between parallel tests calling
    // std::env::set_var. The env var is inherited by all tau subprocesses
    // spawned from this test binary.
    set_mock_sandbox_env_once();

    // ---- Per-package tau.toml manifests ----
    write_package_manifest(
        root,
        "echo-llm",
        "0.1.0",
        "llm-backend",
        "https://example.com/echo-llm.git",
    );
    let include_tool = !tools_in_requires.is_empty();
    if include_tool {
        write_package_manifest(
            root,
            "echo-tool",
            "0.1.0",
            "tool",
            "https://example.com/echo-tool.git",
        );
    }

    // ---- Lockfile (with [package.plugin] entries) ----
    let now = "2026-04-28T00:00:00Z";
    let zero_sha = "0".repeat(40);

    let llm_path = toml_path_string(echo_llm);
    let mut lockfile = format!(
        r#"schema_version = 4
generated_by_tau_version = "0.0.0"
generated_at = "{now}"

[[package]]
name = "echo-llm"
active_version = "0.1.0"
source = "https://example.com/echo-llm.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "{zero_sha}"
sha256 = ""
installed_at = "{now}"

[package.plugin]
binary_path = "{llm_path}"
built_at = "{now}"

[package.plugin.manifest]
provides = "llm_backend"
kind = "rust-cargo"
bin = "echo-llm"
"#
    );

    if include_tool {
        let tool_path = toml_path_string(echo_tool);
        lockfile.push_str(&format!(
            r#"
[[package]]
name = "echo-tool"
active_version = "0.1.0"
source = "https://example.com/echo-tool.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "{zero_sha}"
sha256 = ""
installed_at = "{now}"

[package.plugin]
binary_path = "{tool_path}"
built_at = "{now}"

[package.plugin.manifest]
provides = "tool"
kind = "rust-cargo"
bin = "echo-tool"
"#
        ));
    }

    std::fs::write(root.join("tau-lock.toml"), lockfile).unwrap();

    // ---- Project tau.toml ----
    let tools_field = if tools_in_requires.is_empty() {
        String::new()
    } else {
        let mut buf = String::new();
        for (name, source, version) in tools_in_requires {
            buf.push_str(&format!(
                "\n[[agents.{agent_id}.requires.tools]]\nname = \"{name}\"\nsource = \"{source}\"\n"
            ));
            if let Some(v) = version {
                buf.push_str(&format!("version = \"{v}\"\n"));
            }
        }
        buf
    };
    let config_field = if agent_config_toml.is_empty() {
        String::new()
    } else {
        format!("\n[agents.{agent_id}.config]\n{agent_config_toml}\n")
    };

    // Pin `package = "echo-llm@^0.1"` — `build_agent_definition` looks
    // up the `package` field as the agent's primary tau-pkg package
    // (the manifest read of which seeds capabilities). Echo-llm is
    // always installed, so it doubles as the agent's backing package.
    let project_toml = format!(
        r#"[project]
name = "demo"

[agents.{agent_id}]
display_name = "Echo Agent"
package      = "echo-llm@^0.1"
llm_backend  = "echo-llm"
{tools_field}{config_field}"#
    );
    std::fs::write(root.join("tau.toml"), project_toml).unwrap();

    dir
}

/// Set `TAU_TESTING_ALLOW_MOCK_SANDBOX=1` exactly once per process, safely
/// even when called from parallel test threads. All tau subprocesses
/// spawned from this test binary will inherit the env var.
fn set_mock_sandbox_env_once() {
    use std::sync::OnceLock;
    static MOCK_SANDBOX_SET: OnceLock<()> = OnceLock::new();
    MOCK_SANDBOX_SET.get_or_init(|| {
        // Safety: called once per process; test processes are single-user
        // and the var is test-only (never set in production).
        unsafe { std::env::set_var("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1") };
    });
}

/// Author a package's `tau.toml` at `<root>/.tau/packages/<name>/<version>/tau.toml`.
/// Used by [`setup_echo_project`].
fn write_package_manifest(root: &Path, name: &str, version: &str, kind: &str, source_url: &str) {
    let pkg_dir = root.join(".tau").join("packages").join(name).join(version);
    std::fs::create_dir_all(&pkg_dir).unwrap();
    let manifest = format!(
        r#"name = "{name}"
version = "{version}"
description = "echo plugin fixture"
authors = ["tester <test@example.com>"]
source = "{source_url}"
kind = "{kind}"
dependencies = []
capabilities = []
"#
    );
    std::fs::write(pkg_dir.join("tau.toml"), manifest).unwrap();
}
