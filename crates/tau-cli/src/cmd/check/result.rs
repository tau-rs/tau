//! Pure types for the check subsystem. No I/O.

use std::path::PathBuf;
use std::time::Duration;

/// Which check category produced this result/finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckCategory {
    /// Configuration file validation.
    Config,
    /// Lockfile integrity checks.
    Lockfile,
    /// Package/dependency checks.
    Packages,
    /// Sandbox runtime validation.
    Sandbox,
    /// Plugin metadata and loading.
    Plugins,
    /// Skill manifest and content checks.
    Skills,
    /// MCP pinned-contract drift checks.
    McpContracts,
    /// [allow] constitution enforcement (ADR-0057).
    Governance,
}

impl CheckCategory {
    /// All 8 categories in stable order.
    pub const ALL: [Self; 8] = [
        Self::Config,
        Self::Lockfile,
        Self::Packages,
        Self::Sandbox,
        Self::Plugins,
        Self::Skills,
        Self::McpContracts,
        Self::Governance,
    ];

    /// Display name used in CLI output and JSON `category` fields.
    pub fn name(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Lockfile => "lockfile",
            Self::Packages => "packages",
            Self::Sandbox => "sandbox",
            Self::Plugins => "plugins",
            Self::Skills => "skills",
            Self::McpContracts => "mcp_contracts",
            Self::Governance => "governance",
        }
    }

    /// Whether this category has a meaningful `--fast` variant.
    /// False categories accept the flag but no-op.
    pub fn has_fast_variant(self) -> bool {
        matches!(self, Self::Sandbox | Self::Plugins | Self::Skills)
    }
}

/// Severity of a finding. Drives exit-code computation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// A real bug the user can fix without running a setup command.
    /// Contributes to exit code 2.
    Error,
    /// A setup-needed condition (e.g., missing package).
    /// Contributes to exit code 3.
    NeedsSetup,
    /// Informational; does not affect exit code.
    Warning,
    /// Purely informational transparency (e.g. resolved durability).
    /// Does not affect exit code. Renders as SARIF `note`.
    Note,
}

/// Source location for a finding, used by `--sarif` and `--json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingLocation {
    /// Path relative to project root.
    pub path: PathBuf,
    /// Line number (1-indexed), if known.
    pub line: Option<u32>,
    /// Column number (1-indexed), if known.
    pub column: Option<u32>,
}

/// One failure event emitted by a check category.
#[derive(Debug, Clone)]
pub struct CheckFinding {
    /// Which check category produced this finding.
    pub category: CheckCategory,
    /// Severity level (Error, NeedsSetup, or Warning).
    pub severity: Severity,
    /// Rule identifier for cross-reference.
    pub rule_id: &'static str,
    /// Single-line description of the finding.
    pub summary: String,
    /// Optional additional detail or context.
    pub detail: Option<String>,
    /// Optional source location (file, line, column).
    pub location: Option<FindingLocation>,
    /// Optional remediation advice.
    pub remediation: Option<String>,
    /// Category-specific structured payload for `--json` / SARIF properties.
    pub structured: serde_json::Value,
}

/// Outcome of one category's run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    /// Category ran successfully (findings may exist).
    Ok,
    /// Category encountered an internal failure.
    Failed,
    /// Category was skipped; includes the reason.
    Skipped {
        /// Why the category was skipped.
        reason: String,
    },
}

/// One row in the orchestrator's `Vec<CheckResult>`.
#[derive(Debug, Clone)]
pub struct CheckResult {
    /// Which category this result belongs to.
    pub category: CheckCategory,
    /// Outcome of the check (Ok, Failed, or Skipped).
    pub status: CheckStatus,
    /// All findings from this category's run.
    pub findings: Vec<CheckFinding>,
    /// Wall-clock time spent on this category.
    pub duration: Duration,
}

/// Compute the process exit code from a collection of results.
///
/// Per spec §8:
/// - 0 if no findings
/// - 2 if any `Severity::Error` finding (real bugs win over setup)
/// - 3 if only `Severity::NeedsSetup` findings (setup required)
/// - `Warning` severity is informational only.
pub fn compute_exit(results: &[CheckResult]) -> i32 {
    let mut has_error = false;
    let mut has_setup = false;
    for r in results {
        for f in &r.findings {
            match f.severity {
                Severity::Error => has_error = true,
                Severity::NeedsSetup => has_setup = true,
                Severity::Warning => {}
                Severity::Note => {}
            }
        }
    }
    match (has_error, has_setup) {
        (false, false) => 0,
        (true, _) => 2,
        (false, true) => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn finding(sev: Severity) -> CheckFinding {
        CheckFinding {
            category: CheckCategory::Config,
            severity: sev,
            rule_id: "tau.test",
            summary: "test".into(),
            detail: None,
            location: None,
            remediation: None,
            structured: json!({}),
        }
    }

    fn result_with(findings: Vec<CheckFinding>) -> CheckResult {
        CheckResult {
            category: CheckCategory::Config,
            status: if findings.is_empty() {
                CheckStatus::Ok
            } else {
                CheckStatus::Failed
            },
            findings,
            duration: Duration::from_millis(1),
        }
    }

    #[test]
    fn exit_zero_when_clean() {
        let results = vec![result_with(vec![])];
        assert_eq!(compute_exit(&results), 0);
    }

    #[test]
    fn exit_two_for_any_error() {
        let results = vec![result_with(vec![finding(Severity::Error)])];
        assert_eq!(compute_exit(&results), 2);
    }

    #[test]
    fn exit_three_for_only_needs_setup() {
        let results = vec![result_with(vec![finding(Severity::NeedsSetup)])];
        assert_eq!(compute_exit(&results), 3);
    }

    #[test]
    fn error_wins_over_needs_setup() {
        let results = vec![result_with(vec![
            finding(Severity::NeedsSetup),
            finding(Severity::Error),
        ])];
        assert_eq!(compute_exit(&results), 2);
    }

    #[test]
    fn warning_does_not_affect_exit() {
        let results = vec![result_with(vec![finding(Severity::Warning)])];
        assert_eq!(compute_exit(&results), 0);
    }

    #[test]
    fn warning_with_error_still_two() {
        let results = vec![result_with(vec![
            finding(Severity::Warning),
            finding(Severity::Error),
        ])];
        assert_eq!(compute_exit(&results), 2);
    }
}
