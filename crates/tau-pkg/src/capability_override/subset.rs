//! Reusable capability subset relation (ADR-0057 decision 2) and the shared
//! per-field comparison helpers used by both `compute_effective` (per-package
//! narrowing) and `capability_set_subset` (lattice-link ceiling checks).
//!
//! Story 1.3 scope: the primitive + helpers only. No enforcement wiring
//! (`tau check` = 1.4; lattice traversal = 1.5).

use super::glob_subset::is_glob_subset_set;

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
}
