//! Read-only accessors over the per-scope lockfile.
//!
//! [`list`] and [`get`] are thin wrappers over [`LockFile::load`] —
//! they hide the lockfile-loading mechanism from external callers
//! (tau-cli, future tau-runtime) so consumers don't reach into
//! [`crate::lockfile`]'s internals.
//!
//! Note: every call re-reads the lockfile from disk. For repeated
//! lookups, callers should `LockFile::load` directly and reuse the
//! result.

use tau_domain::PackageName;

use crate::error::RegistryError;
use crate::lockfile::{LockFile, LockedPackage};
use crate::scope::Scope;

/// List every installed package in `scope`, in lockfile order.
///
/// Returns `Ok(Vec::new())` if the lockfile doesn't exist (lazy
/// creation per [`LockFile::load`]).
///
/// # Example
///
/// ```
/// use tau_pkg::{list, Scope};
///
/// # let tmp = tempfile::tempdir().unwrap();
/// # let scope = Scope::new_project(tmp.path()).unwrap();
/// // An empty scope has no installed packages.
/// assert!(list(&scope).unwrap().is_empty());
/// ```
pub fn list(scope: &Scope) -> Result<Vec<LockedPackage>, RegistryError> {
    Ok(LockFile::load(&scope.lockfile_path())?.packages)
}

/// Look up a single installed package by name.
///
/// Returns `Ok(None)` if the package is not in the lockfile (or the
/// lockfile doesn't exist — lazy creation returns the default empty
/// lockfile).
///
/// # Example
///
/// ```
/// use tau_pkg::{get, Scope};
///
/// # let tmp = tempfile::tempdir().unwrap();
/// # let scope = Scope::new_project(tmp.path()).unwrap();
/// let name: tau_domain::PackageName = "acme-tool".parse().unwrap();
/// // Returns `Ok(None)` when the package is not installed.
/// assert!(get(&scope, &name).unwrap().is_none());
/// ```
pub fn get(scope: &Scope, name: &PackageName) -> Result<Option<LockedPackage>, RegistryError> {
    Ok(LockFile::load(&scope.lockfile_path())?.find(name).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::{Duration, UNIX_EPOCH};

    use tempfile::TempDir;

    use crate::lockfile::LockedVersion;

    fn make_scope(tmp: &TempDir) -> Scope {
        Scope::global_at(tmp.path().join("tau-home")).unwrap()
    }

    fn fixture_pkg(name: &str, version: &str) -> LockedPackage {
        LockedPackage {
            name: name.parse().unwrap(),
            active_version: version.parse().unwrap(),
            source: "https://x.com/y.git".parse().unwrap(),
            installed_versions: vec![LockedVersion {
                version: version.parse().unwrap(),
                rev: None,
                resolved_commit: "0123456789abcdef0123456789abcdef01234567".into(),
                sha256: String::new(),
                installed_at: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            }],
            plugin: None,
            skill: None,
            synthesized_from: None,
        }
    }

    #[test]
    fn list_returns_empty_when_lockfile_missing() {
        let tmp = TempDir::new().unwrap();
        let scope = make_scope(&tmp);
        assert!(list(&scope).unwrap().is_empty());
    }

    #[test]
    fn list_returns_all_packages_in_lockfile_order() {
        let tmp = TempDir::new().unwrap();
        let scope = make_scope(&tmp);

        let mut lf = LockFile::default();
        lf.upsert(fixture_pkg("aaa-pkg", "1.0.0"));
        lf.upsert(fixture_pkg("bbb-pkg", "2.0.0"));
        lf.save(&scope.lockfile_path()).unwrap();

        let packages = list(&scope).unwrap();
        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0].name.as_str(), "aaa-pkg");
        assert_eq!(packages[1].name.as_str(), "bbb-pkg");
    }

    #[test]
    fn get_returns_none_for_unknown_package() {
        let tmp = TempDir::new().unwrap();
        let scope = make_scope(&tmp);
        let name: PackageName = "ghost".parse().unwrap();
        assert!(get(&scope, &name).unwrap().is_none());
    }

    #[test]
    fn get_returns_some_for_known_package() {
        let tmp = TempDir::new().unwrap();
        let scope = make_scope(&tmp);

        let mut lf = LockFile::default();
        lf.upsert(fixture_pkg("acme-tool", "1.2.3"));
        lf.save(&scope.lockfile_path()).unwrap();

        let name: PackageName = "acme-tool".parse().unwrap();
        let pkg = get(&scope, &name).unwrap();
        assert!(pkg.is_some());
        assert_eq!(pkg.unwrap().active_version.to_string(), "1.2.3");
    }
}
