//! Sound capability lattice (D3). Structurally-decidable subset + meet over
//! the restricted G2 glob grammar. See
//! docs/superpowers/specs/2026-07-19-d3-sound-capability-lattice-design.md.
pub mod glob;
pub mod host;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::{
    AgentCapability, Capability, FsCapability, NetCapability, ProcessCapability, SkillCapability,
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
pub fn capability_subset(
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
            crate::package::capability::lattice::glob::glob_subset_set(paths, &pp)
                .map_err(|o| (o, "not a subset of any allowed path".into()))
        }
        Capability::Filesystem(FsCapability::Write {
            paths, max_bytes, ..
        }) => {
            let pp = gather_paths(parents);
            crate::package::capability::lattice::glob::glob_subset_set(paths, &pp)
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
        Capability::Network(NetCapability::Http { hosts, .. }) => {
            let ph = gather_hosts(parents);
            crate::package::capability::lattice::host::host_subset_set(hosts, &ph)
                .map_err(|o| (o, "host not in ceiling".into()))
        }
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
/// `capability_subset`); this gathers Read/Write/Exec paths indiscriminately.
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

fn gather_hosts(parents: &[&Capability]) -> Vec<String> {
    parents
        .iter()
        .filter_map(|p| match p {
            Capability::Network(NetCapability::Http { hosts, .. }) => Some(hosts.clone()),
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
    let mut acc: u64 = 0;
    for p in parents {
        if let Capability::Filesystem(FsCapability::Write { max_bytes, .. }) = p {
            // Any unlimited parent makes the whole ceiling unlimited.
            let m = (*max_bytes)?;
            acc = acc.max(m);
        }
    }
    Some(acc)
}

/// Exact-set inclusion: every `child` entry equals some `parent` entry.
/// `Err(offender)` names the first child entry not present in `parent`.
fn string_set_subset(child: &[String], parent: &[String]) -> Result<(), String> {
    for c in child {
        if !parent.iter().any(|p| p == c) {
            return Err(c.clone());
        }
    }
    Ok(())
}

/// `max_bytes` tightening: `child <= parent`. `parent == None` means the
/// ceiling is unlimited (any child is admitted). `Err` carries the child value.
fn max_bytes_le(child: u64, parent: Option<u64>) -> Result<(), String> {
    match parent {
        None => Ok(()),
        Some(max) if child <= max => Ok(()),
        Some(_) => Err(format!("max_bytes={child}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Capability, FsCapability, NetCapability, ProcessCapability};
    use alloc::vec;

    fn read(paths: &[&str]) -> Capability {
        Capability::Filesystem(FsCapability::Read {
            paths: paths.iter().map(|s| s.to_string()).collect(),
        })
    }

    #[test]
    fn path_child_within_ceiling_ok() {
        let child = vec![read(&["/proj/src/**"])];
        let parent = vec![read(&["/proj/**"])];
        assert!(capability_subset(&child, &parent).is_ok());
    }
    #[test]
    fn path_child_outside_ceiling_violates() {
        let child = vec![read(&["/etc/**"])];
        let parent = vec![read(&["/proj/**"])];
        let v = capability_subset(&child, &parent).unwrap_err();
        assert_eq!(v.kind, "fs.read");
        assert_eq!(v.offender, "/etc/**");
    }
    #[test]
    fn kind_not_in_ceiling_violates() {
        let child = vec![Capability::Process(ProcessCapability::Spawn {
            commands: vec!["ls".into()],
        })];
        let parent = vec![read(&["/proj/**"])];
        let v = capability_subset(&child, &parent).unwrap_err();
        assert_eq!(v.reason, "kind not in ceiling");
    }
    #[test]
    fn host_child_within_ceiling_ok() {
        let child = vec![Capability::Network(NetCapability::Http {
            hosts: vec!["api.example.com".into()],
            methods: vec!["GET".into()],
        })];
        let parent = vec![Capability::Network(NetCapability::Http {
            hosts: vec!["*.example.com".into()],
            methods: vec!["GET".into()],
        })];
        assert!(capability_subset(&child, &parent).is_ok());
    }
    // The real sampling-era witness: the old sampler expanded `*` to a fixed
    // "seed" and wrongly admitted `/proj/*` under `/proj/seed*`. The parent
    // `/proj/seed*` is intra-segment → outside G2 → now correctly denied.
    #[test]
    fn intra_segment_ceiling_now_fails_closed() {
        let child = vec![read(&["/proj/*"])];
        let parent = vec![read(&["/proj/seed*"])];
        assert!(capability_subset(&child, &parent).is_err());
    }
}
