//! `tau check mcp_contracts` — verify pinned MCP contracts match the
//! lockfile.
//!
//! Walks `LockFile.mcp_entries`; for each entry with
//! `pinned_contract: Some(path)`, reads the pin and verifies:
//! 1. The pin's self-hash is internally consistent
//!    (`PinnedContract::verify_self_hash`).
//! 2. The pin's `contract_hash_hex` matches the lockfile's
//!    `LockedMcpEntry.contract_hash` (drift detection).

use crate::cmd::check::result::{
    CheckCategory, CheckFinding, CheckResult, CheckStatus, FindingLocation, Severity,
};
use crate::cmd::check::runner::CheckCtx;
use serde_json::json;
use tau_mcp::contract::pinned::PinnedContract;
use tau_pkg::LockFile;

pub fn run_mcp_contracts(ctx: &CheckCtx) -> CheckResult {
    let lockfile_path = ctx.scope.lockfile_path();
    let lockfile: Option<LockFile> = if lockfile_path.exists() {
        LockFile::load(&lockfile_path).ok()
    } else {
        None
    };
    let Some(lockfile) = lockfile else {
        return CheckResult {
            category: CheckCategory::McpContracts,
            status: CheckStatus::Skipped {
                reason: "no lockfile (run tau resolve / tau build)".into(),
            },
            findings: Vec::new(),
            duration: std::time::Duration::ZERO,
        };
    };

    let mut findings = Vec::new();
    for entry in &lockfile.mcp_entries {
        let Some(pin_rel) = entry.pinned_contract.as_deref() else {
            continue;
        };
        let pin_path = ctx.project_root.join(pin_rel);
        let pin_loc = Some(FindingLocation {
            path: pin_path.clone(),
            line: None,
            column: None,
        });

        let bytes = match std::fs::read(&pin_path) {
            Ok(b) => b,
            Err(e) => {
                findings.push(CheckFinding {
                    category: CheckCategory::McpContracts,
                    severity: Severity::Error,
                    rule_id: "tau.mcp.contract.missing",
                    summary: format!(
                        "missing pin file for `{}` at {}",
                        entry.entry,
                        pin_path.display()
                    ),
                    detail: Some(format!("{e}")),
                    location: pin_loc,
                    remediation: Some(format!("tau mcp pin {}", entry.entry)),
                    structured: json!({
                        "entry": &entry.entry,
                        "pinned_contract": pin_rel,
                        "io_error": e.to_string(),
                    }),
                });
                continue;
            }
        };

        let pinned: PinnedContract = match serde_json::from_slice(&bytes) {
            Ok(p) => p,
            Err(e) => {
                findings.push(CheckFinding {
                    category: CheckCategory::McpContracts,
                    severity: Severity::Error,
                    rule_id: "tau.mcp.contract.malformed",
                    summary: format!(
                        "malformed pin file for `{}` at {}",
                        entry.entry,
                        pin_path.display()
                    ),
                    detail: Some(format!("{e}")),
                    location: pin_loc,
                    remediation: Some(format!("tau mcp refresh {}", entry.entry)),
                    structured: json!({
                        "entry": &entry.entry,
                        "pinned_contract": pin_rel,
                        "parse_error": e.to_string(),
                    }),
                });
                continue;
            }
        };

        if let Err(e) = pinned.verify_self_hash() {
            findings.push(CheckFinding {
                category: CheckCategory::McpContracts,
                severity: Severity::Error,
                rule_id: "tau.mcp.contract.self_drift",
                summary: format!("pin self-hash drift for `{}`", entry.entry),
                detail: Some(format!("{e}")),
                location: pin_loc,
                remediation: Some(format!("tau mcp refresh {}", entry.entry)),
                structured: json!({
                    "entry": &entry.entry,
                    "pinned_contract": pin_rel,
                }),
            });
            continue;
        }

        if pinned.contract_hash_hex != entry.contract_hash {
            findings.push(CheckFinding {
                category: CheckCategory::McpContracts,
                severity: Severity::Error,
                rule_id: "tau.mcp.contract.lockfile_drift",
                summary: format!(
                    "pin hash drift for `{}`: lockfile expects `{}`, pin file has `{}`",
                    entry.entry,
                    &entry.contract_hash[..16.min(entry.contract_hash.len())],
                    &pinned.contract_hash_hex[..16.min(pinned.contract_hash_hex.len())],
                ),
                detail: None,
                location: pin_loc,
                remediation: Some(format!(
                    "tau mcp refresh {} (then re-run tau build)",
                    entry.entry
                )),
                structured: json!({
                    "entry": &entry.entry,
                    "pinned_contract": pin_rel,
                    "lockfile_hash": &entry.contract_hash,
                    "pin_hash": &pinned.contract_hash_hex,
                }),
            });
        }
    }

    let status = if findings.is_empty() {
        CheckStatus::Ok
    } else {
        CheckStatus::Failed
    };
    CheckResult {
        category: CheckCategory::McpContracts,
        status,
        findings,
        duration: std::time::Duration::ZERO,
    }
}
