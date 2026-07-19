//! Build-time governance gate (ADR-0057 / D2).
//!
//! `tau check governance` reports the `[allow]` constitution as *findings*
//! (a warning when absent). `tau build` and dev `tau run` are stricter:
//! governed-by-default means an absent `[allow]` is a hard error (`GOV000`)
//! unless the operator explicitly opts out, and any ceiling violation blocks
//! the build. This module maps a project + flags to one of three outcomes and
//! computes the [`GovernanceVerdict`] that gets stamped into the bundle.
//!
//! The gate REUSES the check category's [`governance_findings`] engine for the
//! present-ceiling case: a build blocks exactly when that engine reports an
//! `Error`-severity finding.

use super::categories::governance::governance_findings;
use super::result::{CheckFinding, Severity};
use super::runner::CheckCtx;
use tau_pkg::bundle::GovernanceVerdict;
use tau_pkg::project::ProjectConfig;

/// Build/run governance escape-hatch flags. The two are mutually exclusive at
/// the CLI (`conflicts_with`), so at most one is ever `true`.
#[derive(Debug, Clone, Copy, Default)]
pub struct GovernanceFlags {
    /// `--allow-ungoverned`: authorize a build/run that has NO `[allow]`
    /// ceiling. Distinct from `--no-governance`: this is "no ceiling at all".
    pub allow_ungoverned: bool,
    /// `--no-governance`: the project HAS a ceiling but skip checking it.
    /// Distinct from `--allow-ungoverned`: this is "have a ceiling, skip it".
    pub no_governance: bool,
}

/// Result of the build-time governance gate.
#[derive(Debug)]
pub enum GovernanceOutcome {
    /// Proceed with the build/run; stamp this verdict into the bundle.
    Proceed(GovernanceVerdict),
    /// No `[allow]` ceiling declared and no opt-out → `GOV000` hard error.
    NoConstitution,
    /// A ceiling was declared but violated → block with these `Error` findings.
    Violations(Vec<CheckFinding>),
}

/// Evaluate the governed-by-default gate for a project.
///
/// Decision table (`--no-governance` and `--allow-ungoverned` are mutually
/// exclusive at the CLI):
///
/// | `[allow]` | flag                | outcome                        |
/// |-----------|---------------------|--------------------------------|
/// | present   | `--no-governance`   | `Proceed(Skipped)`             |
/// | present   | —                   | run engine → `Governed` / `Violations` |
/// | absent    | `--allow-ungoverned`| `Proceed(Ungoverned)`          |
/// | absent    | — / `--no-governance` | `NoConstitution` (`GOV000`)  |
///
/// The last row encodes that `--no-governance` ("have a ceiling, skip it") has
/// nothing to skip when there is no ceiling, so it still fails `GOV000`.
pub fn evaluate_governance(
    project: &ProjectConfig,
    ctx: &CheckCtx,
    flags: GovernanceFlags,
) -> GovernanceOutcome {
    match &project.allow {
        Some(_) => {
            if flags.no_governance {
                return GovernanceOutcome::Proceed(GovernanceVerdict::Skipped);
            }
            let tau_toml = ctx.project_root.join("tau.toml");
            let errors: Vec<CheckFinding> = governance_findings(project, &tau_toml, ctx)
                .into_iter()
                .filter(|f| f.severity == Severity::Error)
                .collect();
            if errors.is_empty() {
                GovernanceOutcome::Proceed(GovernanceVerdict::Governed)
            } else {
                GovernanceOutcome::Violations(errors)
            }
        }
        None => {
            if flags.allow_ungoverned {
                GovernanceOutcome::Proceed(GovernanceVerdict::Ungoverned)
            } else {
                GovernanceOutcome::NoConstitution
            }
        }
    }
}

/// Render the `GOV000` "no `[allow]` section" diagnostic, in the tau
/// compiler-diagnostic style (ADR-0057 / D2). Emitted by `tau build` and
/// dev `tau run` when a project declares no ceiling and did not opt out.
pub fn render_no_constitution() -> String {
    [
        "error[GOV000]: no [allow] section declared",
        "  = tau builds are governed by default; declare a ceiling or opt out explicitly",
        "  = help: run `tau init --allow` to scaffold a constitution from installed",
        "          packages' declared capabilities, or pass --allow-ungoverned",
    ]
    .join("\n")
}

/// Render the `GOV000` "ungoverned bundle refused" diagnostic. Emitted by
/// `tau run --bundle` when the bundle's recorded verdict is `Ungoverned`
/// and `--allow-ungoverned` was not passed (governed-by-default on both ends).
pub fn render_ungoverned_bundle_refused() -> String {
    [
        "error[GOV000]: bundle was built without a governance ceiling (verdict: ungoverned)",
        "  = running an ungoverned bundle is governed by default",
        "  = help: pass --allow-ungoverned to run it anyway",
    ]
    .join("\n")
}

/// Render the build-refused diagnostic for a set of `Error`-severity
/// governance findings (the ceiling was declared but violated). Reuses the
/// findings' existing `tau.governance.*` rule ids rather than a GOV code.
pub fn render_violations(findings: &[CheckFinding]) -> String {
    let mut lines = vec![format!(
        "error: [allow] ceiling violated — build refused ({} violation{})",
        findings.len(),
        if findings.len() == 1 { "" } else { "s" }
    )];
    for f in findings {
        lines.push(format!("  {}: {}", f.rule_id, f.summary));
    }
    lines.push(
        "  = help: run `tau check governance` for the full report, or pass --no-governance \
         to build without enforcing (records verdict \"skipped\")"
            .to_string(),
    );
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_pkg::Scope;

    /// Parse + validate a tau.toml string into a `ProjectConfig` under a
    /// project scope, mirroring the governance category's own test harness.
    fn proj(toml: &str) -> (ProjectConfig, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".tau")).unwrap();
        let p = dir.path().join("tau.toml");
        std::fs::write(&p, toml).unwrap();
        let cfg = ProjectConfig::from_path(&p).expect("fixture must parse + validate");
        (cfg, dir)
    }

    fn ctx_for(dir: &tempfile::TempDir) -> CheckCtx {
        let root = dir.path().to_path_buf();
        let scope = Scope::resolve(&root).expect("scope must resolve");
        CheckCtx {
            project_root: root,
            scope,
            project: None,
            fast: false,
            target: None,
        }
    }

    fn no_flags() -> GovernanceFlags {
        GovernanceFlags::default()
    }

    #[test]
    fn render_no_constitution_names_code_and_both_escape_hatches() {
        let s = render_no_constitution();
        assert!(s.starts_with("error[GOV000]:"), "got: {s}");
        assert!(s.contains("no [allow] section declared"), "got: {s}");
        assert!(s.contains("tau init --allow"), "got: {s}");
        assert!(s.contains("--allow-ungoverned"), "got: {s}");
    }

    #[test]
    fn render_ungoverned_bundle_names_gov000_and_flag() {
        let s = render_ungoverned_bundle_refused();
        assert!(s.starts_with("error[GOV000]:"), "got: {s}");
        assert!(s.contains("ungoverned"), "got: {s}");
        assert!(s.contains("--allow-ungoverned"), "got: {s}");
    }

    #[test]
    fn render_violations_lists_each_rule_id_and_count() {
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
        let GovernanceOutcome::Violations(v) = evaluate_governance(&cfg, &ctx, no_flags()) else {
            panic!("expected violations");
        };
        let s = render_violations(&v);
        assert!(s.contains("build refused"), "got: {s}");
        assert!(s.contains("tau.governance.over_reach"), "got: {s}");
        assert!(s.contains("--no-governance"), "got: {s}");
    }

    #[test]
    fn absent_allow_no_flags_is_no_constitution() {
        let (cfg, dir) = proj("[project]\nname = \"demo\"\n");
        let ctx = ctx_for(&dir);
        assert!(matches!(
            evaluate_governance(&cfg, &ctx, no_flags()),
            GovernanceOutcome::NoConstitution
        ));
    }

    #[test]
    fn absent_allow_with_allow_ungoverned_is_ungoverned() {
        let (cfg, dir) = proj("[project]\nname = \"demo\"\n");
        let ctx = ctx_for(&dir);
        let flags = GovernanceFlags {
            allow_ungoverned: true,
            no_governance: false,
        };
        assert!(matches!(
            evaluate_governance(&cfg, &ctx, flags),
            GovernanceOutcome::Proceed(GovernanceVerdict::Ungoverned)
        ));
    }

    #[test]
    fn absent_allow_with_no_governance_is_still_no_constitution() {
        // --no-governance means "have a ceiling, skip it"; with no ceiling
        // there is nothing to skip, so it still fails GOV000.
        let (cfg, dir) = proj("[project]\nname = \"demo\"\n");
        let ctx = ctx_for(&dir);
        let flags = GovernanceFlags {
            allow_ungoverned: false,
            no_governance: true,
        };
        assert!(matches!(
            evaluate_governance(&cfg, &ctx, flags),
            GovernanceOutcome::NoConstitution
        ));
    }

    #[test]
    fn present_clean_allow_is_governed() {
        let (cfg, dir) = proj(
            r#"
[project]
name = "demo"

[allow]
"fs.read" = { paths = ["/proj/**"] }
"#,
        );
        let ctx = ctx_for(&dir);
        assert!(matches!(
            evaluate_governance(&cfg, &ctx, no_flags()),
            GovernanceOutcome::Proceed(GovernanceVerdict::Governed)
        ));
    }

    #[test]
    fn present_violated_allow_is_violations() {
        // A defined tool whose fs.read exceeds the root ceiling → over_reach
        // Error → the build must block.
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
        match evaluate_governance(&cfg, &ctx, no_flags()) {
            GovernanceOutcome::Violations(v) => {
                assert!(
                    v.iter().all(|f| f.severity == Severity::Error),
                    "only Error findings block a build"
                );
                assert!(
                    v.iter().any(|f| f.rule_id == "tau.governance.over_reach"),
                    "expected an over_reach violation"
                );
            }
            other => panic!("expected Violations, got {other:?}"),
        }
    }

    #[test]
    fn present_violated_allow_with_no_governance_is_skipped() {
        // Same violating project, but --no-governance skips checking entirely
        // and records the Skipped verdict.
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
        let flags = GovernanceFlags {
            allow_ungoverned: false,
            no_governance: true,
        };
        assert!(matches!(
            evaluate_governance(&cfg, &ctx, flags),
            GovernanceOutcome::Proceed(GovernanceVerdict::Skipped)
        ));
    }
}
