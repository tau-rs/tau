//! SARIF 2.1.0 output for `tau check`.
//!
//! Hand-rolled — SARIF is plain JSON, no crate dependency.

use crate::cmd::check::result::{CheckResult, Severity};
use serde_json::{json, Value};

/// Build the SARIF document for a run.
pub fn render(results: &[CheckResult]) -> String {
    let mut sarif_results = Vec::new();
    for r in results {
        for f in &r.findings {
            sarif_results.push(json!({
                "ruleId": f.rule_id,
                "level": match f.severity {
                    Severity::Error => "error",
                    Severity::NeedsSetup => "warning",
                    Severity::Warning => "note",
                    Severity::Note => "note",
                },
                "message": {"text": f.summary.clone()},
                "locations": f.location.as_ref().map(|l| vec![json!({
                    "physicalLocation": {
                        "artifactLocation": {"uri": l.path.display().to_string()},
                        "region": {
                            "startLine": l.line,
                            "startColumn": l.column,
                        }
                    }
                })]).unwrap_or_default(),
                "properties": {
                    "severity": match f.severity {
                        Severity::Error => "error",
                        Severity::NeedsSetup => "needs-setup",
                        Severity::Warning => "warning",
                        Severity::Note => "note",
                    },
                    "category": f.category.name(),
                    "structured": f.structured.clone(),
                },
            }));
        }
    }

    let doc = json!({
        "version": "2.1.0",
        "$schema": "https://schemastore.azurewebsites.net/schemas/json/sarif-2.1.0.json",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "tau",
                    "version": env!("CARGO_PKG_VERSION"),
                    "informationUri": "https://github.com/LEBOCQTitouan/tau",
                    "rules": rules_descriptor(),
                }
            },
            "results": sarif_results,
        }]
    });

    serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_string())
}

fn rules_descriptor() -> Value {
    json!([
        {"id":"tau.config.invalid","shortDescription":{"text":"Invalid tau.toml"}},
        {"id":"tau.lockfile.drift","shortDescription":{"text":"Install tree drifted from lockfile"}},
        {"id":"tau.packages.missing","shortDescription":{"text":"Required package not installed"}},
        {"id":"tau.sandbox.plan_invalid","shortDescription":{"text":"Sandbox plan validation failed"}},
        {"id":"tau.plugins.mismatch","shortDescription":{"text":"Plugin contract mismatch"}},
        {"id":"tau.plugins.binary_missing","shortDescription":{"text":"Plugin binary missing"}},
        {"id":"tau.plugins.manifest_unreadable","shortDescription":{"text":"Plugin manifest not readable"}},
        {"id":"tau.skills.mismatch","shortDescription":{"text":"Skill manifest mismatch"}},
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::check::result::{CheckCategory, CheckFinding, CheckStatus, FindingLocation};
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn renders_empty_run() {
        let out = render(&[]);
        assert!(out.contains("\"version\": \"2.1.0\""));
        assert!(out.contains("\"results\": []"));
    }

    #[test]
    fn maps_finding_to_sarif_result() {
        let r = CheckResult {
            category: CheckCategory::Packages,
            status: CheckStatus::Failed,
            findings: vec![CheckFinding {
                category: CheckCategory::Packages,
                severity: Severity::NeedsSetup,
                rule_id: "tau.packages.missing",
                summary: "missing-tool".into(),
                detail: None,
                location: Some(FindingLocation {
                    path: PathBuf::from("tau.toml"),
                    line: Some(17),
                    column: Some(5),
                }),
                remediation: Some("tau resolve".into()),
                structured: serde_json::json!({"package": "missing-tool"}),
            }],
            duration: Duration::from_millis(3),
        };
        let out = render(&[r]);
        assert!(out.contains("\"level\": \"warning\""));
        assert!(out.contains("\"ruleId\": \"tau.packages.missing\""));
        assert!(out.contains("\"startLine\": 17"));
        assert!(out.contains("\"severity\": \"needs-setup\""));
    }
}
