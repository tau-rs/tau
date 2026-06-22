//! ANSI-colored human output for `tau check`.

use crate::cmd::check::result::{CheckResult, CheckStatus, Severity};
use std::fmt::Write;

/// Render a Vec<CheckResult> to a string using ANSI-colored human format.
///
/// Layout: header → one line per category (✓/✗ symbol + name + status
/// summary + timing) → indented findings under failed categories →
/// footer with counts + exit code.
///
/// `use_color` controls ANSI escapes. When false, falls back to ASCII
/// markers and no color codes.
pub fn render(results: &[CheckResult], use_color: bool, exit_code: i32) -> String {
    let mut out = String::new();
    let (ok_sym, fail_sym, skip_sym) = if use_color {
        ("\x1b[32m✓\x1b[0m", "\x1b[31m✗\x1b[0m", "\x1b[33m—\x1b[0m")
    } else {
        ("OK  ", "FAIL", "SKIP")
    };

    out.push_str(&format!("running {} checks…\n", results.len()));

    let mut ok_count = 0;
    let mut fail_count = 0;
    let mut needs_setup = 0;
    let mut fixable = 0;

    for r in results {
        let sym = match &r.status {
            CheckStatus::Ok => {
                ok_count += 1;
                ok_sym
            }
            CheckStatus::Failed => {
                fail_count += 1;
                fail_sym
            }
            CheckStatus::Skipped { .. } => skip_sym,
        };
        let summary = match &r.status {
            CheckStatus::Ok => "ok".to_string(),
            CheckStatus::Failed => format!(
                "{} finding{}",
                r.findings.len(),
                if r.findings.len() == 1 { "" } else { "s" }
            ),
            CheckStatus::Skipped { reason } => format!("skipped — {reason}"),
        };
        let _ = writeln!(
            out,
            "  {sym} {:<10} {}  ({} ms)",
            r.category.name(),
            summary,
            r.duration.as_millis()
        );
        for f in &r.findings {
            match f.severity {
                Severity::Error => fixable += 1,
                Severity::NeedsSetup => needs_setup += 1,
                Severity::Warning => {}
                Severity::Note => {}
            }
            let _ = writeln!(out, "        {}", f.summary);
            if let Some(d) = &f.detail {
                for line in d.lines() {
                    let _ = writeln!(out, "          {line}");
                }
            }
        }
    }

    out.push('\n');
    if fail_count == 0 {
        let _ = writeln!(out, "all {} checks passed. exit 0", ok_count);
    } else {
        let _ = writeln!(
            out,
            "{fail_count} checks failed ({needs_setup} need setup, {fixable} fixable). exit {exit_code}"
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::check::result::{CheckCategory, CheckFinding, CheckResult};
    use serde_json::json;
    use std::time::Duration;

    fn ok_result(cat: CheckCategory, ms: u64) -> CheckResult {
        CheckResult {
            category: cat,
            status: CheckStatus::Ok,
            findings: Vec::new(),
            duration: Duration::from_millis(ms),
        }
    }

    fn failed_result(cat: CheckCategory, ms: u64, sev: Severity, summary: &str) -> CheckResult {
        CheckResult {
            category: cat,
            status: CheckStatus::Failed,
            findings: vec![CheckFinding {
                category: cat,
                severity: sev,
                rule_id: "tau.test",
                summary: summary.into(),
                detail: None,
                location: None,
                remediation: None,
                structured: json!({}),
            }],
            duration: Duration::from_millis(ms),
        }
    }

    #[test]
    fn renders_all_ok() {
        let results = vec![
            ok_result(CheckCategory::Config, 12),
            ok_result(CheckCategory::Lockfile, 43),
        ];
        let out = render(&results, false, 0);
        assert!(out.contains("OK   config"));
        assert!(out.contains("OK   lockfile"));
        assert!(out.contains("all 2 checks passed"));
    }

    #[test]
    fn renders_failed_with_finding() {
        let results = vec![failed_result(
            CheckCategory::Packages,
            3,
            Severity::NeedsSetup,
            "missing-tool",
        )];
        let out = render(&results, false, 3);
        assert!(out.contains("FAIL packages"));
        assert!(out.contains("missing-tool"));
        assert!(out.contains("1 checks failed (1 need setup, 0 fixable). exit 3"));
    }
}
