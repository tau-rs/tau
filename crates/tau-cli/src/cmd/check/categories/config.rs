//! `tau check config` — validate the project's tau.toml.
//!
//! Just re-parses tau.toml via `ProjectConfig::from_path` and translates
//! the `ProjectConfigError` variants into `CheckFinding`s. The runner
//! already attempted this in `CheckCtx::load`, but we re-parse here to
//! ensure determinism and to capture the actual error for reporting.

use crate::cmd::check::result::{
    CheckCategory, CheckFinding, CheckResult, CheckStatus, FindingLocation, Severity,
};
use crate::cmd::check::runner::CheckCtx;
use serde_json::json;
use tau_pkg::project::{ProjectConfig, ProjectConfigError};

/// Run the `config` category. Returns Ok-status when tau.toml parses
/// and validates; otherwise Failed with one finding per error.
pub fn run_config(ctx: &CheckCtx) -> CheckResult {
    let tau_toml_path = ctx.project_root.join("tau.toml");
    let findings = match ProjectConfig::from_path(&tau_toml_path) {
        Ok(_) => Vec::new(),
        Err(e) => vec![error_to_finding(&e, &tau_toml_path)],
    };
    let status = if findings.is_empty() {
        CheckStatus::Ok
    } else {
        CheckStatus::Failed
    };
    CheckResult {
        category: CheckCategory::Config,
        status,
        findings,
        duration: std::time::Duration::ZERO, // overwritten by runner
    }
}

fn error_to_finding(err: &ProjectConfigError, tau_toml: &std::path::Path) -> CheckFinding {
    let location = Some(FindingLocation {
        path: tau_toml.to_path_buf(),
        line: None,
        column: None,
    });
    CheckFinding {
        category: CheckCategory::Config,
        severity: Severity::Error,
        rule_id: "tau.config.invalid",
        summary: err.to_string(),
        detail: None,
        location,
        remediation: Some("fix tau.toml per the error message above".into()),
        structured: json!({"kind": variant_kind(err)}),
    }
}

/// Map each `ProjectConfigError` variant to a stable PascalCase `kind`
/// string. The kind is a contract consumed by `--json`/SARIF output and by
/// tests, so existing strings must not be renamed. Every arm mirrors its
/// variant name verbatim.
fn variant_kind(err: &ProjectConfigError) -> &'static str {
    match err {
        ProjectConfigError::NotFound => "NotFound",
        ProjectConfigError::Read { .. } => "Read",
        ProjectConfigError::Parse { .. } => "Parse",
        ProjectConfigError::EmptyProjectName => "EmptyProjectName",
        ProjectConfigError::AgentValidation { .. } => "AgentValidation",
        ProjectConfigError::AllowValidation { .. } => "AllowValidation",
        ProjectConfigError::CapabilityOverrideExpands { .. } => "CapabilityOverrideExpands",
        ProjectConfigError::PromptAmbiguous { .. } => "PromptAmbiguous",
        ProjectConfigError::RequiresToolsBareStringRejected { .. } => {
            "RequiresToolsBareStringRejected"
        }
        ProjectConfigError::ParseStr { .. } => "ParseStr",
        ProjectConfigError::ToolValidation { .. } => "ToolValidation",
        ProjectConfigError::StepValidation { .. } => "StepValidation",
        ProjectConfigError::TriggerValidation { .. } => "TriggerValidation",
        ProjectConfigError::EmptyPipeline => "EmptyPipeline",
        ProjectConfigError::PipelineValidation { .. } => "PipelineValidation",
        ProjectConfigError::UnsupportedMcpUrl { .. } => "UnsupportedMcpUrl",
        ProjectConfigError::GoalValidation { .. } => "GoalValidation",
        ProjectConfigError::DeliverableValidation { .. } => "DeliverableValidation",
        ProjectConfigError::JudgeAndModelConflict { .. } => "JudgeAndModelConflict",
        ProjectConfigError::DeliverableNoProducer { .. } => "DeliverableNoProducer",
        ProjectConfigError::DeliverableAmbiguousProducer { .. } => "DeliverableAmbiguousProducer",
        ProjectConfigError::DeliverableProducerLacksCapability { .. } => {
            "DeliverableProducerLacksCapability"
        }
        ProjectConfigError::GateAfterProducer { .. } => "GateAfterProducer",
        ProjectConfigError::RetrySpanNoLlm { .. } => "RetrySpanNoLlm",
        ProjectConfigError::UnknownRetryFrom { .. } => "UnknownRetryFrom",
        ProjectConfigError::BadGoalRegex { .. } => "BadGoalRegex",
        ProjectConfigError::UnknownJudgeAgent { .. } => "UnknownJudgeAgent",
        ProjectConfigError::CredentialDeclaration { .. } => "CredentialDeclaration",
        ProjectConfigError::MalformedModelEntry { .. } => "MalformedModelEntry",
        ProjectConfigError::ModelBackendNotDeclared { .. } => "ModelBackendNotDeclared",
        ProjectConfigError::UnknownModelAlias { .. } => "UnknownModelAlias",
        ProjectConfigError::MissingAgentModel { .. } => "MissingAgentModel",
        ProjectConfigError::DirsRequireRoot => "DirsRequireRoot",
        ProjectConfigError::DirsRoot { .. } => "DirsRoot",
        ProjectConfigError::DirsRootsOverlap { .. } => "DirsRootsOverlap",
        ProjectConfigError::DefFile { .. } => "DefFile",
        ProjectConfigError::DuplicateDefinition { .. } => "DuplicateDefinition",
        // tau-pkg uses #[non_exhaustive] — surface unknown variants without panicking.
        _ => "Other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `[dirs]` present with no filesystem root to scan (`parse_str` path)
    /// must map to the typed `"DirsRequireRoot"` kind, not the `"Other"`
    /// catch-all. Guards the per-variant `variant_kind` mapping against
    /// regressing back to the wildcard (companion to the `DirsRoot` case
    /// covered below via `from_path`).
    #[test]
    fn variant_kind_maps_dirs_require_root() {
        let toml = "[project]\nname = \"p\"\n[dirs]\nagents = \"agents\"\n";
        let err = ProjectConfig::parse_str(toml).unwrap_err();
        assert!(matches!(err, ProjectConfigError::DirsRequireRoot));
        assert_eq!(variant_kind(&err), "DirsRequireRoot");
    }

    /// A syntactically invalid `[dirs]` root (absolute path) maps to the
    /// typed `"DirsRoot"` kind. Exercised via `from_path`, the same entry
    /// point `run_config` uses, so this reflects the reachable CLI path.
    #[test]
    fn variant_kind_maps_dirs_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let tau_toml = tmp.path().join("tau.toml");
        std::fs::write(
            &tau_toml,
            "[project]\nname = \"p\"\n[dirs]\nagents = \"/abs/agents\"\n",
        )
        .unwrap();
        let err = ProjectConfig::from_path(&tau_toml).unwrap_err();
        assert!(matches!(err, ProjectConfigError::DirsRoot { .. }));
        assert_eq!(variant_kind(&err), "DirsRoot");
    }
}
