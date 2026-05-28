//! `tau run --bundle` integrity verifier (Phase 2 §C.3).
//!
//! Confirms a `.tau` bundle matches the source tree at `project_root`
//! before the CLI dispatches the run. See spec
//! `2026-05-27-tau-run-bundle-design.md`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::bundle::manifest::{BundleAgent, BundleManifest};
use crate::bundle::verify_error::VerifyError;

/// Schema version this binary can verify.
const SUPPORTED_SCHEMA_VERSION: u32 = 1;

/// Inputs to [`verify_bundle`].
#[derive(Debug, Clone)]
pub struct VerifyOptions {
    /// Path to the `.tau` bundle file.
    pub bundle_path: PathBuf,
    /// Project source tree to verify against (typically cwd).
    pub project_root: PathBuf,
}

/// Result of a successful verification.
#[derive(Debug, Clone)]
pub struct VerifyReport {
    /// The parsed, self-hash-verified manifest.
    pub manifest: BundleManifest,
    /// Per-agent context resolved during verification, keyed by id.
    pub agent_lookup: BTreeMap<String, ResolvedAgent>,
}

/// Per-agent verification result.
#[derive(Debug, Clone)]
pub struct ResolvedAgent {
    /// The bundle's record for this agent.
    pub bundle_entry: BundleAgent,
    /// The verified-clean system-prompt bytes.
    pub system_prompt: Vec<u8>,
}

/// Verify a bundle against the source tree at
/// [`VerifyOptions::project_root`]. Strict: any drift, target
/// mismatch, or missing/altered install state returns an error.
pub fn verify_bundle(opts: VerifyOptions) -> Result<VerifyReport, VerifyError> {
    // Step 1: read.
    let bundle_str =
        std::fs::read_to_string(&opts.bundle_path).map_err(|e| VerifyError::BundleRead {
            path: opts.bundle_path.clone(),
            source: e,
        })?;
    // Step 2: parse.
    let manifest =
        BundleManifest::parse_str(&bundle_str).map_err(|e| VerifyError::BundleParse { source: e })?;
    // Step 3: self-hash.
    verify_self_hash_step(&manifest)?;
    // Step 4: schema version.
    verify_schema_version(&manifest)?;
    // Step 5: target triple matches host.
    verify_target_matches_host(&manifest)?;

    unimplemented!("steps 6-8 in subsequent tasks")
}

/// Step 5: confirm the bundle was built for the running host. A bundle
/// is host-specific; running one built for another target is rejected.
fn verify_target_matches_host(m: &BundleManifest) -> Result<(), VerifyError> {
    let host = tau_ports::target::TargetTriple::host();
    if m.bundle.target != host {
        return Err(VerifyError::TargetMismatch {
            bundle: m.bundle.target,
            host,
        });
    }
    Ok(())
}

/// Step 3: confirm the bundle's recorded self-hash matches its
/// canonical-TOML content, mapping the integrity error into the
/// verify-error namespace.
fn verify_self_hash_step(m: &BundleManifest) -> Result<(), VerifyError> {
    crate::bundle::hash::verify_self_hash(m).map_err(map_integrity_error)
}

/// Translate a [`BundleIntegrityError`] into a [`VerifyError`].
///
/// [`BundleIntegrityError`]: crate::bundle::error::BundleIntegrityError
fn map_integrity_error(e: crate::bundle::error::BundleIntegrityError) -> VerifyError {
    use crate::bundle::error::BundleIntegrityError;
    match e {
        BundleIntegrityError::HashMismatch { claimed, computed } => {
            VerifyError::SelfHashMismatch { claimed, computed }
        }
        // The hash field being empty is still a self-hash failure: the
        // bundle claims no hash, so it cannot be the one we computed.
        BundleIntegrityError::HashFieldEmpty => VerifyError::SelfHashMismatch {
            claimed: String::new(),
            computed: format!("{e}"),
        },
    }
}

/// Step 4: confirm the bundle's `schema_version` is one this binary can
/// verify.
fn verify_schema_version(m: &BundleManifest) -> Result<(), VerifyError> {
    if m.schema_version != SUPPORTED_SCHEMA_VERSION {
        return Err(VerifyError::UnsupportedSchemaVersion {
            found: m.schema_version,
            supported: SUPPORTED_SCHEMA_VERSION,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::build::{build, BuildOptions};
    use tau_ports::target::TargetTriple;
    use tempfile::tempdir;

    /// Writes a minimal single-agent project (inline prompt, no
    /// packages), builds a bundle, returns its path.
    fn build_minimal_bundle(root: &std::path::Path) -> std::path::PathBuf {
        std::fs::write(
            root.join("tau.toml"),
            r#"
[project]
name = "verify-fixture"
version = "0.1.0"

[agents.solo]
display_name = "Solo"
package = "noop@^0.1"
llm_backend = "anthropic"

[agents.solo.prompt]
system = "you are solo"
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("tau.lock"),
            "schema_version = 6\ngenerated_by_tau_version = \"0.1.0\"\ngenerated_at = \"2024-01-01T00:00:00Z\"\n",
        )
        .unwrap();
        let artifact = build(BuildOptions {
            project_root: root.to_path_buf(),
            target: TargetTriple::host(),
            output_path: None,
        })
        .expect("build fixture bundle");
        artifact.path
    }

    fn vopts(bundle_path: std::path::PathBuf, root: &std::path::Path) -> VerifyOptions {
        VerifyOptions {
            bundle_path,
            project_root: root.to_path_buf(),
        }
    }

    #[test]
    fn verify_rejects_missing_bundle_file() {
        let tmp = tempdir().unwrap();
        let err = verify_bundle(vopts(tmp.path().join("nope.tau"), tmp.path())).unwrap_err();
        assert!(matches!(err, VerifyError::BundleRead { .. }), "got {err:?}");
    }

    #[test]
    fn verify_rejects_malformed_bundle_toml() {
        let tmp = tempdir().unwrap();
        let bad = tmp.path().join("bad.tau");
        std::fs::write(&bad, "this is not valid bundle toml @@@").unwrap();
        let err = verify_bundle(vopts(bad, tmp.path())).unwrap_err();
        assert!(matches!(err, VerifyError::BundleParse { .. }), "got {err:?}");
    }

    #[test]
    fn verify_rejects_self_hash_tampered_bundle() {
        let tmp = tempdir().unwrap();
        let path = build_minimal_bundle(tmp.path());
        let content = std::fs::read_to_string(&path).unwrap();
        let tampered = content.replace("verify-fixture", "tampered-name");
        assert_ne!(content, tampered, "replacement must change content");
        std::fs::write(&path, tampered).unwrap();
        let err = verify_bundle(vopts(path, tmp.path())).unwrap_err();
        assert!(
            matches!(err, VerifyError::SelfHashMismatch { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn verify_schema_version_rejects_v2() {
        let tmp = tempdir().unwrap();
        let path = build_minimal_bundle(tmp.path());
        let s = std::fs::read_to_string(&path).unwrap();
        let mut m = BundleManifest::parse_str(&s).unwrap();
        m.schema_version = 2;
        let err = verify_schema_version(&m).unwrap_err();
        assert!(
            matches!(
                err,
                VerifyError::UnsupportedSchemaVersion {
                    found: 2,
                    supported: 1
                }
            ),
            "got {err:?}",
        );
    }

    #[test]
    fn verify_target_rejects_foreign_triple() {
        let tmp = tempdir().unwrap();
        let path = build_minimal_bundle(tmp.path());
        let s = std::fs::read_to_string(&path).unwrap();
        let mut m = BundleManifest::parse_str(&s).unwrap();
        m.bundle.target = TargetTriple::PASSTHROUGH; // never equals a native host
        let err = verify_target_matches_host(&m).unwrap_err();
        assert!(matches!(err, VerifyError::TargetMismatch { .. }), "got {err:?}");
    }

    #[test]
    fn verify_target_accepts_host_triple() {
        let tmp = tempdir().unwrap();
        let path = build_minimal_bundle(tmp.path());
        let s = std::fs::read_to_string(&path).unwrap();
        let m = BundleManifest::parse_str(&s).unwrap();
        // The fixture was built with TargetTriple::host(), so it matches.
        verify_target_matches_host(&m).expect("host triple matches");
    }
}
