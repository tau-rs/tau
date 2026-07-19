//! Shared governance build gate (ADR-0059).
//!
//! `tau build` and dev-mode `tau run` both call [`enforce_or_exit`] so the two
//! entry points apply an identical policy over the relocated core in
//! [`tau_pkg::governance`]:
//!
//! - `--no-governance` → skip the gate, print a WARNING banner naming every
//!   skipped check, and report [`GovernanceOutcome::Skipped`] so the caller can
//!   stamp the bundle's governance record accordingly.
//! - `[allow]` **absent** → fatal (exit 2): a governed build must declare a
//!   constitution or explicitly opt out with `--no-governance` (decision D2-B).
//! - any [`GovSeverity::Error`] finding → fatal (exit 2), each rendered to
//!   stderr in `tau check`'s finding style.
//! - otherwise → [`GovernanceOutcome::Enforced`].
//!
//! `NeedsSetup` / `Warning` / `Note` findings never fail the gate — an
//! uninstalled package can't be lattice-checked at build time, matching
//! `tau check`'s non-fatal treatment of the same conditions.

use crate::output::Output;
use tau_pkg::governance::{enforce_governance, GovSeverity};
use tau_pkg::project::ProjectConfig;
use tau_pkg::Scope;

/// Whether governance was enforced or explicitly skipped. Consumed by
/// `tau build` to record the bundle's governance verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GovernanceOutcome {
    /// The `[allow]` constitution was enforced and passed.
    Enforced,
    /// The gate was skipped via `--no-governance`.
    Skipped,
}

/// The governance passes named in the `--no-governance` warning banner.
const SKIPPED_CHECKS: &str = "over_reach, closed_world, lattice";

/// Run the governance gate, or `std::process::exit(2)` on a fatal outcome.
///
/// Returns the [`GovernanceOutcome`] on success so the caller can record it in
/// the bundle manifest.
pub fn enforce_or_exit(
    project: &ProjectConfig,
    scope: &Scope,
    no_governance: bool,
    output: &mut Output,
) -> GovernanceOutcome {
    if no_governance {
        let _ = output.warn(format!(
            "--no-governance: skipping constitution enforcement ({SKIPPED_CHECKS}); \
             the artifact is unverified against any [allow] ceiling"
        ));
        return GovernanceOutcome::Skipped;
    }

    if project.allow.is_none() {
        let _ = output.error(
            "ungoverned build: no [allow] constitution declared; add an [allow] section \
             to tau.toml, or pass --no-governance to build without enforcement",
        );
        std::process::exit(2);
    }

    let findings = enforce_governance(project, scope);
    let errors: Vec<_> = findings
        .iter()
        .filter(|f| f.severity == GovSeverity::Error)
        .collect();
    if !errors.is_empty() {
        let _ = output.error(format!(
            "governance: {} constitution violation{} — build refused",
            errors.len(),
            if errors.len() == 1 { "" } else { "s" }
        ));
        for f in &errors {
            let _ = output.status(format!("  [{}] {}", f.rule_id, f.summary));
        }
        std::process::exit(2);
    }

    GovernanceOutcome::Enforced
}
