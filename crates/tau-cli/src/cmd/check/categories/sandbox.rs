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

    if plugin_pkgs.is_empty() {
        return skipped("no plugin packages in lockfile");
    }

    let mut findings: Vec<CheckFinding> = Vec::new();

    // --target branch: validate against the target's documented profile
    // instead of the locally resolved adapter.
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
        if tau_runtime::sandbox::registration_for_triple(target).is_none() {
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

fn tier_le(a: tau_pkg::scope::SandboxRequiredTier, b: tau_ports::SandboxTier) -> bool {
    use tau_pkg::scope::SandboxRequiredTier as Req;
    use tau_ports::SandboxTier as Tier;
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
