//! Reusable capability subset relation (ADR-0057 decision 2) and the shared
//! per-field comparison helpers used by both `compute_effective` (per-package
//! narrowing) and `capability_set_subset` (lattice-link ceiling checks).
//!
//! Story 1.3 scope: the primitive + helpers only. No enforcement wiring
//! (`tau check` = 1.4; lattice traversal = 1.5).

use super::glob_subset::is_glob_subset_set;
use tau_domain::{
    AgentCapability, Capability, FsCapability, NetCapability, NetHosts, ProcessCapability,
    SkillCapability,
};

/// A single child capability that exceeds the parent ceiling. The caller
/// (1.4 / 1.5) prepends agent/link framing; this type names only *what*
/// exceeded.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CeilingViolation {
    /// Capability kind that violated (`"fs.read"`, `"agent.spawn"`, `"custom"`, …).
    pub kind: String,
    /// The exact child entry that exceeded (a path / host / command / kind /
    /// skill / mode token), or the kind name when the kind itself is unmatched.
    pub offender: String,
    /// Human-readable reason.
    pub reason: String,
}

/// `child ⊆ parent` over full capability sets, matched by kind (`Custom` by
/// `name`). Returns the first violation. A child kind with no matching parent
/// kind is a violation (deny-by-default ceiling of ∅).
pub fn capability_set_subset(
    child: &[Capability],
    parent: &[Capability],
) -> Result<(), CeilingViolation> {
    for c in child {
        let kind = kind_str(c);
        let matching: Vec<&Capability> = parent.iter().filter(|p| same_kind(c, p)).collect();
        if matching.is_empty() {
            return Err(CeilingViolation {
                kind: kind.to_string(),
                offender: kind.to_string(),
                reason: "kind not in ceiling".to_string(),
            });
        }
        if let Err((offender, reason)) = cap_subset_against(c, &matching) {
            return Err(CeilingViolation {
                kind: kind.to_string(),
                offender,
                reason,
            });
        }
    }
    Ok(())
}

/// Stable kind discriminator. `Custom` → `"custom"`; unknown future variants
/// → `"unknown"` (which never matches a parent → deny-by-default).
fn kind_str(cap: &Capability) -> &'static str {
    match cap {
        Capability::Filesystem(FsCapability::Read { .. }) => "fs.read",
        Capability::Filesystem(FsCapability::Write { .. }) => "fs.write",
        Capability::Filesystem(FsCapability::Exec { .. }) => "fs.exec",
        Capability::Network(NetCapability::Http { .. }) => "net.http",
        Capability::Process(ProcessCapability::Spawn { .. }) => "process.spawn",
        Capability::Agent(AgentCapability::Spawn { .. }) => "agent.spawn",
        Capability::Skill(SkillCapability::Spawn { .. }) => "skill.spawn",
        Capability::TaskList { .. } => "task_list",
        Capability::Plan { .. } => "plan",
        Capability::Custom { .. } => "custom",
        _ => "unknown",
    }
}

/// Same kind for matching purposes. `Custom` matches `Custom` only when the
/// `name` matches. `"unknown"` never matches (fail-closed).
fn same_kind(a: &Capability, b: &Capability) -> bool {
    match (a, b) {
        (Capability::Custom { name: na, .. }, Capability::Custom { name: nb, .. }) => na == nb,
        (Capability::Custom { .. }, _) | (_, Capability::Custom { .. }) => false,
        _ => {
            let k = kind_str(a);
            k != "unknown" && k == kind_str(b)
        }
    }
}

fn mode_rank(mode: &str, allow_manage: bool) -> Option<u8> {
    match mode {
        "read" => Some(0),
        "write" => Some(1),
        "manage" if allow_manage => Some(2),
        _ => None,
    }
}

/// Compare one child cap against all parent caps of the same kind. Returns
/// `Err((offender, reason))` on violation.
fn cap_subset_against(child: &Capability, parents: &[&Capability]) -> Result<(), (String, String)> {
    match child {
        Capability::Filesystem(FsCapability::Read { paths, .. })
        | Capability::Filesystem(FsCapability::Exec { paths, .. }) => {
            let pp = gather_paths(parents);
            paths_subset(paths, &pp).map_err(|o| (o, "not a subset of any allowed path".into()))
        }
        Capability::Filesystem(FsCapability::Write {
            paths, max_bytes, ..
        }) => {
            let pp = gather_paths(parents);
            paths_subset(paths, &pp)
                .map_err(|o| (o, "not a subset of any allowed path".to_string()))?;
            let parent_mb = most_permissive_max_bytes(parents);
            match (max_bytes, parent_mb) {
                (_, None) => Ok(()),
                (None, Some(_)) => Err((
                    "max_bytes=unlimited".to_string(),
                    "child is unlimited but ceiling caps max_bytes".to_string(),
                )),
                (Some(c), Some(_)) => max_bytes_le(*c, parent_mb)
                    .map_err(|tok| (tok, "exceeds ceiling max_bytes".to_string())),
            }
        }
        Capability::Network(NetCapability::Http { hosts, .. }) => hosts_subset(hosts, parents),
        Capability::Process(ProcessCapability::Spawn { commands, .. }) => {
            let pc = gather_commands(parents);
            string_set_subset(commands, &pc).map_err(|o| (o, "command not in ceiling".into()))
        }
        Capability::Agent(AgentCapability::Spawn { allowed_kinds, .. }) => {
            let pk = gather_agent_kinds(parents);
            string_set_subset(allowed_kinds, &pk)
                .map_err(|o| (o, "agent kind not in ceiling".into()))
        }
        Capability::Skill(SkillCapability::Spawn { allowed_skills, .. }) => {
            let ps = gather_skills(parents);
            string_set_subset(allowed_skills, &ps).map_err(|o| (o, "skill not in ceiling".into()))
        }
        Capability::TaskList { mode } => mode_subset(mode, parents, true),
        Capability::Plan { mode } => mode_subset(mode, parents, false),
        Capability::Custom { name, params } => {
            let ok = parents.iter().any(|p| {
                matches!(p, Capability::Custom { name: pn, params: pp } if pn == name && pp == params)
            });
            if ok {
                Ok(())
            } else {
                Err((name.clone(), "custom params do not match ceiling".into()))
            }
        }
        _ => Err((
            kind_str(child).to_string(),
            "unsupported capability kind".into(),
        )),
    }
}

fn mode_subset(
    mode: &str,
    parents: &[&Capability],
    allow_manage: bool,
) -> Result<(), (String, String)> {
    let child_rank = mode_rank(mode, allow_manage)
        .ok_or_else(|| (format!("mode={mode}"), "unknown mode".to_string()))?;
    let parent_rank = parents
        .iter()
        .filter_map(|p| match p {
            Capability::TaskList { mode } | Capability::Plan { mode } => {
                mode_rank(mode, allow_manage)
            }
            _ => None,
        })
        .max();
    match parent_rank {
        Some(pr) if child_rank <= pr => Ok(()),
        Some(_) => Err((format!("mode={mode}"), "mode exceeds ceiling".to_string())),
        None => Err((format!("mode={mode}"), "ceiling mode unknown".to_string())),
    }
}

/// Callers MUST pass a same-kind-filtered parent slice (guaranteed by `same_kind` in
/// `capability_set_subset`); this gathers Read/Write/Exec paths indiscriminately.
fn gather_paths(parents: &[&Capability]) -> Vec<String> {
    parents
        .iter()
        .filter_map(|p| match p {
            Capability::Filesystem(FsCapability::Read { paths, .. })
            | Capability::Filesystem(FsCapability::Write { paths, .. })
            | Capability::Filesystem(FsCapability::Exec { paths, .. }) => Some(paths.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// Host lattice for `net.http` (D7-B `NetHosts`). A parent granting
/// [`NetHosts::Any`] subsumes any child; otherwise a child [`NetHosts::List`]
/// must be a subset of the union of parent lists, and a child
/// [`NetHosts::Any`] is only admitted by a parent `Any`.
fn hosts_subset(child: &NetHosts, parents: &[&Capability]) -> Result<(), (String, String)> {
    let parent_any = parents.iter().any(|p| {
        matches!(
            p,
            Capability::Network(NetCapability::Http {
                hosts: NetHosts::Any,
                ..
            })
        )
    });
    if parent_any {
        return Ok(());
    }
    match child {
        NetHosts::Any => Err((
            "any".to_string(),
            "grants any host but ceiling is a specific host list".into(),
        )),
        NetHosts::List(c) => {
            let ph = gather_hosts(parents);
            string_set_subset(c, &ph).map_err(|o| (o, "host not in ceiling".into()))
        }
    }
}

/// Union of the explicit host lists across same-kind parents. `Any` parents
/// are handled by [`hosts_subset`] before this is called, so they contribute
/// nothing here.
fn gather_hosts(parents: &[&Capability]) -> Vec<String> {
    parents
        .iter()
        .filter_map(|p| match p {
            Capability::Network(NetCapability::Http {
                hosts: NetHosts::List(h),
                ..
            }) => Some(h.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn gather_commands(parents: &[&Capability]) -> Vec<String> {
    parents
        .iter()
        .filter_map(|p| match p {
            Capability::Process(ProcessCapability::Spawn { commands, .. }) => {
                Some(commands.clone())
            }
            _ => None,
        })
        .flatten()
        .collect()
}

fn gather_agent_kinds(parents: &[&Capability]) -> Vec<String> {
    parents
        .iter()
        .filter_map(|p| match p {
            Capability::Agent(AgentCapability::Spawn { allowed_kinds, .. }) => {
                Some(allowed_kinds.clone())
            }
            _ => None,
        })
        .flatten()
        .collect()
}

fn gather_skills(parents: &[&Capability]) -> Vec<String> {
    parents
        .iter()
        .filter_map(|p| match p {
            Capability::Skill(SkillCapability::Spawn { allowed_skills, .. }) => {
                Some(allowed_skills.clone())
            }
            _ => None,
        })
        .flatten()
        .collect()
}

/// Most permissive ceiling cap: `None` (unlimited) if any matching parent is
/// unlimited, else `Some(max of the limits)`.
fn most_permissive_max_bytes(parents: &[&Capability]) -> Option<u64> {
    let mut acc: Option<u64> = Some(0);
    for p in parents {
        if let Capability::Filesystem(FsCapability::Write { max_bytes, .. }) = p {
            match max_bytes {
                None => return None,
                Some(m) => acc = acc.map(|a| a.max(*m)),
            }
        }
    }
    acc
}

/// Globbed path subset: every `child` path is a glob-subset of some `parent`
/// path. `Err(offender)` names the first child path with no admitting parent.
pub(crate) fn paths_subset(child: &[String], parent: &[String]) -> Result<(), String> {
    is_glob_subset_set(child, parent)
}

/// Exact-set inclusion: every `child` entry equals some `parent` entry.
/// `Err(offender)` names the first child entry not present in `parent`.
pub(crate) fn string_set_subset(child: &[String], parent: &[String]) -> Result<(), String> {
    for c in child {
        if !parent.iter().any(|p| p == c) {
            return Err(c.clone());
        }
    }
    Ok(())
}

/// `max_bytes` tightening: `child <= parent`. `parent == None` means the
/// ceiling is unlimited (any child is admitted). `Err` carries the child value.
pub(crate) fn max_bytes_le(child: u64, parent: Option<u64>) -> Result<(), String> {
    match parent {
        None => Ok(()),
        Some(max) if child <= max => Ok(()),
        Some(_) => Err(format!("max_bytes={child}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tau_domain::Capability;

    fn cap(json: &str) -> Capability {
        serde_json::from_str(json).expect("valid capability JSON")
    }

    #[test]
    fn fs_read_paths_within_ceiling_ok() {
        let child = vec![cap(r#"{"kind":"fs.read","paths":["/proj/src/**"]}"#)];
        let parent = vec![cap(r#"{"kind":"fs.read","paths":["/proj/**"]}"#)];
        assert!(capability_set_subset(&child, &parent).is_ok());
    }

    #[test]
    fn fs_read_path_outside_ceiling_violation_names_offender() {
        let child = vec![cap(r#"{"kind":"fs.read","paths":["/etc/**"]}"#)];
        let parent = vec![cap(r#"{"kind":"fs.read","paths":["/proj/**"]}"#)];
        let v = capability_set_subset(&child, &parent).unwrap_err();
        assert_eq!(v.kind, "fs.read");
        assert_eq!(v.offender, "/etc/**");
    }

    #[test]
    fn fs_write_max_bytes_higher_rejected() {
        let child = vec![cap(
            r#"{"kind":"fs.write","paths":["/p/**"],"max_bytes":9000}"#,
        )];
        let parent = vec![cap(
            r#"{"kind":"fs.write","paths":["/p/**"],"max_bytes":5000}"#,
        )];
        let v = capability_set_subset(&child, &parent).unwrap_err();
        assert_eq!(v.kind, "fs.write");
        assert!(v.offender.contains("9000"), "got {}", v.offender);
    }

    #[test]
    fn fs_write_unlimited_child_under_capped_ceiling_rejected() {
        let child = vec![cap(r#"{"kind":"fs.write","paths":["/p/**"]}"#)];
        let parent = vec![cap(
            r#"{"kind":"fs.write","paths":["/p/**"],"max_bytes":5000}"#,
        )];
        let v = capability_set_subset(&child, &parent).unwrap_err();
        assert_eq!(v.kind, "fs.write");
        assert_eq!(v.offender, "max_bytes=unlimited");
    }

    #[test]
    fn net_http_host_in_ceiling_ok_method_diff_ignored() {
        // methods differ but are NOT checked in 1.3
        let child = vec![cap(
            r#"{"kind":"net.http","hosts":["api.x.com"],"methods":["POST"]}"#,
        )];
        let parent = vec![cap(
            r#"{"kind":"net.http","hosts":["api.x.com"],"methods":["GET"]}"#,
        )];
        assert!(capability_set_subset(&child, &parent).is_ok());
    }

    #[test]
    fn net_http_host_outside_ceiling_rejected() {
        let child = vec![cap(
            r#"{"kind":"net.http","hosts":["evil.com"],"methods":[]}"#,
        )];
        let parent = vec![cap(
            r#"{"kind":"net.http","hosts":["api.x.com"],"methods":[]}"#,
        )];
        let v = capability_set_subset(&child, &parent).unwrap_err();
        assert_eq!(v.offender, "evil.com");
    }

    #[test]
    fn process_spawn_command_outside_ceiling_rejected() {
        let child = vec![cap(r#"{"kind":"process.spawn","commands":["rm"]}"#)];
        let parent = vec![cap(r#"{"kind":"process.spawn","commands":["git"]}"#)];
        assert_eq!(
            capability_set_subset(&child, &parent).unwrap_err().offender,
            "rm"
        );
    }

    #[test]
    fn agent_spawn_kind_outside_ceiling_rejected() {
        let child = vec![cap(r#"{"kind":"agent.spawn","allowed_kinds":["root"]}"#)];
        let parent = vec![cap(r#"{"kind":"agent.spawn","allowed_kinds":["worker"]}"#)];
        assert_eq!(
            capability_set_subset(&child, &parent).unwrap_err().offender,
            "root"
        );
    }

    #[test]
    fn skill_spawn_skill_outside_ceiling_rejected() {
        let child = vec![cap(r#"{"kind":"skill.spawn","allowed_skills":["b"]}"#)];
        let parent = vec![cap(r#"{"kind":"skill.spawn","allowed_skills":["a"]}"#)];
        assert_eq!(
            capability_set_subset(&child, &parent).unwrap_err().offender,
            "b"
        );
    }

    #[test]
    fn task_list_mode_within_ceiling_ok_and_exceed_rejected() {
        let read = vec![cap(r#"{"kind":"task_list","mode":"read"}"#)];
        let manage = vec![cap(r#"{"kind":"task_list","mode":"manage"}"#)];
        assert!(capability_set_subset(&read, &manage).is_ok());
        assert!(capability_set_subset(&manage, &read).is_err());
    }

    #[test]
    fn plan_mode_ordering() {
        let read = vec![cap(r#"{"kind":"plan","mode":"read"}"#)];
        let write = vec![cap(r#"{"kind":"plan","mode":"write"}"#)];
        assert!(capability_set_subset(&read, &write).is_ok());
        assert!(capability_set_subset(&write, &read).is_err());
    }

    #[test]
    fn custom_exact_match_ok_param_diff_rejected() {
        let child = vec![cap(r#"{"kind":"mcp.tool.use","tool":"x"}"#)];
        let same = vec![cap(r#"{"kind":"mcp.tool.use","tool":"x"}"#)];
        let diff = vec![cap(r#"{"kind":"mcp.tool.use","tool":"y"}"#)];
        assert!(capability_set_subset(&child, &same).is_ok());
        assert!(capability_set_subset(&child, &diff).is_err());
    }

    #[test]
    fn kind_absent_from_ceiling_is_deny_by_default() {
        let child = vec![cap(r#"{"kind":"net.http","hosts":["x.com"],"methods":[]}"#)];
        let parent = vec![cap(r#"{"kind":"fs.read","paths":["/**"]}"#)];
        let v = capability_set_subset(&child, &parent).unwrap_err();
        assert_eq!(v.kind, "net.http");
        assert_eq!(v.offender, "net.http");
        assert!(v.reason.contains("not in ceiling"), "got {}", v.reason);
    }

    #[test]
    fn paths_subset_admits_glob_child() {
        assert!(paths_subset(&["/proj/src/**".into()], &["/proj/**".into()]).is_ok());
    }

    #[test]
    fn paths_subset_rejects_outside_and_names_offender() {
        let err = paths_subset(&["/etc/**".into()], &["/proj/**".into()]).unwrap_err();
        assert_eq!(err, "/etc/**");
    }

    #[test]
    fn string_set_subset_admits_member() {
        assert!(string_set_subset(&["git".into()], &["git".into(), "rg".into()]).is_ok());
    }

    #[test]
    fn string_set_subset_rejects_nonmember_and_names_offender() {
        let err = string_set_subset(&["rm".into()], &["git".into()]).unwrap_err();
        assert_eq!(err, "rm");
    }

    #[test]
    fn max_bytes_le_lower_ok_higher_err_none_unlimited() {
        assert!(max_bytes_le(1000, Some(5000)).is_ok());
        assert!(max_bytes_le(1000, None).is_ok());
        assert_eq!(
            max_bytes_le(9000, Some(5000)).unwrap_err(),
            "max_bytes=9000"
        );
    }

    // N2 — multi-parent union: fs.write ceiling = max(5000, 8000) = 8000

    #[test]
    fn multi_parent_fs_write_union_ceiling_admits_child_within_max() {
        let child = vec![cap(
            r#"{"kind":"fs.write","paths":["/p/**"],"max_bytes":7000}"#,
        )];
        let parent = vec![
            cap(r#"{"kind":"fs.write","paths":["/p/**"],"max_bytes":5000}"#),
            cap(r#"{"kind":"fs.write","paths":["/p/**"],"max_bytes":8000}"#),
        ];
        assert!(capability_set_subset(&child, &parent).is_ok());
    }

    // N2 — multi-parent union: net.http host admitted by second parent

    #[test]
    fn multi_parent_net_http_union_admits_host_from_second_parent() {
        let child = vec![cap(r#"{"kind":"net.http","hosts":["b.com"],"methods":[]}"#)];
        let parent = vec![
            cap(r#"{"kind":"net.http","hosts":["a.com"],"methods":[]}"#),
            cap(r#"{"kind":"net.http","hosts":["b.com"],"methods":[]}"#),
        ];
        assert!(capability_set_subset(&child, &parent).is_ok());
    }

    // N1 — unknown mode on child yields "unknown mode" reason.
    // Constructed directly (serde folds unknown modes into Custom, bypassing
    // the mode_subset path; we want to exercise mode_rank returning None).

    #[test]
    fn task_list_child_unknown_mode_yields_unknown_mode_reason() {
        let child = vec![Capability::TaskList {
            mode: "bogus".to_string(),
        }];
        let parent = vec![cap(r#"{"kind":"task_list","mode":"manage"}"#)];
        let v = capability_set_subset(&child, &parent).unwrap_err();
        assert!(
            v.reason.contains("unknown mode"),
            "got reason: {}",
            v.reason
        );
    }

    // N1 — unknown mode on parent yields "ceiling mode unknown" reason.

    #[test]
    fn task_list_parent_unknown_mode_yields_ceiling_mode_unknown_reason() {
        let child = vec![cap(r#"{"kind":"task_list","mode":"read"}"#)];
        let parent = vec![Capability::TaskList {
            mode: "bogus".to_string(),
        }];
        let v = capability_set_subset(&child, &parent).unwrap_err();
        assert!(
            v.reason.contains("ceiling mode unknown"),
            "got reason: {}",
            v.reason
        );
    }
}
