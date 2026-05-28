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
    let manifest = BundleManifest::parse_str(&bundle_str)
        .map_err(|e| VerifyError::BundleParse { source: e })?;
    // Step 3: self-hash.
    verify_self_hash_step(&manifest)?;
    // Step 4: schema version.
    verify_schema_version(&manifest)?;
    // Step 5: target triple matches host.
    verify_target_matches_host(&manifest)?;
    // Step 6: cwd tau.toml matches the bundle's recorded hash.
    verify_tau_toml_sha256(&manifest, &opts.project_root)?;
    // Step 7: every bundled package is installed and its tree is intact.
    verify_packages_installed_and_hashed(&manifest, &opts.project_root)?;
    // Step 8: agent prompts + build agent_lookup.
    let agent_lookup = verify_agent_prompts(&manifest, &opts.project_root)?;
    Ok(VerifyReport {
        manifest,
        agent_lookup,
    })
}

/// Step 8: for every agent recorded in the bundle, re-resolve its system
/// prompt from the cwd's project config and confirm the SHA-256 still
/// matches the value the build recorded. Detects prompt drift (inline or
/// file-based) since build time, and an agent set that no longer matches
/// the bundle.
///
/// The cwd's `tau.toml` was already proven byte-clean in step 6, so we
/// load it through the SAME pipeline `build.rs` step 1 used
/// ([`UncheckedProjectConfig`] → `validate()`). Prompt bytes are
/// resolved via the shared [`crate::bundle::build::resolve_agent_prompt_bytes`]
/// helper that build's step 5 also calls — so a clean verify can never
/// spuriously fail on a prompt hash.
fn verify_agent_prompts(
    m: &BundleManifest,
    project_root: &std::path::Path,
) -> Result<BTreeMap<String, ResolvedAgent>, VerifyError> {
    use crate::project::project::UncheckedProjectConfig;

    let path = project_root.join("tau.toml");
    // Step 6 already confirmed these bytes match the bundle, so any read
    // failure here is genuinely a missing/unreadable file.
    let bytes = std::fs::read(&path).map_err(|source| VerifyError::ProjectTomlRead {
        path: path.clone(),
        source,
    })?;
    // Parse + validate failures map to ProjectTomlRead too — the cwd's
    // config could not be loaded. (Step 6's clean hash means this is
    // unexpected, but we surface it rather than panic.)
    let to_io = |e: String| VerifyError::ProjectTomlRead {
        path: path.clone(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
    };
    let tau_toml_str =
        std::str::from_utf8(&bytes).map_err(|e| to_io(format!("tau.toml is not utf-8: {e}")))?;
    let unchecked: UncheckedProjectConfig =
        toml::from_str(tau_toml_str).map_err(|e| to_io(format!("parse {path:?}: {e}")))?;
    let project_config = unchecked
        .validate()
        .map_err(|e| to_io(format!("validate {path:?}: {e}")))?;

    let mut agent_lookup: BTreeMap<String, ResolvedAgent> = BTreeMap::new();
    for agent in &m.agents {
        let id = agent.id.as_str().to_string();
        let entry = project_config
            .agents
            .get(&id)
            .ok_or_else(|| VerifyError::AgentSetMismatch { id: id.clone() })?;
        let prompt_bytes =
            crate::bundle::build::resolve_agent_prompt_bytes(&entry.prompt, project_root).map_err(
                |source| VerifyError::AgentPromptResolve {
                    id: id.clone(),
                    source,
                },
            )?;
        let computed = crate::bundle::build::sha256_hex(&prompt_bytes);
        if computed != agent.system_prompt_sha256 {
            return Err(VerifyError::AgentPromptDrift {
                id,
                claimed: agent.system_prompt_sha256.clone(),
                computed,
            });
        }
        agent_lookup.insert(
            id,
            ResolvedAgent {
                bundle_entry: agent.clone(),
                system_prompt: prompt_bytes,
            },
        );
    }
    Ok(agent_lookup)
}

/// Step 7: confirm each package recorded in the bundle is installed at
/// `<project_root>/.tau/packages/<name>/<version>/` and that its tree
/// hash still matches the value the build recorded. Detects both a
/// missing install and post-build tampering.
fn verify_packages_installed_and_hashed(
    m: &BundleManifest,
    project_root: &std::path::Path,
) -> Result<(), VerifyError> {
    for pkg in &m.packages {
        let dir = project_root
            .join(".tau/packages")
            .join(&pkg.name)
            .join(pkg.version.to_string());
        if !dir.exists() {
            return Err(VerifyError::PackageMissing {
                name: pkg.name.clone(),
                expected_path: dir,
            });
        }
        let computed =
            crate::tree_hash::tree_hash(&dir).map_err(|e| VerifyError::PackageTreeHash {
                name: pkg.name.clone(),
                source: e,
            })?;
        if computed != pkg.tree_sha256 {
            return Err(VerifyError::PackageDrift {
                name: pkg.name.clone(),
                claimed: pkg.tree_sha256.clone(),
                computed,
            });
        }
    }
    Ok(())
}

/// Step 6: confirm the cwd's `tau.toml` hashes to the value recorded in
/// the bundle, rejecting any drift since build time.
fn verify_tau_toml_sha256(
    m: &BundleManifest,
    project_root: &std::path::Path,
) -> Result<(), VerifyError> {
    let path = project_root.join("tau.toml");
    let bytes = std::fs::read(&path).map_err(|e| VerifyError::ProjectTomlRead {
        path: path.clone(),
        source: e,
    })?;
    let computed = crate::bundle::build::sha256_hex(&bytes);
    if computed != m.project.tau_toml_sha256 {
        return Err(VerifyError::TauTomlDrift {
            claimed: m.project.tau_toml_sha256.clone(),
            computed,
        });
    }
    Ok(())
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
            agent_filter: None,
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
        assert!(
            matches!(err, VerifyError::BundleParse { .. }),
            "got {err:?}"
        );
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
        assert!(
            matches!(err, VerifyError::TargetMismatch { .. }),
            "got {err:?}"
        );
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

    #[test]
    fn verify_tau_toml_drift_detected() {
        let tmp = tempdir().unwrap();
        let path = build_minimal_bundle(tmp.path());
        // Mutate tau.toml after the build so its sha256 changes.
        std::fs::write(
            tmp.path().join("tau.toml"),
            r#"
[project]
name = "verify-fixture"
version = "0.2.0"

[agents.solo]
display_name = "Solo"
package = "noop@^0.1"
llm_backend = "anthropic"

[agents.solo.prompt]
system = "you are solo"
"#,
        )
        .unwrap();
        let s = std::fs::read_to_string(&path).unwrap();
        let m = BundleManifest::parse_str(&s).unwrap();
        let err = verify_tau_toml_sha256(&m, tmp.path()).unwrap_err();
        assert!(
            matches!(err, VerifyError::TauTomlDrift { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn verify_tau_toml_clean_passes() {
        let tmp = tempdir().unwrap();
        let path = build_minimal_bundle(tmp.path());
        let s = std::fs::read_to_string(&path).unwrap();
        let m = BundleManifest::parse_str(&s).unwrap();
        verify_tau_toml_sha256(&m, tmp.path()).expect("unchanged tau.toml verifies");
    }

    /// Writes a project + lockfile + one installed package dir, builds a
    /// bundle, returns (bundle_path, package_dir).
    fn build_bundle_with_one_package(
        root: &std::path::Path,
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        std::fs::write(
            root.join("tau.toml"),
            r#"
[project]
name = "pkg-fixture"
version = "0.1.0"

[agents.solo]
display_name = "Solo"
package = "demo@^0.1"
llm_backend = "anthropic"

[agents.solo.prompt]
system = "hi"
"#,
        )
        .unwrap();
        let pkg_dir = root.join(".tau/packages/demo/0.1.0");
        std::fs::create_dir_all(pkg_dir.join("src")).unwrap();
        std::fs::write(pkg_dir.join("Cargo.toml"), "[package]\nname=\"demo\"\n").unwrap();
        std::fs::write(pkg_dir.join("src/lib.rs"), "// demo\n").unwrap();
        std::fs::write(
            root.join("tau.lock"),
            r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"

[[package]]
name = "demo"
active_version = "0.1.0"
source = "https://example.com/demo.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0000000000000000000000000000000000000001"
installed_at = "2024-01-01T00:00:00Z"
"#,
        )
        .unwrap();
        let artifact = build(BuildOptions {
            project_root: root.to_path_buf(),
            target: TargetTriple::host(),
            output_path: None,
            agent_filter: None,
        })
        .expect("build bundle with package");
        (artifact.path, pkg_dir)
    }

    #[test]
    fn verify_package_missing_detected() {
        let tmp = tempdir().unwrap();
        let (path, pkg_dir) = build_bundle_with_one_package(tmp.path());
        std::fs::remove_dir_all(&pkg_dir).unwrap(); // uninstall after build
        let s = std::fs::read_to_string(&path).unwrap();
        let m = BundleManifest::parse_str(&s).unwrap();
        let err = verify_packages_installed_and_hashed(&m, tmp.path()).unwrap_err();
        assert!(
            matches!(err, VerifyError::PackageMissing { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn verify_package_tree_drift_detected() {
        let tmp = tempdir().unwrap();
        let (path, pkg_dir) = build_bundle_with_one_package(tmp.path());
        std::fs::write(pkg_dir.join("src/lib.rs"), "// tampered\n").unwrap();
        let s = std::fs::read_to_string(&path).unwrap();
        let m = BundleManifest::parse_str(&s).unwrap();
        let err = verify_packages_installed_and_hashed(&m, tmp.path()).unwrap_err();
        assert!(
            matches!(err, VerifyError::PackageDrift { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn verify_packages_clean_passes() {
        let tmp = tempdir().unwrap();
        let (path, _pkg_dir) = build_bundle_with_one_package(tmp.path());
        let s = std::fs::read_to_string(&path).unwrap();
        let m = BundleManifest::parse_str(&s).unwrap();
        verify_packages_installed_and_hashed(&m, tmp.path()).expect("clean packages verify");
    }

    #[test]
    fn verify_happy_path_returns_report_with_agent_lookup() {
        let tmp = tempdir().unwrap();
        let path = build_minimal_bundle(tmp.path());
        let report = verify_bundle(vopts(path, tmp.path())).expect("verify succeeds");
        assert_eq!(report.manifest.project.name, "verify-fixture");
        assert!(report.agent_lookup.contains_key("solo"));
        assert_eq!(report.agent_lookup["solo"].system_prompt, b"you are solo");
    }

    #[test]
    fn verify_agent_prompt_file_drift_detected() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("tau.toml"),
            r#"
[project]
name = "file-prompt"
version = "0.1.0"

[agents.writer]
display_name = "Writer"
package = "noop@^0.1"
llm_backend = "anthropic"

[agents.writer.prompt]
system_file = "prompt.md"
"#,
        )
        .unwrap();
        std::fs::write(tmp.path().join("prompt.md"), "original prompt").unwrap();
        std::fs::write(
            tmp.path().join("tau.lock"),
            "schema_version = 6\ngenerated_by_tau_version = \"0.1.0\"\ngenerated_at = \"2024-01-01T00:00:00Z\"\n",
        )
        .unwrap();
        let artifact = build(BuildOptions {
            project_root: tmp.path().to_path_buf(),
            target: TargetTriple::host(),
            output_path: None,
            agent_filter: None,
        })
        .unwrap();
        // Mutate the prompt FILE after build (tau.toml unchanged, so step 6
        // passes; step 8 must catch the prompt drift).
        std::fs::write(tmp.path().join("prompt.md"), "tampered prompt").unwrap();
        let err = verify_bundle(vopts(artifact.path, tmp.path())).unwrap_err();
        assert!(
            matches!(err, VerifyError::AgentPromptDrift { .. }),
            "got {err:?}"
        );
    }
}
