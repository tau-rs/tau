//! Skill resolution for `skill.<name>.spawn` virtual tool dispatch.
//!
//! Skills-4 (ROADMAP §16). Three pure-ish functions:
//!
//! - [`substitute_skill_dir`] — replace `${SKILL_DIR}` literal in
//!   path-based capabilities.
//! - [`apply_scope_paths`] — narrow fs.* capability paths per
//!   caller's `scope_paths` arg.
//! - [`resolve_skill_for_spawn`] — end-to-end: lookup + substitute +
//!   scope + subset check. Returns a `SkillSpawnRequest` ready for
//!   the existing v1.1 spawn machinery.
//!   **Requires the `host-fs` feature** (reads SKILL.md via `std::fs`).

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use std::path::Path;

use globset::GlobBuilder;
use tau_domain::{Capability, FsCapability, SKILL_DIR_VAR};
use tau_pkg::{find_installed_skill, Scope};

use crate::orchestration::error::OrchestrationError;
use crate::orchestration::virtual_tools::check_capability_subset;

/// A validated, ready-to-spawn skill invocation.
///
/// Produced by [`resolve_skill_for_spawn`]; consumed by the kernel's
/// `skill.<name>.spawn` virtual-tool handler. Fields carry the
/// fully-resolved capability grant and initial message for the child run.
///
/// # Example
///
/// ```
/// use std::path::PathBuf;
/// use tau_runtime_core::orchestration::skill_resolve::SkillSpawnRequest;
///
/// // Demonstrate field access (in practice, SkillSpawnRequest is produced
/// // by resolve_skill_for_spawn, not constructed directly).
/// let req = SkillSpawnRequest {
///     skill_name: "critic".into(),
///     install_path: PathBuf::from("/skills/critic"),
///     system_prompt: "You are a critical reviewer.".into(),
///     grant: vec![],
///     message: "Review this draft.".into(),
/// };
/// assert_eq!(req.skill_name, "critic");
/// assert_eq!(req.message, "Review this draft.");
/// assert!(req.grant.is_empty());
/// ```
#[derive(Debug, Clone)]
pub struct SkillSpawnRequest {
    /// Skill name (as referenced in `skill.<name>.spawn`).
    pub skill_name: String,
    /// Absolute path to the installed skill directory.
    pub install_path: std::path::PathBuf,
    /// SKILL.md body (or caller's override if supplied).
    pub system_prompt: String,
    /// Resolved + narrowed capability grant for the child agent.
    pub grant: Vec<Capability>,
    /// Initial user message for the spawned child.
    pub message: String,
}

/// Caller's spawn args.
///
/// # Example
///
/// ```
/// use tau_runtime_core::orchestration::skill_resolve::SkillSpawnArgs;
///
/// let args = SkillSpawnArgs {
///     message: "Summarise this document.".into(),
///     system_prompt: Some("Be concise.".into()),
///     scope_paths: Some(vec!["/workspace/docs/**".into()]),
/// };
/// assert_eq!(args.message, "Summarise this document.");
/// assert_eq!(args.system_prompt.as_deref(), Some("Be concise."));
/// ```
#[derive(Debug, Clone, Default)]
pub struct SkillSpawnArgs {
    /// Required initial user message.
    pub message: String,
    /// Optional override for the skill's default SKILL.md body.
    pub system_prompt: Option<String>,
    /// Optional path narrowing. Each entry must be covered by at
    /// least one declared fs.* path.
    pub scope_paths: Option<Vec<String>>,
}

/// Substitute `${SKILL_DIR}` literal in capability `paths` entries
/// with `install_path.display()`. Non-path capabilities (net.http,
/// task_list, plan, agent.spawn, skill.spawn, custom) pass through.
///
/// # Example
///
/// ```
/// use std::path::Path;
/// use tau_domain::{Capability, FsCapability, SKILL_DIR_VAR};
/// use tau_runtime_core::orchestration::skill_resolve::substitute_skill_dir;
///
/// let cap: Capability = serde_json::from_value(serde_json::json!({
///     "kind": "fs.read",
///     "paths": [format!("{SKILL_DIR_VAR}/refs/**")]
/// })).expect("valid capability");
///
/// let out = substitute_skill_dir(&[cap], Path::new("/skills/critic/1.0.0"));
/// if let Capability::Filesystem(FsCapability::Read { paths, .. }) = &out[0] {
///     assert_eq!(paths[0], "/skills/critic/1.0.0/refs/**");
/// }
/// ```
pub fn substitute_skill_dir(caps: &[Capability], install_path: &Path) -> Vec<Capability> {
    let install_str = install_path.display().to_string();
    let subst = |paths: &[String]| -> Vec<String> {
        paths
            .iter()
            .map(|p| p.replace(SKILL_DIR_VAR, &install_str))
            .collect()
    };
    caps.iter()
        .map(|c| match c {
            Capability::Filesystem(FsCapability::Read { paths, .. }) => {
                // Use serde round-trip to avoid #[non_exhaustive] struct-construction ban.
                serde_json::from_value(serde_json::json!({
                    "kind": "fs.read",
                    "paths": subst(paths)
                }))
                .unwrap_or_else(|_| c.clone())
            }
            Capability::Filesystem(FsCapability::Write {
                paths, max_bytes, ..
            }) => {
                let mut v = serde_json::json!({
                    "kind": "fs.write",
                    "paths": subst(paths)
                });
                if let Some(mb) = max_bytes {
                    v["max_bytes"] = serde_json::json!(mb);
                }
                serde_json::from_value(v).unwrap_or_else(|_| c.clone())
            }
            Capability::Filesystem(FsCapability::Exec { paths, .. }) => {
                serde_json::from_value(serde_json::json!({
                    "kind": "fs.exec",
                    "paths": subst(paths)
                }))
                .unwrap_or_else(|_| c.clone())
            }
            other => other.clone(),
        })
        .collect()
}

/// Test if `glob` covers `candidate` (i.e. candidate is a subset).
fn glob_covers(glob: &str, candidate: &str) -> bool {
    // Strip ${SKILL_DIR} prefix if both sides have it — globset's
    // `{` char would otherwise be parsed as alternation.
    match GlobBuilder::new(glob).literal_separator(false).build() {
        Ok(g) => g.compile_matcher().is_match(candidate),
        Err(_) => false,
    }
}

/// Narrow fs.* paths per caller's `scope_paths`. For each fs.*
/// capability the skill declares, the child's effective paths =
/// intersect_with_scope(declared, scope). If empty → drop the
/// capability entirely. Non-fs caps pass through.
///
/// Hard-fail if any `scope_path` is not covered by ANY declared
/// fs.* path (typo detection).
///
/// # Example
///
/// ```
/// use tau_domain::Capability;
/// use tau_runtime_core::orchestration::skill_resolve::apply_scope_paths;
///
/// let cap: Capability = serde_json::from_value(serde_json::json!({
///     "kind": "fs.read", "paths": ["/workspace/**"]
/// })).expect("valid capability");
///
/// // Narrowing: only keep the sub-path.
/// let narrowed = apply_scope_paths(vec![cap], &["/workspace/project/**".to_string()])
///     .expect("path is covered");
/// assert_eq!(narrowed.len(), 1);
///
/// // Out-of-scope path → error.
/// let cap2: Capability = serde_json::from_value(serde_json::json!({
///     "kind": "fs.read", "paths": ["/workspace/**"]
/// })).expect("valid capability");
/// let err = apply_scope_paths(vec![cap2], &["/home/alice/**".to_string()]);
/// assert!(err.is_err());
/// ```
pub fn apply_scope_paths(
    caps: Vec<Capability>,
    scope_paths: &[String],
) -> Result<Vec<Capability>, OrchestrationError> {
    if scope_paths.is_empty() {
        return Ok(caps);
    }

    // Collect every declared fs.* path across all declared capabilities.
    let all_fs_paths: Vec<&str> = caps
        .iter()
        .flat_map(|c| match c {
            Capability::Filesystem(FsCapability::Read { paths, .. }) => paths.iter(),
            Capability::Filesystem(FsCapability::Write { paths, .. }) => paths.iter(),
            Capability::Filesystem(FsCapability::Exec { paths, .. }) => paths.iter(),
            _ => [].iter(),
        })
        .map(|s| s.as_str())
        .collect();

    // Typo check: each scope_path must be covered by at least one declared.
    for sp in scope_paths {
        let covered = all_fs_paths.iter().any(|d| glob_covers(d, sp));
        if !covered {
            return Err(OrchestrationError::SkillScopePathNotCovered { path: sp.clone() });
        }
    }

    let intersect = |declared: &[String]| -> Vec<String> {
        scope_paths
            .iter()
            .filter(|sp| declared.iter().any(|d| glob_covers(d, sp)))
            .cloned()
            .collect()
    };

    let mut out = Vec::new();
    for c in caps {
        match c {
            Capability::Filesystem(FsCapability::Read { paths, .. }) => {
                let narrowed = intersect(&paths);
                if !narrowed.is_empty() {
                    // Use serde round-trip to avoid #[non_exhaustive] ban.
                    if let Ok(cap) = serde_json::from_value(serde_json::json!({
                        "kind": "fs.read",
                        "paths": narrowed
                    })) {
                        out.push(cap);
                    }
                }
            }
            Capability::Filesystem(FsCapability::Write {
                paths, max_bytes, ..
            }) => {
                let narrowed = intersect(&paths);
                if !narrowed.is_empty() {
                    let mut v = serde_json::json!({
                        "kind": "fs.write",
                        "paths": narrowed
                    });
                    if let Some(mb) = max_bytes {
                        v["max_bytes"] = serde_json::json!(mb);
                    }
                    if let Ok(cap) = serde_json::from_value(v) {
                        out.push(cap);
                    }
                }
            }
            Capability::Filesystem(FsCapability::Exec { paths, .. }) => {
                let narrowed = intersect(&paths);
                if !narrowed.is_empty() {
                    if let Ok(cap) = serde_json::from_value(serde_json::json!({
                        "kind": "fs.exec",
                        "paths": narrowed
                    })) {
                        out.push(cap);
                    }
                }
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

/// End-to-end resolution. Looks up skill, substitutes ${SKILL_DIR},
/// narrows by scope_paths, verifies subset law, returns request.
///
/// `parent_grant` is the parent agent's effective capability grant —
/// used for the v1.1 capability subset law.
///
/// **Requires the `host-fs` feature**: reads SKILL.md from the
/// filesystem via `std::fs::read_to_string`. Embassy callers (no fs)
/// must always supply a `system_prompt` override in `args` and never
/// reach this function, OR rely on a future no-fs variant.
#[cfg(feature = "host-fs")]
pub fn resolve_skill_for_spawn(
    skill_name: &str,
    args: &SkillSpawnArgs,
    parent_grant: &[Capability],
    scope: &Scope,
) -> Result<SkillSpawnRequest, OrchestrationError> {
    let installed = match find_installed_skill(scope, skill_name) {
        Ok(Some(s)) => s,
        Ok(None) => {
            return Err(OrchestrationError::SkillNotInstalled {
                name: skill_name.to_string(),
            });
        }
        Err(tau_pkg::FindSkillError::InstallPathMissing { path, .. }) => {
            return Err(OrchestrationError::SkillInstallPathMissing {
                name: skill_name.to_string(),
                expected_path: path,
            });
        }
        Err(e) => {
            return Err(OrchestrationError::SkillContentInvalid {
                name: skill_name.to_string(),
                detail: e.to_string(),
            });
        }
    };

    // 1. system_prompt: caller override OR read SKILL.md body.
    let system_prompt = if let Some(sp) = &args.system_prompt {
        sp.clone()
    } else {
        let skill_md_path = installed.install_path.join(&installed.skill.content);
        // std::fs::read_to_string — gated by host-fs feature.
        let text = std::fs::read_to_string(&skill_md_path).map_err(|e| {
            OrchestrationError::SkillContentInvalid {
                name: skill_name.to_string(),
                detail: alloc::format!("reading SKILL.md at {skill_md_path:?}: {e}"),
            }
        })?;
        let parsed = tau_domain::parse_skill_md(&text).map_err(|e| {
            OrchestrationError::SkillContentInvalid {
                name: skill_name.to_string(),
                detail: alloc::format!("parsing SKILL.md: {e}"),
            }
        })?;
        parsed.body
    };

    // 2. ${SKILL_DIR} substitution in capabilities.
    let substituted = substitute_skill_dir(&installed.capabilities, &installed.install_path);

    // 3. Apply caller's scope_paths if provided.
    let scoped = if let Some(sp) = &args.scope_paths {
        apply_scope_paths(substituted, sp)?
    } else {
        substituted
    };

    // 4. Subset law: child grant ⊆ parent grant.
    check_capability_subset(parent_grant, &scoped)?;

    Ok(SkillSpawnRequest {
        skill_name: skill_name.to_string(),
        install_path: installed.install_path,
        system_prompt,
        grant: scoped,
        message: args.message.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;
    use tau_domain::NetCapability;

    fn fs_read(paths: Vec<&str>) -> Capability {
        let paths_json: Vec<serde_json::Value> = paths
            .iter()
            .map(|p| serde_json::Value::String(p.to_string()))
            .collect();
        serde_json::from_value(serde_json::json!({
            "kind": "fs.read",
            "paths": paths_json
        }))
        .unwrap()
    }

    fn fs_write(paths: Vec<&str>) -> Capability {
        let paths_json: Vec<serde_json::Value> = paths
            .iter()
            .map(|p| serde_json::Value::String(p.to_string()))
            .collect();
        serde_json::from_value(serde_json::json!({
            "kind": "fs.write",
            "paths": paths_json
        }))
        .unwrap()
    }

    fn net_http(hosts: Vec<&str>) -> Capability {
        let hosts_json: Vec<serde_json::Value> = hosts
            .iter()
            .map(|h| serde_json::Value::String(h.to_string()))
            .collect();
        serde_json::from_value(serde_json::json!({
            "kind": "net.http",
            "hosts": hosts_json,
            "methods": ["GET"]
        }))
        .unwrap()
    }

    #[test]
    fn substitute_skill_dir_replaces_in_fs_read() {
        let caps = vec![fs_read(vec!["${SKILL_DIR}/refs/**"])];
        let out = substitute_skill_dir(
            &caps,
            std::path::Path::new("/scope/.tau/packages/critic/0.1.0"),
        );
        match &out[0] {
            Capability::Filesystem(FsCapability::Read { paths, .. }) => {
                assert_eq!(paths[0], "/scope/.tau/packages/critic/0.1.0/refs/**");
            }
            other => panic!("expected fs.read, got {other:?}"),
        }
    }

    #[test]
    fn substitute_skill_dir_passes_through_non_fs() {
        let caps = vec![net_http(vec!["api.example.com"])];
        let out = substitute_skill_dir(&caps, std::path::Path::new("/scope"));
        assert_eq!(out.len(), 1);
        match &out[0] {
            Capability::Network(NetCapability::Http {
                hosts, methods: _, ..
            }) => {
                assert_eq!(hosts[0], "api.example.com");
            }
            other => panic!("expected net.http, got {other:?}"),
        }
    }

    #[test]
    fn apply_scope_paths_empty_returns_verbatim() {
        let caps = vec![fs_read(vec!["/a/**"])];
        let out = apply_scope_paths(caps.clone(), &[]).unwrap();
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn apply_scope_paths_intersects_fs_read() {
        let caps = vec![fs_read(vec!["/workspace/**"])];
        let scope = vec!["/workspace/project-A/**".to_string()];
        let out = apply_scope_paths(caps, &scope).unwrap();
        match &out[0] {
            Capability::Filesystem(FsCapability::Read { paths, .. }) => {
                assert_eq!(paths, &vec!["/workspace/project-A/**".to_string()]);
            }
            other => panic!("expected fs.read, got {other:?}"),
        }
    }

    #[test]
    fn apply_scope_paths_drops_capability_when_intersection_empty() {
        // fs.write declared on /drafts/**; scope_paths = /workspace/A
        // — scope is covered by fs.read but not by fs.write.
        let caps = vec![fs_read(vec!["/workspace/**"]), fs_write(vec!["/drafts/**"])];
        let scope = vec!["/workspace/project-A/**".to_string()];
        let out = apply_scope_paths(caps, &scope).unwrap();
        // fs.write dropped.
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0],
            Capability::Filesystem(FsCapability::Read { .. })
        ));
    }

    #[test]
    fn apply_scope_paths_errors_when_path_uncovered() {
        let caps = vec![fs_read(vec!["/workspace/**"])];
        let scope = vec!["/home/alice/**".to_string()];
        let err = apply_scope_paths(caps, &scope).unwrap_err();
        assert!(matches!(
            err,
            OrchestrationError::SkillScopePathNotCovered { .. }
        ));
    }

    #[test]
    fn apply_scope_paths_passes_through_non_fs() {
        let caps = vec![fs_read(vec!["/a/**"]), net_http(vec!["api.example.com"])];
        let scope = vec!["/a/sub/**".to_string()];
        let out = apply_scope_paths(caps, &scope).unwrap();
        assert_eq!(out.len(), 2);
        // net.http retained verbatim.
        assert!(matches!(
            &out[1],
            Capability::Network(NetCapability::Http { .. })
        ));
    }
}
