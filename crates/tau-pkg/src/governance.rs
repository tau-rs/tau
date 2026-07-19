//! Governance enforcement core — the single implementation of the root
//! `[allow]` constitution lattice (ADR-0057), promoted here from `tau check`
//! so that `tau build` can gate on it without CLI coupling (ADR-0059).
//!
//! Two callers consume [`enforce_governance`]:
//!
//! - `tau check` maps each [`GovFinding`] to a `CheckFinding` (adding
//!   presentation such as source location + remediation) and reports it at the
//!   mapped severity — a `no_constitution` finding stays a non-fatal warning.
//! - `tau build` / dev-mode `tau run` treat every [`GovSeverity::Error`]
//!   finding as fatal (exit 2), and additionally treat an *absent* `[allow]`
//!   section as fatal unless `--no-governance` is passed. That policy split
//!   lives in the caller; the facts reported here are identical, so the two
//!   entry points cannot drift on what counts as a violation.
//!
//! The checks are: over-reach (declared caps ⊆ root ceiling), closed-world
//! (every referenced/defined tool and tool→mcp binding registered under
//! `[allow.*]`), and the L0–L3 capability lattice.

use serde_json::json;

use crate::capability_override::{
    capability_set_subset, compute_effective, CeilingViolation, EffectiveCapability,
    OverrideExpandError,
};
use crate::project::allow::AllowConfig;
use crate::project::{AgentEntry, ProjectConfig, ToolBinding};
use crate::{read_manifest, LockFile, Scope};
use tau_domain::Capability;

/// Severity of a governance finding. Mirrors `tau check`'s `Severity` so the
/// CLI mapping is total; the exit-code *policy* per severity is the caller's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GovSeverity {
    /// A constitution violation. Fatal at build (exit 2); fixable at check.
    Error,
    /// A setup-needed condition (e.g. an uninstalled package). Non-fatal;
    /// the lattice cannot be fully evaluated until `tau resolve` runs.
    NeedsSetup,
    /// Informational; never affects exit code.
    Warning,
    /// Pure transparency (e.g. runtime-enforced spawn). Never affects exit.
    Note,
}

/// One governance finding. Presentation-free: `summary` and `structured`
/// carry the full semantic content; the CLI layer adds category, location,
/// and remediation keyed by [`rule_id`](GovFinding::rule_id).
#[derive(Debug, Clone)]
pub struct GovFinding {
    /// Severity of this finding.
    pub severity: GovSeverity,
    /// Stable rule identifier (`tau.governance.*`).
    pub rule_id: &'static str,
    /// Single-line, self-contained description.
    pub summary: String,
    /// Machine-readable payload for `--json` / SARIF properties.
    pub structured: serde_json::Value,
}

/// Run the full `[allow]` constitution lattice over `project`, resolving each
/// agent's package capabilities via `scope` (lockfile → installed manifest).
///
/// Returns every finding — errors, the setup-needed lattice gaps, the
/// absent-constitution warning, and the spawn transparency note — in a stable
/// order (over-reach, then closed-world, then lattice). Callers filter by
/// [`GovSeverity`] to apply their own exit policy.
pub fn enforce_governance(project: &ProjectConfig, scope: &Scope) -> Vec<GovFinding> {
    let Some(allow) = &project.allow else {
        return vec![GovFinding {
            severity: GovSeverity::Warning,
            rule_id: "tau.governance.no_constitution",
            summary: "no [allow] constitution declared; governance is not enforced".to_string(),
            structured: json!({ "check": "no_constitution" }),
        }];
    };
    let mut out = Vec::new();
    over_reach(project, allow, &mut out);
    closed_world(project, allow, &mut out);
    lattice(project, allow, scope, &mut out);
    out
}

fn over_reach(project: &ProjectConfig, allow: &AllowConfig, out: &mut Vec<GovFinding>) {
    for (name, tool) in &project.tools {
        if let Err(v) = capability_set_subset(&tool.capabilities, &allow.ceiling) {
            out.push(over_reach_finding(&format!("tool '{name}'"), &v));
        }
    }
    for (name, entry) in &allow.tools {
        if let Err(v) = capability_set_subset(&entry.ceiling, &allow.ceiling) {
            out.push(over_reach_finding(
                &format!("[allow.tools.{name}] ceiling"),
                &v,
            ));
        }
    }
    for agent in project.agents.values() {
        for ov in &agent.capability_overrides {
            let Some(list) = &ov.allow else { continue };
            let Some(synth) = synth_cap(&ov.kind, list) else {
                continue;
            };
            if let Err(v) = capability_set_subset(&[synth], &allow.ceiling) {
                out.push(over_reach_finding(
                    &format!("agent '{}': override", agent.id),
                    &v,
                ));
            }
        }
    }
}

fn over_reach_finding(subject: &str, v: &CeilingViolation) -> GovFinding {
    GovFinding {
        severity: GovSeverity::Error,
        rule_id: "tau.governance.over_reach",
        summary: format!(
            "{subject}: capability {} \"{}\" exceeds [allow] ceiling ({})",
            v.kind, v.offender, v.reason
        ),
        structured: json!({ "check": "over_reach", "subject": subject, "kind": v.kind, "offender": v.offender }),
    }
}

fn closed_world(project: &ProjectConfig, allow: &AllowConfig, out: &mut Vec<GovFinding>) {
    // Agent tool refs registered. (Model refs are validated upstream by validate_models.)
    for agent in project.agents.values() {
        for t in &agent.tool_refs {
            if !allow.tools.contains_key(t) {
                out.push(unregistered(
                    "unregistered_tool",
                    "tau.governance.unregistered_tool",
                    &format!(
                        "agent '{}' references unregistered tool '{t}' — add [allow.tools.{t}]",
                        agent.id
                    ),
                    t,
                ));
            }
        }
    }
    // Defined tools registered.
    for name in project.tools.keys() {
        if !allow.tools.contains_key(name) {
            out.push(unregistered(
                "unregistered_tool_def",
                "tau.governance.unregistered_tool_def",
                &format!("tool '{name}' is defined but not registered in [allow.tools]"),
                name,
            ));
        }
    }
    // Tool→MCP bindings registered.
    for (name, entry) in &allow.tools {
        if let ToolBinding::Mcp(mcp_name) = &entry.binding {
            if !allow.mcp.contains_key(mcp_name) {
                out.push(unregistered(
                    "unregistered_mcp",
                    "tau.governance.unregistered_mcp",
                    &format!(
                        "[allow.tools.{name}] binds MCP '{mcp_name}' which is not registered in [allow.mcp]"
                    ),
                    mcp_name,
                ));
            }
        }
    }
}

fn unregistered(check: &str, rule_id: &'static str, summary: &str, subject: &str) -> GovFinding {
    GovFinding {
        severity: GovSeverity::Error,
        rule_id,
        summary: summary.to_string(),
        structured: json!({ "check": check, "subject": subject }),
    }
}

fn lattice(project: &ProjectConfig, allow: &AllowConfig, scope: &Scope, out: &mut Vec<GovFinding>) {
    // L0: each defined tool's caps ⊆ its registered [allow.tools] ceiling.
    for (name, def) in &project.tools {
        if let Some(reg) = allow.tools.get(name) {
            if let Err(v) = capability_set_subset(&def.capabilities, &reg.ceiling) {
                out.push(GovFinding {
                    severity: GovSeverity::Error,
                    rule_id: "tau.governance.tool_exceeds_ceiling",
                    summary: format!(
                        "tool '{name}': capability {} \"{}\" exceeds its [allow.tools.{name}] ceiling ({})",
                        v.kind, v.offender, v.reason
                    ),
                    structured: json!({ "check": "tool_exceeds_ceiling", "tool": name, "kind": v.kind, "offender": v.offender }),
                });
            }
        }
    }

    // L1/L2/L3: per-agent package capability lattice.
    let mut any_spawn = false;
    for agent in project.agents.values() {
        match resolve_agent_caps(agent, scope) {
            AgentCaps::NotInstalled => {
                out.push(GovFinding {
                    severity: GovSeverity::NeedsSetup,
                    rule_id: "tau.governance.agent_caps_unresolved",
                    summary: format!(
                        "agent '{}' package '{}' not installed — run `tau resolve` to check the lattice",
                        agent.id, agent.package
                    ),
                    structured: json!({ "check": "agent_caps_unresolved", "agent": agent.id }),
                });
            }
            AgentCaps::OverrideExpands(e) => {
                out.push(GovFinding {
                    severity: GovSeverity::Error,
                    rule_id: "tau.governance.override_expands_package",
                    summary: format!("agent '{}': {}", agent.id, e),
                    structured: json!({ "check": "override_expands_package", "agent": agent.id }),
                });
            }
            AgentCaps::Resolved {
                manifest,
                effective,
            } => {
                // L1: package manifest ⊆ root.
                if let Err(v) = capability_set_subset(&manifest, &allow.ceiling) {
                    out.push(lattice_error(
                        "package_exceeds_allow",
                        "tau.governance.package_exceeds_allow",
                        &format!(
                            "agent '{}' package capability {} \"{}\" exceeds [allow] ceiling ({})",
                            agent.id, v.kind, v.offender, v.reason
                        ),
                    ));
                }
                // L2: each referenced IR tool ⊆ agent effective caps.
                for t in &agent.tool_refs {
                    if let Some(def) = project.tools.get(t) {
                        if let Err(v) = capability_set_subset(&def.capabilities, &effective) {
                            out.push(lattice_error(
                                "tool_exceeds_agent",
                                "tau.governance.tool_exceeds_agent",
                                &format!(
                                    "agent '{}': tool '{t}' capability {} \"{}\" exceeds the agent's effective grant ({})",
                                    agent.id, v.kind, v.offender, v.reason
                                ),
                            ));
                        }
                    }
                }
                // L3 transparency: agent ⊇ spawn is runtime-enforced + build-deferred.
                if manifest.iter().any(|c| matches!(c, Capability::Agent(_))) {
                    any_spawn = true;
                }
            }
        }
    }
    if any_spawn {
        out.push(GovFinding {
            severity: GovSeverity::Note,
            rule_id: "tau.governance.spawn_runtime_enforced",
            summary: "agent ⊇ spawn is enforced at runtime; build-time enforcement is deferred pending per-kind agent definitions (EPIC 4)".to_string(),
            structured: json!({ "check": "spawn_runtime_enforced" }),
        });
    }
}

fn lattice_error(check: &str, rule_id: &'static str, summary: &str) -> GovFinding {
    GovFinding {
        severity: GovSeverity::Error,
        rule_id,
        summary: summary.to_string(),
        structured: json!({ "check": check }),
    }
}

fn synth_cap(kind: &str, allow: &[String]) -> Option<Capability> {
    let field = match kind {
        "fs.read" | "fs.write" | "fs.exec" => "paths",
        "net.http" => "hosts",
        "process.spawn" => "commands",
        _ => return None,
    };
    let v = json!({ "kind": kind, field: allow });
    Some(
        serde_json::from_value::<Capability>(v)
            .expect("synth_cap builds a valid Capability for known kinds"),
    )
}

// ---------------------------------------------------------------------------
// Agent package-capability resolution (Story 1.5 L1/L2): lockfile → installed
// manifest → compute_effective → materialize back into concrete Capabilities.
// ---------------------------------------------------------------------------

/// Outcome of resolving an agent's package capabilities.
enum AgentCaps {
    /// Package installed; `manifest` = declared caps, `effective` = manifest ∩ override.
    Resolved {
        manifest: Vec<Capability>,
        effective: Vec<Capability>,
    },
    /// No lockfile / package not in lockfile / install dir or manifest missing.
    NotInstalled,
    /// The agent's override expanded its package grant (compute_effective failed).
    OverrideExpands(OverrideExpandError),
}

/// Materialize an EffectiveCapability into a concrete Capability by applying
/// the override deltas to `source` via the serde bridge.
///
/// `deny` is only applied to list-bearing kinds (fs/net/process); for other
/// kinds it is a no-op (bounded; no defined semantics today).
fn materialize(e: &EffectiveCapability) -> Capability {
    if e.allow_override.is_none() && e.deny.is_empty() && e.max_bytes_override.is_none() {
        return e.source.clone();
    }
    let mut v = match serde_json::to_value(&e.source) {
        Ok(serde_json::Value::Object(o)) => o,
        _ => return e.source.clone(),
    };
    let field = match v.get("kind").and_then(|k| k.as_str()) {
        Some("fs.read") | Some("fs.write") | Some("fs.exec") => Some("paths"),
        Some("net.http") => Some("hosts"),
        Some("process.spawn") => Some("commands"),
        _ => None,
    };
    if let Some(field) = field {
        let mut list: Vec<String> = match &e.allow_override {
            Some(a) => a.clone(),
            None => v
                .get(field)
                .and_then(|x| x.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|s| s.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
        };
        list.retain(|s| !e.deny.contains(s));
        v.insert(field.to_string(), json!(list));
    }
    if let Some(mb) = e.max_bytes_override {
        v.insert("max_bytes".to_string(), json!(mb));
    }
    serde_json::from_value::<Capability>(serde_json::Value::Object(v))
        .unwrap_or_else(|_| e.source.clone())
}

/// Resolve the agent's package capabilities (manifest + effective) via `scope`.
fn resolve_agent_caps(agent: &AgentEntry, scope: &Scope) -> AgentCaps {
    let pkg_name = agent.package.split('@').next().unwrap_or(&agent.package);

    let lockfile_path = scope.lockfile_path();
    let Ok(lockfile) = LockFile::load(&lockfile_path) else {
        return AgentCaps::NotInstalled;
    };
    let Some(pkg) = lockfile
        .packages
        .iter()
        .find(|p| p.name.as_str() == pkg_name)
    else {
        return AgentCaps::NotInstalled;
    };

    let toml_path = scope
        .package_dir(&pkg.name, &pkg.active_version)
        .join("tau.toml");
    let Ok(manifest) = read_manifest(&toml_path) else {
        return AgentCaps::NotInstalled;
    };
    let manifest_caps: Vec<Capability> = manifest.capabilities().to_vec();

    match compute_effective(&manifest_caps, &agent.capability_overrides) {
        Ok(eff) => AgentCaps::Resolved {
            manifest: manifest_caps,
            effective: eff.iter().map(materialize).collect(),
        },
        Err(e) => AgentCaps::OverrideExpands(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability_override::CapabilityOverride;

    /// Parse + validate a tau.toml string into a `ProjectConfig` plus a
    /// project-`Scope` rooted at the same tempdir. Fixtures MUST validate
    /// under 1.2 (every agent needs a model registered in
    /// [models]/[allow.models]; backends must be declared packages).
    fn proj(toml: &str) -> (ProjectConfig, Scope, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        // Mark as a project scope so Scope::resolve binds to this dir.
        std::fs::create_dir_all(dir.path().join(".tau")).unwrap();
        let p = dir.path().join("tau.toml");
        std::fs::write(&p, toml).unwrap();
        let cfg = ProjectConfig::from_path(&p).expect("fixture must parse + validate");
        let scope = Scope::resolve(dir.path()).expect("scope must resolve");
        (cfg, scope, dir)
    }

    fn summaries(f: &[GovFinding]) -> String {
        f.iter()
            .map(|x| x.summary.clone())
            .collect::<Vec<_>>()
            .join(" | ")
    }

    fn cap(j: &str) -> Capability {
        serde_json::from_str(j).unwrap()
    }

    #[test]
    fn absent_allow_emits_single_warning() {
        let (cfg, scope, _dir) = proj("[project]\nname = \"demo\"\n");
        let f = enforce_governance(&cfg, &scope);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, GovSeverity::Warning);
        assert_eq!(f[0].rule_id, "tau.governance.no_constitution");
    }

    #[test]
    fn empty_allow_no_violations() {
        let (cfg, scope, _dir) = proj(
            r#"
[project]
name = "demo"

[allow]
"fs.read" = { paths = ["/proj/**"] }
"#,
        );
        let f = enforce_governance(&cfg, &scope);
        assert!(f.is_empty(), "got {}", summaries(&f));
    }

    #[test]
    fn tool_caps_exceeding_root_flagged() {
        let (cfg, scope, _dir) = proj(
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
        let f = enforce_governance(&cfg, &scope);
        assert!(
            f.iter().any(|x| x.rule_id == "tau.governance.over_reach"
                && x.summary.contains("/etc/**")
                && x.summary.contains("fetch")),
            "got: {}",
            summaries(&f)
        );
    }

    #[test]
    fn allow_tools_ceiling_exceeding_root_flagged() {
        let (cfg, scope, _dir) = proj(
            r#"
[project]
name = "demo"

[allow]
"net.http" = { hosts = ["api.x.com"] }

[allow.tools.fetch]
native = "Fetch"
"net.http" = { hosts = ["evil.com"] }
"#,
        );
        let f = enforce_governance(&cfg, &scope);
        assert!(
            f.iter().any(|x| x.rule_id == "tau.governance.over_reach"
                && x.summary.contains("evil.com")),
            "got: {}",
            summaries(&f)
        );
    }

    #[test]
    fn unregistered_tool_ref_and_defined_tool_flagged() {
        let (cfg, scope, _dir) = proj(
            r#"
packages = ["demo"]

[project]
name = "demo"

[allow]
"fs.read" = { paths = ["/proj/**"] }

[allow.models.fast]
backend = "demo"
model = "m-1"

[tools.fetch]
native = "Fetch"
capabilities = []

[agents.solo]
display_name = "Solo"
package = "demo@^0.1"
model = "fast"
tool_refs = ["fetch"]
"#,
        );
        let f = enforce_governance(&cfg, &scope);
        assert!(
            f.iter()
                .any(|x| x.rule_id == "tau.governance.unregistered_tool"
                    && x.summary.contains("fetch")),
            "tool ref: {}",
            summaries(&f)
        );
        assert!(
            f.iter()
                .any(|x| x.rule_id == "tau.governance.unregistered_tool_def"
                    && x.summary.contains("fetch")),
            "tool def: {}",
            summaries(&f)
        );
    }

    #[test]
    fn tool_binds_unregistered_mcp_flagged() {
        let (cfg, scope, _dir) = proj(
            r#"
[project]
name = "demo"

[allow]
"net.http" = { hosts = ["api.x.com"] }

[allow.tools.weather]
mcp = "weather"
"#,
        );
        let f = enforce_governance(&cfg, &scope);
        assert!(
            f.iter()
                .any(|x| x.rule_id == "tau.governance.unregistered_mcp"
                    && x.summary.contains("weather")),
            "got: {}",
            summaries(&f)
        );
    }

    #[test]
    fn fully_registered_within_ceiling_is_clean() {
        let (cfg, scope, _dir) = proj(
            r#"
packages = ["demo"]

[project]
name = "demo"

[allow]
"fs.read" = { paths = ["/proj/**"] }

[allow.models.fast]
backend = "demo"
model = "m-1"

[allow.mcp.weather]
url = "https://api.weather.com/mcp"

[allow.tools.read_temp]
native = "ReadTemp"
"fs.read" = { paths = ["/proj/sensors/**"] }

[tools.read_temp]
native = "ReadTemp"
capabilities = [{ kind = "fs.read", paths = ["/proj/sensors/**"] }]

[agents.solo]
display_name = "Solo"
package = "demo@^0.1"
model = "fast"
tool_refs = ["read_temp"]
"#,
        );
        let f = enforce_governance(&cfg, &scope);
        let errs: Vec<_> = f
            .iter()
            .filter(|x| x.severity == GovSeverity::Error)
            .collect();
        assert!(
            errs.is_empty(),
            "expected no errors, got: {}",
            summaries(&f)
        );
    }

    #[test]
    fn tool_caps_exceeding_its_registered_ceiling_flagged() {
        let (cfg, scope, _dir) = proj(
            r#"
[project]
name = "demo"

[allow]
"fs.read" = { paths = ["/proj/**"] }

[allow.tools.grep_tool]
native = "Grep"
"fs.read" = { paths = ["/proj/src/**"] }

[tools.grep_tool]
native = "Grep"
capabilities = [{ kind = "fs.read", paths = ["/proj/**"] }]
"#,
        );
        let f = enforce_governance(&cfg, &scope);
        assert!(
            f.iter()
                .any(|x| x.rule_id == "tau.governance.tool_exceeds_ceiling"
                    && x.summary.contains("grep_tool")
                    && x.summary.contains("/proj/**")),
            "got: {}",
            summaries(&f)
        );
    }

    #[test]
    fn agent_override_exceeding_root_flagged() {
        let (cfg, scope, _dir) = proj(
            r#"
packages = ["demo"]

[project]
name = "demo"

[allow]
"fs.read" = { paths = ["/proj/**"] }

[allow.models.fast]
backend = "demo"
model = "m-1"

[agents.solo]
display_name = "Solo"
package = "demo@^0.1"
model = "fast"
capabilities = [{ kind = "fs.read", allow_paths = ["/etc/**"] }]
"#,
        );
        let f = enforce_governance(&cfg, &scope);
        assert!(
            f.iter().any(|x| x.rule_id == "tau.governance.over_reach"
                && x.summary.contains("/etc/**")
                && x.summary.contains("solo")),
            "got: {}",
            summaries(&f)
        );
    }

    #[test]
    fn materialize_passthrough_when_no_override() {
        let pkg = vec![cap(r#"{"kind":"fs.read","paths":["/proj/**"]}"#)];
        let eff = compute_effective(&pkg, &[]).unwrap();
        let m = materialize(&eff[0]);
        assert_eq!(m, cap(r#"{"kind":"fs.read","paths":["/proj/**"]}"#));
    }

    #[test]
    fn materialize_applies_allow_override_and_deny() {
        let pkg = vec![cap(r#"{"kind":"fs.read","paths":["/proj/**"]}"#)];
        let ov = vec![CapabilityOverride::new(
            "fs.read".into(),
            Some(vec!["/proj/src/**".into(), "/proj/docs/**".into()]),
            vec!["/proj/docs/**".into()],
            None,
        )];
        let eff = compute_effective(&pkg, &ov).unwrap();
        let m = materialize(&eff[0]);
        // allow_override minus deny = ["/proj/src/**"]
        assert_eq!(m, cap(r#"{"kind":"fs.read","paths":["/proj/src/**"]}"#));
    }
}
