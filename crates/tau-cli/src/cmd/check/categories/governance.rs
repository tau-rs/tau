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
use tau_domain::Capability;
use tau_pkg::capability_override::{capability_set_subset, CeilingViolation};
use tau_pkg::project::allow::AllowConfig;
use tau_pkg::project::{ProjectConfig, ToolBinding};

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
    let Some(allow) = &project.allow else {
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
    let mut out = Vec::new();
    over_reach(project, allow, tau_toml, &mut out);
    closed_world(project, allow, tau_toml, &mut out);
    out
}

fn over_reach(
    project: &ProjectConfig,
    allow: &AllowConfig,
    tau_toml: &Path,
    out: &mut Vec<CheckFinding>,
) {
    for (name, tool) in &project.tools {
        if let Err(v) = capability_set_subset(&tool.capabilities, &allow.ceiling) {
            out.push(over_reach_finding(&format!("tool '{name}'"), &v, tau_toml));
        }
    }
    for (name, entry) in &allow.tools {
        if let Err(v) = capability_set_subset(&entry.ceiling, &allow.ceiling) {
            out.push(over_reach_finding(
                &format!("[allow.tools.{name}] ceiling"),
                &v,
                tau_toml,
            ));
        }
    }
    for agent in project.agents.values() {
        for ov in &agent.capability_overrides {
            let Some(list) = &ov.allow else { continue };
            let Some(synth) = synth_cap(&ov.kind, list) else {
                continue;
            };
            if let Err(v) = capability_set_subset(&[synth], &allow.ceiling) {
                out.push(over_reach_finding(
                    &format!("agent '{}': override", agent.id),
                    &v,
                    tau_toml,
                ));
            }
        }
    }
}

fn over_reach_finding(subject: &str, v: &CeilingViolation, tau_toml: &Path) -> CheckFinding {
    CheckFinding {
        category: CheckCategory::Governance,
        severity: Severity::Error,
        rule_id: "tau.governance.over_reach",
        summary: format!(
            "{subject}: capability {} \"{}\" exceeds [allow] ceiling ({})",
            v.kind, v.offender, v.reason
        ),
        detail: None,
        location: Some(loc(tau_toml)),
        remediation: Some("narrow the capability or widen the [allow] ceiling".to_string()),
        structured: json!({ "check": "over_reach", "subject": subject, "kind": v.kind, "offender": v.offender }),
    }
}

fn closed_world(
    project: &ProjectConfig,
    allow: &AllowConfig,
    tau_toml: &Path,
    out: &mut Vec<CheckFinding>,
) {
    // Agent tool refs registered. (Model refs are validated upstream by validate_models.)
    for agent in project.agents.values() {
        for t in &agent.tool_refs {
            if !allow.tools.contains_key(t) {
                out.push(unregistered(
                    "unregistered_tool",
                    "tau.governance.unregistered_tool",
                    &format!(
                        "agent '{}' references unregistered tool '{t}' — add [allow.tools.{t}]",
                        agent.id
                    ),
                    tau_toml,
                ));
            }
        }
    }
    // Defined tools registered.
    for name in project.tools.keys() {
        if !allow.tools.contains_key(name) {
            out.push(unregistered(
                "unregistered_tool_def",
                "tau.governance.unregistered_tool_def",
                &format!("tool '{name}' is defined but not registered in [allow.tools]"),
                tau_toml,
            ));
        }
    }
    // Tool→MCP bindings registered.
    for (name, entry) in &allow.tools {
        if let ToolBinding::Mcp(mcp_name) = &entry.binding {
            if !allow.mcp.contains_key(mcp_name) {
                out.push(unregistered(
                    "unregistered_mcp",
                    "tau.governance.unregistered_mcp",
                    &format!(
                        "[allow.tools.{name}] binds MCP '{mcp_name}' which is not registered in [allow.mcp]"
                    ),
                    tau_toml,
                ));
            }
        }
    }
}

fn unregistered(
    check: &str,
    rule_id: &'static str,
    summary: &str,
    tau_toml: &Path,
) -> CheckFinding {
    CheckFinding {
        category: CheckCategory::Governance,
        severity: Severity::Error,
        rule_id,
        summary: summary.to_string(),
        detail: None,
        location: Some(loc(tau_toml)),
        remediation: Some("register the resource in the corresponding [allow.*] table".to_string()),
        structured: json!({ "check": check }),
    }
}

fn synth_cap(kind: &str, allow: &[String]) -> Option<Capability> {
    let field = match kind {
        "fs.read" | "fs.write" | "fs.exec" => "paths",
        "net.http" => "hosts",
        "process.spawn" => "commands",
        _ => return None,
    };
    let v = json!({ "kind": kind, field: allow });
    serde_json::from_value::<Capability>(v).ok()
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

    #[test]
    fn tool_caps_exceeding_root_flagged() {
        let cfg = proj(
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
        let f = governance_findings(&cfg, Path::new("tau.toml"));
        assert!(
            f.iter().any(|x| x.rule_id == "tau.governance.over_reach"
                && x.summary.contains("/etc/**")
                && x.summary.contains("fetch")),
            "got: {}",
            summaries(&f)
        );
    }

    #[test]
    fn allow_tools_ceiling_exceeding_root_flagged() {
        let cfg = proj(
            r#"
[project]
name = "demo"

[allow]
"net.http" = { hosts = ["api.x.com"] }

[allow.tools.fetch]
native = "Fetch"
"net.http" = { hosts = ["evil.com"] }
"#,
        );
        let f = governance_findings(&cfg, Path::new("tau.toml"));
        assert!(
            f.iter().any(|x| x.rule_id == "tau.governance.over_reach"
                && x.summary.contains("evil.com")),
            "got: {}", summaries(&f)
        );
    }

    #[test]
    fn unregistered_tool_ref_and_defined_tool_flagged() {
        let cfg = proj(
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
        let f = governance_findings(&cfg, Path::new("tau.toml"));
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

    #[test]
    fn tool_binds_unregistered_mcp_flagged() {
        let cfg = proj(
            r#"
[project]
name = "demo"

[allow]
"net.http" = { hosts = ["api.x.com"] }

[allow.tools.weather]
mcp = "weather"
"#,
        );
        let f = governance_findings(&cfg, Path::new("tau.toml"));
        assert!(
            f.iter()
                .any(|x| x.rule_id == "tau.governance.unregistered_mcp"
                    && x.summary.contains("weather")),
            "got: {}",
            summaries(&f)
        );
    }

    #[test]
    fn fully_registered_within_ceiling_is_clean() {
        let cfg = proj(
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
        let f = governance_findings(&cfg, Path::new("tau.toml"));
        let errs: Vec<_> = f.iter().filter(|x| x.severity == Severity::Error).collect();
        assert!(
            errs.is_empty(),
            "expected no errors, got: {}",
            summaries(&f)
        );
    }

    #[test]
    fn agent_override_exceeding_root_flagged() {
        let cfg = proj(
            r#"
packages = ["demo"]

[project]
name = "demo"

[allow]
"fs.read" = { paths = ["/proj/**"] }

[allow.models.fast]
backend = "demo"
model = "m-1"

[agents.solo]
display_name = "Solo"
package = "demo@^0.1"
model = "fast"
capabilities = [{ kind = "fs.read", allow_paths = ["/etc/**"] }]
"#,
        );
        let f = governance_findings(&cfg, Path::new("tau.toml"));
        assert!(
            f.iter().any(|x| x.rule_id == "tau.governance.over_reach"
                && x.summary.contains("/etc/**")
                && x.summary.contains("solo")),
            "got: {}",
            summaries(&f)
        );
    }
}
