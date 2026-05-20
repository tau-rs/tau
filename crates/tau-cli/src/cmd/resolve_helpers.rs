//! Shared helpers for resolve-then-install flows used by
//! `tau run`, `tau chat`, and `tau resolve`.
//!
//! Centralizes the npm-style progress output (status + json events) +
//! the install loop. Each consumer is responsible for assembling the
//! `Vec<(AgentId, RequiredTool)>` input — single-agent (run/chat) or
//! all-agents (resolve).

use std::path::Path;
use std::str::FromStr;

use anyhow::Context as _;

use crate::config::AgentEntry;
use crate::output::Output;
use tau_pkg::scope::{SandboxRequirements, ScopeConfig};
use tau_runtime::sandbox::{
    build_plan, resolve_adapter, resolve_strict_for_validation, validate_plan_against_adapter,
    ResolutionError, SandboxAdapter, SandboxValidationError,
};

/// Resolve + (optionally) install requires.tools for one agent.
///
/// Builds the `(AgentId, RequiredTool)` list from the agent's entry,
/// resolves via `tau_pkg::resolve_requires_tools`, emits npm-style
/// progress lines, and either installs the planned packages or
/// short-circuits with copy-pasteable hints when `no_install` is set.
pub(crate) fn resolve_and_install_for_agent(
    entry: &AgentEntry,
    scope: &tau_pkg::Scope,
    no_install: bool,
    output: &mut Output,
) -> anyhow::Result<()> {
    let agent_id = tau_domain::AgentId::from_str(&entry.id).expect("AgentId from validated entry");
    let requires: Vec<(tau_domain::AgentId, tau_pkg::RequiredTool)> = entry
        .requires
        .tools
        .iter()
        .map(|t| (agent_id.clone(), t.clone()))
        .collect();
    let plan = tau_pkg::resolve_requires_tools(&requires, scope)
        .with_context(|| format!("resolving requires.tools for agent {:?}", entry.id))?;

    emit_resolve_start(&plan, output)?;

    if no_install && !plan.installs.is_empty() {
        emit_no_install_hints(&plan, output)?;
        anyhow::bail!("--no-install set; {} tool(s) missing", plan.installs.len());
    }

    install_planned(&plan, scope, output)?;
    Ok(())
}

/// Resolve + (optionally) install requires.tools for ALL agents in the
/// project tau.toml. Used by `tau resolve` (Task 8).
#[allow(dead_code)] // wired up by Task 8
pub(crate) fn resolve_and_install_for_project(
    agents: impl IntoIterator<Item = AgentEntry>,
    scope: &tau_pkg::Scope,
    no_install: bool,
    dry_run: bool,
    output: &mut Output,
) -> anyhow::Result<()> {
    let mut requires: Vec<(tau_domain::AgentId, tau_pkg::RequiredTool)> = Vec::new();
    for agent in agents {
        let agent_id =
            tau_domain::AgentId::from_str(&agent.id).expect("AgentId from validated entry");
        for tool in &agent.requires.tools {
            requires.push((agent_id.clone(), tool.clone()));
        }
    }
    let plan = tau_pkg::resolve_requires_tools(&requires, scope)
        .with_context(|| "resolving requires.tools for project".to_string())?;

    emit_resolve_start(&plan, output)?;

    if dry_run {
        for install in &plan.installs {
            output.status(format!(
                "[plan] {} {} from {} (would install)",
                install.name, install.version, install.source
            ))?;
        }
        return Ok(());
    }

    if no_install && !plan.installs.is_empty() {
        emit_no_install_hints(&plan, output)?;
        anyhow::bail!("--no-install set; {} tool(s) missing", plan.installs.len());
    }

    install_planned(&plan, scope, output)?;
    Ok(())
}

fn emit_resolve_start(plan: &tau_pkg::ResolutionPlan, output: &mut Output) -> anyhow::Result<()> {
    output.status(format!(
        "[resolve] {} required tools — {} already installed, {} to fetch",
        plan.installs.len() + plan.reuses.len(),
        plan.reuses.len(),
        plan.installs.len(),
    ))?;
    output.json(&serde_json::json!({
        "event": "resolve_start",
        "required": plan.installs.len() + plan.reuses.len(),
        "installed": plan.reuses.len(),
        "to_fetch": plan.installs.len(),
    }))?;
    Ok(())
}

fn install_planned(
    plan: &tau_pkg::ResolutionPlan,
    scope: &tau_pkg::Scope,
    output: &mut Output,
) -> anyhow::Result<()> {
    let resolve_start = std::time::Instant::now();
    for install in &plan.installs {
        output.status(format!(
            "[install] {} {} from {}",
            install.name, install.version, install.source
        ))?;
        output.json(&serde_json::json!({
            "event": "install_start",
            "name": install.name.as_str(),
            "version": install.version.to_string(),
            "source": install.source.to_string(),
        }))?;
        let started = std::time::Instant::now();
        let _installed = tau_pkg::install_with_options(
            &install.source,
            scope,
            tau_pkg::InstallOptions::default(),
        )
        .with_context(|| format!("installing {}", install.name))?;
        let elapsed = started.elapsed().as_millis();
        output.status(format!(
            "[install] {} {} ({}ms)",
            install.name, install.version, elapsed
        ))?;
        output.json(&serde_json::json!({
            "event": "install_complete",
            "name": install.name.as_str(),
            "version": install.version.to_string(),
            "duration_ms": elapsed as u64,
        }))?;
    }
    let total_ms = resolve_start.elapsed().as_millis();
    if !plan.installs.is_empty() {
        output.status(format!("[resolve] done in {total_ms}ms"))?;
        output.json(&serde_json::json!({
            "event": "resolve_complete",
            "duration_ms": total_ms as u64,
        }))?;
    }
    Ok(())
}

fn emit_no_install_hints(
    plan: &tau_pkg::ResolutionPlan,
    output: &mut Output,
) -> anyhow::Result<()> {
    output.warn(format!(
        "tau: {} tool(s) missing; --no-install set. To install:",
        plan.installs.len(),
    ))?;
    for install in &plan.installs {
        output.warn(format!("  tau install {}", install.source))?;
    }
    Ok(())
}

/// Read `[sandbox]` from the active scope's `config.toml`.
///
/// Returns `SandboxRequirements::default()` if the file is missing,
/// unreadable, or malformed. Errors are intentionally swallowed: the
/// `tau check config` and `tau check lockfile` categories are
/// responsible for reporting config-file issues; the sandbox check
/// should not double-report them.
pub(crate) fn read_sandbox_requirements_for_check(scope: &tau_pkg::Scope) -> SandboxRequirements {
    let path = scope.config_path();
    if !path.exists() {
        return SandboxRequirements::default();
    }
    let Ok(text) = std::fs::read_to_string(&path) else {
        return SandboxRequirements::default();
    };
    match ScopeConfig::read_from_str(&text) {
        Ok(cfg) => cfg.sandbox,
        Err(_) => SandboxRequirements::default(),
    }
}

/// Resolve the sandbox adapter for check flows.
///
/// When `required_tier == None` the runtime resolver picks Passthrough,
/// which trivially accepts every plan. Check flows skip passthrough
/// and pick the highest-priority non-passthrough adapter via
/// `resolve_strict_for_validation`, so the report shows what would
/// happen if the user strengthens the requirement. If no
/// non-passthrough adapter is available on this platform, the error
/// from `resolve_strict_for_validation` propagates.
pub(crate) async fn resolve_sandbox_check_adapter(
    requirements: &SandboxRequirements,
) -> Result<SandboxAdapter, ResolutionError> {
    use tau_pkg::scope::SandboxRequiredTier;
    if matches!(requirements.required_tier, SandboxRequiredTier::None) {
        resolve_strict_for_validation().await
    } else {
        resolve_adapter(requirements, &[]).await
    }
}

/// Outcome of validating one plugin's sandbox plan.
///
/// Captures all error messages as owned `String`s so callers don't need
/// to thread runtime error types through their own match arms.
#[derive(Debug)]
pub(crate) enum SandboxPluginOutcome {
    /// Plan built and validated cleanly against the adapter (or built
    /// cleanly in fast mode where no adapter is given).
    Ok,
    /// `build_plan` returned an error.
    BuildPlanFailed(String),
    /// `validate_plan_against_adapter` returned one or more errors.
    ValidateFailed(Vec<SandboxValidationError>),
    /// Manifest at `<pkg>/tau.toml` could not be read.
    ManifestUnreadable(String),
}

/// Build and (optionally) validate one plugin's sandbox plan.
///
/// Reads the plugin's manifest from `manifest_path`, calls `build_plan`
/// on its declared capabilities, and (when `adapter` is `Some`) calls
/// `validate_plan_against_adapter`. When `adapter` is `None`, runs in
/// "fast mode" — only `build_plan` is exercised; on success returns `Ok`.
///
/// Never panics; never logs. All outcomes (including manifest read
/// errors and validation failures) come back through
/// [`SandboxPluginOutcome`].
pub(crate) fn check_plugin_sandbox(
    plugin_id: &str,
    manifest_path: &Path,
    adapter: Option<&SandboxAdapter>,
) -> SandboxPluginOutcome {
    let package_caps = match tau_pkg::read_manifest(manifest_path) {
        Ok(manifest) => manifest.capabilities().to_vec(),
        Err(e) => return SandboxPluginOutcome::ManifestUnreadable(e.to_string()),
    };

    let plan = match build_plan(&package_caps, &[], None, None) {
        Ok(p) => p,
        Err(e) => return SandboxPluginOutcome::BuildPlanFailed(e.to_string()),
    };

    match adapter {
        Some(adapter) => match validate_plan_against_adapter(plugin_id, &plan, adapter) {
            Ok(()) => SandboxPluginOutcome::Ok,
            Err(errors) => SandboxPluginOutcome::ValidateFailed(errors),
        },
        None => SandboxPluginOutcome::Ok,
    }
}

/// Validate one plugin against a target capability profile.
///
/// Reads the plugin manifest, then checks that every declared capability
/// shape is contained in `profile.required_shapes`. No adapter is
/// instantiated — this is purely a static cross-check against the
/// target's documented matrix.
pub(crate) fn check_plugin_sandbox_against_profile(
    plugin_id: &str,
    manifest_path: &Path,
    profile: &tau_ports::target::TargetCapabilityProfile,
) -> SandboxPluginOutcome {
    let package_caps = match tau_pkg::read_manifest(manifest_path) {
        Ok(manifest) => manifest.capabilities().to_vec(),
        Err(e) => return SandboxPluginOutcome::ManifestUnreadable(e.to_string()),
    };

    let plan = match build_plan(&package_caps, &[], None, None) {
        Ok(p) => p,
        Err(e) => return SandboxPluginOutcome::BuildPlanFailed(e.to_string()),
    };

    let mut errors: Vec<SandboxValidationError> = Vec::new();
    for cap in &plan.capabilities {
        let required = cap.required_shape();
        if !profile.required_shapes.contains(&required) {
            errors.push(SandboxValidationError::new(
                plugin_id,
                cap.clone(),
                format!(
                    "target `{}` does not enforce shape {:?}",
                    profile.triple, required
                ),
            ));
        }
    }
    if errors.is_empty() {
        SandboxPluginOutcome::Ok
    } else {
        SandboxPluginOutcome::ValidateFailed(errors)
    }
}

#[cfg(test)]
mod check_sandbox_tests {
    use super::*;
    use std::path::PathBuf;

    /// `MockSandbox` supports the 5 standard CapabilityShapes (fs.read,
    /// fs.write, net.http, exec, env). It is reachable via
    /// `TAU_TESTING_ALLOW_MOCK_SANDBOX=1` through `resolve_adapter`. We
    /// use it here directly to keep tests platform-independent.
    fn mock_adapter() -> tau_runtime::sandbox::SandboxAdapter {
        // Force the Mock branch of resolve_adapter via the env var so the
        // returned SandboxAdapter::Mock variant exists for the test.
        std::env::set_var("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        let adapter = rt.block_on(async {
            tau_runtime::sandbox::resolve_adapter(
                &tau_pkg::scope::SandboxRequirements::default(),
                &[],
            )
            .await
            .expect("mock adapter")
        });
        std::env::remove_var("TAU_TESTING_ALLOW_MOCK_SANDBOX");
        adapter
    }

    fn write_manifest(dir: &std::path::Path, body: &str) -> PathBuf {
        let path = dir.join("tau.toml");
        std::fs::write(&path, body).expect("write manifest");
        path
    }

    #[test]
    fn check_plugin_sandbox_ok_for_benign_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let manifest_path = write_manifest(
            tmp.path(),
            r#"
name = "test-plugin"
version = "0.1.0"
description = "A test plugin"
authors = []
source = "https://example.com/test-plugin.git"
kind = "tool"
dependencies = []

[[capabilities]]
kind = "fs.read"
paths = ["/tmp/**"]
"#,
        );
        let adapter = mock_adapter();
        let outcome = check_plugin_sandbox("test-plugin", &manifest_path, Some(&adapter));
        assert!(
            matches!(outcome, SandboxPluginOutcome::Ok),
            "expected Ok, got {outcome:?}"
        );
    }

    #[test]
    fn check_plugin_sandbox_manifest_unreadable_for_missing_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let manifest_path = tmp.path().join("nonexistent.toml");
        let adapter = mock_adapter();
        let outcome = check_plugin_sandbox("ghost-plugin", &manifest_path, Some(&adapter));
        match outcome {
            SandboxPluginOutcome::ManifestUnreadable(msg) => {
                assert!(!msg.is_empty(), "expected non-empty error message");
            }
            other => panic!("expected ManifestUnreadable, got {other:?}"),
        }
    }

    #[test]
    fn check_plugin_sandbox_ok_in_fast_mode_without_adapter() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let manifest_path = write_manifest(
            tmp.path(),
            r#"
name = "fast-plugin"
version = "0.1.0"
description = "A fast plugin"
authors = []
source = "https://example.com/fast-plugin.git"
kind = "tool"
dependencies = []

[[capabilities]]
kind = "fs.read"
paths = ["/tmp/**"]
"#,
        );
        // adapter = None → fast mode: build_plan only.
        let outcome = check_plugin_sandbox("fast-plugin", &manifest_path, None);
        assert!(
            matches!(outcome, SandboxPluginOutcome::Ok),
            "expected Ok in fast mode, got {outcome:?}"
        );
    }

    #[test]
    fn check_against_profile_passes_when_shapes_are_subset() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let manifest_path = write_manifest(
            tmp.path(),
            r#"
name = "subset-plugin"
version = "0.1.0"
description = "A plugin needing fs.read only"
authors = []
source = "https://example.com/sub.git"
kind = "tool"
dependencies = []

[[capabilities]]
kind = "fs.read"
paths = ["/tmp/**"]
"#,
        );
        let entry = tau_ports::target::lookup(&"linux-native-strict".parse().unwrap()).unwrap();
        let profile = entry.profile();
        let outcome =
            check_plugin_sandbox_against_profile("subset-plugin", &manifest_path, &profile);
        assert!(
            matches!(outcome, SandboxPluginOutcome::Ok),
            "expected Ok, got {outcome:?}"
        );
    }

    #[test]
    fn check_against_profile_fails_when_shape_outside_target() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let manifest_path = write_manifest(
            tmp.path(),
            r#"
name = "agent-spawner"
version = "0.1.0"
description = "Plugin needs agent.spawn"
authors = []
source = "https://example.com/as.git"
kind = "tool"
dependencies = []

[[capabilities]]
kind = "agent.spawn"
"#,
        );
        let entry = tau_ports::target::lookup(&"linux-native-strict".parse().unwrap()).unwrap();
        let profile = entry.profile();
        let outcome =
            check_plugin_sandbox_against_profile("agent-spawner", &manifest_path, &profile);
        match outcome {
            SandboxPluginOutcome::ValidateFailed(errors) => {
                assert_eq!(errors.len(), 1);
                assert!(
                    errors[0].reason.contains("AgentSpawn")
                        || errors[0].reason.contains("agent.spawn"),
                    "unexpected reason: {}",
                    errors[0].reason
                );
            }
            other => panic!("expected ValidateFailed, got {other:?}"),
        }
    }
}
