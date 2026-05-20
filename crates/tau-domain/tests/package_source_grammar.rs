//! Integration test: `PackageSource` grammar across known cases.
//!
//! Hand-picked cases complement the proptest fuzzing in
//! `proptest_package_source.rs` by pinning behaviour at boundary inputs
//! (empty rev marker, unsupported scheme, scp without user).

use std::str::FromStr;

use assert_matches::assert_matches;
use tau_domain::{GitLocation, PackageSource, PackageSourceError};

#[test]
fn https_no_rev() {
    let s = PackageSource::from_str("https://example.com/r.git").unwrap();
    assert_matches!(
        s,
        PackageSource::Git {
            location: GitLocation::Url(u),
            rev,
        } => {
            assert_eq!(u.scheme(), "https");
            assert_eq!(rev, None);
        }
    );
}

#[test]
fn ssh_with_rev() {
    let s = PackageSource::from_str("ssh://git@example.com/r.git#main").unwrap();
    assert_matches!(
        s,
        PackageSource::Git { rev, .. } => {
            assert_eq!(rev.as_deref(), Some("main"));
        }
    );
}

#[test]
fn scp_no_user() {
    let s = PackageSource::from_str("example.com:r.git").unwrap();
    assert_matches!(
        s,
        PackageSource::Git {
            location: GitLocation::Scp { user, host, path },
            ..
        } => {
            assert!(user.is_none());
            assert_eq!(host, "example.com");
            assert_eq!(path, "r.git");
        }
    );
}

#[test]
fn rejects_ftp() {
    assert!(matches!(
        PackageSource::from_str("ftp://example.com/r.git"),
        Err(PackageSourceError::UnsupportedScheme { .. }),
    ));
}

#[test]
fn rejects_empty_rev_marker() {
    assert_eq!(
        PackageSource::from_str("https://example.com/r.git#"),
        Err(PackageSourceError::EmptyRevision),
    );
}
