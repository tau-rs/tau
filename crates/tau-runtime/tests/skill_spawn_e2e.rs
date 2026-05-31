//! End-to-end tests for `skill.<name>.spawn` virtual tool dispatch.
//!
//! Skills-4 (ROADMAP §16), Task 8. Covers six scenarios:
//!
//! 1. Happy path: parent spawns `critic` skill and receives its response.
//! 2. `system_prompt` override replaces the skill's SKILL.md body.
//! 3. `scope_paths` narrows the child's fs grant (no error, grant verified
//!    at the unit-test level — e2e asserts success only).
//! 4. Spawn denied when parent lacks `Capability::Skill(Spawn)`.
//! 5. Skill not installed → `is_error` tool result.
//! 6. Install path missing → `is_error` tool result.
//!
//! # Cwd serialisation
//!
//! `stream.rs` resolves the scope by calling `Scope::resolve(current_dir())`.
//! `set_current_dir` is process-global: to avoid races between tests that
//! temporarily change cwd, all tests that need a scoped tempdir acquire
//! `SCOPE_MUTEX` first and restore cwd on exit.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tau_runtime::RuntimeShellExt;

use tau_pkg::lockfile::LockFile;
use tau_ports::RunBudget;
use tau_runtime::Runtime;

// ---------------------------------------------------------------------------
// Process-global mutex to serialise cwd changes across tests.
//
// All tests in this file run in the same process (single test binary).
// `set_current_dir` is not thread-safe when called concurrently — the mutex
// ensures at most one test owns the cwd at a time. Lock is taken inside
// `with_project_scope` and released on drop.
// ---------------------------------------------------------------------------

static SCOPE_MUTEX: Mutex<()> = Mutex::new(());

/// RAII guard: changes cwd on construction, restores on drop.
struct CwdGuard {
    original: PathBuf,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl CwdGuard {
    fn enter(new_cwd: &Path) -> Self {
        let lock = SCOPE_MUTEX
            .lock()
            .expect("SCOPE_MUTEX poisoned — a prior test panicked while holding it");
        let original = std::env::current_dir().expect("current_dir must be readable");
        std::env::set_current_dir(new_cwd).expect("set_current_dir must succeed");
        Self {
            original,
            _lock: lock,
        }
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        // Best-effort: if restore fails (e.g. directory deleted), log it.
        if let Err(e) = std::env::set_current_dir(&self.original) {
            eprintln!("CwdGuard::drop: failed to restore cwd: {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// Build a minimal SKILL.md body with the given text (already parsed body, no frontmatter).
/// The `resolve_skill_for_spawn` path reads the SKILL.md body after `parse_skill_md`.
fn write_skill_md(install_dir: &Path, body: &str) {
    // parse_skill_md expects YAML frontmatter + body separated by `---`.
    let content = format!("---\nname: critic\ndescription: Reviews drafts.\n---\n{body}\n");
    fs::write(install_dir.join("SKILL.md"), content).expect("write SKILL.md");
}

/// Write a minimal `kind = "skill"` tau.toml to `install_dir`.
/// `capabilities_toml` is a TOML fragment appended after the header fields.
fn write_skill_tau_toml(install_dir: &Path, capabilities_toml: &str) {
    let content = format!(
        r#"name = "critic"
version = "0.1.0"
description = "Reviews drafts."
authors = []
source = "https://example.com/critic.git"
kind = "skill"
dependencies = []
{capabilities_toml}

[skill]
"#
    );
    fs::write(install_dir.join("tau.toml"), content).expect("write tau.toml");
}

/// Write a minimal `.tau/config.toml` so that `Scope::resolve` finds a project scope.
fn write_scope_config(tau_dir: &Path) {
    fs::create_dir_all(tau_dir).expect("create .tau dir");
    fs::write(
        tau_dir.join("config.toml"),
        "schema_version = 3\nkind = \"project\"\ncreated_at = \"2026-05-14T00:00:00Z\"\n\
         created_by_tau_version = \"0.0.0\"\n\n[sandbox]\nrequired_tier = \"none\"\n",
    )
    .expect("write .tau/config.toml");
}

/// Write a `LockedPackage` for the `critic` skill into `tau-lock.toml` at `project_root`.
///
/// Uses TOML deserialization (not struct literals) because `LockedPackage`,
/// `LockedVersion`, and `SkillFrontmatterSnapshot` are all `#[non_exhaustive]`.
fn write_lockfile_with_critic(project_root: &Path) {
    // Build the lockfile TOML directly. The `[[package]]` key is the serde
    // rename for `LockFile::packages` (see lockfile.rs `#[serde(rename = "package")]`).
    let toml_str = r#"schema_version = 6
generated_by_tau_version = "0.0.0"
generated_at = "2026-05-14T00:00:00Z"

[[package]]
name = "critic"
active_version = "0.1.0"
source = "https://example.com/critic.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0000000000000000000000000000000000000000"
sha256 = ""
installed_at = "2026-05-13T00:00:00Z"

[package.skill]
content_sha256 = "deadbeef"

[package.skill.frontmatter]
name = "critic"
description = "Reviews drafts."
"#;
    let lf: LockFile = toml::from_str(toml_str).expect("lockfile TOML must parse");
    lf.save(&project_root.join("tau-lock.toml"))
        .expect("save lockfile");
}

/// Full project-scope setup in `tmp`:
/// - `tmp/.tau/config.toml` (scope marker)
/// - `tmp/tau-lock.toml` (lockfile with critic v0.1.0)
/// - `tmp/.tau/packages/critic/0.1.0/tau.toml` (skill manifest)
/// - `tmp/.tau/packages/critic/0.1.0/SKILL.md`
///
/// Returns the `install_dir` path for callers that need to mutate files.
///
/// `capabilities_toml` is appended to the tau.toml body (e.g.
/// `"capabilities = []\n"` or `"[[capabilities]]\nkind = \"fs.read\"\npaths = [\"/tmp/**\"]\n"`).
///
/// `skill_md_body` is the SKILL.md body text (below the frontmatter `---`).
fn setup_critic_project(tmp: &Path, capabilities_toml: &str, skill_md_body: &str) -> PathBuf {
    let tau_dir = tmp.join(".tau");
    let install_dir = tau_dir.join("packages").join("critic").join("0.1.0");
    fs::create_dir_all(&install_dir).expect("create install_dir");
    write_scope_config(&tau_dir);
    write_lockfile_with_critic(tmp);
    write_skill_tau_toml(&install_dir, capabilities_toml);
    write_skill_md(&install_dir, skill_md_body);
    install_dir
}

/// Build a manifest TOML with a single `Capability::Skill(Spawn)` granting `critic`.
fn manifest_with_skill_spawn_cap(extra_caps: &str) -> tau_domain::PackageManifest {
    let toml_body = format!(
        r#"
name = "test-pkg"
version = "0.1.0"
description = "test package"
authors = []
source = "https://example.com/test.git"
kind = "tool"
dependencies = []

[[capabilities]]
kind = "skill.spawn"
allowed_skills = ["critic"]
{extra_caps}
"#
    );
    let unchecked: tau_domain::UncheckedManifest =
        toml::from_str(&toml_body).expect("manifest TOML must parse");
    unchecked.validate().expect("manifest must be valid")
}

/// Build a manifest with NO `Capability::Skill(Spawn)`.
fn manifest_with_no_skill_cap() -> tau_domain::PackageManifest {
    common::manifest_with_no_capabilities()
}

// ---------------------------------------------------------------------------
// Test 1: happy path — parent spawns critic, receives its text response.
// ---------------------------------------------------------------------------

/// Turn order (shared MockLlmBackend, FIFO queue):
///   Pop 1 → parent turn 1: tool_call("skill.critic.spawn")
///   Pop 2 → child turn 1:  text("critic feedback: looks great")
///   Pop 3 → parent turn 2: text("done: critic feedback: looks great")
#[tokio::test]
async fn happy_path_parent_spawns_critic_and_receives_response() {
    let tmp = tempfile::tempdir().expect("tempdir");
    setup_critic_project(
        tmp.path(),
        "capabilities = []\n",
        "You are a helpful critic.",
    );

    let _guard = CwdGuard::enter(tmp.path());

    let backend = common::MockLlmBackend::new("test-llm")
        .add_tool_call(
            "skill.critic.spawn",
            serde_json::from_value(serde_json::json!({
                "message": "review my draft"
            }))
            .expect("args round-trip"),
        )
        // Child turn 1: plain text response.
        .add_text("critic feedback: looks great")
        // Parent turn 2: acknowledge child's result.
        .add_text("done: critic feedback: looks great");

    let runtime = Arc::new(
        Runtime::builder()
            .with_llm_backend(backend)
            .build()
            .expect("build runtime"),
    );

    let agent_def = common::agent_def("parent", "Parent Agent", "test-pkg@0.1.0", "test-llm");
    let manifest = manifest_with_skill_spawn_cap("");
    let initial = common::user_message("please review my draft");

    let snapshot = runtime
        .spawn_root_agent(
            agent_def,
            manifest,
            initial,
            RunBudget::default(),
            tmp.path().to_path_buf(),
        )
        .await
        .expect("spawn_root_agent must succeed");

    assert_eq!(
        snapshot.status,
        tau_ports::RunStatus::Completed,
        "run must complete; got {:?}",
        snapshot.status
    );
    // At least one child agent was spawned.
    assert!(
        snapshot.agents_spawned >= 1,
        "agents_spawned must be >= 1; got {}",
        snapshot.agents_spawned
    );
}

// ---------------------------------------------------------------------------
// Test 2: system_prompt override replaces the skill's SKILL.md body.
// ---------------------------------------------------------------------------

/// When caller passes `system_prompt: "overridden"`, the child agent's
/// first LLM call must use that prompt — NOT the SKILL.md body.
///
/// We verify by inspecting `MockLlmBackend::received_requests()`: the
/// second recorded request (index 1) corresponds to the child's first turn.
///
/// Turn order:
///   Pop 1 → parent turn 1: tool_call("skill.critic.spawn", {system_prompt: "overridden"})
///   Pop 2 → child turn 1:  text("child ran")
///   Pop 3 → parent turn 2: text("parent done")
#[tokio::test]
async fn system_prompt_override_replaces_skill_default() {
    let tmp = tempfile::tempdir().expect("tempdir");
    setup_critic_project(
        tmp.path(),
        "capabilities = []\n",
        "Default SKILL.md body — must NOT appear in child's system prompt.",
    );

    let _guard = CwdGuard::enter(tmp.path());

    let backend = Arc::new(
        common::MockLlmBackend::new("test-llm")
            .add_tool_call(
                "skill.critic.spawn",
                serde_json::from_value(serde_json::json!({
                    "message": "review draft",
                    "system_prompt": "overridden system prompt"
                }))
                .expect("args round-trip"),
            )
            .add_text("child ran")
            .add_text("parent done"),
    );
    let backend_arc = backend.clone();

    // Build the Runtime with the shared backend arc.
    let runtime = Arc::new(
        Runtime::builder()
            .with_dyn_llm_backend(backend_arc)
            .build()
            .expect("build runtime"),
    );

    let agent_def = common::agent_def("parent", "Parent Agent", "test-pkg@0.1.0", "test-llm");
    let manifest = manifest_with_skill_spawn_cap("");
    let initial = common::user_message("go");

    runtime
        .spawn_root_agent(
            agent_def,
            manifest,
            initial,
            RunBudget::default(),
            tmp.path().to_path_buf(),
        )
        .await
        .expect("spawn_root_agent must succeed");

    // The second request (index 1) is the child's first LLM call.
    let requests = backend.received_requests();
    assert!(
        requests.len() >= 2,
        "expected at least 2 LLM calls (parent turn1 + child turn1); got {}",
        requests.len()
    );
    let child_request = &requests[1];
    let system = child_request.system.as_deref().unwrap_or("");
    assert!(
        system.contains("overridden system prompt"),
        "child's system prompt must contain the override; got: {system:?}"
    );
    assert!(
        !system.contains("Default SKILL.md body"),
        "child's system prompt must NOT contain the default SKILL.md body; got: {system:?}"
    );

    // Sanity-check that the mock actually drove the test end-to-end.
    // The script has 3 turns (parent tool-call → child text → parent
    // text); the run must consume all of them, otherwise either the
    // script and the code-under-test have drifted apart or the run
    // exited early.
    backend.verify_fully_consumed();
    backend.verify_invocation_count(3);
}

// ---------------------------------------------------------------------------
// Test 3: scope_paths narrows child grant (success path only).
// ---------------------------------------------------------------------------

/// Critic declares `fs.read` on `/tmp/**`; parent grants `fs.read /tmp/**`
/// AND `skill.spawn` for critic; spawn args include `scope_paths: ["/tmp/proj/**"]`.
/// The narrowed grant is validated by `apply_scope_paths` (unit-tested in T3).
/// This e2e test asserts the call SUCCEEDS (no `SkillScopePathNotCovered` error).
///
/// Turn order:
///   Pop 1 → parent turn 1: tool_call with scope_paths
///   Pop 2 → child turn 1:  text response
///   Pop 3 → parent turn 2: text response
#[tokio::test]
async fn scope_paths_narrows_child_grant() {
    let tmp = tempfile::tempdir().expect("tempdir");

    // Critic declares fs.read on ${SKILL_DIR}/** (will be substituted).
    // We also declare /tmp/** so scope_path "/tmp/proj/**" is covered.
    let caps_toml = r#"
[[capabilities]]
kind = "fs.read"
paths = ["/tmp/**"]
"#;
    setup_critic_project(tmp.path(), caps_toml, "Scope-aware critic.");

    let _guard = CwdGuard::enter(tmp.path());

    let backend = common::MockLlmBackend::new("test-llm")
        .add_tool_call(
            "skill.critic.spawn",
            serde_json::from_value(serde_json::json!({
                "message": "review narrowed",
                "scope_paths": ["/tmp/proj"]
            }))
            .expect("args round-trip"),
        )
        .add_text("child responded")
        .add_text("parent done");

    // Parent manifest also needs fs.read /tmp/** for the subset law.
    let manifest_toml = r#"
name = "test-pkg"
version = "0.1.0"
description = "test package"
authors = []
source = "https://example.com/test.git"
kind = "tool"
dependencies = []

[[capabilities]]
kind = "skill.spawn"
allowed_skills = ["critic"]

[[capabilities]]
kind = "fs.read"
paths = ["/tmp/**"]
"#;
    let unchecked: tau_domain::UncheckedManifest =
        toml::from_str(manifest_toml).expect("manifest TOML must parse");
    let manifest = unchecked.validate().expect("manifest must be valid");

    let runtime = Arc::new(
        Runtime::builder()
            .with_llm_backend(backend)
            .build()
            .expect("build runtime"),
    );

    let agent_def = common::agent_def("parent", "Parent Agent", "test-pkg@0.1.0", "test-llm");
    let initial = common::user_message("go");

    let snapshot = runtime
        .spawn_root_agent(
            agent_def,
            manifest,
            initial,
            RunBudget::default(),
            tmp.path().to_path_buf(),
        )
        .await
        .expect("spawn_root_agent must succeed");

    // Narrowing must not cause an error — only success is required here.
    assert_eq!(
        snapshot.status,
        tau_ports::RunStatus::Completed,
        "run must complete (scope_paths narrowing must succeed); got {:?}",
        snapshot.status
    );
}

// ---------------------------------------------------------------------------
// Test 4: spawn denied when parent lacks Capability::Skill(Spawn).
// ---------------------------------------------------------------------------

/// Parent manifest has NO `Capability::Skill(Spawn)`.
///
/// When the LLM emits `skill.critic.spawn`, the kernel's virtual-tool
/// capability check fires BEFORE the skill-spawn branch: the capability
/// check at stream.rs line 331 finds `Capability::Skill(Spawn)` missing
/// from the parent's grant and terminates the run with `PolicyDenied`
/// (`RunOutcome::Failed`). The agent does NOT get a second turn —
/// `PolicyDenied` is a hard stop per ADR-0006.
///
/// We assert:
/// - `spawn_root_agent` does NOT return `Err(RuntimeError)` — the
///   denial is surfaced via `RunOutcome::Failed` (a normal run termination).
/// - `RunSnapshot.status` is `Failed` (not `Completed`).
/// - `agents_spawned == 0` — no child run was started.
///
/// NOTE: the "recoverable tool-result error" path (is_error=true tool
/// result returned to parent) only fires when the auth check inside
/// `validate_skill_spawn` fires (which is reached via the `is_skill_spawn`
/// branch AFTER the virtual-tool capability check). With no capability at
/// all, the outer capability check terminates the run first.
///
/// Turn order:
///   Pop 1 → parent turn 1: tool_call skill.critic.spawn
///           → kernel: capability check fails → PolicyDenied → run ends
///           (Pop 2 is never used)
#[tokio::test]
async fn spawn_denied_when_parent_lacks_skill_capability() {
    // Scope with no skill installed is fine; the auth check comes first.
    let tmp = tempfile::tempdir().expect("tempdir");
    setup_critic_project(tmp.path(), "capabilities = []\n", "critic body");

    let _guard = CwdGuard::enter(tmp.path());

    let backend = common::MockLlmBackend::new("test-llm")
        .add_tool_call(
            "skill.critic.spawn",
            serde_json::from_value(serde_json::json!({
                "message": "review"
            }))
            .expect("args"),
        )
        // Turn 2 is NOT consumed because the run terminates at capability denial.
        .add_text("this turn is never reached");

    let runtime = Arc::new(
        Runtime::builder()
            .with_llm_backend(backend)
            .build()
            .expect("build runtime"),
    );

    let agent_def = common::agent_def("parent", "Parent Agent", "test-pkg@0.1.0", "test-llm");
    // Manifest has NO skill.spawn capability.
    let manifest = manifest_with_no_skill_cap();
    let initial = common::user_message("go");

    // spawn_root_agent must NOT error — PolicyDenied is RunOutcome::Failed,
    // not RuntimeError. The snapshot is returned normally.
    let snapshot = runtime
        .spawn_root_agent(
            agent_def,
            manifest,
            initial,
            RunBudget::default(),
            tmp.path().to_path_buf(),
        )
        .await
        .expect("spawn_root_agent must not return Err on capability denial");

    // The run is DENIED (not completed) — PolicyDenied terminates the run.
    assert_eq!(
        snapshot.status,
        tau_ports::RunStatus::Failed,
        "run must be denied (Failed) when parent lacks skill.spawn capability; got {:?}",
        snapshot.status
    );
    // No child was spawned.
    assert_eq!(
        snapshot.agents_spawned, 0,
        "no child should be spawned on capability denial"
    );
}

// ---------------------------------------------------------------------------
// Test 5: skill not installed → is_error tool result.
// ---------------------------------------------------------------------------

/// Parent has `skill.spawn` capability for `missing`, but no such skill
/// is installed in the scope. `validate_skill_spawn` returns
/// `SkillNotInstalled`. Parent turn 2 acknowledges.
#[tokio::test]
async fn skill_not_installed_returns_is_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Set up a scope but do NOT install the "missing" skill.
    let tau_dir = tmp.path().join(".tau");
    write_scope_config(&tau_dir);
    // Write an empty lockfile (no packages).
    LockFile::default()
        .save(&tmp.path().join("tau-lock.toml"))
        .expect("save empty lockfile");

    let _guard = CwdGuard::enter(tmp.path());

    let backend = common::MockLlmBackend::new("test-llm")
        .add_tool_call(
            "skill.missing.spawn",
            serde_json::from_value(serde_json::json!({
                "message": "do something"
            }))
            .expect("args"),
        )
        .add_text("understood, skill not available");

    let runtime = Arc::new(
        Runtime::builder()
            .with_llm_backend(backend)
            .build()
            .expect("build runtime"),
    );

    let agent_def = common::agent_def("parent", "Parent Agent", "test-pkg@0.1.0", "test-llm");
    // Manifest grants skill.spawn for "missing".
    let manifest_toml = r#"
name = "test-pkg"
version = "0.1.0"
description = "test package"
authors = []
source = "https://example.com/test.git"
kind = "tool"
dependencies = []

[[capabilities]]
kind = "skill.spawn"
allowed_skills = ["missing"]
"#;
    let unchecked: tau_domain::UncheckedManifest =
        toml::from_str(manifest_toml).expect("manifest must parse");
    let manifest = unchecked.validate().expect("manifest must be valid");
    let initial = common::user_message("go");

    let snapshot = runtime
        .spawn_root_agent(
            agent_def,
            manifest,
            initial,
            RunBudget::default(),
            tmp.path().to_path_buf(),
        )
        .await
        .expect("spawn_root_agent must not error (tool result error, not RuntimeError)");

    assert_eq!(
        snapshot.status,
        tau_ports::RunStatus::Completed,
        "run must complete after skill-not-installed tool error; got {:?}",
        snapshot.status
    );
    assert_eq!(
        snapshot.agents_spawned, 0,
        "no child should be spawned when skill is not installed"
    );
}

// ---------------------------------------------------------------------------
// Test 6: install path missing → is_error tool result.
// ---------------------------------------------------------------------------

/// Lockfile has a `critic` entry but `tau.toml` was deleted from disk.
/// `find_installed_skill` returns `FindSkillError::InstallPathMissing`.
/// `validate_skill_spawn` maps it to `SkillInstallPathMissing`.
/// Parent receives an `is_error` tool result.
#[tokio::test]
async fn install_path_missing_returns_is_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let tau_dir = tmp.path().join(".tau");
    let install_dir = tau_dir.join("packages").join("critic").join("0.1.0");

    // Set up a full project scope including the install files.
    setup_critic_project(tmp.path(), "capabilities = []\n", "critic body");

    // Remove the tau.toml so the install path appears missing.
    fs::remove_file(install_dir.join("tau.toml")).expect("remove tau.toml");

    let _guard = CwdGuard::enter(tmp.path());

    let backend = common::MockLlmBackend::new("test-llm")
        .add_tool_call(
            "skill.critic.spawn",
            serde_json::from_value(serde_json::json!({
                "message": "review"
            }))
            .expect("args"),
        )
        .add_text("understood, install path missing");

    let runtime = Arc::new(
        Runtime::builder()
            .with_llm_backend(backend)
            .build()
            .expect("build runtime"),
    );

    let agent_def = common::agent_def("parent", "Parent Agent", "test-pkg@0.1.0", "test-llm");
    let manifest = manifest_with_skill_spawn_cap("");
    let initial = common::user_message("go");

    let snapshot = runtime
        .spawn_root_agent(
            agent_def,
            manifest,
            initial,
            RunBudget::default(),
            tmp.path().to_path_buf(),
        )
        .await
        .expect("spawn_root_agent must not error (tool result error, not RuntimeError)");

    assert_eq!(
        snapshot.status,
        tau_ports::RunStatus::Completed,
        "run must complete after install-path-missing tool error; got {:?}",
        snapshot.status
    );
    assert_eq!(
        snapshot.agents_spawned, 0,
        "no child should be spawned when install path is missing"
    );
}
