//! `tau check` — pre-flight validation aggregator.
//!
//! See spec at `docs/superpowers/specs/2026-05-18-tau-check-design.md`.
//!
//! Bare `tau check` runs all 6 categories; subcommands run one each.
//! Output: human (default), `--json` (JSONL), `--sarif` (SARIF 2.1.0).
//! Exit codes: 0 clean / 2 fixable / 3 needs-setup / 64 usage / 70 internal.

mod categories;
mod output;
mod result;
mod runner;

pub use result::{
    compute_exit, CheckCategory, CheckFinding, CheckResult, CheckStatus, FindingLocation, Severity,
};

use anyhow::Result;
use std::str::FromStr;
use tau_ports::target::TargetTriple;

/// Entry point for `tau check`. Parses CheckArgs, builds CheckCtx,
/// selects category list, runs the orchestrator, dispatches output
/// (Tasks 10-12 wire renderers), and exits with the appropriate code.
pub async fn run(args: crate::cli::CheckArgs) -> Result<()> {
    // Resolve project root.
    let project_root = match args.project.as_ref() {
        Some(p) => p.clone(),
        None => std::env::current_dir()?,
    };

    let target: Option<TargetTriple> = if let Some(s) = args.target.as_deref() {
        match TargetTriple::from_str(s) {
            Ok(t) => {
                if tau_ports::target::lookup(&t).is_none() {
                    eprintln!("error: unknown triple `{t}` (parses but not registered)");
                    std::process::exit(64);
                }
                Some(t)
            }
            Err(e) => {
                eprintln!("error: could not parse triple `{s}`: {e}");
                std::process::exit(64);
            }
        }
    } else {
        None
    };

    let ctx = runner::CheckCtx::load(project_root, args.fast, target).await?;

    // --auto-resolve: attempt to install missing required tools for all agents
    // before running checks. Errors are non-fatal — the packages category will
    // report any remaining missing tools as findings.
    if args.auto_resolve {
        if let Some(project) = &ctx.project {
            let mut resolve_output = crate::output::Output::with_writers(
                Box::new(std::io::sink()),
                Box::new(std::io::stderr()),
                false,
                false,
                crate::output::ColorChoice::Never,
            );
            if let Err(e) = crate::cmd::resolve_helpers::resolve_and_install_for_project(
                project.agents.values().cloned(),
                &ctx.scope,
                false, // no_install = false → actually install
                false, // dry_run = false → apply changes
                &mut resolve_output,
            ) {
                // Non-fatal: warn on stderr and continue; packages check
                // will surface any remaining gaps as findings.
                eprintln!("tau check: --auto-resolve encountered errors (continuing): {e:#}");
            }
        }
    }

    // Determine category list.
    let categories: Vec<CheckCategory> = match args.category.as_deref() {
        None => CheckCategory::ALL.to_vec(),
        Some("config") => vec![CheckCategory::Config],
        Some("lockfile") => vec![CheckCategory::Lockfile],
        Some("packages") => vec![CheckCategory::Packages],
        Some("sandbox") => vec![CheckCategory::Sandbox],
        Some("plugins") => vec![CheckCategory::Plugins],
        Some("skills") => vec![CheckCategory::Skills],
        Some(other) => anyhow::bail!("unknown check category: {other}"),
    };

    let results = runner::run_categories(&ctx, &categories).await;
    let exit_code = compute_exit(&results);

    if let Some(path) = args.sarif.as_ref() {
        let rendered = output::sarif::render(&results);
        if path == "-" {
            print!("{rendered}");
        } else {
            std::fs::write(path, rendered)?;
        }
    } else if args.json {
        let rendered = output::json::render(
            &ctx.project_root,
            &categories,
            args.fast,
            &results,
            exit_code,
        );
        print!("{rendered}");
    } else {
        let use_color = std::env::var_os("NO_COLOR").is_none();
        let rendered = output::human::render(&results, use_color, exit_code);
        print!("{rendered}");
    }

    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
}
