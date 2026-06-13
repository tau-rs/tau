//! Path validation + glob admission for `fs-write`.
//!
//! Ported verbatim from the `fs-read` plugin (reason strings
//! re-prefixed). See
//! `docs/superpowers/specs/2026-06-13-fs-write-edit-plugin-design.md`.

/// Reasons a path is rejected at validation time.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BadArgs {
    /// The path string was empty.
    Empty,
    /// The path contained a NUL byte.
    NullByte,
    /// The path was relative; absolute paths required.
    NotAbsolute,
    /// The path contained a `..` segment.
    Traversal,
    /// The path was outside the agent's fs.write capability scope.
    NotInScope,
}

impl BadArgs {
    /// Human-readable reason string surfaced in `ToolError::BadArgs`.
    pub(crate) fn reason(&self) -> String {
        match self {
            BadArgs::Empty => "fs-write: path is empty".into(),
            BadArgs::NullByte => "fs-write: path contains a NUL byte".into(),
            BadArgs::NotAbsolute => "fs-write: path is not absolute".into(),
            BadArgs::Traversal => "fs-write: path contains a `..` segment".into(),
            BadArgs::NotInScope => "fs-write: path is not in capability scope".into(),
        }
    }
}

/// Validate the syntactic shape of a path. Returns the path on
/// success, or a [`BadArgs`] reason on failure.
pub(crate) fn validate_path(path: &str) -> Result<&str, BadArgs> {
    if path.is_empty() {
        return Err(BadArgs::Empty);
    }
    if path.bytes().any(|b| b == 0) {
        return Err(BadArgs::NullByte);
    }
    if !std::path::Path::new(path).is_absolute() {
        return Err(BadArgs::NotAbsolute);
    }
    if path.split(std::path::MAIN_SEPARATOR).any(|seg| seg == "..") {
        return Err(BadArgs::Traversal);
    }
    Ok(path)
}

/// Check whether `path` is admissible under the active glob list.
/// Returns true iff at least one glob matches.
pub(crate) fn admit(path: &str, allowed_globs: &[String]) -> bool {
    use globset::Glob;
    allowed_globs.iter().any(|g| {
        Glob::new(g)
            .ok()
            .map(|gl| gl.compile_matcher().is_match(path))
            .unwrap_or(false)
    })
}

/// Check `path` is admitted by the allow-list AND not denied. Deny
/// wins. Reuses [`admit`] for both checks.
pub(crate) fn admit_with_deny(path: &str, allow: &[String], deny: &[String]) -> bool {
    if !admit(path, allow) {
        return false;
    }
    !admit(path, deny)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_path_empty_rejected() {
        assert_eq!(validate_path(""), Err(BadArgs::Empty));
    }

    #[test]
    fn validate_path_null_byte_rejected() {
        assert_eq!(validate_path("/tmp/foo\0bar"), Err(BadArgs::NullByte));
    }

    #[test]
    fn validate_path_relative_rejected() {
        assert_eq!(validate_path("./foo"), Err(BadArgs::NotAbsolute));
        assert_eq!(validate_path("foo/bar"), Err(BadArgs::NotAbsolute));
    }

    #[cfg(unix)]
    #[test]
    fn validate_path_traversal_rejected_dotdot_segment() {
        assert_eq!(validate_path("/../etc/passwd"), Err(BadArgs::Traversal));
    }

    #[cfg(unix)]
    #[test]
    fn validate_path_traversal_rejected_in_middle() {
        assert_eq!(validate_path("/tmp/../etc/passwd"), Err(BadArgs::Traversal));
    }

    #[cfg(unix)]
    #[test]
    fn validate_path_happy_path_returns_path() {
        assert_eq!(validate_path("/tmp/foo.txt"), Ok("/tmp/foo.txt"));
    }

    #[test]
    fn admit_matches_simple_glob() {
        let globs = vec!["/tmp/**".to_string()];
        assert!(admit("/tmp/foo.txt", &globs));
        assert!(admit("/tmp/sub/bar.txt", &globs));
    }

    #[test]
    fn admit_does_not_match_outside_scope() {
        let globs = vec!["/var/**".to_string()];
        assert!(!admit("/tmp/foo.txt", &globs));
    }

    #[test]
    fn admit_returns_false_for_invalid_glob() {
        let globs = vec!["[unclosed".to_string()];
        assert!(!admit("/tmp/foo", &globs));
    }

    #[test]
    fn admit_empty_glob_list_returns_false() {
        let globs: Vec<String> = vec![];
        assert!(!admit("/tmp/foo", &globs));
    }

    #[test]
    fn admit_multiple_globs_first_match_wins() {
        let globs = vec!["/var/**".to_string(), "/tmp/**".to_string()];
        assert!(admit("/tmp/foo", &globs));
    }

    #[test]
    fn admit_with_deny_denies_when_deny_matches() {
        let allow = vec!["/proj/**".to_string()];
        let deny = vec!["/proj/secrets/**".to_string()];
        assert!(!admit_with_deny("/proj/secrets/api.key", &allow, &deny));
    }

    #[test]
    fn admit_with_deny_admits_when_no_deny_matches() {
        let allow = vec!["/proj/**".to_string()];
        let deny = vec!["/proj/secrets/**".to_string()];
        assert!(admit_with_deny("/proj/src/main.rs", &allow, &deny));
    }

    #[test]
    fn admit_with_deny_denies_when_allow_misses() {
        let allow = vec!["/proj/**".to_string()];
        let deny: Vec<String> = vec![];
        assert!(!admit_with_deny("/etc/passwd", &allow, &deny));
    }

    #[test]
    fn admit_with_deny_empty_deny_falls_through_to_allow() {
        let allow = vec!["/proj/**".to_string()];
        let deny: Vec<String> = vec![];
        assert!(admit_with_deny("/proj/foo", &allow, &deny));
    }
}
