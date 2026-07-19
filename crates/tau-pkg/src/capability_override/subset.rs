//! Reusable capability subset relation (ADR-0057 decision 2) and the shared
//! per-field comparison helpers used by both `compute_effective` (per-package
//! narrowing) and `capability_set_subset` (lattice-link ceiling checks).
//!
//! D3 task 6: `capability_set_subset` and `CeilingViolation` now delegate to
//! the sound `tau-domain` lattice primitive (`capability_subset`); the
//! sampling-era per-kind comparison logic that used to live here has been
//! deleted. `paths_subset` delegates to the sound G2 glob engine
//! (`tau_domain::package::capability::lattice::glob::glob_subset_set`).
//! `string_set_subset` and `max_bytes_le` remain — `compute_effective`
//! (mod.rs) still calls them directly for per-package allow/max_bytes
//! narrowing, which is a distinct relation from the lattice-link ceiling
//! check above.

use tau_domain::package::capability::lattice::glob::glob_subset_set;

pub use tau_domain::{capability_subset as capability_set_subset, CeilingViolation};

/// Globbed path subset: every `child` path is a glob-subset of some `parent`
/// path. `Err(offender)` names the first child path with no admitting parent.
pub(crate) fn paths_subset(child: &[String], parent: &[String]) -> Result<(), String> {
    glob_subset_set(child, parent)
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
        // D3: fs.write subset now requires JOINT per-grant coverage — a
        // parent entry only "covers" the child if that SAME entry's
        // max_bytes admits the child's max_bytes. Here the only parent
        // entry (max_bytes=5000) does not cover the child's 9000, so it is
        // excluded entirely and the child's path has zero eligible parent
        // paths left to match against — the offender is now the path
        // itself, not a "max_bytes=<n>" token.
        let child = vec![cap(
            r#"{"kind":"fs.write","paths":["/p/**"],"max_bytes":9000}"#,
        )];
        let parent = vec![cap(
            r#"{"kind":"fs.write","paths":["/p/**"],"max_bytes":5000}"#,
        )];
        let v = capability_set_subset(&child, &parent).unwrap_err();
        assert_eq!(v.kind, "fs.write");
        assert_eq!(v.offender, "/p/**");
        assert!(
            v.reason.contains("max_bytes"),
            "got reason: {}",
            v.reason
        );
    }

    #[test]
    fn fs_write_unlimited_child_under_capped_ceiling_rejected() {
        // D3: same joint-coverage rule as above — an unlimited child is not
        // covered by a capped parent entry, so that entry's paths are
        // excluded and the offender names the uncovered path rather than a
        // synthetic "max_bytes=unlimited" token.
        let child = vec![cap(r#"{"kind":"fs.write","paths":["/p/**"]}"#)];
        let parent = vec![cap(
            r#"{"kind":"fs.write","paths":["/p/**"],"max_bytes":5000}"#,
        )];
        let v = capability_set_subset(&child, &parent).unwrap_err();
        assert_eq!(v.kind, "fs.write");
        assert_eq!(v.offender, "/p/**");
    }

    #[test]
    fn net_http_method_outside_ceiling_now_rejected() {
        // D3: methods are now enforced (were ignored by the old sampler).
        // The child requests POST but the only ceiling entry for that host
        // grants GET only, so no ceiling entry jointly covers (host, POST)
        // and the request is denied.
        let child = vec![cap(
            r#"{"kind":"net.http","hosts":["api.x.com"],"methods":["POST"]}"#,
        )];
        let parent = vec![cap(
            r#"{"kind":"net.http","hosts":["api.x.com"],"methods":["GET"]}"#,
        )];
        assert!(capability_set_subset(&child, &parent).is_err());
    }

    #[test]
    fn net_http_host_outside_ceiling_rejected() {
        // D3: `methods` now participate in the joint (host, method)
        // coverage check, and an empty method list is vacuously satisfied
        // (no method requested ⟹ nothing to cover). To keep exercising the
        // host-mismatch path under the sound semantics, both sides now
        // request a concrete method ("GET") so the per-method host check
        // actually runs.
        let child = vec![cap(
            r#"{"kind":"net.http","hosts":["evil.com"],"methods":["GET"]}"#,
        )];
        let parent = vec![cap(
            r#"{"kind":"net.http","hosts":["api.x.com"],"methods":["GET"]}"#,
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
        // D3: methods must be non-empty or the sound per-method check is vacuous.
        // Child GET on b.com is granted by the second parent entry (b.com, GET).
        let child = vec![cap(r#"{"kind":"net.http","hosts":["b.com"],"methods":["GET"]}"#)];
        let parent = vec![
            cap(r#"{"kind":"net.http","hosts":["a.com"],"methods":["GET"]}"#),
            cap(r#"{"kind":"net.http","hosts":["b.com"],"methods":["GET"]}"#),
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
