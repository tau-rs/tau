//! JSONL output for `tau check`. One JSON object per line.

use crate::cmd::check::result::{CheckCategory, CheckResult, CheckStatus, Severity};
use serde_json::{json, Value};
use std::path::Path;

/// Render `Vec<CheckResult>` as a JSONL stream.
pub fn render(
    project_root: &Path,
    categories: &[CheckCategory],
    fast: bool,
    results: &[CheckResult],
    exit_code: i32,
) -> String {
    let mut out = String::new();
    push_line(
        &mut out,
        &json!({
            "type": "run_started",
            "project_root": project_root.display().to_string(),
            "categories": categories.iter().map(|c| c.name()).collect::<Vec<_>>(),
            "fast": fast,
        }),
    );
    let total_duration: u128 = results.iter().map(|r| r.duration.as_millis()).sum();
    let mut ok = 0;
    let mut failed = 0;
    let mut by_error = 0;
    let mut by_setup = 0;
    for r in results {
        match r.status {
            CheckStatus::Ok => ok += 1,
            CheckStatus::Failed => failed += 1,
            CheckStatus::Skipped { .. } => {}
        }
        for f in &r.findings {
            match f.severity {
                Severity::Error => by_error += 1,
                Severity::NeedsSetup => by_setup += 1,
                Severity::Warning => {}
                Severity::Note => {}
            }
        }
        push_line(&mut out, &check_finished_event(r));
    }
    push_line(
        &mut out,
        &json!({
            "type": "run_finished",
            "duration_ms": total_duration,
            "summary": {
                "ok": ok,
                "failed": failed,
                "by_severity": { "error": by_error, "needs-setup": by_setup },
            },
            "exit_code": exit_code,
        }),
    );
    out
}

fn check_finished_event(r: &CheckResult) -> Value {
    let status = match &r.status {
        CheckStatus::Ok => json!("ok"),
        CheckStatus::Failed => json!("failed"),
        CheckStatus::Skipped { reason } => json!({"skipped": reason}),
    };
    json!({
        "type": "check_finished",
        "category": r.category.name(),
        "status": status,
        "duration_ms": r.duration.as_millis(),
        "findings": r.findings.iter().map(finding_to_json).collect::<Vec<_>>(),
    })
}

fn finding_to_json(f: &crate::cmd::check::result::CheckFinding) -> Value {
    json!({
        "category": f.category.name(),
        "severity": match f.severity {
            Severity::Error => "error",
            Severity::NeedsSetup => "needs-setup",
            Severity::Warning => "warning",
            Severity::Note => "note",
        },
        "rule_id": f.rule_id,
        "summary": f.summary,
        "detail": f.detail,
        "location": f.location.as_ref().map(|l| json!({
            "path": l.path.display().to_string(),
            "line": l.line,
            "column": l.column,
        })),
        "remediation": f.remediation,
        "structured": f.structured,
    })
}

fn push_line(out: &mut String, v: &Value) {
    out.push_str(&v.to_string());
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::check::result::{CheckCategory, CheckFinding, FindingLocation};
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn renders_run_started_and_run_finished() {
        let out = render(
            Path::new("/proj"),
            &[CheckCategory::Config],
            false,
            &[CheckResult {
                category: CheckCategory::Config,
                status: CheckStatus::Ok,
                findings: vec![],
                duration: Duration::from_millis(10),
            }],
            0,
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("\"type\":\"run_started\""));
        assert!(lines[1].contains("\"type\":\"check_finished\""));
        assert!(lines[2].contains("\"type\":\"run_finished\""));
        assert!(lines[2].contains("\"exit_code\":0"));
    }

    #[test]
    fn includes_finding_fields() {
        let r = CheckResult {
            category: CheckCategory::Packages,
            status: CheckStatus::Failed,
            findings: vec![CheckFinding {
                category: CheckCategory::Packages,
                severity: Severity::NeedsSetup,
                rule_id: "tau.packages.missing",
                summary: "missing".into(),
                detail: None,
                location: Some(FindingLocation {
                    path: PathBuf::from("tau.toml"),
                    line: Some(17),
                    column: None,
                }),
                remediation: Some("tau resolve".into()),
                structured: serde_json::json!({"package": "missing-tool"}),
            }],
            duration: Duration::from_millis(3),
        };
        let out = render(Path::new("/p"), &[CheckCategory::Packages], false, &[r], 3);
        assert!(out.contains("tau.packages.missing"));
        assert!(out.contains("\"line\":17"));
        assert!(out.contains("\"package\":\"missing-tool\""));
        assert!(out.contains("\"exit_code\":3"));
    }
}
