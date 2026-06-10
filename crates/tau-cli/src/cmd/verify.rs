//! `tau verify` — recompute install-tree hashes, compare to lockfile.
//!
//! Per spec §3:
//!
//! - Resolves the active [`Scope`] (project or global).
//! - Parses the optional package name and version.
//! - Delegates to [`tau_pkg::verify()`] (single version) or
//!   [`tau_pkg::verify_all`] (all installed packages).
//! - Prints either a human-readable summary (§3.3) or, when `--json`
//!   is set, per-line JSON events (§3.4).
//!
//! Exit codes per ADR-0007 §7:
//! - 0: all packages `Ok` or `Unverified` (no drift detected).
//! - 2: any `TreeDrift`, `BinaryDrift`, or `Missing` status.
//!
//! Orphan detection (install dirs not in the lockfile) is skipped in
//! v0.1; the lockfile is the source of truth and orphan cleanup is a
//! separate concern. This can be added in a future iteration.

use std::str::FromStr;

use semver::Version;
use tau_domain::PackageName;
use tau_pkg::{
    verify, verify_all_with_options, AnthropicConformanceIssue, LockFile, Scope, VerifyReport,
    VerifyStatus,
};

use crate::cli::VerifyArgs;
use crate::output::Output;

/// Run `tau verify`.
pub async fn run(args: &VerifyArgs, output: &mut Output) -> anyhow::Result<()> {
    // 0. Reproducibility branch (Phase 2 §E). Mutually exclusive with
    //    the package positional (enforced by clap `conflicts_with`).
    if let Some(bundle_path) = args.bundle.clone() {
        return run_reproducibility_check(&bundle_path, output);
    }

    // 1. Resolve scope.
    let scope = if args.global {
        Scope::global()?
    } else {
        let cwd = std::env::current_dir()?;
        Scope::resolve(&cwd)?
    };

    // 2. Collect reports.
    let reports: Vec<VerifyReport> = match &args.package {
        None => {
            // No package filter — verify everything in the lockfile.
            // If the lockfile doesn't exist yet, treat as empty (0 packages).
            if !scope.lockfile_path().exists() {
                vec![]
            } else {
                verify_all_with_options(&scope, args.anthropic_strict)
                    .map_err(|e| anyhow::anyhow!("{}", e))?
            }
        }
        Some(pkg_str) => {
            let name = PackageName::from_str(pkg_str)
                .map_err(|e| anyhow::anyhow!("invalid package name {:?}: {}", pkg_str, e))?;

            match &args.version {
                Some(v_str) => {
                    // Single (pkg, version) pair.
                    let version = Version::parse(v_str)
                        .map_err(|e| anyhow::anyhow!("invalid version {:?}: {}", v_str, e))?;
                    let report =
                        verify(&scope, &name, &version).map_err(|e| anyhow::anyhow!("{}", e))?;
                    vec![report]
                }
                None => {
                    // All versions of the named package.
                    let lockfile = LockFile::load(&scope.lockfile_path())
                        .map_err(|e| anyhow::anyhow!("loading lockfile: {}", e))?;
                    let pkg = lockfile
                        .find(&name)
                        .ok_or_else(|| anyhow::anyhow!("package {:?} not installed", pkg_str))?;
                    let mut reports = Vec::new();
                    for lv in &pkg.installed_versions {
                        let report = verify(&scope, &name, &lv.version)
                            .map_err(|e| anyhow::anyhow!("{}", e))?;
                        reports.push(report);
                    }
                    reports
                }
            }
        }
    };

    let total = reports.len();

    // 3. JSON: emit verify_started.
    if output.is_json() {
        output.json(&serde_json::json!({
            "event": "verify_started",
            "total": total,
        }))?;
    }

    // 4. Emit per-package events and track drift.
    let mut ok_count: usize = 0;
    let mut drift_count: usize = 0;
    let mut unverified_count: usize = 0;

    for report in &reports {
        if output.is_json() {
            emit_json_event(report, output)?;
        } else {
            emit_human_line(report, output)?;
        }

        match &report.status {
            VerifyStatus::Ok => ok_count += 1,
            VerifyStatus::Unverified => unverified_count += 1,
            VerifyStatus::TreeDrift { .. }
            | VerifyStatus::BinaryDrift { .. }
            | VerifyStatus::Missing { .. }
            | VerifyStatus::SkillContentDrift { .. }
            | VerifyStatus::AnthropicConformance { .. } => drift_count += 1,
            // The enum is #[non_exhaustive] — any future variant is
            // treated conservatively as non-drift to avoid false exits.
            _ => unverified_count += 1,
        }
    }

    // 5. Summary line / JSON completed event.
    if output.is_json() {
        output.json(&serde_json::json!({
            "event": "verify_completed",
            "total": total,
            "ok": ok_count,
            "drift": drift_count,
            "unverified": unverified_count,
        }))?;
    } else {
        output.human(&format!(
            "\n{} package{} verified, {} drifted.",
            total,
            if total == 1 { "" } else { "s" },
            drift_count,
        ))?;
    }

    // 6. Exit 2 if any drift detected.
    if drift_count > 0 {
        return Err(anyhow::anyhow!(
            "{} package{} drifted",
            drift_count,
            if drift_count == 1 { "" } else { "s" }
        ));
    }

    Ok(())
}

/// Emit a single per-package JSON event.
fn emit_json_event(report: &VerifyReport, output: &mut Output) -> anyhow::Result<()> {
    let name = report.name.as_str();
    let version = report.version.to_string();
    let event = match &report.status {
        VerifyStatus::Ok => {
            serde_json::json!({
                "event": "verify_package",
                "name": name,
                "version": version,
                "status": "ok",
            })
        }
        VerifyStatus::Unverified => {
            serde_json::json!({
                "event": "verify_package",
                "name": name,
                "version": version,
                "status": "unverified",
            })
        }
        VerifyStatus::TreeDrift { expected, actual } => {
            serde_json::json!({
                "event": "verify_package",
                "name": name,
                "version": version,
                "status": "drift",
                "kind": "tree",
                "expected": expected,
                "actual": actual,
            })
        }
        VerifyStatus::BinaryDrift {
            path,
            expected,
            actual,
        } => {
            serde_json::json!({
                "event": "verify_package",
                "name": name,
                "version": version,
                "status": "drift",
                "kind": "binary",
                "path": path.to_string_lossy(),
                "expected": expected,
                "actual": actual,
            })
        }
        VerifyStatus::Missing { path } => {
            serde_json::json!({
                "event": "verify_package",
                "name": name,
                "version": version,
                "status": "drift",
                "kind": "missing",
                "path": path.to_string_lossy(),
            })
        }
        VerifyStatus::SkillContentDrift {
            name: skill_name,
            expected,
            got,
        } => {
            serde_json::json!({
                "event": "verify_package",
                "name": name,
                "version": version,
                "status": "drift",
                "kind": "skill_content",
                "skill_name": skill_name,
                "expected": expected,
                "actual": got,
            })
        }
        VerifyStatus::AnthropicConformance { skill_name, issue } => {
            let (issue_kind, detail) = match issue {
                AnthropicConformanceIssue::MissingDescription => ("missing_description", None),
                AnthropicConformanceIssue::EmptyBody => ("empty_body", None),
                AnthropicConformanceIssue::MalformedFrontmatter { detail } => {
                    ("malformed_frontmatter", Some(detail.as_str()))
                }
                _ => ("unknown_issue", None),
            };
            if let Some(d) = detail {
                serde_json::json!({
                    "event": "verify_package",
                    "name": name,
                    "version": version,
                    "status": "drift",
                    "kind": "anthropic_conformance",
                    "skill_name": skill_name,
                    "issue": issue_kind,
                    "detail": d,
                })
            } else {
                serde_json::json!({
                    "event": "verify_package",
                    "name": name,
                    "version": version,
                    "status": "drift",
                    "kind": "anthropic_conformance",
                    "skill_name": skill_name,
                    "issue": issue_kind,
                })
            }
        }
        // Future variants: emit as unverified.
        _ => {
            serde_json::json!({
                "event": "verify_package",
                "name": name,
                "version": version,
                "status": "unverified",
            })
        }
    };
    output.json(&event)?;
    Ok(())
}

/// Emit a human-readable line for one package verification result.
///
/// Per spec §3.3:
/// ```text
/// verify <pkg>@1.0.0... ok
/// verify <other>@2.1.0... ✗ drift (tree)
///   expected: abc123...
///   actual:   xyz789...
/// verify <plugin>@1.2.0... ✗ drift (binary)
///   path: ...
///   expected: def...
///   actual:   ghi...
/// verify <missing>@1.0.0... ✗ drift (missing)
///   path: ...
/// verify <unverified>@1.0.0... (unverified — no checksum recorded)
/// ```
fn emit_human_line(report: &VerifyReport, output: &mut Output) -> anyhow::Result<()> {
    let prefix = format!("verify {}@{}... ", report.name.as_str(), report.version);
    match &report.status {
        VerifyStatus::Ok => {
            output.human(&format!("{}ok", prefix))?;
        }
        VerifyStatus::Unverified => {
            output.human(&format!(
                "{}(unverified \u{2014} no checksum recorded)",
                prefix
            ))?;
        }
        VerifyStatus::TreeDrift { expected, actual } => {
            output.human(&format!("{}\u{2717} drift (tree)", prefix))?;
            output.human(&format!("  expected: {}", expected))?;
            output.human(&format!("  actual:   {}", actual))?;
        }
        VerifyStatus::BinaryDrift {
            path,
            expected,
            actual,
        } => {
            output.human(&format!("{}\u{2717} drift (binary)", prefix))?;
            output.human(&format!("  path: {}", path.display()))?;
            output.human(&format!("  expected: {}", expected))?;
            output.human(&format!("  actual:   {}", actual))?;
        }
        VerifyStatus::Missing { path } => {
            output.human(&format!("{}\u{2717} drift (missing)", prefix))?;
            output.human(&format!("  path: {}", path.display()))?;
        }
        VerifyStatus::SkillContentDrift {
            name: skill_name,
            expected,
            got,
        } => {
            output.human(&format!("{}\u{2717} drift (skill content)", prefix))?;
            output.human(&format!("  skill: {}", skill_name))?;
            output.human(&format!("  expected: {}", expected))?;
            output.human(&format!("  actual:   {}", got))?;
        }
        VerifyStatus::AnthropicConformance { skill_name, issue } => {
            output.human(&format!(
                "{}\u{2717} AnthropicConformance (skill: {})",
                prefix, skill_name
            ))?;
            match issue {
                AnthropicConformanceIssue::MissingDescription => {
                    output.human("  issue: description field is missing or empty")?;
                }
                AnthropicConformanceIssue::EmptyBody => {
                    output.human("  issue: SKILL.md body is empty or whitespace-only")?;
                }
                AnthropicConformanceIssue::MalformedFrontmatter { detail } => {
                    output.human(&format!("  issue: malformed frontmatter — {}", detail))?;
                }
                _ => {
                    output.human("  issue: unknown conformance issue")?;
                }
            }
        }
        // Future variants: print as unverified.
        _ => {
            output.human(&format!("{}(unverified \u{2014} unknown status)", prefix))?;
        }
    }
    Ok(())
}

/// `tau verify --bundle <PATH>` — rebuild the bundle from the local tree
/// and compare to the shipped `.tau` file (Phase 2 §E).
///
/// Exit codes:
/// - 0: reproducible (rebuilt self-hash matches shipped).
/// - 2: not reproducible, or the shipped bundle is unreadable/corrupt.
/// - 3: rebuild blocked by install state (missing lockfile or an
///   uninstalled package).
/// - 70: internal/IO failure during the rebuild.
fn run_reproducibility_check(
    bundle_path: &std::path::Path,
    output: &mut Output,
) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;

    // Lower the IR from the current project tree so the rebuild embeds the
    // same payload as the original `tau build` invocation. Without this,
    // the rebuilt bundle has no ir_payload while the shipped bundle does,
    // causing a reproducibility failure.
    let shipped_str = std::fs::read_to_string(bundle_path)?;
    let shipped = tau_pkg::bundle::BundleManifest::parse_str(&shipped_str)
        .map_err(|e| anyhow::anyhow!("bundle parse failed: {e}"))?;
    let ir_payload = if shipped.ir_payload.is_some() {
        // Shipped bundle has an IR payload → rebuild with the same IR lowering.
        // For reproducibility verification, skip live MCP resolution and use an
        // empty cache; the reproduce check compares manifests field-by-field and
        // MCP entries are expected to match via pinned contracts already on disk.
        let empty_mcp_cache = std::collections::BTreeMap::new();
        crate::cmd::build::lower_ir(&cwd, &shipped.bundle.target, &empty_mcp_cache, None)
    } else {
        None
    };

    let report = match tau_pkg::bundle::verify_reproducible(tau_pkg::bundle::ReproOptions {
        bundle_path: bundle_path.to_path_buf(),
        project_root: cwd,
        ir_payload,
    }) {
        Ok(r) => r,
        Err(e) => {
            output.error(&e)?;
            std::process::exit(repro_error_exit_code(&e));
        }
    };

    if output.is_json() {
        render_repro_json(&report, output)?;
    } else {
        render_repro_human(&report, output)?;
    }

    if report.reproducible {
        Ok(())
    } else {
        std::process::exit(2);
    }
}

/// Map a [`ReproError`] to a CLI exit code.
fn repro_error_exit_code(e: &tau_pkg::bundle::ReproError) -> i32 {
    use tau_pkg::bundle::ReproError as E;
    match e {
        E::BundleRead { .. } | E::BundleParse { .. } | E::ShippedSelfHashInvalid { .. } => 2,
        E::Rebuild { source } if is_install_state_error(source) => 3,
        E::TempDir { .. } | E::RebuiltRead { .. } | E::RebuiltParse { .. } | E::Rebuild { .. } => {
            70
        }
    }
}

/// True for [`BuildError`] variants that indicate the local install state
/// is incomplete (exit 3) rather than an internal failure (exit 70).
fn is_install_state_error(e: &tau_pkg::bundle::BuildError) -> bool {
    use tau_pkg::bundle::BuildError as B;
    matches!(e, B::MissingLockfile | B::PackageNotInstalled { .. })
}

/// Abbreviate a long hex hash for human display.
fn abbrev(h: &str) -> String {
    if h.len() <= 12 {
        h.to_string()
    } else {
        format!("{}\u{2026}{}", &h[..6], &h[h.len() - 6..])
    }
}

/// Human-readable reproducibility report.
fn render_repro_human(
    report: &tau_pkg::bundle::ReproReport,
    output: &mut Output,
) -> anyhow::Result<()> {
    if report.reproducible {
        output.human(&format!(
            "\u{2713} Reproducible \u{2014} rebuilt bundle matches (sha256: {})",
            abbrev(&report.shipped_sha256)
        ))?;
    } else {
        // Diagnosis lines go to stderr (plain, no `error:` prefix) so the
        // multi-line report reads cleanly; exit code 2 carries the result.
        output.status("\u{2717} NOT reproducible")?;
        output.status(format!("  shipped: {}", abbrev(&report.shipped_sha256)))?;
        output.status(format!("  rebuilt: {}", abbrev(&report.rebuilt_sha256)))?;
        output.status("  divergences:")?;
        for d in &report.diffs {
            output.status(format!("    - {}", format_diff(d)))?;
        }
    }
    Ok(())
}

/// Format a single [`ManifestDiff`] as a one-line human description.
fn format_diff(d: &tau_pkg::bundle::ManifestDiff) -> String {
    use tau_pkg::bundle::ManifestDiff as D;
    match d {
        D::ProjectField {
            field,
            shipped,
            rebuilt,
        } => format!("project {field}: {shipped} \u{2192} {rebuilt}"),
        D::PackageMissing { name, side } => format!("package `{name}` present only in {side:?}"),
        D::PackageField {
            name,
            field,
            shipped,
            rebuilt,
        } => format!("package `{name}` {field}: {shipped} \u{2192} {rebuilt}"),
        D::AgentMissing { id, side } => format!("agent `{id}` present only in {side:?}"),
        D::AgentField {
            id,
            field,
            shipped,
            rebuilt,
        } => format!("agent `{id}` {field}: {shipped} \u{2192} {rebuilt}"),
        D::BundleMetaField {
            field,
            shipped,
            rebuilt,
        } => format!("{field}: {shipped} \u{2192} {rebuilt}"),
        D::SchemaVersionMismatch { shipped, rebuilt } => {
            format!("schema_version: {shipped} \u{2192} {rebuilt}")
        }
        D::IrPayloadHashMismatch { shipped, rebuilt } => {
            format!("ir_payload.canonical_ir_hash: {shipped} \u{2192} {rebuilt}")
        }
        D::IrPayloadPresence { present_on } => {
            format!(
                "ir_payload present in {side} but missing in the other build",
                side = format!("{present_on:?}").to_lowercase()
            )
        }
    }
}

/// JSON reproducibility report (single object, emitted via [`Output::json`]).
fn render_repro_json(
    report: &tau_pkg::bundle::ReproReport,
    output: &mut Output,
) -> anyhow::Result<()> {
    use tau_pkg::bundle::ManifestDiff as D;
    let diffs: Vec<serde_json::Value> = report
        .diffs
        .iter()
        .map(|d| match d {
            D::ProjectField {
                field,
                shipped,
                rebuilt,
            } => serde_json::json!({"kind":"project_field","field":field,"shipped":shipped,"rebuilt":rebuilt}),
            D::PackageMissing { name, side } => {
                serde_json::json!({"kind":"package_missing","name":name,"side":format!("{side:?}")})
            }
            D::PackageField {
                name,
                field,
                shipped,
                rebuilt,
            } => serde_json::json!({"kind":"package_field","name":name,"field":field,"shipped":shipped,"rebuilt":rebuilt}),
            D::AgentMissing { id, side } => {
                serde_json::json!({"kind":"agent_missing","id":id,"side":format!("{side:?}")})
            }
            D::AgentField {
                id,
                field,
                shipped,
                rebuilt,
            } => serde_json::json!({"kind":"agent_field","id":id,"field":field,"shipped":shipped,"rebuilt":rebuilt}),
            D::BundleMetaField {
                field,
                shipped,
                rebuilt,
            } => serde_json::json!({"kind":"bundle_meta_field","field":field,"shipped":shipped,"rebuilt":rebuilt}),
            D::SchemaVersionMismatch { shipped, rebuilt } => {
                serde_json::json!({"kind":"schema_version_mismatch","shipped":shipped,"rebuilt":rebuilt})
            }
            D::IrPayloadHashMismatch { shipped, rebuilt } => {
                serde_json::json!({"kind":"ir_payload_hash_mismatch","shipped":shipped,"rebuilt":rebuilt})
            }
            D::IrPayloadPresence { present_on } => {
                serde_json::json!({"kind":"ir_payload_presence","present_on":format!("{present_on:?}").to_lowercase()})
            }
        })
        .collect();
    let obj = serde_json::json!({
        "reproducible": report.reproducible,
        "shipped_sha256": report.shipped_sha256,
        "rebuilt_sha256": report.rebuilt_sha256,
        "diffs": diffs,
    });
    output.json(&obj)?;
    Ok(())
}
