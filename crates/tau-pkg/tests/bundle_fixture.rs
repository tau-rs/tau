//! Integration test: realistic bundle round-trips through file IO.
//!
//! This is a Layer 3 check on top of the unit tests in `tau-pkg::bundle`.
//! It builds a plausible bundle, writes it to disk, reads it back, and
//! verifies the self-hash matches.

use tau_pkg::bundle::{
    BackendRef, BundleAgent, BundleEffectiveCapabilities, BundleManifest, BundleMeta,
    BundlePackage, ProjectInfo,
};

#[test]
fn realistic_bundle_round_trips_through_disk() {
    let mut manifest = BundleManifest {
        schema_version: 1,
        bundle: BundleMeta {
            sha256: String::new(),
            created_at: "2026-05-19T13:42:11Z".into(),
            tau_version: "0.1.0".into(),
            target: "linux-native-strict".parse().unwrap(),
        },
        project: ProjectInfo {
            name: "support-bot".into(),
            version: semver::Version::parse("0.3.2").unwrap(),
            tau_toml_sha256: "a".repeat(64),
        },
        packages: vec![
            BundlePackage {
                name: "tau-plugin-fs-read".into(),
                version: semver::Version::parse("0.2.1").unwrap(),
                source: tau_domain::PackageSource::Git {
                    location: tau_domain::GitLocation::Url(
                        "https://github.com/example/fs-read.git".parse().unwrap(),
                    ),
                    rev: Some("v0.2.1".into()),
                },
                tree_sha256: "1".repeat(64),
                binary_sha256: Some("2".repeat(64)),
                required_shapes: vec![tau_domain::CapabilityShape::FilesystemRead],
            },
            BundlePackage {
                name: "tau-plugin-shell".into(),
                version: semver::Version::parse("0.4.0").unwrap(),
                source: tau_domain::PackageSource::Git {
                    location: tau_domain::GitLocation::Url(
                        "https://github.com/example/shell.git".parse().unwrap(),
                    ),
                    rev: Some("v0.4.0".into()),
                },
                tree_sha256: "3".repeat(64),
                binary_sha256: Some("4".repeat(64)),
                required_shapes: vec![tau_domain::CapabilityShape::ProcessExec],
            },
        ],
        agents: vec![BundleAgent {
            id: "researcher".parse().unwrap(),
            backend: BackendRef {
                kind: "ollama".into(),
                model: Some("llama3.1:8b".into()),
                extra: std::collections::BTreeMap::new(),
            },
            system_prompt_sha256: "7".repeat(64),
            required_tools: vec!["tau-plugin-fs-read".into()],
            effective_capabilities: BundleEffectiveCapabilities {
                allow_fs_read: vec!["/data/**".into()],
                deny_fs_read: vec!["/data/secrets/**".into()],
                ..Default::default()
            },
        }],
    };

    // Compute the self-hash and store it.
    manifest.bundle.sha256 = manifest.compute_self_hash();

    // Write canonical TOML to a tempdir.
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("support-bot.tau");
    std::fs::write(&path, manifest.to_canonical_toml()).expect("write");

    // Read back from disk and verify.
    let parsed = BundleManifest::from_path(&path).expect("from_path");
    parsed.verify_self_hash().expect("verify");

    // Spot-check a few fields survived the round trip.
    assert_eq!(parsed.project.name, "support-bot");
    assert_eq!(parsed.packages.len(), 2);
    assert_eq!(parsed.agents.len(), 1);
    assert_eq!(parsed.agents[0].id.as_str(), "researcher");
}

#[test]
fn tampered_bundle_on_disk_fails_verification() {
    let mut manifest = BundleManifest {
        schema_version: 1,
        bundle: BundleMeta {
            sha256: String::new(),
            created_at: "2026-05-19T13:42:11Z".into(),
            tau_version: "0.1.0".into(),
            target: "passthrough".parse().unwrap(),
        },
        project: ProjectInfo {
            name: "tiny".into(),
            version: semver::Version::parse("0.1.0").unwrap(),
            tau_toml_sha256: "a".repeat(64),
        },
        packages: vec![],
        agents: vec![],
    };
    manifest.bundle.sha256 = manifest.compute_self_hash();

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("tiny.tau");

    // Tamper: change project name AFTER hashing.
    let mut tampered = manifest.clone();
    tampered.project.name = "huge".into();
    // Keep the original (now-stale) hash; write tampered body.
    tampered.bundle.sha256 = manifest.bundle.sha256.clone();
    std::fs::write(&path, tampered.to_canonical_toml()).expect("write");

    let parsed = BundleManifest::from_path(&path).expect("from_path");
    let err = parsed.verify_self_hash().expect_err("should detect tamper");
    let msg = err.to_string();
    assert!(msg.contains("mismatch"), "unexpected error: {msg}");
}
