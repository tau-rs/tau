//! `tau check plugins` — cross-check each installed plugin's manifest
//! against its runtime-described capabilities.
//!
//! Default (full): spawn the plugin binary, send meta.handshake, then
//! tool.describe_capabilities per method, diff vs manifest.
//! `--fast`: just check the binary exists at the lockfile-recorded path
//! and is executable. No spawn.

use crate::cmd::check::result::{
    CheckCategory, CheckFinding, CheckResult, CheckStatus, FindingLocation, Severity,
};
use crate::cmd::check::runner::CheckCtx;
use serde_json::json;
use tau_pkg::{sandbox_check::cross_check_plugin_capabilities, LockFile};

pub async fn run_plugins(ctx: &CheckCtx) -> CheckResult {
    let lockfile_path = ctx.scope.lockfile_path();
    if !lockfile_path.exists() {
        return CheckResult {
            category: CheckCategory::Plugins,
            status: CheckStatus::Skipped {
                reason: "no lockfile (no plugins installed)".into(),
            },
            findings: Vec::new(),
            duration: std::time::Duration::ZERO,
        };
    }

    let lockfile = match LockFile::load(&lockfile_path) {
        Ok(l) => l,
        Err(e) => {
            return CheckResult {
                category: CheckCategory::Plugins,
                status: CheckStatus::Failed,
                findings: vec![CheckFinding {
                    category: CheckCategory::Plugins,
                    severity: Severity::Error,
                    rule_id: "tau.plugins.lockfile_unreadable",
                    summary: format!("lockfile read failed: {e}"),
                    detail: None,
                    location: Some(FindingLocation {
                        path: lockfile_path.to_path_buf(),
                        line: None,
                        column: None,
                    }),
                    remediation: Some("inspect the lockfile or re-run `tau install`".into()),
                    structured: json!({"kind": "LockFileLoad"}),
                }],
                duration: std::time::Duration::ZERO,
            };
        }
    };

    let mut findings = Vec::new();
    let mut plugins_seen = 0usize;

    // D7 stage 2: every `[models].backend` package must expose LLM completion
    // (its installed plugin must declare `provides = "llm_backend"`). This is
    // offline — it reads the lockfile-recorded plugin manifest, no spawn — so
    // it runs in both `--fast` and full modes.
    check_models_backends(ctx, &lockfile, &mut findings);

    // NOTE: `plugin: Option<LockedPlugin>` is on `LockedPackage` (not on
    // `LockedVersion`). Each package has at most one plugin binary; the
    // `active_version` field carries the canonical version string.
    for pkg in &lockfile.packages {
        let Some(plugin) = &pkg.plugin else { continue };
        plugins_seen += 1;
        let binary_path = &plugin.binary_path;
        let pkg_name = pkg.name.to_string();
        let pkg_version = pkg.active_version.to_string();

        // --fast: existence + executable check only.
        if ctx.fast {
            if !binary_path.exists() {
                findings.push(missing_binary_finding(&pkg_name, &pkg_version, binary_path));
            }
            continue;
        }

        // Full: spawn + handshake + describe_capabilities + diff.
        // Read manifest from the plugin's install dir.
        let manifest_path = binary_path
            .parent()
            .map(|p| p.join("tau.toml"))
            .unwrap_or_else(|| std::path::PathBuf::from("tau.toml"));
        let manifest = match tau_pkg::read_manifest(&manifest_path) {
            Ok(m) => m,
            Err(e) => {
                findings.push(CheckFinding {
                    category: CheckCategory::Plugins,
                    severity: Severity::Error,
                    rule_id: "tau.plugins.manifest_unreadable",
                    summary: format!(
                        "plugin {}@{} manifest not readable at {}: {}",
                        pkg_name,
                        pkg_version,
                        manifest_path.display(),
                        e
                    ),
                    detail: None,
                    location: Some(FindingLocation {
                        path: manifest_path,
                        line: None,
                        column: None,
                    }),
                    remediation: Some("tau install".into()),
                    structured: json!({
                        "package": pkg_name,
                        "version": pkg_version,
                    }),
                });
                continue;
            }
        };

        match cross_check_plugin_capabilities(binary_path, &manifest).await {
            Ok(_) => {}
            Err(e) => {
                findings.push(CheckFinding {
                    category: CheckCategory::Plugins,
                    severity: Severity::Error,
                    rule_id: "tau.plugins.mismatch",
                    summary: format!(
                        "plugin {}@{} cross-check failed: {}",
                        pkg_name, pkg_version, e
                    ),
                    detail: None,
                    location: Some(FindingLocation {
                        path: binary_path.to_path_buf(),
                        line: None,
                        column: None,
                    }),
                    remediation: Some("tau install --force or report the plugin author".into()),
                    structured: json!({
                        "package": pkg_name,
                        "version": pkg_version,
                        "kind": "CrossCheckError",
                    }),
                });
            }
        }
    }

    let status = if findings.is_empty() {
        if plugins_seen == 0 {
            CheckStatus::Skipped {
                reason: "no plugins installed".into(),
            }
        } else {
            CheckStatus::Ok
        }
    } else {
        CheckStatus::Failed
    };
    CheckResult {
        category: CheckCategory::Plugins,
        status,
        findings,
        duration: std::time::Duration::ZERO,
    }
}

/// D7 stage 2: probe each distinct `[models].backend` package and emit a
/// `BackendNotLlmCapable` finding when an *installed* backend package does not
/// expose LLM completion (its plugin does not declare `provides = "llm_backend"`,
/// or it is a data-only package with no plugin at all).
///
/// A backend that is declared but not installed is left to the `packages`
/// category — this probe only judges capability of what IS installed.
fn check_models_backends(ctx: &CheckCtx, lockfile: &LockFile, findings: &mut Vec<CheckFinding>) {
    let Some(project) = &ctx.project else { return };
    let mut seen = std::collections::BTreeSet::new();
    for (alias, model) in project.effective_models() {
        if !seen.insert(model.backend.clone()) {
            continue;
        }
        let Some(pkg) = lockfile
            .packages
            .iter()
            .find(|p| p.name.to_string() == model.backend)
        else {
            // Not installed — `packages`/`config` categories own that diagnosis.
            continue;
        };
        let provides = pkg.plugin.as_ref().map(|p| p.manifest.provides);
        if provides == Some(tau_domain::PortKind::LlmBackend) {
            continue;
        }
        let actual = match provides {
            Some(kind) => format!("provides `{kind}`"),
            None => "is a data-only package (no plugin)".to_string(),
        };
        findings.push(CheckFinding {
            category: CheckCategory::Plugins,
            severity: Severity::Error,
            rule_id: "tau.models.backend_not_llm_capable",
            summary: format!(
                "model backend `{}` (alias `{alias}`) does not expose LLM completion: it {actual}",
                model.backend
            ),
            detail: Some(
                "Every [models] backend must resolve to a plugin that declares \
                 `provides = \"llm_backend\"`."
                    .into(),
            ),
            location: None,
            remediation: Some(format!(
                "point alias `{alias}` at an llm_backend package, or install one for `{}`",
                model.backend
            )),
            structured: json!({
                "kind": "BackendNotLlmCapable",
                "alias": alias,
                "backend": model.backend,
                "provides": provides.map(|k| k.to_string()),
            }),
        });
    }
}

fn missing_binary_finding(
    package: &str,
    version: &str,
    binary_path: &std::path::Path,
) -> CheckFinding {
    CheckFinding {
        category: CheckCategory::Plugins,
        severity: Severity::NeedsSetup,
        rule_id: "tau.plugins.binary_missing",
        summary: format!(
            "plugin {}@{} binary missing at {}",
            package,
            version,
            binary_path.display()
        ),
        detail: None,
        location: Some(FindingLocation {
            path: binary_path.to_path_buf(),
            line: None,
            column: None,
        }),
        remediation: Some("tau install --force".into()),
        structured: json!({
            "package": package,
            "version": version,
            "binary_path": binary_path.display().to_string(),
        }),
    }
}
