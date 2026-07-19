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

use crate::package::capability::lattice::glob::{glob_canon, glob_meet};
use crate::package::capability::lattice::host::host_meet;

/// Greatest lower bound of two capability sets. Total on G2.
pub fn meet(a: &[Capability], b: &[Capability]) -> Vec<Capability> {
    let mut out: Vec<Capability> = Vec::new();
    for ca in a {
        for cb in b {
            if same_kind(ca, cb) {
                if let Some(m) = meet_pair(ca, cb) {
                    push_cap(&mut out, m);
                }
            }
        }
    }
    canon_caps(&out)
}

fn meet_pair(a: &Capability, b: &Capability) -> Option<Capability> {
    use Capability::*;
    match (a, b) {
        (Filesystem(FsCapability::Read { paths: pa }), Filesystem(FsCapability::Read { paths: pb })) => {
            let m = glob_meet(pa, pb);
            (!m.is_empty()).then(|| Filesystem(FsCapability::Read { paths: m }))
        }
        (Filesystem(FsCapability::Exec { paths: pa }), Filesystem(FsCapability::Exec { paths: pb })) => {
            let m = glob_meet(pa, pb);
            (!m.is_empty()).then(|| Filesystem(FsCapability::Exec { paths: m }))
        }
        (
            Filesystem(FsCapability::Write { paths: pa, max_bytes: ma }),
            Filesystem(FsCapability::Write { paths: pb, max_bytes: mb }),
        ) => {
            let m = glob_meet(pa, pb);
            if m.is_empty() { return None; }
            let max_bytes = min_max_bytes(*ma, *mb);
            Some(Filesystem(FsCapability::Write { paths: m, max_bytes }))
        }
        (
            Network(NetCapability::Http { hosts: ha, methods: mea }),
            Network(NetCapability::Http { hosts: hb, methods: meb }),
        ) => {
            let hosts = host_meet(ha, hb);
            let methods = str_intersect(mea, meb);
            (!hosts.is_empty() && !methods.is_empty())
                .then(|| Network(NetCapability::Http { hosts, methods }))
        }
        (Process(ProcessCapability::Spawn { commands: a }), Process(ProcessCapability::Spawn { commands: b })) => {
            let c = str_intersect(a, b);
            (!c.is_empty()).then(|| Process(ProcessCapability::Spawn { commands: c }))
        }
        (Agent(AgentCapability::Spawn { allowed_kinds: a }), Agent(AgentCapability::Spawn { allowed_kinds: b })) => {
            let k = str_intersect(a, b);
            (!k.is_empty()).then(|| Agent(AgentCapability::Spawn { allowed_kinds: k }))
        }
        (Skill(SkillCapability::Spawn { allowed_skills: a }), Skill(SkillCapability::Spawn { allowed_skills: b })) => {
            let s = str_intersect(a, b);
            (!s.is_empty()).then(|| Skill(SkillCapability::Spawn { allowed_skills: s }))
        }
        (TaskList { mode: a }, TaskList { mode: b }) => min_mode(a, b, true).map(|mode| TaskList { mode }),
        (Plan { mode: a }, Plan { mode: b }) => min_mode(a, b, false).map(|mode| Plan { mode }),
        (Custom { name: na, params: pa }, Custom { name: nb, params: pb }) if na == nb && pa == pb => {
            Some(Custom { name: na.clone(), params: pa.clone() })
        }
        _ => None,
    }
}

fn str_intersect(a: &[String], b: &[String]) -> Vec<String> {
    let mut out: Vec<String> = a.iter().filter(|x| b.contains(x)).cloned().collect();
    out.sort();
    out.dedup();
    out
}

fn min_max_bytes(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some(x), Some(y)) => Some(x.min(y)),
    }
}

fn min_mode(a: &str, b: &str, allow_manage: bool) -> Option<String> {
    let ra = mode_rank(a, allow_manage)?;
    let rb = mode_rank(b, allow_manage)?;
    let lo = ra.min(rb);
    Some(match lo { 0 => "read", 1 => "write", _ => "manage" }.to_string())
}

fn push_cap(out: &mut Vec<Capability>, cap: Capability) {
    if !out.contains(&cap) { out.push(cap); }
}

/// Canonical form: same-kind caps merged (their pattern/token lists unioned —
/// the ceiling code's `gather_*` already treats a kind's grants as the union
/// across entries), glob/host/token lists canonicalized, empty (⊥) caps
/// dropped, result ordered so the lattice law holds as structural equality.
pub fn canon_caps(caps: &[Capability]) -> Vec<Capability> {
    // 1. merge every capability of the same kind into one
    let mut merged: Vec<Capability> = Vec::new();
    for c in caps {
        if let Some(existing) = merged.iter_mut().find(|m| same_kind(m, c)) {
            union_into(existing, c);
        } else {
            merged.push(c.clone());
        }
    }
    // 2. canonicalize each merged cap's lists; drop empties (⊥)
    let mut out: Vec<Capability> = Vec::new();
    for c in &merged {
        if let Some(cc) = canon_one(c) {
            out.push(cc);
        }
    }
    out.sort_by(|x, y| kind_str(x).cmp(kind_str(y)).then_with(|| render_cap(x).cmp(&render_cap(y))));
    out
}

/// Union `src`'s grant lists into `dst` (same kind guaranteed by the caller).
fn union_into(dst: &mut Capability, src: &Capability) {
    use Capability::*;
    match (dst, src) {
        (Filesystem(FsCapability::Read { paths: d }), Filesystem(FsCapability::Read { paths: s }))
        | (Filesystem(FsCapability::Exec { paths: d }), Filesystem(FsCapability::Exec { paths: s })) => {
            d.extend(s.iter().cloned());
        }
        (
            Filesystem(FsCapability::Write { paths: d, max_bytes: md }),
            Filesystem(FsCapability::Write { paths: s, max_bytes: ms }),
        ) => {
            d.extend(s.iter().cloned());
            *md = match (*md, *ms) { (None, _) | (_, None) => None, (Some(a), Some(b)) => Some(a.max(b)) };
        }
        (
            Network(NetCapability::Http { hosts: dh, methods: dm }),
            Network(NetCapability::Http { hosts: sh, methods: sm }),
        ) => {
            dh.extend(sh.iter().cloned());
            dm.extend(sm.iter().cloned());
        }
        (Process(ProcessCapability::Spawn { commands: d }), Process(ProcessCapability::Spawn { commands: s })) => {
            d.extend(s.iter().cloned());
        }
        (Agent(AgentCapability::Spawn { allowed_kinds: d }), Agent(AgentCapability::Spawn { allowed_kinds: s })) => {
            d.extend(s.iter().cloned());
        }
        (Skill(SkillCapability::Spawn { allowed_skills: d }), Skill(SkillCapability::Spawn { allowed_skills: s })) => {
            d.extend(s.iter().cloned());
        }
        (TaskList { mode: d }, TaskList { mode: s }) => {
            if mode_rank(s, true) > mode_rank(d, true) { *d = s.clone(); }
        }
        (Plan { mode: d }, Plan { mode: s }) => {
            if mode_rank(s, false) > mode_rank(d, false) { *d = s.clone(); }
        }
        _ => {} // custom: same-kind ⟹ same name+params (same_kind contract) — keep dst
    }
}

/// Canonicalize one merged cap's lists; `None` if its grant list is empty (⊥).
fn canon_one(c: &Capability) -> Option<Capability> {
    use Capability::*;
    fn sorted(v: &[String]) -> Vec<String> { let mut o = v.to_vec(); o.sort(); o.dedup(); o }
    let cc = match c {
        Filesystem(FsCapability::Read { paths }) => Filesystem(FsCapability::Read { paths: glob_canon(paths) }),
        Filesystem(FsCapability::Exec { paths }) => Filesystem(FsCapability::Exec { paths: glob_canon(paths) }),
        Filesystem(FsCapability::Write { paths, max_bytes }) =>
            Filesystem(FsCapability::Write { paths: glob_canon(paths), max_bytes: *max_bytes }),
        Network(NetCapability::Http { hosts, methods }) =>
            Network(NetCapability::Http { hosts: sorted(hosts), methods: sorted(methods) }),
        Process(ProcessCapability::Spawn { commands }) =>
            Process(ProcessCapability::Spawn { commands: sorted(commands) }),
        Agent(AgentCapability::Spawn { allowed_kinds }) =>
            Agent(AgentCapability::Spawn { allowed_kinds: sorted(allowed_kinds) }),
        Skill(SkillCapability::Spawn { allowed_skills }) =>
            Skill(SkillCapability::Spawn { allowed_skills: sorted(allowed_skills) }),
        other => other.clone(),
    };
    let empty = match &cc {
        Filesystem(FsCapability::Read { paths } | FsCapability::Exec { paths } | FsCapability::Write { paths, .. }) =>
            paths.is_empty(),
        Network(NetCapability::Http { hosts, methods }) => hosts.is_empty() || methods.is_empty(),
        Process(ProcessCapability::Spawn { commands }) => commands.is_empty(),
        Agent(AgentCapability::Spawn { allowed_kinds }) => allowed_kinds.is_empty(),
        Skill(SkillCapability::Spawn { allowed_skills }) => allowed_skills.is_empty(),
        _ => false,
    };
    if empty { None } else { Some(cc) }
}

fn render_cap(c: &Capability) -> String {
    // stable string key for ordering; not a wire format
    let mut s = String::from(kind_str(c));
    match c {
        Capability::Filesystem(FsCapability::Read { paths })
        | Capability::Filesystem(FsCapability::Exec { paths })
        | Capability::Filesystem(FsCapability::Write { paths, .. }) => {
            for p in paths { s.push('|'); s.push_str(p); }
        }
        Capability::Network(NetCapability::Http { hosts, methods }) => {
            for h in hosts { s.push('|'); s.push_str(h); }
            for m in methods { s.push('#'); s.push_str(m); }
        }
        Capability::Custom { name, .. } => { s.push('|'); s.push_str(name); }
        _ => {}
    }
    s
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

    #[test]
    fn meet_paths_intersects() {
        let a = vec![read(&["/a/**"])];
        let b = vec![read(&["/*/b/**"])];
        assert_eq!(meet(&a, &b), vec![read(&["/a/b/**"])]);
    }
    #[test]
    fn meet_drops_kind_absent_in_one() {
        let a = vec![read(&["/a/**"])];
        let b = vec![Capability::Process(ProcessCapability::Spawn { commands: vec!["ls".into()] })];
        assert!(meet(&a, &b).is_empty());
    }

    use proptest::prelude::*;

    // small G2 path generator over a fixed alphabet
    fn path_strat() -> impl Strategy<Value = String> {
        let seg = prop_oneof![Just("a"), Just("b"), Just("c"), Just("*")];
        prop::collection::vec(seg, 1..4).prop_flat_map(|segs| {
            prop_oneof![Just(false), Just(true)].prop_map(move |dbl| {
                let mut p = String::new();
                for s in &segs { p.push('/'); p.push_str(s); }
                if dbl { p.push_str("/**"); }
                p
            })
        })
    }
    // One cap of a randomly-chosen kind (Read paths or Process.Spawn commands),
    // so property tests exercise cross-kind AND multi-same-kind (0..3 caps can
    // yield two Reads → the merge path in canon_caps).
    fn cap_strat() -> impl Strategy<Value = Capability> {
        prop_oneof![
            prop::collection::vec(path_strat(), 1..3).prop_map(|p| {
                read(&p.iter().map(|s| s.as_str()).collect::<Vec<_>>())
            }),
            prop::collection::vec(prop_oneof![Just("ls"), Just("cat"), Just("rm")], 1..3).prop_map(|c| {
                Capability::Process(ProcessCapability::Spawn {
                    commands: c.iter().map(|s| s.to_string()).collect(),
                })
            }),
        ]
    }
    fn caps_strat() -> impl Strategy<Value = Vec<Capability>> {
        prop::collection::vec(cap_strat(), 0..3)
    }

    proptest! {
        #[test]
        fn prop_meet_subset_of_both(a in caps_strat(), b in caps_strat()) {
            let m = meet(&a, &b);
            prop_assert!(capability_subset(&m, &a).is_ok());
            prop_assert!(capability_subset(&m, &b).is_ok());
        }
        #[test]
        fn prop_meet_idempotent(a in caps_strat()) {
            prop_assert_eq!(meet(&a, &a), canon_caps(&a));
        }
        #[test]
        fn prop_meet_commutative(a in caps_strat(), b in caps_strat()) {
            prop_assert_eq!(meet(&a, &b), meet(&b, &a));
        }
        #[test]
        fn prop_lattice_law(a in caps_strat(), b in caps_strat()) {
            let subset_ok = capability_subset(&a, &b).is_ok();
            let meet_eq = meet(&a, &b) == canon_caps(&a);
            prop_assert_eq!(subset_ok, meet_eq);
        }
    }
}
