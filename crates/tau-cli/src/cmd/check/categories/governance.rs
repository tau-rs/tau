//! `tau check governance` — enforce the root `[allow]` constitution
//! (ADR-0057). Over-reach: tau.toml-declared caps ⊆ root ceiling. Closed-world:
//! every referenced/defined tool and every tool→mcp binding registered in
//! `[allow.*]`. Model references are validated upstream by `validate_models`.
//! Lattice: L0 (tool⊆ceiling), L1 (package manifest⊆root), L2 (IR tool⊆agent
//! effective caps), L3 transparency Note (agent⊇spawn, runtime-enforced).
//!
//! This category is a thin presentation wrapper over the relocated core in
//! [`tau_pkg::governance`] (ADR-0059) — `tau check` and `tau build` share one
//! implementation of the lattice and cannot drift on what counts as a
//! violation. The wrapper only maps [`GovSeverity`] → [`Severity`] and adds
//! CLI presentation (source location + per-rule remediation).

use std::path::Path;
use std::time::Duration;

use crate::cmd::check::result::{
    CheckCategory, CheckFinding, CheckResult, CheckStatus, FindingLocation, Severity,
};
use crate::cmd::check::runner::CheckCtx;
use tau_pkg::governance::{enforce_governance, GovFinding, GovSeverity};
use tau_pkg::project::ProjectConfig;

pub fn run_governance(ctx: &CheckCtx) -> CheckResult {
    let project = match &ctx.project {
        Some(p) => p,
        None => {
            return CheckResult {
                category: CheckCategory::Governance,
                status: CheckStatus::Skipped {
                    reason: "tau.toml did not parse".to_string(),
                },
                findings: Vec::new(),
                duration: Duration::ZERO,
            };
        }
    };
    let tau_toml = ctx.project_root.join("tau.toml");
    let findings = governance_findings(project, &tau_toml, ctx);
    CheckResult {
        category: CheckCategory::Governance,
        status: CheckStatus::Ok,
        findings,
        duration: Duration::ZERO,
    }
}

pub(crate) fn governance_findings(
    project: &ProjectConfig,
    tau_toml: &Path,
    ctx: &CheckCtx,
) -> Vec<CheckFinding> {
    enforce_governance(project, &ctx.scope)
        .into_iter()
        .map(|g| to_check_finding(g, tau_toml))
        .collect()
}

/// Map a presentation-free [`GovFinding`] to a `CheckFinding`, restoring the
/// CLI-facing severity, source location, and remediation. The `summary` and
/// `structured` fields carry over verbatim so text/JSON/SARIF output is
/// byte-identical to the pre-relocation category.
fn to_check_finding(g: GovFinding, tau_toml: &Path) -> CheckFinding {
    let severity = match g.severity {
        GovSeverity::Error => Severity::Error,
        GovSeverity::NeedsSetup => Severity::NeedsSetup,
        GovSeverity::Warning => Severity::Warning,
        GovSeverity::Note => Severity::Note,
    };
    let (remediation, located) = presentation(g.rule_id);
    CheckFinding {
        category: CheckCategory::Governance,
        severity,
        rule_id: g.rule_id,
        summary: g.summary,
        detail: None,
        location: located.then(|| loc(tau_toml)),
        remediation: remediation.map(str::to_string),
        structured: g.structured,
    }
}

/// Per-rule CLI presentation: remediation advice + whether the finding anchors
/// to `tau.toml`. Keyed by `rule_id` so the mapping stays exhaustive as the
/// core adds rules.
fn presentation(rule_id: &str) -> (Option<&'static str>, bool) {
    match rule_id {
        "tau.governance.no_constitution" => (
            Some("add an [allow] section to tau.toml to enforce a capability ceiling"),
            true,
        ),
        "tau.governance.over_reach" => (
            Some("narrow the capability or widen the [allow] ceiling"),
            true,
        ),
        "tau.governance.unregistered_tool"
        | "tau.governance.unregistered_tool_def"
        | "tau.governance.unregistered_mcp" => (
            Some("register the resource in the corresponding [allow.*] table"),
            true,
        ),
        "tau.governance.tool_exceeds_ceiling" => (
            Some("narrow the tool or widen its [allow.tools] ceiling"),
            true,
        ),
        "tau.governance.agent_caps_unresolved" => (Some("tau resolve"), true),
        "tau.governance.override_expands_package" => {
            (Some("narrow the agent's [capabilities] override"), true)
        }
        "tau.governance.package_exceeds_allow" | "tau.governance.tool_exceeds_agent" => {
            (Some("narrow the capability or widen the ceiling"), true)
        }
        // Transparency note: no location, no remediation.
        "tau.governance.spawn_runtime_enforced" => (None, false),
        // Unknown rule: safe default (located, no advice).
        _ => (None, true),
    }
}

fn loc(tau_toml: &Path) -> FindingLocation {
    FindingLocation {
        path: tau_toml.to_path_buf(),
        line: None,
        column: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_pkg::Scope;

    /// Parse + validate a tau.toml string into a `ProjectConfig`. Fixtures MUST
    /// validate under 1.2 (every agent needs a model registered in
    /// [models]/[allow.models]; backends must be declared packages).
    fn proj(toml: &str) -> (ProjectConfig, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        // Mark as a project scope so Scope::resolve binds to this dir.
        std::fs::create_dir_all(dir.path().join(".tau")).unwrap();
        let p = dir.path().join("tau.toml");
        std::fs::write(&p, toml).unwrap();
        let cfg = ProjectConfig::from_path(&p).expect("fixture must parse + validate");
        (cfg, dir)
    }

    /// Build a minimal `CheckCtx` for unit-test use (no lockfile, so all agents
    /// resolve to `NotInstalled` — only Error-severity assertions hold).
    fn ctx_for(dir: &tempfile::TempDir) -> CheckCtx {
        let root = dir.path().to_path_buf();
        let scope = Scope::resolve(&root).expect("scope must resolve");
        CheckCtx {
            project_root: root,
            scope,
            project: None, // not used inside governance_findings directly
            fast: false,
            target: None,
        }
    }

    fn summaries(f: &[CheckFinding]) -> String {
        f.iter()
            .map(|x| x.summary.clone())
            .collect::<Vec<_>>()
            .join(" | ")
    }

    #[test]
    fn absent_allow_emits_single_warning() {
        let (cfg, dir) = proj("[project]\nname = \"demo\"\n");
        let ctx = ctx_for(&dir);
        let f = governance_findings(&cfg, Path::new("tau.toml"), &ctx);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Warning);
        assert_eq!(f[0].rule_id, "tau.governance.no_constitution");
        // The wrapper restores CLI presentation.
        assert!(f[0].location.is_some());
        assert!(f[0].remediation.is_some());
    }

    #[test]
    fn tool_caps_exceeding_root_flagged() {
        let (cfg, dir) = proj(
            r#"
[project]
name = "demo"

[allow]
"fs.read" = { paths = ["/proj/**"] }

[allow.tools.fetch]
native = "Fetch"

[tools.fetch]
native = "Fetch"
capabilities = [{ kind = "fs.read", paths = ["/etc/**"] }]
"#,
        );
        let ctx = ctx_for(&dir);
        let f = governance_findings(&cfg, Path::new("tau.toml"), &ctx);
        assert!(
            f.iter().any(|x| x.rule_id == "tau.governance.over_reach"
                && x.severity == Severity::Error
                && x.summary.contains("/etc/**")
                && x.summary.contains("fetch")),
            "got: {}",
            summaries(&f)
        );
    }

    #[test]
    fn unregistered_tool_ref_and_defined_tool_flagged() {
        let (cfg, dir) = proj(
            r#"
packages = ["demo"]

[project]
name = "demo"

[allow]
"fs.read" = { paths = ["/proj/**"] }

[allow.models.fast]
backend = "demo"
model = "m-1"

[tools.fetch]
native = "Fetch"
capabilities = []

[agents.solo]
display_name = "Solo"
package = "demo@^0.1"
model = "fast"
tool_refs = ["fetch"]
"#,
        );
        let ctx = ctx_for(&dir);
        let f = governance_findings(&cfg, Path::new("tau.toml"), &ctx);
        assert!(
            f.iter()
                .any(|x| x.rule_id == "tau.governance.unregistered_tool"
                    && x.summary.contains("fetch")),
            "tool ref: {}",
            summaries(&f)
        );
        assert!(
            f.iter()
                .any(|x| x.rule_id == "tau.governance.unregistered_tool_def"
                    && x.summary.contains("fetch")),
            "tool def: {}",
            summaries(&f)
        );
    }

    /// `tau check` and `tau build` MUST NOT drift: both consume the same
    /// `tau_pkg::enforce_governance`, so the Error findings surfaced by the
    /// check wrapper must equal — by `(rule_id, structured)` value, not by
    /// rendered string — the Error findings the build gate filters on.
    #[test]
    fn check_and_build_agree_on_violations() {
        use tau_pkg::governance::{enforce_governance, GovSeverity};
        let toml = r#"
[project]
name = "demo"

[allow]
"fs.read" = { paths = ["/proj/**"] }

[allow.tools.fetch]
native = "Fetch"

[tools.fetch]
native = "Fetch"
capabilities = [{ kind = "fs.read", paths = ["/etc/**"] }]
"#;
        let (cfg, dir) = proj(toml);
        let ctx = ctx_for(&dir);

        // Check path: relocated core → CheckFinding mapping.
        let check_errors: Vec<(&'static str, serde_json::Value)> =
            governance_findings(&cfg, Path::new("tau.toml"), &ctx)
                .into_iter()
                .filter(|f| f.severity == Severity::Error)
                .map(|f| (f.rule_id, f.structured))
                .collect();

        // Build path: the gate filters the same core's Error findings.
        let build_errors: Vec<(&'static str, serde_json::Value)> =
            enforce_governance(&cfg, &ctx.scope)
                .into_iter()
                .filter(|f| f.severity == GovSeverity::Error)
                .map(|f| (f.rule_id, f.structured))
                .collect();

        assert!(!check_errors.is_empty(), "fixture must produce violations");
        assert_eq!(
            check_errors, build_errors,
            "check and build disagree on the violation set"
        );
    }

    #[test]
    fn fully_registered_within_ceiling_is_clean() {
        let (cfg, dir) = proj(
            r#"
packages = ["demo"]

[project]
name = "demo"

[allow]
"fs.read" = { paths = ["/proj/**"] }

[allow.models.fast]
backend = "demo"
model = "m-1"

[allow.mcp.weather]
url = "https://api.weather.com/mcp"

[allow.tools.read_temp]
native = "ReadTemp"
"fs.read" = { paths = ["/proj/sensors/**"] }

[tools.read_temp]
native = "ReadTemp"
capabilities = [{ kind = "fs.read", paths = ["/proj/sensors/**"] }]

[agents.solo]
display_name = "Solo"
package = "demo@^0.1"
model = "fast"
tool_refs = ["read_temp"]
"#,
        );
        let ctx = ctx_for(&dir);
        let f = governance_findings(&cfg, Path::new("tau.toml"), &ctx);
        let errs: Vec<_> = f.iter().filter(|x| x.severity == Severity::Error).collect();
        assert!(
            errs.is_empty(),
            "expected no errors, got: {}",
            summaries(&f)
        );
    }
}
