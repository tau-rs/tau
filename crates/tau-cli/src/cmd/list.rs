//! `tau list` — list installed packages or project-declared agents.
//!
//! Per spec §3.13 list row.
//!
//! - `tau list` / `tau list packages` reads the per-scope lockfile via
//!   [`tau_pkg::list`] and prints one row per installed package.
//!   `--global` forces global scope, `--all` lists project + global
//!   side-by-side, otherwise scope is auto-resolved per cwd.
//! - `tau list agents` reads the project `tau.toml` via
//!   [`crate::config::ProjectConfig::from_path`] and prints one row per
//!   declared agent. Errors with exit 2 if no project tau.toml is found.
//! - `--dry-run` is rejected as nonsensical for a read-only command.
//!
//! Both modes support `--json` for scriptable output.

use serde::Serialize;

use crate::cli::{ListArgs, ListResource};
use crate::config::{ProjectConfig, ProjectConfigError};
use crate::output::Output;

/// Run `tau list`.
pub async fn run(args: &ListArgs, output: &mut Output) -> anyhow::Result<()> {
    if args.dry_run {
        anyhow::bail!("--dry-run is not supported on tau list (read-only command)");
    }

    match args.resource {
        ListResource::Packages => list_packages(args, output),
        ListResource::Agents => list_agents(args, output),
    }
}

/// Tabular row produced for one installed package.
#[derive(Debug, Serialize)]
struct PackageRow {
    name: String,
    version: String,
    source: String,
    scope: String,
    version_count: usize,
}

/// Tabular row produced for one declared agent.
#[derive(Debug, Serialize)]
struct AgentRow {
    id: String,
    display_name: String,
    package: String,
    llm_backend: String,
    /// Effective capability set after applying the project override.
    /// `None` when --capabilities flag is not set, OR the package is
    /// not installed.
    #[serde(skip_serializing_if = "Option::is_none")]
    effective_capabilities: Option<Vec<EffectiveCapabilityRow>>,
}

/// Per-capability render row in the JSON output. Field naming mirrors
/// the project tau.toml schema (allow_paths/deny_paths for fs.*,
/// allow_hosts/deny_hosts for net.http, allow_commands/deny_commands
/// for process.spawn, max_bytes for fs.write).
#[derive(Debug, Serialize)]
struct EffectiveCapabilityRow {
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    allow_paths: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    deny_paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    allow_hosts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    deny_hosts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    allow_commands: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    deny_commands: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_bytes: Option<u64>,
}

fn list_packages(args: &ListArgs, output: &mut Output) -> anyhow::Result<()> {
    use tau_pkg::{Scope, ScopeKind};

    let cwd = std::env::current_dir()?;

    let mut rows: Vec<PackageRow> = Vec::new();

    if args.all {
        // Project (if any) + global, side-by-side.
        // We can only detect a project scope by walking up from cwd; if the
        // resolved scope is global, there's no project to list.
        let resolved = Scope::resolve(&cwd)?;
        if matches!(resolved.kind(), ScopeKind::Project) {
            collect_packages(&resolved, "project", &mut rows)?;
        }
        let global_scope = Scope::global()?;
        collect_packages(&global_scope, "global", &mut rows)?;
    } else if args.global {
        let global_scope = Scope::global()?;
        collect_packages(&global_scope, "global", &mut rows)?;
    } else {
        let scope = Scope::resolve(&cwd)?;
        let label = scope_label(scope.kind());
        collect_packages(&scope, label, &mut rows)?;
    }

    if output.is_json() {
        output.json(&rows)?;
    } else if rows.is_empty() {
        output.human("(no packages installed)")?;
    } else {
        output.human(
            "name                version    source                                   scope",
        )?;
        output
            .human("--                  --         --                                       --")?;
        for r in &rows {
            output.human(&format!(
                "{:<20}{:<11}{:<41}{}",
                truncate(&r.name, 19),
                truncate(&r.version, 10),
                truncate(&r.source, 40),
                r.scope,
            ))?;
        }
    }

    Ok(())
}

fn collect_packages(
    scope: &tau_pkg::Scope,
    label: &str,
    rows: &mut Vec<PackageRow>,
) -> anyhow::Result<()> {
    let listing = tau_pkg::list(scope).map_err(|e| anyhow::anyhow!("listing packages: {e}"))?;
    for pkg in listing {
        rows.push(PackageRow {
            name: pkg.name.as_str().to_owned(),
            version: pkg.active_version.to_string(),
            source: pkg.source.to_string(),
            scope: label.to_owned(),
            version_count: pkg.installed_versions.len(),
        });
    }
    Ok(())
}

fn scope_label(kind: tau_pkg::ScopeKind) -> &'static str {
    match kind {
        tau_pkg::ScopeKind::Global => "global",
        tau_pkg::ScopeKind::Project => "project",
        // ScopeKind is `#[non_exhaustive]`; future-proof the match.
        _ => "unknown",
    }
}

/// Render `s` truncated to at most `n` characters, no wrapping. Tabular
/// rows use this so a long URL or version doesn't blow out the column.
fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_owned()
    } else {
        let cut: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{cut}\u{2026}")
    }
}

fn list_agents(args: &ListArgs, output: &mut Output) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let path = cwd.join("tau.toml");

    let config = match ProjectConfig::from_path(&path) {
        Ok(cfg) => cfg,
        Err(ProjectConfigError::NotFound) => {
            anyhow::bail!("no project tau.toml found at {path:?}; run `tau init` to create one");
        }
        Err(e) => return Err(e.into()),
    };

    // For --capabilities, resolve the package scope once. Failure here
    // (e.g. cwd has no scope) is not fatal for the basic listing —
    // each per-agent lookup will independently fall back to "no manifest".
    let scope = if args.capabilities {
        Some(tau_pkg::Scope::resolve(&cwd)?)
    } else {
        None
    };

    let mut rows: Vec<AgentRow> = Vec::with_capacity(config.agents.len());
    for agent in config.agents.values() {
        let effective_capabilities = if args.capabilities {
            // Best-effort: if the manifest can't be loaded, render row without
            // the effective set. compute_effective failure (override expands)
            // is a hard error.
            match crate::config::build_agent_definition(
                agent,
                &cwd,
                scope
                    .as_ref()
                    .expect("scope was resolved when --capabilities was set"),
            ) {
                Ok((_def, manifest)) => {
                    let effective = tau_runtime_tokio::capability_override::compute_effective(
                        manifest.capabilities(),
                        &agent.capability_overrides,
                    )
                    .map_err(|e| anyhow::anyhow!("agent {:?}: {}", agent.id, e))?;
                    Some(
                        effective
                            .iter()
                            .map(effective_capability_to_row)
                            .collect::<Vec<_>>(),
                    )
                }
                Err(_) => None, // package not installed; non-fatal
            }
        } else {
            None
        };

        rows.push(AgentRow {
            id: agent.id.clone(),
            display_name: agent.display_name.clone(),
            package: agent.package.clone(),
            llm_backend: agent.llm_backend.clone(),
            effective_capabilities,
        });
    }

    if output.is_json() {
        output.json(&rows)?;
    } else if rows.is_empty() {
        output.human("(no agents declared in project tau.toml)")?;
    } else if args.capabilities {
        output.human(
            "id              display_name              package                  llm_backend       effective_capabilities",
        )?;
        output.human("--              --                        --                       --                --")?;
        for r in &rows {
            let caps_str = match &r.effective_capabilities {
                Some(cap_rows) => format_effective_caps_human(cap_rows),
                None => "(package not installed)".to_string(),
            };
            output.human(&format!(
                "{:<16}{:<26}{:<25}{:<18}{}",
                truncate(&r.id, 15),
                truncate(&r.display_name, 25),
                truncate(&r.package, 24),
                truncate(&r.llm_backend, 17),
                caps_str,
            ))?;
        }
    } else {
        output.human(
            "id              display_name              package                  llm_backend",
        )?;
        output.human("--              --                        --                       --")?;
        for r in &rows {
            output.human(&format!(
                "{:<16}{:<26}{:<25}{}",
                truncate(&r.id, 15),
                truncate(&r.display_name, 25),
                truncate(&r.package, 24),
                r.llm_backend,
            ))?;
        }
    }

    Ok(())
}

/// Build an `EffectiveCapabilityRow` for the JSON/human output from a
/// runtime `EffectiveCapability`. Field selection is per-kind: paths
/// for fs.*, hosts for net.http, commands for process.spawn.
fn effective_capability_to_row(
    eff: &tau_runtime_tokio::capability_override::EffectiveCapability,
) -> EffectiveCapabilityRow {
    use tau_domain::{Capability, FsCapability, NetCapability, ProcessCapability};
    let kind = match &eff.source {
        Capability::Filesystem(FsCapability::Read { .. }) => "fs.read",
        Capability::Filesystem(FsCapability::Write { .. }) => "fs.write",
        Capability::Filesystem(FsCapability::Exec { .. }) => "fs.exec",
        Capability::Network(NetCapability::Http { .. }) => "net.http",
        Capability::Process(ProcessCapability::Spawn { .. }) => "process.spawn",
        Capability::Agent(_) => "agent.spawn",
        Capability::Custom { name, .. } => {
            return EffectiveCapabilityRow {
                kind: name.clone(),
                allow_paths: None,
                deny_paths: Vec::new(),
                allow_hosts: None,
                deny_hosts: Vec::new(),
                allow_commands: None,
                deny_commands: Vec::new(),
                max_bytes: None,
            }
        }
        _ => "unknown",
    }
    .to_string();
    let mut row = EffectiveCapabilityRow {
        kind: kind.clone(),
        allow_paths: None,
        deny_paths: Vec::new(),
        allow_hosts: None,
        deny_hosts: Vec::new(),
        allow_commands: None,
        deny_commands: Vec::new(),
        max_bytes: eff.max_bytes_override,
    };
    match kind.as_str() {
        "fs.read" | "fs.write" | "fs.exec" => {
            row.allow_paths = eff.allow_override.clone();
            row.deny_paths = eff.deny.clone();
        }
        "net.http" => {
            row.allow_hosts = eff.allow_override.clone();
            row.deny_hosts = eff.deny.clone();
        }
        "process.spawn" => {
            row.allow_commands = eff.allow_override.clone();
            row.deny_commands = eff.deny.clone();
        }
        _ => {}
    }
    row
}

/// Render an `EffectiveCapabilityRow` set as a single human-readable line.
fn format_effective_caps_human(rows: &[EffectiveCapabilityRow]) -> String {
    if rows.is_empty() {
        return "(none)".into();
    }
    rows.iter()
        .map(format_one_cap)
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_one_cap(row: &EffectiveCapabilityRow) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(allow) = &row.allow_paths {
        parts.push(format!("allow={}", allow.join(",")));
    } else if let Some(allow) = &row.allow_hosts {
        parts.push(format!("allow={}", allow.join(",")));
    } else if let Some(allow) = &row.allow_commands {
        parts.push(format!("allow={}", allow.join(",")));
    }
    let denies = if !row.deny_paths.is_empty() {
        Some(&row.deny_paths)
    } else if !row.deny_hosts.is_empty() {
        Some(&row.deny_hosts)
    } else if !row.deny_commands.is_empty() {
        Some(&row.deny_commands)
    } else {
        None
    };
    if let Some(d) = denies {
        parts.push(format!("deny={}", d.join(",")));
    }
    if let Some(mb) = row.max_bytes {
        parts.push(format!("max_bytes={mb}"));
    }
    if parts.is_empty() {
        // Capability declared but no narrowing — package's grant unchanged.
        format!("{}[unchanged]", row.kind)
    } else {
        format!("{}[{}]", row.kind, parts.join(";"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_label_handles_global_and_project() {
        assert_eq!(scope_label(tau_pkg::ScopeKind::Global), "global");
        assert_eq!(scope_label(tau_pkg::ScopeKind::Project), "project");
    }

    #[test]
    fn truncate_keeps_short_strings_unchanged() {
        assert_eq!(truncate("abc", 10), "abc");
    }

    #[test]
    fn truncate_appends_ellipsis_when_too_long() {
        let out = truncate("abcdefghij", 5);
        assert_eq!(out.chars().count(), 5);
        assert!(out.ends_with('\u{2026}'));
    }

    #[test]
    fn format_one_cap_renders_allow_and_deny() {
        let row = EffectiveCapabilityRow {
            kind: "fs.read".into(),
            allow_paths: Some(vec!["/proj/src/**".into()]),
            deny_paths: vec!["/proj/.env".into()],
            allow_hosts: None,
            deny_hosts: Vec::new(),
            allow_commands: None,
            deny_commands: Vec::new(),
            max_bytes: None,
        };
        let out = format_one_cap(&row);
        assert!(out.contains("fs.read"));
        assert!(out.contains("allow=/proj/src/**"));
        assert!(out.contains("deny=/proj/.env"));
    }

    #[test]
    fn format_one_cap_unchanged_when_no_narrowing() {
        let row = EffectiveCapabilityRow {
            kind: "fs.read".into(),
            allow_paths: None,
            deny_paths: Vec::new(),
            allow_hosts: None,
            deny_hosts: Vec::new(),
            allow_commands: None,
            deny_commands: Vec::new(),
            max_bytes: None,
        };
        let out = format_one_cap(&row);
        assert!(out.contains("fs.read"));
        assert!(out.contains("unchanged"));
    }
}
