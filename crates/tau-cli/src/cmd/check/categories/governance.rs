//! `tau check governance` — enforce the root `[allow]` constitution
//! (ADR-0057). Over-reach: tau.toml-declared caps ⊆ root ceiling. Closed-world:
//! every referenced/defined tool and every tool→mcp binding registered in
//! `[allow.*]`. Model references are validated upstream by `validate_models`.
//!
//! Manifest-free (Option A): package effective-caps and the relative
//! agent⊇spawn⊇tool links are out of scope (followup / Story 1.5).

use std::path::Path;
use std::time::Duration;

use serde_json::json;

use crate::cmd::check::result::{
    CheckCategory, CheckFinding, CheckResult, CheckStatus, FindingLocation, Severity,
};
use crate::cmd::check::runner::CheckCtx;
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
    let findings = governance_findings(project, &tau_toml);
    CheckResult {
        category: CheckCategory::Governance,
        status: CheckStatus::Ok,
        findings,
        duration: Duration::ZERO,
    }
}

pub(crate) fn governance_findings(project: &ProjectConfig, tau_toml: &Path) -> Vec<CheckFinding> {
    let Some(_allow) = &project.allow else {
        return vec![CheckFinding {
            category: CheckCategory::Governance,
            severity: Severity::Warning,
            rule_id: "tau.governance.no_constitution",
            summary: "no [allow] constitution declared; governance is not enforced".to_string(),
            detail: None,
            location: Some(loc(tau_toml)),
            remediation: Some(
                "add an [allow] section to tau.toml to enforce a capability ceiling".to_string(),
            ),
            structured: json!({ "check": "no_constitution" }),
        }];
    };
    // Over-reach + closed-world checks added in Tasks 3 and 4.
    Vec::new()
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

    /// Parse + validate a tau.toml string into a `ProjectConfig`. Fixtures MUST
    /// validate under 1.2 (every agent needs a model registered in
    /// [models]/[allow.models]; backends must be declared packages).
    fn proj(toml: &str) -> ProjectConfig {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("tau.toml");
        std::fs::write(&p, toml).unwrap();
        ProjectConfig::from_path(&p).expect("fixture must parse + validate")
    }

    fn summaries(f: &[CheckFinding]) -> String {
        f.iter()
            .map(|x| x.summary.clone())
            .collect::<Vec<_>>()
            .join(" | ")
    }

    #[test]
    fn absent_allow_emits_single_warning() {
        let cfg = proj("[project]\nname = \"demo\"\n");
        let f = governance_findings(&cfg, Path::new("tau.toml"));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Warning);
        assert_eq!(f[0].rule_id, "tau.governance.no_constitution");
    }

    #[test]
    fn empty_allow_no_violations() {
        let cfg = proj(
            r#"
[project]
name = "demo"

[allow]
"fs.read" = { paths = ["/proj/**"] }
"#,
        );
        let f = governance_findings(&cfg, Path::new("tau.toml"));
        assert!(f.is_empty(), "got {}", summaries(&f));
    }
}
