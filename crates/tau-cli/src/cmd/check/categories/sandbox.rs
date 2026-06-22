//! `tau check sandbox` — validate sandbox plans for each installed plugin.
//!
//! Default (full): build plan AND validate against the resolved adapter.
//! `--fast`: build plan only; skip adapter probe + validation.
//!
//! The per-plugin build/validate loop lives in
//! `crate::cmd::resolve_helpers::check_plugin_sandbox`; this module
//! handles the check-aggregator-specific output mapping (severity policy,
//! `CheckFinding` synthesis, fast-mode adapter elision).

use crate::cmd::check::result::{
    CheckCategory, CheckFinding, CheckResult, CheckStatus, FindingLocation, Severity,
};
use crate::cmd::check::runner::CheckCtx;
use crate::cmd::resolve_helpers::{
    check_plugin_sandbox, check_plugin_sandbox_against_profile,
    read_sandbox_requirements_for_check, resolve_sandbox_check_adapter, SandboxPluginOutcome,
};
use serde_json::json;

pub async fn run_sandbox(ctx: &CheckCtx) -> CheckResult {
    // project.is_none() means tau.toml is malformed — the config check
    // reports this; we just skip to avoid duplicate noise.
    if ctx.project.is_none() {
        return skipped("tau.toml malformed (see config check)");
    }

    let tau_toml_path = ctx.project_root.join("tau.toml");
    let sandbox_requirements = read_sandbox_requirements_for_check(&ctx.scope);

    // Load the lockfile. If missing or unreadable, skip — the lockfile
    // check will already report this.
    let lockfile_path = ctx.scope.lockfile_path();
    if !lockfile_path.exists() {
        return skipped("lockfile missing or unreadable (see lockfile check)");
    }
    let lockfile = match tau_pkg::LockFile::load(&lockfile_path) {
        Ok(lf) => lf,
        Err(_) => return skipped("lockfile missing or unreadable (see lockfile check)"),
    };

    // Collect only packages that have a plugin entry — data-only packages
    // don't need sandbox plans.
    let plugin_pkgs: Vec<_> = lockfile
        .packages
        .iter()
        .filter(|p| p.plugin.is_some())
        .collect();

    let mut findings: Vec<CheckFinding> = Vec::new();

    // --target branch: validate against the target's documented profile
    // instead of the locally resolved adapter.
    //
    // NOTE: the `plugin_pkgs.is_empty()` skip is intentionally NOT placed
    // before this block.  Durability is an agent concern, independent of
    // plugins.  A project with durable agents and no plugin packages must
    // still receive durability findings (and the hard-fail path) when
    // `--target` is set.  The per-plugin loop below no-ops naturally when
    // `plugin_pkgs` is empty.
    if let Some(target) = &ctx.target {
        let Some(entry) = tau_ports::target::lookup(target) else {
            // Should not happen — dispatch already validated the triple.
            findings.push(CheckFinding {
                category: CheckCategory::Sandbox,
                severity: Severity::Error,
                rule_id: "tau.sandbox.target_unknown",
                summary: format!("target `{target}` is not registered"),
                detail: None,
                location: None,
                remediation: Some("tau target list".into()),
                structured: json!({ "kind": "TargetUnknown", "target": target.to_string() }),
            });
            return CheckResult {
                category: CheckCategory::Sandbox,
                status: CheckStatus::Failed,
                findings,
                duration: std::time::Duration::ZERO,
            };
        };
        let profile = entry.profile();

        // EPIC 6.1: print the host-resolved durability per durable agent.
        if let Some(project) = &ctx.project {
            findings.extend(durability_findings(&project.agents, target));
        }

        // Reserved → advisory Warning, but still validate against documented matrix.
        if let tau_ports::target::TripleStatus::Reserved { reason } = entry.status {
            findings.push(CheckFinding {
                category: CheckCategory::Sandbox,
                severity: Severity::Warning,
                rule_id: "tau.sandbox.target_reserved",
                summary: format!("target `{target}` is reserved: {reason}"),
                detail: Some(
                    "Reserved triples have a documented capability matrix but no shipping adapter; bundles compiled for them will not yet execute anywhere.".into(),
                ),
                location: None,
                remediation: None,
                structured: json!({ "kind": "TargetReserved", "target": target.to_string(), "reason": reason }),
            });
        }

        // Adapter-availability check (Warning if no local adapter satisfies the triple).
        if tau_runtime_tokio::process_gate::registration_for_triple(target).is_none() {
            findings.push(CheckFinding {
                category: CheckCategory::Sandbox,
                severity: Severity::Warning,
                rule_id: "tau.sandbox.target_no_local_adapter",
                summary: format!(
                    "no local adapter satisfies target `{target}`; cross-check is static only"
                ),
                detail: None,
                location: None,
                remediation: None,
                structured: json!({ "kind": "TargetNoLocalAdapter", "target": target.to_string() }),
            });
        }

        // Project required_tier must be ≤ target tier.
        let project_tier = sandbox_requirements.required_tier;
        let target_tier = target.tier;
        if !tier_le(project_tier, target_tier) {
            findings.push(CheckFinding {
                category: CheckCategory::Sandbox,
                severity: Severity::Error,
                rule_id: "tau.sandbox.target_tier_mismatch",
                summary: format!(
                    "project requires tier {project_tier:?} but target `{target}` provides tier {target_tier:?}"
                ),
                detail: None,
                location: Some(FindingLocation { path: tau_toml_path.clone(), line: None, column: None }),
                remediation: None,
                structured: json!({
                    "kind": "TargetTierMismatch",
                    "target": target.to_string(),
                    "project_tier": format!("{project_tier:?}"),
                    "target_tier": format!("{target_tier:?}"),
                }),
            });
        }

        // Per-plugin shape check.
        for pkg in &plugin_pkgs {
            let plugin_id = pkg.name.as_str().to_owned();
            let pkg_dir = ctx.scope.package_dir(&pkg.name, &pkg.active_version);
            let manifest_path = pkg_dir.join("tau.toml");

            match check_plugin_sandbox_against_profile(&plugin_id, &manifest_path, &profile) {
                SandboxPluginOutcome::Ok => {}
                SandboxPluginOutcome::BuildPlanFailed(msg) => {
                    findings.push(build_plan_finding(&plugin_id, msg, &tau_toml_path));
                }
                SandboxPluginOutcome::ValidateFailed(errors) => {
                    for err in errors {
                        findings.push(CheckFinding {
                            category: CheckCategory::Sandbox,
                            severity: Severity::Error,
                            rule_id: "tau.sandbox.target_shape_unsupported",
                            summary: format!("plugin `{plugin_id}`: {}", err.reason),
                            detail: None,
                            location: Some(FindingLocation {
                                path: tau_toml_path.clone(),
                                line: None,
                                column: None,
                            }),
                            remediation: None,
                            structured: json!({
                                "kind": "TargetShapeUnsupported",
                                "plugin_id": plugin_id,
                                "reason": err.reason,
                            }),
                        });
                    }
                }
                SandboxPluginOutcome::ManifestUnreadable(msg) => {
                    if !ctx.fast {
                        findings.push(CheckFinding {
                            category: CheckCategory::Sandbox,
                            severity: Severity::Warning,
                            rule_id: "tau.sandbox.manifest_unreadable",
                            summary: format!(
                                "could not read manifest for `{plugin_id}`: {msg} — skipping capability check"
                            ),
                            detail: None,
                            location: Some(FindingLocation {
                                path: manifest_path,
                                line: None,
                                column: None,
                            }),
                            remediation: Some("tau resolve".into()),
                            structured: json!({
                                "plugin_id": plugin_id,
                                "kind": "ManifestUnreadable",
                                "error": msg,
                            }),
                        });
                    }
                }
            }
        }

        let status = if findings.iter().any(|f| f.severity == Severity::Error) {
            CheckStatus::Failed
        } else {
            CheckStatus::Ok
        };
        return CheckResult {
            category: CheckCategory::Sandbox,
            status,
            findings,
            duration: std::time::Duration::ZERO,
        };
    }

    // No-target path: skip entirely when there are no plugin packages to
    // validate. (The --target path above already returned, so we only reach
    // here when ctx.target is None.)
    if plugin_pkgs.is_empty() {
        return skipped("no plugin packages in lockfile");
    }

    // Resolve adapter unless we're in --fast mode.
    //
    // When required_tier is None the runtime would pick Passthrough, which
    // trivially accepts every plan. We use resolve_strict_for_validation
    // via the helper, which picks the highest-priority non-passthrough
    // adapter instead to surface what would happen if the user strengthens
    // the requirement.
    let adapter_opt = if ctx.fast {
        None
    } else {
        match resolve_sandbox_check_adapter(&sandbox_requirements).await {
            Ok(a) => Some(a),
            Err(e) => {
                // No adapter available — emit an advisory warning and skip
                // validation rather than hard-failing.
                findings.push(CheckFinding {
                    category: CheckCategory::Sandbox,
                    severity: Severity::Warning,
                    rule_id: "tau.sandbox.no_adapter",
                    summary: format!("no sandbox adapter available for validation: {e}"),
                    detail: Some(
                        "Sandbox plan shapes could not be validated. \
                         Install a sandbox adapter (e.g. tau-sandbox-darwin) to enable full checks."
                            .into(),
                    ),
                    location: None,
                    remediation: None,
                    structured: json!({ "kind": "NoAdapterAvailable", "error": e.to_string() }),
                });
                return CheckResult {
                    category: CheckCategory::Sandbox,
                    status: CheckStatus::Ok, // advisory only, not a hard failure
                    findings,
                    duration: std::time::Duration::ZERO,
                };
            }
        }
    };

    for pkg in &plugin_pkgs {
        let plugin_id = pkg.name.as_str().to_owned();
        let pkg_dir = ctx.scope.package_dir(&pkg.name, &pkg.active_version);
        let manifest_path = pkg_dir.join("tau.toml");

        match check_plugin_sandbox(&plugin_id, &manifest_path, adapter_opt.as_ref()) {
            SandboxPluginOutcome::Ok => {}
            SandboxPluginOutcome::BuildPlanFailed(msg) => {
                findings.push(build_plan_finding(&plugin_id, msg, &tau_toml_path));
            }
            SandboxPluginOutcome::ValidateFailed(errors) => {
                for err in errors {
                    findings.push(CheckFinding {
                        category: CheckCategory::Sandbox,
                        severity: Severity::Error,
                        rule_id: "tau.sandbox.plan_invalid",
                        summary: format!("plugin `{plugin_id}`: {}", err.reason),
                        detail: None,
                        location: Some(FindingLocation {
                            path: tau_toml_path.clone(),
                            line: None,
                            column: None,
                        }),
                        remediation: None,
                        structured: json!({
                            "plugin_id": plugin_id,
                            "kind": "SandboxValidationFailed",
                            "reason": err.reason,
                        }),
                    });
                }
            }
            SandboxPluginOutcome::ManifestUnreadable(msg) => {
                // Fast mode preserves the prior silent-skip behavior; full
                // mode surfaces a Warning so users see why a plugin was
                // skipped without changing the result status.
                if !ctx.fast {
                    findings.push(CheckFinding {
                        category: CheckCategory::Sandbox,
                        severity: Severity::Warning,
                        rule_id: "tau.sandbox.manifest_unreadable",
                        summary: format!(
                            "could not read manifest for `{plugin_id}`: {msg} — skipping capability check"
                        ),
                        detail: None,
                        location: Some(FindingLocation {
                            path: manifest_path,
                            line: None,
                            column: None,
                        }),
                        remediation: Some("tau resolve".into()),
                        structured: json!({
                            "plugin_id": plugin_id,
                            "kind": "ManifestUnreadable",
                            "error": msg,
                        }),
                    });
                }
            }
        }
    }

    let status = if findings.iter().any(|f| f.severity == Severity::Error) {
        CheckStatus::Failed
    } else {
        CheckStatus::Ok
    };
    CheckResult {
        category: CheckCategory::Sandbox,
        status,
        findings,
        duration: std::time::Duration::ZERO,
    }
}

fn skipped(reason: &str) -> CheckResult {
    CheckResult {
        category: CheckCategory::Sandbox,
        status: CheckStatus::Skipped {
            reason: reason.into(),
        },
        findings: Vec::new(),
        duration: std::time::Duration::ZERO,
    }
}

fn tier_le(a: tau_pkg::scope::SandboxRequiredTier, b: tau_ports::CapabilityTier) -> bool {
    use tau_pkg::scope::SandboxRequiredTier as Req;
    use tau_ports::CapabilityTier as Tier;
    // SandboxTier is #[non_exhaustive]; catch-all is required for external match.
    #[allow(unreachable_patterns)]
    let to_rank = |t: Tier| match t {
        Tier::None => 0,
        Tier::Light => 1,
        Tier::Strict => 2,
        _ => 0,
    };
    // SandboxRequiredTier is also #[non_exhaustive]; catch-all required.
    #[allow(unreachable_patterns)]
    let req_rank = match a {
        Req::None => 0,
        Req::Light => 1,
        Req::Strict => 2,
        _ => 0,
    };
    req_rank <= to_rank(b)
}

/// Build the per-agent durability resolution findings for `tau check --target`.
/// Honored → an informational `Note`; Unsupported → an `Error`.
fn durability_findings(
    agents: &std::collections::BTreeMap<String, tau_pkg::project::AgentEntry>,
    target: &tau_ports::target::TargetTriple,
) -> Vec<CheckFinding> {
    use tau_runtime_core::Support;
    let mut out = Vec::new();
    for (id, agent) in agents {
        let Some(entry) = agent.durable.as_ref() else {
            continue;
        };
        let durability = tau_ir_lower::durable_entry_to_ir(entry);
        let resolved = tau_runtime_core::resolve_durability(&durability, target);
        let form = if resolved.from_intent.is_some() {
            "intent"
        } else {
            "explicit"
        };
        let ckpt = match resolved.checkpoint {
            tau_ir::durable::CheckpointGranularity::PerTurn => "per_turn",
            tau_ir::durable::CheckpointGranularity::PerToolCall => "per_tool_call",
            _ => "per_turn",
        };
        let store = match resolved.store {
            tau_ir::durable::DurableStore::File => "file",
            _ => "file",
        };
        let detail = match resolved.from_intent {
            Some(_) => format!("survive-restarts → {ckpt} checkpoints, {store} store"),
            None => format!("explicit {ckpt} + {store}"),
        };
        #[allow(unreachable_patterns)]
        match resolved.support {
            Support::Honored => out.push(CheckFinding {
                category: CheckCategory::Sandbox,
                severity: Severity::Note,
                rule_id: "tau.durability.resolved",
                summary: format!("{id}: {detail}  [resolved for {target}]"),
                detail: None,
                location: None,
                remediation: None,
                structured: json!({
                    "kind": "DurabilityResolved",
                    "agent": id,
                    "form": form,
                    "checkpoint": ckpt,
                    "store": store,
                    "support": "honored",
                    "target": target.to_string(),
                }),
            }),
            Support::Unsupported { reason } => out.push(CheckFinding {
                category: CheckCategory::Sandbox,
                severity: Severity::Error,
                rule_id: "tau.durability.unsupported",
                summary: format!("{id}: target `{target}` cannot honor durability: {reason}"),
                detail: None,
                location: None,
                remediation: None,
                structured: json!({
                    "kind": "DurabilityUnsupported",
                    "agent": id,
                    "support": "unsupported",
                    "reason": reason,
                    "target": target.to_string(),
                }),
            }),
            _ => {}
        }
    }
    out
}

fn build_plan_finding(
    plugin_id: &str,
    message: String,
    tau_toml_path: &std::path::Path,
) -> CheckFinding {
    CheckFinding {
        category: CheckCategory::Sandbox,
        severity: Severity::Error,
        rule_id: "tau.sandbox.plan_invalid",
        summary: format!("build_plan failed for `{plugin_id}`: {message}"),
        detail: None,
        location: Some(FindingLocation {
            path: tau_toml_path.to_path_buf(),
            line: None,
            column: None,
        }),
        remediation: None,
        structured: json!({
            "plugin_id": plugin_id,
            "kind": "BuildPlanFailed",
            "error": message,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tau_pkg::project::project::{AgentEntry, DurableEntry, PromptEntry, RequiresEntry};

    // ---------------------------------------------------------------------------
    // Regression test for the `plugin_pkgs.is_empty()` early-return gap.
    //
    // Before the fix, `run_sandbox` would return `Skipped("no plugin packages in
    // lockfile")` even when `--target` was supplied and the project had a durable
    // agent, so durability findings were never emitted and the hard-fail path
    // (`DurabilityUnsupported`) was silently bypassed.
    // ---------------------------------------------------------------------------

    /// Build a minimal CheckCtx pointing at a temp-dir project that has:
    ///   - a tau.toml with one durable agent (survive-restarts intent)
    ///   - a lockfile with NO plugin packages
    ///   - a target triple that honors durability
    fn make_ctx_durable_no_plugins(
        project_root: &std::path::Path,
        target: tau_ports::target::TargetTriple,
    ) -> CheckCtx {
        // Create .tau/ so Scope::resolve finds a project scope.
        std::fs::create_dir_all(project_root.join(".tau")).unwrap();

        let scope = tau_pkg::Scope::resolve(project_root).unwrap();

        // Write an empty lockfile (no plugin packages).
        let lf = tau_pkg::lockfile::LockFile::default();
        lf.save(&scope.lockfile_path()).unwrap();

        // Build a ProjectConfig with one durable agent inline.
        // NOTE: `packages = ["mock-llm"]` declares the backend so the
        // validator accepts `backend = "mock-llm"` in [models].
        let toml_src = r#"
packages = ["mock-llm"]

[project]
name = "test-durable"

[models]
default = { backend = "mock-llm", model = "mock" }

[agents.bot]
display_name = "Bot"
package      = "p@^0.1"
model        = "default"
tool_refs    = []
durable      = "survive-restarts"
"#;
        let project = tau_pkg::project::ProjectConfig::parse_str(toml_src).ok();

        CheckCtx {
            project_root: project_root.to_path_buf(),
            scope,
            project,
            fast: false,
            target: Some(target),
        }
    }

    #[tokio::test]
    async fn run_sandbox_with_target_emits_durability_finding_even_with_no_plugin_packages() {
        let tmp = tempfile::tempdir().unwrap();
        let target: tau_ports::target::TargetTriple = "any-wasi-strict".parse().unwrap();
        let ctx = make_ctx_durable_no_plugins(tmp.path(), target);

        let result = run_sandbox(&ctx).await;

        // Must NOT be Skipped — durability is an agent concern, not a plugin concern.
        assert!(
            !matches!(result.status, CheckStatus::Skipped { .. }),
            "expected non-Skipped result but got {:?}",
            result.status
        );

        // Must contain at least one durability finding for the durable agent.
        let durability_findings: Vec<_> = result
            .findings
            .iter()
            .filter(|f| f.rule_id == "tau.durability.resolved")
            .collect();
        assert!(
            !durability_findings.is_empty(),
            "expected tau.durability.resolved finding; got findings: {:#?}",
            result.findings
        );
        // The finding is a Note (Honored on a registered target).
        assert_eq!(durability_findings[0].severity, Severity::Note);
    }

    fn agent_with_durable(id: &str, durable: DurableEntry) -> AgentEntry {
        let mut entry = AgentEntry::new(
            id.to_string(),
            id.to_string(),
            "p@^0.1".to_string(),
            RequiresEntry::default(),
            BTreeMap::new(),
            PromptEntry::None,
            vec![],
        );
        entry.durable = Some(durable);
        entry.model = "default".to_string();
        entry
    }

    #[test]
    fn durability_findings_intent_yields_note_with_survive_restarts() {
        let mut agents: BTreeMap<String, AgentEntry> = BTreeMap::new();
        agents.insert(
            "bot".to_string(),
            agent_with_durable("bot", DurableEntry::Intent("survive-restarts".to_string())),
        );
        let target: tau_ports::target::TargetTriple = "any-wasi-strict".parse().unwrap();
        let findings = durability_findings(&agents, &target);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Note);
        assert_eq!(findings[0].rule_id, "tau.durability.resolved");
        assert!(
            findings[0].summary.contains("survive-restarts"),
            "summary: {}",
            findings[0].summary
        );
        assert!(
            findings[0].summary.contains("per_turn"),
            "summary: {}",
            findings[0].summary
        );
    }

    #[test]
    fn durability_findings_explicit_yields_note() {
        let mut agents: BTreeMap<String, AgentEntry> = BTreeMap::new();
        agents.insert(
            "bot2".to_string(),
            agent_with_durable(
                "bot2",
                DurableEntry::Explicit {
                    checkpoint: "per_tool_call".to_string(),
                    store: "file".to_string(),
                },
            ),
        );
        let target: tau_ports::target::TargetTriple = "passthrough".parse().unwrap();
        let findings = durability_findings(&agents, &target);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Note);
        assert_eq!(findings[0].rule_id, "tau.durability.resolved");
        assert!(
            findings[0].summary.contains("per_tool_call"),
            "summary: {}",
            findings[0].summary
        );
    }

    #[test]
    fn durability_findings_no_durable_yields_no_findings() {
        let mut agents: BTreeMap<String, AgentEntry> = BTreeMap::new();
        let entry = AgentEntry::new(
            "nodurable".to_string(),
            "no durable".to_string(),
            "p@^0.1".to_string(),
            RequiresEntry::default(),
            BTreeMap::new(),
            PromptEntry::None,
            vec![],
        );
        agents.insert("nodurable".to_string(), entry);
        let target: tau_ports::target::TargetTriple = "passthrough".parse().unwrap();
        let findings = durability_findings(&agents, &target);
        assert!(findings.is_empty());
    }
}
