//! Capability satisfies-relation. Determines whether an agent's
//! granted capabilities cover the capabilities required by a tool
//! invocation. Pure logic, no I/O.
//!
//! All helpers are `pub(crate)` — capability satisfaction is a kernel-
//! internal concern (used by the dispatcher to gate tool invocations);
//! it is intentionally not part of the public `tau-runtime-core` API.
//!
//! # Conservatism note
//!
//! v0.1 chooses safe-conservative defaults at every ambiguous edge:
//! a bounded grant cannot satisfy an unbounded request; an unknown
//! `Custom` namespace requires exact key+value parity. Tightening
//! later is non-breaking; loosening later is a security regression.
//!
//! # Dead-code allow
//!
//! Most helpers in this module are exercised both by the in-module
//! `tests` submodule and by the runtime agent run loop ([`crate::run`],
//! Task 10). A few internals (e.g. the per-namespace `*_satisfies`
//! helpers) are reached only transitively through
//! [`capability_satisfies`] and would warn under the
//! `dead_code` lint when their direct callers are limited to tests; we
//! keep the module-level `allow` to suppress those rather than
//! one-by-one annotations.

#![allow(dead_code)]

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use tau_domain::{
    AgentCapability, Capability, FsCapability, NetCapability, NetHosts, ProcessCapability,
    SkillCapability, Value,
};

/// Returns `true` iff `granted` covers `required` for one capability pair.
///
/// Different namespaces never satisfy one another — a `Filesystem(Read)`
/// grant does not cover a `Filesystem(Write)` request, nor does any
/// `Filesystem` grant cover a `Network` request.
pub fn capability_satisfies(granted: &Capability, required: &Capability) -> bool {
    match (granted, required) {
        (Capability::Filesystem(g), Capability::Filesystem(r)) => fs_satisfies(g, r),
        (Capability::Network(g), Capability::Network(r)) => net_satisfies(g, r),
        (Capability::Process(g), Capability::Process(r)) => process_satisfies(g, r),
        (Capability::Agent(g), Capability::Agent(r)) => agent_satisfies(g, r),
        (Capability::TaskList { mode: g }, Capability::TaskList { mode: r }) => {
            task_list_satisfies(g, r)
        }
        (Capability::Plan { mode: g }, Capability::Plan { mode: r }) => plan_satisfies(g, r),
        (Capability::Skill(g), Capability::Skill(r)) => skill_satisfies(g, r),
        (
            Capability::Custom {
                name: gn,
                params: gp,
            },
            Capability::Custom {
                name: rn,
                params: rp,
            },
        ) => gn == rn && custom_params_satisfy(gp, rp),
        _ => false,
    }
}

/// Top-level check: every required capability must be satisfied by at
/// least one grant. Returns `Some(&first_missing)` on the first
/// required capability with no covering grant, or `None` if every
/// required is covered.
pub fn check_capabilities<'a>(
    granted: &[Capability],
    required: &'a [Capability],
) -> Option<&'a Capability> {
    required
        .iter()
        .find(|req| !granted.iter().any(|g| capability_satisfies(g, req)))
}

/// Tool-dispatch wrapper around [`check_capabilities`] that owns the
/// ADR-0006 §3.9 `capability.check` span and the five capability
/// lifecycle events.
///
/// Lives in core (vs. the host shell's identical helper, pre-β.1.3.5c)
/// because `stream.rs` — also now in core — needs to call it during
/// per-tool dispatch. The pure `check_capabilities` is reachable from
/// build-time tool-spec filtering; this wrapper is the dispatch-site
/// wrapper that emits the per-tool tracing vocabulary.
#[tracing::instrument(
    name = "capability.check",
    skip_all,
    fields(tool_name = %tool_name),
)]
pub fn check_capabilities_for_tool<'a>(
    tool_name: &str,
    granted: &[Capability],
    required: &'a [Capability],
) -> Option<&'a Capability> {
    use crate::vocabulary as v;
    tracing::debug!(
        name = v::EV_CAPABILITY_REQUIRED_LOADED,
        required_count = required.len(),
    );
    tracing::debug!(
        name = v::EV_CAPABILITY_GRANTED_LOADED,
        granted_count = granted.len(),
    );
    let missing = check_capabilities(granted, required);
    tracing::debug!(
        name = v::EV_CAPABILITY_SATISFIES_CHECK,
        satisfied = missing.is_none(),
    );
    match missing {
        None => {
            tracing::info!(
                name = v::EV_CAPABILITY_ALLOW,
                tool_name = %tool_name,
            );
            None
        }
        Some(cap) => {
            let kind = capability_kind_str(cap);
            tracing::warn!(
                name = v::EV_CAPABILITY_DENY,
                tool_name = %tool_name,
                missing_kind = %kind,
            );
            Some(cap)
        }
    }
}

/// Top-level capability kind string used in
/// `CapabilityDenial::required_kind` and the `capability.deny` event.
pub fn capability_kind_str(cap: &Capability) -> String {
    use tau_domain::{
        AgentCapability, FsCapability, NetCapability, ProcessCapability, SkillCapability,
    };
    match cap {
        Capability::Filesystem(FsCapability::Read { .. }) => "fs.read".into(),
        Capability::Filesystem(FsCapability::Write { .. }) => "fs.write".into(),
        Capability::Filesystem(FsCapability::Exec { .. }) => "fs.exec".into(),
        Capability::Network(NetCapability::Http { .. }) => "net.http".into(),
        Capability::Process(ProcessCapability::Spawn { .. }) => "process.spawn".into(),
        Capability::Agent(AgentCapability::Spawn { .. }) => "agent.spawn".into(),
        Capability::TaskList { .. } => "task_list".into(),
        Capability::Plan { .. } => "plan".into(),
        Capability::Skill(SkillCapability::Spawn { .. }) => "skill.spawn".into(),
        Capability::Custom { name, .. } => name.clone(),
        // `Capability` is `#[non_exhaustive]`; future variants degrade
        // to a generic tag — additive evolution must not silently
        // mis-classify them.
        _ => "unknown".into(),
    }
}

/// Filesystem-capability subsumption check.
pub fn fs_satisfies(granted: &FsCapability, required: &FsCapability) -> bool {
    match (granted, required) {
        (FsCapability::Read { paths: gp, .. }, FsCapability::Read { paths: rp, .. }) => {
            paths_subset(gp, rp)
        }
        (
            FsCapability::Write {
                paths: gp,
                max_bytes: gmb,
                ..
            },
            FsCapability::Write {
                paths: rp,
                max_bytes: rmb,
                ..
            },
        ) => paths_subset(gp, rp) && max_bytes_satisfies(*gmb, *rmb),
        (FsCapability::Exec { paths: gp, .. }, FsCapability::Exec { paths: rp, .. }) => {
            paths_subset(gp, rp)
        }
        _ => false,
    }
}

/// Network-capability subsumption check.
pub fn net_satisfies(granted: &NetCapability, required: &NetCapability) -> bool {
    match (granted, required) {
        (
            NetCapability::Http {
                hosts: gh,
                methods: gm,
                ..
            },
            NetCapability::Http {
                hosts: rh,
                methods: rm,
                ..
            },
        ) => net_hosts_subsume(gh, rh) && string_subset(gm, rm),
        // Future `NetCapability` variants added in tau-domain default
        // to deny — additive evolution must not silently widen grants.
        _ => false,
    }
}

/// Process-capability subsumption check.
pub fn process_satisfies(granted: &ProcessCapability, required: &ProcessCapability) -> bool {
    match (granted, required) {
        (
            ProcessCapability::Spawn { commands: gc, .. },
            ProcessCapability::Spawn { commands: rc, .. },
        ) => paths_subset(gc, rc),
        _ => false,
    }
}

/// Orchestration TaskList subsumption: `manage` ⊇ `write` ⊇ `read`.
///
/// Mode strings are validated at parse time in `tau-domain` (unknown
/// modes route to `Capability::Custom`), but this helper is defensive:
/// any unrecognised mode on either side denies rather than panics.
pub fn task_list_satisfies(granted: &str, required: &str) -> bool {
    let rank = |s: &str| -> Option<u8> {
        match s {
            "read" => Some(0),
            "write" => Some(1),
            "manage" => Some(2),
            _ => None,
        }
    };
    match (rank(granted), rank(required)) {
        (Some(g), Some(r)) => g >= r,
        _ => false,
    }
}

/// Orchestration Plan subsumption: `write` ⊇ `read`.
///
/// Same defensive deny-on-unknown-mode contract as
/// [`task_list_satisfies`].
pub fn plan_satisfies(granted: &str, required: &str) -> bool {
    let rank = |s: &str| -> Option<u8> {
        match s {
            "read" => Some(0),
            "write" => Some(1),
            _ => None,
        }
    };
    match (rank(granted), rank(required)) {
        (Some(g), Some(r)) => g >= r,
        _ => false,
    }
}

/// Agent-capability subsumption check.
pub fn agent_satisfies(granted: &AgentCapability, required: &AgentCapability) -> bool {
    match (granted, required) {
        (
            AgentCapability::Spawn {
                allowed_kinds: gk, ..
            },
            AgentCapability::Spawn {
                allowed_kinds: rk, ..
            },
        ) => string_subset(gk, rk),
        _ => false,
    }
}

/// `Skill` satisfaction: granted `allowed_skills` is a superset of required.
///
/// `required_capability()` in virtual_tools.rs emits
/// `SkillCapability::Spawn { allowed_skills: [] }` (empty = "just needs
/// any skill.spawn capability"), which is trivially satisfied by any
/// granted skill.spawn entry. The specific `allowed_skills` membership
/// check (which skill is actually being spawned) is deferred to
/// `validate_skill_spawn`, which inspects the spawned skill name against
/// the actual `allowed_skills` list in the grant.
pub fn skill_satisfies(granted: &SkillCapability, required: &SkillCapability) -> bool {
    match (granted, required) {
        (
            SkillCapability::Spawn {
                allowed_skills: gs, ..
            },
            SkillCapability::Spawn {
                allowed_skills: rs, ..
            },
        ) => string_subset(gs, rs),
        // `SkillCapability` is `#[non_exhaustive]`; future variants
        // default to deny — additive evolution must not silently widen grants.
        _ => false,
    }
}

/// Conservative v0.1 rule: every required key must exist in granted
/// with an equal `Value`. Extra granted keys are allowed; extra
/// required keys (missing from granted) deny.
/// Custom-namespace params subsumption: every required key must exist in granted with equal value.
pub fn custom_params_satisfy(
    granted: &BTreeMap<String, Value>,
    required: &BTreeMap<String, Value>,
) -> bool {
    required
        .iter()
        .all(|(k, rv)| granted.get(k).is_some_and(|gv| gv == rv))
}

/// `max_bytes` satisfies if the grant is unbounded (`None`) OR the
/// grant's bound is `>=` the request's bound. A bounded grant cannot
/// cover an unbounded request — that would silently widen the cap.
fn max_bytes_satisfies(granted: Option<u64>, required: Option<u64>) -> bool {
    match (granted, required) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(g), Some(r)) => g >= r,
    }
}

/// Every required string must match at least one grant pattern under
/// the path/host glob rules (`**`, `*`, exact).
fn paths_subset(granted: &[String], required: &[String]) -> bool {
    required
        .iter()
        .all(|r| granted.iter().any(|g| glob_matches(g, r)))
}

/// Every required string must equal at least one grant string. Used
/// for HTTP methods and agent-spawn kinds — non-glob string sets.
fn string_subset(granted: &[String], required: &[String]) -> bool {
    required.iter().all(|r| granted.contains(r))
}

/// Host subsumption for `net.http` (D7-B `NetHosts`). A granted `Any`
/// subsumes everything; a granted list subsumes a required list under the
/// host-glob rules but never subsumes a required `Any` (fail-closed).
fn net_hosts_subsume(granted: &NetHosts, required: &NetHosts) -> bool {
    match (granted, required) {
        (NetHosts::Any, _) => true,
        (NetHosts::List(_), NetHosts::Any) => false,
        (NetHosts::List(g), NetHosts::List(r)) => paths_subset(g, r),
    }
}

/// Glob matcher. Splits on `/`. `**` matches zero or more segments,
/// `*` matches exactly one segment (no `/`), other segments match
/// literally. v0.1 does NOT support partial-segment globs like
/// `pre*.txt` — full segment only or full wildcard. Used for both
/// filesystem paths AND HTTP hosts (`*.example.com`); host segments
/// are split on `/` which is fine because hostnames don't contain `/`.
fn glob_matches(pattern: &str, candidate: &str) -> bool {
    let p_segs: Vec<&str> = pattern.split('/').collect();
    let c_segs: Vec<&str> = candidate.split('/').collect();
    glob_segs(&p_segs, &c_segs)
}

fn glob_segs(pattern: &[&str], candidate: &[&str]) -> bool {
    match (pattern.first(), candidate.first()) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(&"**"), _) => {
            // `**` matches zero or more segments. Try every suffix.
            (0..=candidate.len()).any(|i| glob_segs(&pattern[1..], &candidate[i..]))
        }
        (Some(_), None) => false,
        (Some(&p), Some(&c)) => segment_matches(p, c) && glob_segs(&pattern[1..], &candidate[1..]),
    }
}

/// Single-segment match. Three accepted forms:
///
/// 1. `*` — covers any single segment.
/// 2. `*.suffix` — host-style leading wildcard; matches if the
///    candidate ends with `.suffix` (e.g. `*.example.com` covers
///    `api.example.com`). The candidate must be strictly longer than
///    the suffix so that bare `example.com` is NOT covered by
///    `*.example.com` — that would silently widen the grant.
/// 3. exact string equality.
///
/// Other partial-segment forms (`pre*.txt`, `*foo*`) are deliberately
/// unsupported at v0.1 — full segment, full wildcard, or host suffix.
fn segment_matches(pattern: &str, candidate: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        // Bare `*.` is meaningless; treat as no-match.
        if suffix.is_empty() {
            return false;
        }
        // Candidate must end with `.suffix` AND be strictly longer
        // than `.suffix` (so the wildcard prefix is non-empty).
        let dotted = alloc::format!(".{suffix}");
        return candidate.len() > dotted.len() && candidate.ends_with(&dotted);
    }
    pattern == candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct CapWrapper {
        cap: Capability,
    }

    /// Construct a `Capability` from a TOML fragment that defines
    /// `[cap]` with the flat dot-namespaced shape (per ADR-0002).
    /// Variant-level `#[non_exhaustive]` blocks struct-literal
    /// construction from outside `tau-domain`, so tests round-trip
    /// through the canonical TOML form instead.
    fn cap(toml_str: &str) -> Capability {
        toml::from_str::<CapWrapper>(toml_str)
            .expect("test capability TOML must parse")
            .cap
    }

    // -------------------- Filesystem --------------------

    #[test]
    fn fs_read_grant_satisfies_fs_read_required() {
        let granted = cap(r#"[cap]
kind = "fs.read"
paths = ["/tmp/**"]
"#);
        let required = cap(r#"[cap]
kind = "fs.read"
paths = ["/tmp/foo.txt"]
"#);
        assert!(capability_satisfies(&granted, &required));
    }

    #[test]
    fn fs_read_grant_does_not_satisfy_fs_write_required() {
        let granted = cap(r#"[cap]
kind = "fs.read"
paths = ["/tmp/**"]
"#);
        let required = cap(r#"[cap]
kind = "fs.write"
paths = ["/tmp/foo.txt"]
"#);
        assert!(!capability_satisfies(&granted, &required));
    }

    #[test]
    fn fs_glob_grant_satisfies_specific_required() {
        let granted = cap(r#"[cap]
kind = "fs.read"
paths = ["/tmp/**"]
"#);
        let required = cap(r#"[cap]
kind = "fs.read"
paths = ["/tmp/sub/file.txt"]
"#);
        assert!(capability_satisfies(&granted, &required));
    }

    #[test]
    fn fs_specific_grant_does_not_satisfy_glob_required() {
        // A grant of one literal path does NOT cover an open-ended
        // `/tmp/**` request — that would silently widen the cap.
        let granted = cap(r#"[cap]
kind = "fs.read"
paths = ["/tmp/foo.txt"]
"#);
        let required = cap(r#"[cap]
kind = "fs.read"
paths = ["/tmp/**"]
"#);
        assert!(!capability_satisfies(&granted, &required));
    }

    #[test]
    fn fs_write_max_bytes_unbounded_grant_satisfies_bounded_required() {
        let granted = cap(r#"[cap]
kind = "fs.write"
paths = ["/tmp/**"]
"#);
        let required = cap(r#"[cap]
kind = "fs.write"
paths = ["/tmp/foo.txt"]
max_bytes = 1024
"#);
        assert!(capability_satisfies(&granted, &required));
    }

    #[test]
    fn fs_write_bounded_grant_satisfies_smaller_required() {
        let granted = cap(r#"[cap]
kind = "fs.write"
paths = ["/tmp/**"]
max_bytes = 2048
"#);
        let required = cap(r#"[cap]
kind = "fs.write"
paths = ["/tmp/foo.txt"]
max_bytes = 1024
"#);
        assert!(capability_satisfies(&granted, &required));
    }

    #[test]
    fn fs_write_bounded_grant_does_not_satisfy_unbounded_required() {
        let granted = cap(r#"[cap]
kind = "fs.write"
paths = ["/tmp/**"]
max_bytes = 2048
"#);
        let required = cap(r#"[cap]
kind = "fs.write"
paths = ["/tmp/foo.txt"]
"#);
        assert!(!capability_satisfies(&granted, &required));
    }

    // -------------------- Network --------------------

    #[test]
    fn net_http_grant_satisfies_subset_methods() {
        let granted = cap(r#"[cap]
kind = "net.http"
hosts = ["api.example.com"]
methods = ["GET", "POST"]
"#);
        let required = cap(r#"[cap]
kind = "net.http"
hosts = ["api.example.com"]
methods = ["GET"]
"#);
        assert!(capability_satisfies(&granted, &required));
    }

    #[test]
    fn net_http_grant_does_not_satisfy_method_outside_grant() {
        let granted = cap(r#"[cap]
kind = "net.http"
hosts = ["api.example.com"]
methods = ["GET"]
"#);
        let required = cap(r#"[cap]
kind = "net.http"
hosts = ["api.example.com"]
methods = ["DELETE"]
"#);
        assert!(!capability_satisfies(&granted, &required));
    }

    #[test]
    fn net_http_host_glob_satisfies() {
        let granted = cap(r#"[cap]
kind = "net.http"
hosts = ["*.example.com"]
methods = ["GET"]
"#);
        let required = cap(r#"[cap]
kind = "net.http"
hosts = ["api.example.com"]
methods = ["GET"]
"#);
        assert!(capability_satisfies(&granted, &required));
    }

    // -------------------- Process --------------------

    #[test]
    fn process_spawn_subset_satisfies() {
        let granted = cap(r#"[cap]
kind = "process.spawn"
commands = ["git", "cargo"]
"#);
        let required = cap(r#"[cap]
kind = "process.spawn"
commands = ["git"]
"#);
        assert!(capability_satisfies(&granted, &required));
    }

    // -------------------- Agent --------------------

    #[test]
    fn agent_spawn_subset_satisfies() {
        let granted = cap(r#"[cap]
kind = "agent.spawn"
allowed_kinds = ["worker", "planner"]
"#);
        let required = cap(r#"[cap]
kind = "agent.spawn"
allowed_kinds = ["worker"]
"#);
        assert!(capability_satisfies(&granted, &required));
    }

    // -------------------- Skill --------------------

    #[test]
    fn skill_spawn_subset_satisfies() {
        let granted = cap(r#"[cap]
kind = "skill.spawn"
allowed_skills = ["critic", "fact-checker"]
"#);
        let required = cap(r#"[cap]
kind = "skill.spawn"
allowed_skills = ["critic"]
"#);
        assert!(capability_satisfies(&granted, &required));
    }

    #[test]
    fn skill_spawn_superset_denies() {
        let granted = cap(r#"[cap]
kind = "skill.spawn"
allowed_skills = ["critic"]
"#);
        let required = cap(r#"[cap]
kind = "skill.spawn"
allowed_skills = ["critic", "fact-checker"]
"#);
        assert!(!capability_satisfies(&granted, &required));
    }

    // -------------------- Custom --------------------

    #[test]
    fn custom_params_exact_match_satisfies() {
        let granted = cap(r#"[cap]
kind = "mcp.tool.use"
servers = ["fs-mcp"]
"#);
        let required = cap(r#"[cap]
kind = "mcp.tool.use"
servers = ["fs-mcp"]
"#);
        assert!(capability_satisfies(&granted, &required));
    }

    #[test]
    fn custom_params_extra_required_key_fails() {
        // Required has an extra `mode` key not present in the grant —
        // conservative deny.
        let granted = cap(r#"[cap]
kind = "mcp.tool.use"
servers = ["fs-mcp"]
"#);
        let required = cap(r#"[cap]
kind = "mcp.tool.use"
servers = ["fs-mcp"]
mode = "strict"
"#);
        assert!(!capability_satisfies(&granted, &required));
    }

    #[test]
    fn custom_different_names_fail() {
        let granted = cap(r#"[cap]
kind = "mcp.tool.use"
servers = ["fs-mcp"]
"#);
        let required = cap(r#"[cap]
kind = "mcp.resource.read"
servers = ["fs-mcp"]
"#);
        assert!(!capability_satisfies(&granted, &required));
    }

    // -------------------- TaskList --------------------

    #[test]
    fn task_list_manage_satisfies_write() {
        let granted = cap(r#"[cap]
kind = "task_list"
mode = "manage"
"#);
        let required = cap(r#"[cap]
kind = "task_list"
mode = "write"
"#);
        assert!(capability_satisfies(&granted, &required));
    }

    #[test]
    fn task_list_manage_satisfies_read() {
        let granted = cap(r#"[cap]
kind = "task_list"
mode = "manage"
"#);
        let required = cap(r#"[cap]
kind = "task_list"
mode = "read"
"#);
        assert!(capability_satisfies(&granted, &required));
    }

    #[test]
    fn task_list_write_satisfies_read() {
        let granted = cap(r#"[cap]
kind = "task_list"
mode = "write"
"#);
        let required = cap(r#"[cap]
kind = "task_list"
mode = "read"
"#);
        assert!(capability_satisfies(&granted, &required));
    }

    #[test]
    fn task_list_read_does_not_satisfy_write() {
        let granted = cap(r#"[cap]
kind = "task_list"
mode = "read"
"#);
        let required = cap(r#"[cap]
kind = "task_list"
mode = "write"
"#);
        assert!(!capability_satisfies(&granted, &required));
    }

    #[test]
    fn task_list_unknown_mode_does_not_satisfy() {
        // Parse-time validation in tau-domain rejects unknown modes for
        // the `task_list` kind by routing them to `Capability::Custom`,
        // so construct the TaskList variant directly here to exercise
        // the defensive deny path inside `task_list_satisfies`.
        let granted = Capability::TaskList {
            mode: "frobnicate".into(),
        };
        let required = Capability::TaskList {
            mode: "write".into(),
        };
        assert!(!capability_satisfies(&granted, &required));
    }

    // -------------------- Plan --------------------

    #[test]
    fn plan_write_satisfies_read() {
        let granted = cap(r#"[cap]
kind = "plan"
mode = "write"
"#);
        let required = cap(r#"[cap]
kind = "plan"
mode = "read"
"#);
        assert!(capability_satisfies(&granted, &required));
    }

    #[test]
    fn plan_read_does_not_satisfy_write() {
        let granted = cap(r#"[cap]
kind = "plan"
mode = "read"
"#);
        let required = cap(r#"[cap]
kind = "plan"
mode = "write"
"#);
        assert!(!capability_satisfies(&granted, &required));
    }

    #[test]
    fn tasklist_does_not_cover_plan() {
        // Cross-namespace negative: a `task_list` grant — even at the
        // strongest `manage` mode — must never satisfy a `plan`
        // request. The top-level enum mismatch fires before the
        // mode-rank comparison.
        let granted = cap(r#"[cap]
kind = "task_list"
mode = "write"
"#);
        let required = cap(r#"[cap]
kind = "plan"
mode = "write"
"#);
        assert!(!capability_satisfies(&granted, &required));
    }

    // -------------------- check_capabilities --------------------

    #[test]
    fn check_capabilities_with_empty_required_returns_none() {
        let granted: Vec<Capability> = alloc::vec![];
        let required: Vec<Capability> = alloc::vec![];
        assert!(check_capabilities(&granted, &required).is_none());
    }

    #[test]
    fn check_capabilities_returns_first_missing_when_some_unsatisfied() {
        let granted = alloc::vec![cap(r#"[cap]
kind = "fs.read"
paths = ["/tmp/**"]
"#)];
        let required = alloc::vec![
            cap(r#"[cap]
kind = "fs.read"
paths = ["/tmp/foo.txt"]
"#),
            cap(r#"[cap]
kind = "fs.write"
paths = ["/tmp/foo.txt"]
"#),
            cap(r#"[cap]
kind = "net.http"
hosts = ["api.example.com"]
methods = ["GET"]
"#),
        ];
        let missing =
            check_capabilities(&granted, &required).expect("expected a missing capability");
        // First missing = the fs.write request (index 1), since the
        // fs.read at index 0 is satisfied by the /tmp/** grant.
        match missing {
            Capability::Filesystem(FsCapability::Write { .. }) => {}
            other => panic!("expected first missing = fs.write, got {:?}", other),
        }
    }

    #[test]
    fn check_capabilities_returns_none_when_all_satisfied() {
        let granted = alloc::vec![
            cap(r#"[cap]
kind = "fs.read"
paths = ["/tmp/**"]
"#),
            cap(r#"[cap]
kind = "net.http"
hosts = ["*.example.com"]
methods = ["GET", "POST"]
"#),
        ];
        let required = alloc::vec![
            cap(r#"[cap]
kind = "fs.read"
paths = ["/tmp/foo.txt"]
"#),
            cap(r#"[cap]
kind = "net.http"
hosts = ["api.example.com"]
methods = ["GET"]
"#),
        ];
        assert!(check_capabilities(&granted, &required).is_none());
    }
}

#[cfg(test)]
mod proptests {
    //! Property tests for [`capability_satisfies`]. Three primary
    //! invariants per spec §4:
    //!
    //! 1. **Reflexivity**: any capability satisfies itself.
    //! 2. **Wrong-variant rejection**: top-level enum mismatch always
    //!    denies (a `Filesystem` grant never covers a `Network`
    //!    request, etc).
    //! 3. **Glob superset**: a `/<prefix>/**` grant satisfies any
    //!    specific path under that prefix.
    //!
    //! Lives in the same file as the satisfies-relation because
    //! `capability_satisfies` is `pub(crate)` and not reachable from
    //! integration-test crates without a dedicated test feature flag —
    //! keeping the proptest co-located with the function under test
    //! avoids that machinery.
    //!
    //! Capability instances are constructed by round-tripping through
    //! the canonical TOML wire form (per ADR-0002) because variant-
    //! level `#[non_exhaustive]` blocks struct-literal construction
    //! from outside `tau-domain`. The `Custom` namespace is exercised
    //! via the wire form too, since `Value` is `#[non_exhaustive]`.
    //!
    //! Each property runs 256 cases. The `proptest!` macro flattens
    //! into one `#[test]` per property, so this submodule contributes
    //! three to the unit-test count.
    use super::*;
    use proptest::prelude::*;
    use tau_domain::Capability;
    use tau_domain::FsCapability;
    use tau_domain::NetCapability;
    use tau_domain::ProcessCapability;

    #[derive(serde::Deserialize)]
    struct CapWrapper {
        cap: Capability,
    }

    fn cap_from_toml(t: &str) -> Capability {
        toml::from_str::<CapWrapper>(t)
            .expect("proptest-generated TOML must parse")
            .cap
    }

    /// Path segment alphabet, no `*`/`/` so glob metacharacters never
    /// leak into the strategy by accident — the glob property
    /// constructs them explicitly.
    const SEG: &str = "[a-z][a-z0-9]{0,7}";

    /// A POSIX-shaped path with one or two segments and a short ext.
    fn path_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            (SEG, SEG, "[a-z]{2,4}").prop_map(|(a, b, ext)| alloc::format!("/{a}/{b}.{ext}")),
            (SEG, SEG, SEG, "[a-z]{2,4}")
                .prop_map(|(a, b, c, ext)| alloc::format!("/{a}/{b}/{c}.{ext}")),
        ]
    }

    fn paths_vec_strategy() -> impl Strategy<Value = Vec<String>> {
        prop::collection::vec(path_strategy(), 1..=3)
    }

    fn fs_capability_strategy() -> impl Strategy<Value = Capability> {
        prop_oneof![
            paths_vec_strategy().prop_map(|paths| {
                let toml_paths = render_string_list(&paths);
                cap_from_toml(&alloc::format!(
                    "[cap]\nkind = \"fs.read\"\npaths = {toml_paths}\n"
                ))
            }),
            (paths_vec_strategy(), prop::option::of(1u64..=4096)).prop_map(|(paths, max_bytes)| {
                let toml_paths = render_string_list(&paths);
                let mut s = alloc::format!("[cap]\nkind = \"fs.write\"\npaths = {toml_paths}\n");
                if let Some(b) = max_bytes {
                    s.push_str(&alloc::format!("max_bytes = {b}\n"));
                }
                cap_from_toml(&s)
            }),
            paths_vec_strategy().prop_map(|paths| {
                let toml_paths = render_string_list(&paths);
                cap_from_toml(&alloc::format!(
                    "[cap]\nkind = \"fs.exec\"\npaths = {toml_paths}\n"
                ))
            }),
        ]
    }

    fn host_strategy() -> impl Strategy<Value = String> {
        ("[a-z]{2,8}", "[a-z]{2,8}").prop_map(|(sub, base)| alloc::format!("{sub}.{base}.com"))
    }

    fn net_capability_strategy() -> impl Strategy<Value = Capability> {
        let methods_pool: Vec<&'static str> = alloc::vec!["GET", "POST", "PUT", "DELETE"];
        (
            prop::collection::vec(host_strategy(), 1..=3),
            prop::sample::subsequence(methods_pool, 1..=4),
        )
            .prop_map(|(hosts, methods)| {
                let methods_owned: Vec<String> = methods.into_iter().map(String::from).collect();
                let toml_hosts = render_string_list(&hosts);
                let toml_methods = render_string_list(&methods_owned);
                cap_from_toml(&alloc::format!(
                    "[cap]\nkind = \"net.http\"\nhosts = {toml_hosts}\nmethods = {toml_methods}\n"
                ))
            })
    }

    fn process_capability_strategy() -> impl Strategy<Value = Capability> {
        let pool: Vec<&'static str> = alloc::vec!["git", "cargo", "ls", "cat", "rg"];
        prop::sample::subsequence(pool, 1..=5).prop_map(|cmds| {
            let owned: Vec<String> = cmds.into_iter().map(String::from).collect();
            let toml_cmds = render_string_list(&owned);
            cap_from_toml(&alloc::format!(
                "[cap]\nkind = \"process.spawn\"\ncommands = {toml_cmds}\n"
            ))
        })
    }

    fn agent_capability_strategy() -> impl Strategy<Value = Capability> {
        let pool: Vec<&'static str> = alloc::vec!["worker", "planner", "reviewer"];
        prop::sample::subsequence(pool, 1..=3).prop_map(|kinds| {
            let owned: Vec<String> = kinds.into_iter().map(String::from).collect();
            let toml_kinds = render_string_list(&owned);
            cap_from_toml(&alloc::format!(
                "[cap]\nkind = \"agent.spawn\"\nallowed_kinds = {toml_kinds}\n"
            ))
        })
    }

    /// Top-level capability strategy. `Custom` is intentionally
    /// excluded at v0.1: `Value` is `#[non_exhaustive]` and the
    /// custom-params satisfies-relation already has dedicated unit
    /// tests upthread; the additional combinatorial coverage of
    /// fuzzing it here would be marginal versus the strategy
    /// scaffolding cost.
    fn capability_strategy() -> impl Strategy<Value = Capability> {
        prop_oneof![
            fs_capability_strategy(),
            net_capability_strategy(),
            process_capability_strategy(),
            agent_capability_strategy(),
        ]
    }

    /// Render a Rust `&[String]` into a TOML inline-array literal,
    /// e.g. `["a","b"]`. Strategy values come from controlled regex
    /// alphabets so we don't need to escape — but we still wrap each
    /// element in `"…"` for safety.
    fn render_string_list(items: &[String]) -> String {
        let mut out = String::from("[");
        for (i, s) in items.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push('"');
            out.push_str(s);
            out.push('"');
        }
        out.push(']');
        out
    }

    /// `true` iff the two capabilities are at the same top-level enum
    /// variant. Used to filter wrong-variant pairs in property 2.
    fn same_top_level(a: &Capability, b: &Capability) -> bool {
        matches!(
            (a, b),
            (Capability::Filesystem(_), Capability::Filesystem(_))
                | (Capability::Network(_), Capability::Network(_))
                | (Capability::Process(_), Capability::Process(_))
                | (Capability::Agent(_), Capability::Agent(_))
                | (Capability::Skill(_), Capability::Skill(_))
                | (Capability::Custom { .. }, Capability::Custom { .. },)
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 256,
            ..ProptestConfig::default()
        })]

        /// Property 1 — reflexivity: every capability satisfies itself.
        #[test]
        fn reflexivity(c in capability_strategy()) {
            prop_assert!(capability_satisfies(&c, &c));
        }

        /// Property 2 — top-level variant mismatch always denies.
        ///
        /// Pairs that happen to land on the same top-level variant
        /// are skipped via `prop_assume!` rather than `prop_filter`
        /// to keep the strategy tree simple.
        #[test]
        fn wrong_variant_rejection(
            granted in capability_strategy(),
            required in capability_strategy(),
        ) {
            prop_assume!(!same_top_level(&granted, &required));
            prop_assert!(!capability_satisfies(&granted, &required));
        }

        /// Property 3 — glob superset.
        ///
        /// A grant of `/<prefix>/**` always satisfies any required
        /// `fs.read` for a specific path under that prefix. Both
        /// single-segment and multi-segment suffixes are covered by
        /// the `suffix` strategy.
        #[test]
        fn glob_superset(
            prefix in "[a-z][a-z0-9]{0,7}",
            mid in SEG,
            leaf in SEG,
            ext in "[a-z]{2,4}",
            depth in 1usize..=3,
        ) {
            let mut suffix = String::new();
            for _ in 0..depth {
                suffix.push_str(&mid);
                suffix.push('/');
            }
            suffix.push_str(&leaf);
            suffix.push('.');
            suffix.push_str(&ext);

            let grant = cap_from_toml(&alloc::format!(
                "[cap]\nkind = \"fs.read\"\npaths = [\"/{prefix}/**\"]\n"
            ));
            let required = cap_from_toml(&alloc::format!(
                "[cap]\nkind = \"fs.read\"\npaths = [\"/{prefix}/{suffix}\"]\n"
            ));

            // Defensive variant assertions — if the wire-form
            // round-trip ever drifted these would catch it before the
            // satisfies assertion below. Pulled into `let` bindings
            // because `prop_assert!` runs its arg through a format
            // string and balks on the `{ .. }` pattern in `matches!`.
            let grant_is_fs_read =
                matches!(&grant, Capability::Filesystem(FsCapability::Read { .. }));
            let required_is_fs_read =
                matches!(&required, Capability::Filesystem(FsCapability::Read { .. }));
            prop_assert!(grant_is_fs_read);
            prop_assert!(required_is_fs_read);

            prop_assert!(capability_satisfies(&grant, &required));
        }
    }

    // Touch the imported strategy types in a `const` to keep
    // `unused_imports` quiet without resorting to `#[allow]`. The
    // strategies above already construct these via TOML, so the type
    // names aren't otherwise mentioned in this submodule.
    #[allow(dead_code)]
    fn _imports_used(_n: NetCapability, _p: ProcessCapability) {}
}
