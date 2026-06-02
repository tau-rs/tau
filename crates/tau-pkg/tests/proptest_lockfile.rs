//! Property tests for `LockFile` TOML round-trip.
//!
//! Per QG5: parsers of external input earn proptest coverage. The
//! lockfile is parsed from disk on every install/uninstall.
//!
//! Note: `LockedPackage` and `LockedVersion` are `#[non_exhaustive]`
//! and have no public constructors, so they cannot be built via struct
//! literals from integration-test binaries.  Instead we generate valid
//! TOML strings directly — this mirrors what `LockFile::load` reads from
//! disk — and verify that `toml::from_str` → re-serialize → `toml::from_str`
//! preserves schema_version, package count, and active_version strings.
//!
//! ## `source` field TOML shape
//!
//! Per ADR-0005, `PackageSource` serializes through its `Display`/`FromStr`
//! string form: `source = "https://example.com/x/y.git"` (optionally with
//! `#<rev>`). No nested tables.

use proptest::prelude::*;
use tau_pkg::{LockFile, RegistryError};
use tau_ports::fixtures::scratch_dir;

/// Generate a small semver version string (avoid 0.0.0 edge cases).
fn arb_version_str() -> impl Strategy<Value = String> {
    (1u64..=9, 0u64..=9, 0u64..=9).prop_map(|(maj, min, pat)| format!("{maj}.{min}.{pat}"))
}

/// Generate a syntactically-valid lowercase package name.
fn arb_package_name_str() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9-]{0,15}".prop_map(String::from)
}

/// Generate a 40-char lowercase hex string (resolved_commit).
fn arb_commit_sha() -> impl Strategy<Value = String> {
    "[0-9a-f]{40}".prop_map(String::from)
}

/// Generate a whole-second RFC-3339 timestamp.
/// Whole-second granularity avoids humantime-serde precision loss.
fn arb_timestamp_str() -> impl Strategy<Value = String> {
    (0u32..86400u32).prop_map(|secs| {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("2026-01-01T{h:02}:{m:02}:{s:02}Z")
    })
}

/// Build a TOML string for a single `[[package.versions]]` entry.
fn arb_locked_version_toml() -> impl Strategy<Value = String> {
    (arb_version_str(), arb_commit_sha(), arb_timestamp_str()).prop_map(|(version, commit, ts)| {
        format!(
            "[[package.versions]]\n\
                 version = \"{version}\"\n\
                 resolved_commit = \"{commit}\"\n\
                 sha256 = \"\"\n\
                 installed_at = \"{ts}\"\n"
        )
    })
}

/// Build a TOML string fragment for one `[[package]]` table.
///
/// Per ADR-0005, `source` is the `PackageSource` string form (via
/// `Display`/`FromStr`), so it appears as a scalar field on the package
/// table — no nested `[source.Git.location]` table needed.
fn arb_locked_package_toml() -> impl Strategy<Value = String> {
    (
        arb_package_name_str(),
        arb_version_str(),
        arb_locked_version_toml(),
    )
        .prop_map(|(name, active_version, version_toml)| {
            format!(
                "[[package]]\n\
                 name = \"{name}\"\n\
                 active_version = \"{active_version}\"\n\
                 source = \"https://example.com/x/y.git\"\n\
                 \n\
                 {version_toml}"
            )
        })
}

/// Build a full `LockFile` TOML string with 0–3 packages.
fn arb_lockfile_toml() -> impl Strategy<Value = String> {
    (
        arb_timestamp_str(),
        prop::collection::vec(arb_locked_package_toml(), 0..=3),
    )
        .prop_map(|(ts, packages)| {
            let mut s = format!(
                "schema_version = 2\n\
                 generated_by_tau_version = \"0.0.0\"\n\
                 generated_at = \"{ts}\"\n\n"
            );
            for pkg in &packages {
                s.push_str(pkg);
                s.push('\n');
            }
            s
        })
}

proptest! {
    /// Parse a generated lockfile TOML, then re-serialize and parse again.
    /// Asserts structural equality: schema_version, package count, and
    /// active_version strings survive the double round-trip.
    ///
    /// Timestamp comparison is skipped in the proptest body: our generated
    /// timestamps are already whole-second, so humantime-serde precision
    /// loss cannot cause spurious failures.
    #[test]
    fn lockfile_roundtrips_through_toml(toml_str in arb_lockfile_toml()) {
        // First parse — this is what LockFile::load does internally.
        let lf: LockFile = toml::from_str(&toml_str).expect("first parse");

        // Re-serialize (what LockFile::save does).
        let serialized = toml::to_string_pretty(&lf).expect("serialize");

        // Second parse.
        let parsed: LockFile = toml::from_str(&serialized).expect("second parse");

        prop_assert_eq!(parsed.schema_version, lf.schema_version);
        prop_assert_eq!(parsed.packages.len(), lf.packages.len());

        for (orig, got) in lf.packages.iter().zip(parsed.packages.iter()) {
            prop_assert_eq!(orig.name.as_str(), got.name.as_str());
            prop_assert_eq!(orig.active_version.to_string(), got.active_version.to_string());
            prop_assert_eq!(orig.installed_versions.len(), got.installed_versions.len());
        }
    }
}

#[test]
fn lockfile_load_rejects_too_new_schema_version() {
    let tmp = scratch_dir("lockfile-schema-too-new");
    let path = tmp.path().join("tau-lock.toml");
    std::fs::write(
        &path,
        r#"
            schema_version = 999
            generated_by_tau_version = "0.0.0"
            generated_at = "2026-04-27T10:00:00Z"
        "#,
    )
    .unwrap();

    let err = LockFile::load(&path).unwrap_err();
    assert!(matches!(
        err,
        RegistryError::SchemaTooNew {
            found: 999,
            supported: 7,
        }
    ));
}
