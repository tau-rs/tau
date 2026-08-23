//! Sound capability lattice (D3). Structurally-decidable subset + meet over
//! the restricted G2 glob grammar. See
//! docs/superpowers/specs/2026-07-19-d3-sound-capability-lattice-design.md.
pub mod glob;
pub mod host;

use crate::package::host::{HostSet, HttpMethod};
use crate::{
    AgentCapability, Capability, FsCapability, NetCapability, ProcessCapability, SkillCapability,
};
use alloc::collections::BTreeSet;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Concrete method set: `None` (the top element, "all methods") expands to the
/// full nine-verb universe so the sound per-method coverage check below can
/// iterate a finite set.
fn method_set(m: &Option<BTreeSet<HttpMethod>>) -> BTreeSet<HttpMethod> {
    match m {
        None => HttpMethod::ALL.iter().copied().collect(),
        Some(s) => s.clone(),
    }
}

/// Meet (intersection) of two method grants, preserving `None` (= all) as the
/// top element — so `meet(a, a)` structurally equals `canon(a)` for a `None`
/// grant (the lattice idempotence law).
fn method_meet(
    a: &Option<BTreeSet<HttpMethod>>,
    b: &Option<BTreeSet<HttpMethod>>,
) -> Option<BTreeSet<HttpMethod>> {
    match (a, b) {
        (None, x) | (x, None) => x.clone(),
        (Some(sa), Some(sb)) => Some(sa.intersection(sb).copied().collect()),
    }
}

/// A method grant denotes ⊥ iff it is `Some(∅)` (deny all). `None` (= all
/// methods) is never empty.
fn method_is_empty(m: &Option<BTreeSet<HttpMethod>>) -> bool {
    matches!(m, Some(s) if s.is_empty())
}

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
        // Forward-compat escape hatch: opaque like `Custom`, refined by
        // (kind + params) in `same_kind`/`cap_subset_against` (fail-closed).
        Capability::Forward { .. } => "forward",
    }
}

/// Same kind for matching purposes. `Custom` matches `Custom` only when the
/// `name` matches. `"unknown"` never matches (fail-closed).
fn same_kind(a: &Capability, b: &Capability) -> bool {
    match (a, b) {
        (Capability::Custom { name: na, .. }, Capability::Custom { name: nb, .. }) => na == nb,
        (Capability::Custom { .. }, _) | (_, Capability::Custom { .. }) => false,
        (Capability::Forward { kind: ka, .. }, Capability::Forward { kind: kb, .. }) => ka == kb,
        (Capability::Forward { .. }, _) | (_, Capability::Forward { .. }) => false,
        _ => {
            let k = kind_str(a);
            k != "unknown" && k == kind_str(b)
        }
    }
}

/// Whether two caps fold into one in `canon_caps`. Single-dimension kinds
/// (read/exec paths, token sets, modes) merge by kind because unioning their
/// one dimension is language-preserving. The two-dimensional kinds — `net.http`
/// (hosts × methods) and `fs.write` (paths × max_bytes) — must NOT fold, since
/// unioning each dimension independently is a bounding-box that widens the
/// grant; distinct rectangles stay separate and are absorbed by 2-D containment
/// in `canon_caps`. `Custom` folds only on identical (name+params).
fn mergeable(a: &Capability, b: &Capability) -> bool {
    match (a, b) {
        (
            Capability::Custom {
                name: na,
                params: pa,
            },
            Capability::Custom {
                name: nb,
                params: pb,
            },
        ) => na == nb && pa == pb,
        (
            Capability::Forward {
                kind: ka,
                params: pa,
            },
            Capability::Forward {
                kind: kb,
                params: pb,
            },
        ) => ka == kb && pa == pb,
        // Multi-dimensional rectangles never fold (would bounding-box).
        (
            Capability::Network(NetCapability::Http { .. }),
            Capability::Network(NetCapability::Http { .. }),
        )
        | (
            Capability::Filesystem(FsCapability::Write { .. }),
            Capability::Filesystem(FsCapability::Write { .. }),
        ) => false,
        _ => same_kind(a, b),
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
            // Sound joint coverage: each child path must be writable up to the
            // child's max_bytes by a SINGLE parent entry (path + byte-limit from
            // the same grant). Restrict to parent entries whose limit covers the
            // child's (None = unlimited), then require path coverage against
            // exactly those entries' paths.
            let eligible: Vec<String> = parents
                .iter()
                .filter_map(|p| match p {
                    Capability::Filesystem(FsCapability::Write {
                        paths: pp,
                        max_bytes: pb,
                    }) => {
                        let covers = match (pb, max_bytes) {
                            (None, _) => true,        // parent unlimited covers any child
                            (Some(_), None) => false, // child unlimited, parent capped
                            (Some(pbv), Some(cbv)) => pbv >= cbv,
                        };
                        if covers {
                            Some(pp.clone())
                        } else {
                            None
                        }
                    }
                    _ => None,
                })
                .flatten()
                .collect();
            crate::package::capability::lattice::glob::glob_subset_set(paths, &eligible)
                .map_err(|o| (o, "not writable within max_bytes in ceiling".to_string()))
        }
        Capability::Network(NetCapability::Http { hosts, methods }) => {
            // Sound joint coverage: for each method the child requests, its hosts
            // must be granted by parent entries that ALSO grant that method — a
            // single grant must cover both dimensions, not the bounding box of
            // independently-unioned hosts × methods. `methods = None` (all) is
            // expanded to the finite nine-verb universe (see `method_set`).
            use crate::package::capability::lattice::host;
            for m in method_set(methods) {
                let union = host::host_union(parents.iter().filter_map(|p| match p {
                    Capability::Network(NetCapability::Http {
                        hosts: ph,
                        methods: pm,
                    }) if method_set(pm).contains(&m) => Some(ph),
                    _ => None,
                }));
                if !union.subsumes(hosts) {
                    let offender = host::host_offender(hosts, &union);
                    let reason = format!(
                        "host {offender} not granted for method {} in ceiling",
                        m.as_str()
                    );
                    return Err((offender, reason));
                }
            }
            Ok(())
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
        // Forward-compat escape hatch: opaque, so subset only holds against an
        // identical (kind + params) parent Forward — fail-closed otherwise.
        Capability::Forward { kind, params } => {
            let ok = parents.iter().any(|p| {
                matches!(p, Capability::Forward { kind: pk, params: pp } if pk == kind && pp == params)
            });
            if ok {
                Ok(())
            } else {
                Err((kind.clone(), "forward params do not match ceiling".into()))
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

use crate::package::capability::lattice::glob::{glob_canon, glob_meet};
use crate::package::capability::lattice::host::{host_canon, host_is_empty, host_meet};

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
        (
            Filesystem(FsCapability::Read { paths: pa }),
            Filesystem(FsCapability::Read { paths: pb }),
        ) => {
            let m = glob_meet(pa, pb);
            (!m.is_empty()).then_some(Filesystem(FsCapability::Read { paths: m }))
        }
        (
            Filesystem(FsCapability::Exec { paths: pa }),
            Filesystem(FsCapability::Exec { paths: pb }),
        ) => {
            let m = glob_meet(pa, pb);
            (!m.is_empty()).then_some(Filesystem(FsCapability::Exec { paths: m }))
        }
        (
            Filesystem(FsCapability::Write {
                paths: pa,
                max_bytes: ma,
            }),
            Filesystem(FsCapability::Write {
                paths: pb,
                max_bytes: mb,
            }),
        ) => {
            let m = glob_meet(pa, pb);
            if m.is_empty() {
                return None;
            }
            let max_bytes = min_max_bytes(*ma, *mb);
            Some(Filesystem(FsCapability::Write {
                paths: m,
                max_bytes,
            }))
        }
        (
            Network(NetCapability::Http {
                hosts: ha,
                methods: mea,
            }),
            Network(NetCapability::Http {
                hosts: hb,
                methods: meb,
            }),
        ) => {
            let hosts = host_meet(ha, hb);
            let methods = method_meet(mea, meb);
            (!host_is_empty(&hosts) && !method_is_empty(&methods))
                .then_some(Network(NetCapability::Http { hosts, methods }))
        }
        (
            Process(ProcessCapability::Spawn { commands: a }),
            Process(ProcessCapability::Spawn { commands: b }),
        ) => {
            let c = str_intersect(a, b);
            (!c.is_empty()).then_some(Process(ProcessCapability::Spawn { commands: c }))
        }
        (
            Agent(AgentCapability::Spawn { allowed_kinds: a }),
            Agent(AgentCapability::Spawn { allowed_kinds: b }),
        ) => {
            let k = str_intersect(a, b);
            (!k.is_empty()).then_some(Agent(AgentCapability::Spawn { allowed_kinds: k }))
        }
        (
            Skill(SkillCapability::Spawn { allowed_skills: a }),
            Skill(SkillCapability::Spawn { allowed_skills: b }),
        ) => {
            let s = str_intersect(a, b);
            (!s.is_empty()).then_some(Skill(SkillCapability::Spawn { allowed_skills: s }))
        }
        (TaskList { mode: a }, TaskList { mode: b }) => {
            min_mode(a, b, true).map(|mode| TaskList { mode })
        }
        (Plan { mode: a }, Plan { mode: b }) => min_mode(a, b, false).map(|mode| Plan { mode }),
        // Fully-qualified `Capability::Custom` (not the bare `use`d form) so the
        // escape-hatch registry scanner doesn't mistake these match patterns for
        // an undocumented variant declaration once rustfmt puts them on their own
        // line. The documented declaration lives on the enum in capability.rs.
        (
            Capability::Custom {
                name: na,
                params: pa,
            },
            Capability::Custom {
                name: nb,
                params: pb,
            },
        ) if na == nb && pa == pb => Some(Capability::Custom {
            name: na.clone(),
            params: pa.clone(),
        }),
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
    Some(
        match lo {
            0 => "read",
            1 => "write",
            _ => "manage",
        }
        .to_string(),
    )
}

fn push_cap(out: &mut Vec<Capability>, cap: Capability) {
    if !out.contains(&cap) {
        out.push(cap);
    }
}

/// Canonical form of a capability set. Single-dimension same-kind caps are
/// merged (union is language-preserving); two-dimensional kinds (`net.http`,
/// `fs.write`) are kept as separate rectangles and absorbed by 2-D containment
/// rather than bounding-box-merged. Internal glob/host/token lists are
/// canonicalized, empty (⊥) caps dropped, and the result is a deterministically
/// ordered antichain — so `meet` never widens and the lattice law holds as
/// structural equality.
pub fn canon_caps(caps: &[Capability]) -> Vec<Capability> {
    // 1. fold single-dimension same-kind caps; multi-dimensional rectangles and
    //    distinct-params customs stay separate (see `mergeable`).
    let mut merged: Vec<Capability> = Vec::new();
    for c in caps {
        if let Some(existing) = merged.iter_mut().find(|m| mergeable(m, c)) {
            union_into(existing, c);
        } else {
            merged.push(c.clone());
        }
    }
    // 2. canonicalize each cap's internal lists; drop empties (⊥).
    let canon: Vec<Capability> = merged.iter().filter_map(canon_one).collect();
    // 3. absorb: drop any cap that is a subset of another (2-D containment for
    //    http/write rectangles; a no-op for the folded single-entry kinds). The
    //    `j < i` guard keeps exactly one survivor for an equal pair.
    let mut kept: Vec<Capability> = Vec::new();
    'outer: for (i, ci) in canon.iter().enumerate() {
        for (j, cj) in canon.iter().enumerate() {
            if i != j && cap_contained(ci, cj) && !(cap_contained(cj, ci) && j < i) {
                continue 'outer;
            }
        }
        kept.push(ci.clone());
    }
    // 4. deterministic total order.
    kept.sort_by(|x, y| {
        kind_str(x)
            .cmp(kind_str(y))
            .then_with(|| render_cap(x).cmp(&render_cap(y)))
    });
    kept
}

/// `inner ⊆ outer` for two single capabilities, via the sound per-kind subset
/// (2-D rectangle containment for http/write). Used by `canon_caps`'s absorb.
fn cap_contained(inner: &Capability, outer: &Capability) -> bool {
    capability_subset(core::slice::from_ref(inner), core::slice::from_ref(outer)).is_ok()
}

/// Union `src`'s grant list into `dst` for a **single-dimension** kind (same
/// kind guaranteed by `mergeable`). The two-dimensional kinds (`net.http`,
/// `fs.write`) are intentionally ABSENT: unioning each of their dimensions
/// independently is a bounding-box that widens the grant, so `mergeable` never
/// routes them here — they stay as separate rectangles and are absorbed by 2-D
/// containment in `canon_caps`. Adding an http/write arm here would reintroduce
/// the unsound widening.
fn union_into(dst: &mut Capability, src: &Capability) {
    use Capability::*;
    match (dst, src) {
        (
            Filesystem(FsCapability::Read { paths: d }),
            Filesystem(FsCapability::Read { paths: s }),
        )
        | (
            Filesystem(FsCapability::Exec { paths: d }),
            Filesystem(FsCapability::Exec { paths: s }),
        ) => {
            d.extend(s.iter().cloned());
        }
        (
            Process(ProcessCapability::Spawn { commands: d }),
            Process(ProcessCapability::Spawn { commands: s }),
        ) => {
            d.extend(s.iter().cloned());
        }
        (
            Agent(AgentCapability::Spawn { allowed_kinds: d }),
            Agent(AgentCapability::Spawn { allowed_kinds: s }),
        ) => {
            d.extend(s.iter().cloned());
        }
        (
            Skill(SkillCapability::Spawn { allowed_skills: d }),
            Skill(SkillCapability::Spawn { allowed_skills: s }),
        ) => {
            d.extend(s.iter().cloned());
        }
        (TaskList { mode: d }, TaskList { mode: s }) if mode_rank(s, true) > mode_rank(d, true) => {
            *d = s.clone();
        }
        (Plan { mode: d }, Plan { mode: s }) if mode_rank(s, false) > mode_rank(d, false) => {
            *d = s.clone();
        }
        // custom (same name+params per mergeable) and the lower-mode cases keep dst.
        _ => {}
    }
}

/// Canonicalize one merged cap's lists; `None` if its grant list is empty (⊥).
fn canon_one(c: &Capability) -> Option<Capability> {
    use Capability::*;
    fn sorted(v: &[String]) -> Vec<String> {
        let mut o = v.to_vec();
        o.sort();
        o.dedup();
        o
    }
    let cc = match c {
        Filesystem(FsCapability::Read { paths }) => Filesystem(FsCapability::Read {
            paths: glob_canon(paths),
        }),
        Filesystem(FsCapability::Exec { paths }) => Filesystem(FsCapability::Exec {
            paths: glob_canon(paths),
        }),
        Filesystem(FsCapability::Write { paths, max_bytes }) => Filesystem(FsCapability::Write {
            paths: glob_canon(paths),
            max_bytes: *max_bytes,
        }),
        Network(NetCapability::Http { hosts, methods }) => Network(NetCapability::Http {
            hosts: host_canon(hosts),
            // `Option<BTreeSet<HttpMethod>>` is already canonical: `None` (all)
            // stays `None`; a `BTreeSet` is sorted+deduped.
            methods: methods.clone(),
        }),
        Process(ProcessCapability::Spawn { commands }) => Process(ProcessCapability::Spawn {
            commands: sorted(commands),
        }),
        Agent(AgentCapability::Spawn { allowed_kinds }) => Agent(AgentCapability::Spawn {
            allowed_kinds: sorted(allowed_kinds),
        }),
        Skill(SkillCapability::Spawn { allowed_skills }) => Skill(SkillCapability::Spawn {
            allowed_skills: sorted(allowed_skills),
        }),
        other => other.clone(),
    };
    let empty = match &cc {
        Filesystem(
            FsCapability::Read { paths }
            | FsCapability::Exec { paths }
            | FsCapability::Write { paths, .. },
        ) => paths.is_empty(),
        Network(NetCapability::Http { hosts, methods }) => {
            host_is_empty(hosts) || method_is_empty(methods)
        }
        Process(ProcessCapability::Spawn { commands }) => commands.is_empty(),
        Agent(AgentCapability::Spawn { allowed_kinds }) => allowed_kinds.is_empty(),
        Skill(SkillCapability::Spawn { allowed_skills }) => allowed_skills.is_empty(),
        _ => false,
    };
    if empty {
        None
    } else {
        Some(cc)
    }
}

fn render_cap(c: &Capability) -> String {
    // stable string key for ordering; not a wire format
    let mut s = String::from(kind_str(c));
    match c {
        Capability::Filesystem(FsCapability::Read { paths })
        | Capability::Filesystem(FsCapability::Exec { paths }) => {
            for p in paths {
                s.push('|');
                s.push_str(p);
            }
        }
        Capability::Filesystem(FsCapability::Write { paths, max_bytes }) => {
            for p in paths {
                s.push('|');
                s.push_str(p);
            }
            match max_bytes {
                Some(n) => {
                    s.push('~');
                    s.push_str(&n.to_string());
                }
                None => s.push_str("~*"),
            }
        }
        Capability::Network(NetCapability::Http { hosts, methods }) => {
            match hosts {
                HostSet::Any => s.push_str("|any"),
                HostSet::Exact(hs) => {
                    for h in hs {
                        s.push('|');
                        s.push_str(h.as_str());
                    }
                }
            }
            match methods {
                None => s.push_str("#*"),
                Some(ms) => {
                    for m in ms {
                        s.push('#');
                        s.push_str(m.as_str());
                    }
                }
            }
        }
        Capability::Custom { name, .. } => {
            s.push('|');
            s.push_str(name);
        }
        _ => {}
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::host::{HostName, HostSet, HttpMethod};
    use crate::{Capability, FsCapability, NetCapability, ProcessCapability, Value};
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
        // Suffix globs are gone (ADR-0064); the wide ceiling is `Any`, which
        // subsumes an exact child host.
        let child = vec![http(&["api.example.com"], &["GET"])];
        let parent = vec![http(&["any"], &["GET"])];
        assert!(capability_subset(&child, &parent).is_ok());
    }

    #[test]
    fn exact_host_outside_exact_ceiling_violates() {
        let child = vec![http(&["evil.com"], &["GET"])];
        let parent = vec![http(&["api.example.com"], &["GET"])];
        let v = capability_subset(&child, &parent).unwrap_err();
        assert_eq!(v.kind, "net.http");
        assert_eq!(v.offender, "evil.com");
    }

    #[test]
    fn exact_child_not_under_any_child_ceiling() {
        // `Any` child needs an `Any` grant for the method; an exact ceiling
        // cannot subsume it.
        let child = vec![http(&["any"], &["GET"])];
        let parent = vec![http(&["api.example.com"], &["GET"])];
        assert!(capability_subset(&child, &parent).is_err());
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
        let b = vec![Capability::Process(ProcessCapability::Spawn {
            commands: vec!["ls".into()],
        })];
        assert!(meet(&a, &b).is_empty());
    }

    #[test]
    fn law_holds_for_net_http_methods() {
        // Regression: subset once ignored methods → admitted GET under a
        // POST-only ceiling, diverging from meet (which intersects methods).
        let a = vec![http(&["api.example.com"], &["GET"])];
        let b = vec![http(&["api.example.com"], &["POST"])];
        assert!(capability_subset(&a, &b).is_err()); // methods now enforced
        assert!(meet(&a, &b).is_empty()); // methods disjoint → cap dropped
    }

    #[test]
    fn canon_keeps_distinct_params_customs() {
        use alloc::collections::BTreeMap;
        // p1 empty, p2 has one entry ⟹ params differ (the Value is irrelevant).
        let p1: BTreeMap<String, Value> = BTreeMap::new();
        let mut p2: BTreeMap<String, Value> = BTreeMap::new();
        p2.insert("k".into(), Value::Bool(true));
        let a = vec![
            Capability::Custom {
                name: "x".into(),
                params: p1.clone(),
            },
            Capability::Custom {
                name: "x".into(),
                params: p2,
            },
        ];
        assert_eq!(canon_caps(&a).len(), 2); // distinct params ⟹ not merged
        let b = vec![Capability::Custom {
            name: "x".into(),
            params: p1,
        }];
        assert!(capability_subset(&a, &b).is_err());
    }

    fn http(hosts: &[&str], methods: &[&str]) -> Capability {
        // `["any"]` → HostSet::Any; else an exact host set. Methods are always
        // an explicit `Some(set)` here (tests that need "all methods" build the
        // cap with `methods: None` directly).
        let hosts = if hosts == ["any"] {
            HostSet::Any
        } else {
            HostSet::Exact(hosts.iter().map(|h| HostName::parse(h).unwrap()).collect())
        };
        Capability::Network(NetCapability::Http {
            hosts,
            methods: Some(
                methods
                    .iter()
                    .map(|m| HttpMethod::parse(m).unwrap())
                    .collect(),
            ),
        })
    }
    fn write(paths: &[&str], max_bytes: Option<u64>) -> Capability {
        Capability::Filesystem(FsCapability::Write {
            paths: paths.iter().map(|s| s.to_string()).collect(),
            max_bytes,
        })
    }

    #[test]
    fn canon_keeps_incomparable_http_rectangles() {
        // a×GET and b×POST are incomparable → both survive (NOT
        // bounding-boxed into {a,b}×{GET,POST}).
        let a = vec![
            http(&["a.example.com"], &["GET"]),
            http(&["b.example.com"], &["POST"]),
        ];
        assert_eq!(canon_caps(&a).len(), 2);
    }

    #[test]
    fn canon_absorbs_contained_http_rectangle() {
        // a×GET ⊆ {a,b}×{GET,POST} → absorbed to the single wider rectangle.
        let a = vec![
            http(&["a.example.com"], &["GET"]),
            http(&["a.example.com", "b.example.com"], &["GET", "POST"]),
        ];
        assert_eq!(
            canon_caps(&a),
            vec![http(&["a.example.com", "b.example.com"], &["GET", "POST"])]
        );
    }

    #[test]
    fn subset_and_meet_sound_for_multientry_http() {
        // Ceiling grants (a, GET) and (b, POST) — never (b, GET).
        let a = vec![http(&["a.example.com", "b.example.com"], &["GET"])];
        let b = vec![
            http(&["a.example.com"], &["GET"]),
            http(&["b.example.com"], &["POST"]),
        ];
        // subset: for GET the ceiling only grants host a → child's b.example.com
        // is uncovered → denied (no bounding-box union across methods).
        assert!(capability_subset(&a, &b).is_err());
        // meet must NOT widen: the only overlap is a.example.com × GET.
        assert_eq!(meet(&a, &b), vec![http(&["a.example.com"], &["GET"])]);
    }

    #[test]
    fn subset_sound_for_multientry_write() {
        // child wants /a/** up to 1000 bytes.
        let a = vec![write(&["/a/**"], Some(1000))];
        // ceiling: /a/** capped at 100, /b/** at 1000 — no single entry grants
        // /a/** at 1000 → denied.
        let b = vec![write(&["/a/**"], Some(100)), write(&["/b/**"], Some(1000))];
        assert!(capability_subset(&a, &b).is_err());
        // a single entry that does cover it → admitted.
        assert!(capability_subset(&a, &[write(&["/a/**"], Some(1000))]).is_ok());
    }

    use proptest::prelude::*;

    // small G2 path generator over a fixed alphabet
    fn path_strat() -> impl Strategy<Value = String> {
        let seg = prop_oneof![Just("a"), Just("b"), Just("c"), Just("*")];
        prop::collection::vec(seg, 1..4).prop_flat_map(|segs| {
            prop_oneof![Just(false), Just(true)].prop_map(move |dbl| {
                let mut p = String::new();
                for s in &segs {
                    p.push('/');
                    p.push_str(s);
                }
                if dbl {
                    p.push_str("/**");
                }
                p
            })
        })
    }
    // One cap of a randomly-chosen kind (Read paths or Process.Spawn commands),
    // so property tests exercise cross-kind AND multi-same-kind (0..3 caps can
    // yield two Reads → the merge path in canon_caps).
    fn cap_strat() -> impl Strategy<Value = Capability> {
        prop_oneof![
            prop::collection::vec(path_strat(), 1..3)
                .prop_map(|p| { read(&p.iter().map(|s| s.as_str()).collect::<Vec<_>>()) }),
            prop::collection::vec(prop_oneof![Just("ls"), Just("cat"), Just("rm")], 1..3).prop_map(
                |c| {
                    Capability::Process(ProcessCapability::Spawn {
                        commands: c.iter().map(|s| s.to_string()).collect(),
                    })
                }
            ),
            // net.http: two-dimensional (hosts × methods). Exercises BOTH top
            // elements (`HostSet::Any`, `methods = None`) and finite `Exact` /
            // `Some` sets, so the antichain / joint-coverage soundness is tested
            // against the widest grants too.
            (
                prop_oneof![
                    Just(HostSet::Any),
                    prop::collection::vec(
                        prop_oneof![Just("a.example.com"), Just("b.example.com")],
                        1..3
                    )
                    .prop_map(|hs| HostSet::Exact(
                        hs.iter().map(|h| HostName::parse(h).unwrap()).collect()
                    )),
                ],
                prop_oneof![
                    Just(None),
                    prop::collection::vec(
                        prop_oneof![Just(HttpMethod::Get), Just(HttpMethod::Post)],
                        1..3
                    )
                    .prop_map(|ms| Some(ms.into_iter().collect())),
                ],
            )
                .prop_map(|(hosts, methods)| {
                    Capability::Network(NetCapability::Http { hosts, methods })
                }),
            // fs.write: two-dimensional (paths × max_bytes), exercises the
            // write antichain / max_bytes-filtered subset.
            (
                prop::collection::vec(path_strat(), 1..3),
                prop_oneof![Just(None), Just(Some(100u64)), Just(Some(1000u64))],
            )
                .prop_map(|(paths, max_bytes)| {
                    Capability::Filesystem(FsCapability::Write {
                        paths: paths.iter().map(|s| s.to_string()).collect(),
                        max_bytes,
                    })
                }),
        ]
    }
    // 0..4 so a single set can carry TWO http (or two write) entries → the
    // multi-entry rectangle-antichain path in canon/meet/subset gets sampled.
    fn caps_strat() -> impl Strategy<Value = Vec<Capability>> {
        prop::collection::vec(cap_strat(), 0..4)
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
            // Lattice equality is language-equivalence (mutual subset), NOT Rust
            // structural ==: a union of 2-D rectangles (net.http hosts×methods,
            // fs.write paths×max_bytes) has no unique minimal cover, so meet(a,b)
            // and canon_caps(a) can denote the same language via different covers
            // (e.g. {api×{GET,POST}} vs {api×GET, api×POST}). The lattice law is
            // `a ⊑ b  ⟺  a ⊓ b = a` under that equality.
            let subset_ok = capability_subset(&a, &b).is_ok();
            let m = meet(&a, &b);
            let meet_lang_eq_a =
                capability_subset(&m, &a).is_ok() && capability_subset(&a, &m).is_ok();
            prop_assert_eq!(subset_ok, meet_lang_eq_a);
        }
    }
}
