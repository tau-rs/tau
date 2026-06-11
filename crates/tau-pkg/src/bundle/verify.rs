//! `tau run --bundle` integrity verifier (Phase 2 §C.3).
//!
//! Confirms a `.tau` bundle matches the source tree at `project_root`
//! before the CLI dispatches the run. See spec
//! `2026-05-27-tau-run-bundle-design.md`.
//!
//! # What "verified" means here
//!
//! This pipeline provides two guarantees and deliberately *not* a third:
//!
//! - **Integrity** (step 3, self-hash): the bundle's bytes have not been
//!   corrupted or altered since its builder sealed it. This is a checksum
//!   the builder computed over its own output — **not** a signature.
//! - **Source correspondence** (steps 6, 9, 10): the cwd `tau.toml`, the
//!   embedded IR bytes, and the IR the source lowers to all agree, so the
//!   executed workflow matches the source the user inspected.
//! - **Authenticity is *not* provided.** Nothing here proves *who* built
//!   the bundle or that its author is trustworthy; there is no signature.
//!   Trusting a bundle still means trusting whoever produced its source
//!   (see the `tau install` trust boundary in `SECURITY.md`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::bundle::manifest::{BundleAgent, BundleManifest};
use crate::bundle::verify_error::VerifyError;

/// Maximum schema version this binary can verify.
const MAX_SUPPORTED_SCHEMA_VERSION: u32 = 2;

/// Inputs to [`verify_bundle`].
#[derive(Debug, Clone)]
pub struct VerifyOptions {
    /// Path to the `.tau` bundle file.
    pub bundle_path: PathBuf,
    /// Project source tree to verify against (typically cwd).
    pub project_root: PathBuf,
    /// The canonical IR hash the caller recomputed by re-lowering the
    /// cwd source (tau-cli owns lowering — see the design doc's layering
    /// note). `None` means the caller could not lower the source; for a
    /// v2 bundle that is a fail-closed refusal (`IrSourceUnverifiable`).
    pub recomputed_ir_hash: Option<String>,
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
    // Step 9: IR payload integrity (v2 bundles only).
    verify_ir_payload(&manifest)?;
    // Step 10: cross-check the embedded IR against the verified source.
    // Steps 6 + 9 prove the tau.toml and the IR bytes individually; this
    // ties them together so the executed IR cannot diverge from the
    // inspected source. See the fn doc for the v1 / fail-closed cases.
    verify_ir_matches_source(&manifest, opts.recomputed_ir_hash.as_deref())?;
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

/// Step 10: cross-check the bundle's embedded IR against the verified
/// source. `recomputed_ir_hash` is the canonical IR hash the caller
/// produced by re-lowering the cwd `tau.toml` (already proven byte-clean
/// by step 6).
///
/// This is the edge that turns the pipeline's *integrity* guarantee into
/// a *source-correspondence* guarantee: combined with step 9 (stored
/// hash == embedded IR bytes), a match here proves the executed IR is
/// exactly what the inspected `tau.toml` lowers to.
///
/// - v1 bundle (`ir_payload` is `None`): no IR to diverge — `Ok`.
/// - v2 bundle + `recomputed_ir_hash` `Some`: the hashes must be equal,
///   else [`VerifyError::IrSourceDivergence`].
/// - v2 bundle + `recomputed_ir_hash` `None`: the source could not be
///   re-lowered to authenticate the IR — fail closed with
///   [`VerifyError::IrSourceUnverifiable`].
fn verify_ir_matches_source(
    m: &BundleManifest,
    recomputed_ir_hash: Option<&str>,
) -> Result<(), VerifyError> {
    let Some(ir) = &m.ir_payload else {
        return Ok(()); // v1 bundle — nothing to cross-check.
    };
    match recomputed_ir_hash {
        Some(source_hash) => {
            if source_hash != ir.canonical_ir_hash {
                return Err(VerifyError::IrSourceDivergence {
                    bundle_hash: ir.canonical_ir_hash.clone(),
                    source_hash: source_hash.to_string(),
                });
            }
            Ok(())
        }
        None => Err(VerifyError::IrSourceUnverifiable),
    }
}

/// Step 9: if `manifest.ir_payload` is `Some`, verify that the SHA-256
/// of `canonical_ir_bytes_hex` (decoded) matches the stored `canonical_ir_hash`.
/// This detects post-build tampering of the IR bytes independent of the
/// bundle's overall self-hash check (step 3).
fn verify_ir_payload(m: &BundleManifest) -> Result<(), VerifyError> {
    use sha2::{Digest, Sha256};
    if let Some(ir) = &m.ir_payload {
        // Decode the hex-encoded IR bytes.
        let bytes = ir
            .canonical_ir_bytes()
            .map_err(|e| VerifyError::IrPayloadDrift {
                claimed: ir.canonical_ir_hash.clone(),
                computed: format!("hex decode failed: {e}"),
            })?;
        // Recompute SHA-256 of the decoded bytes.
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let computed_bytes: [u8; 32] = hasher.finalize().into();
        let computed_hex = crate::tree_hash::to_hex_lower(&computed_bytes);
        if computed_hex != ir.canonical_ir_hash {
            return Err(VerifyError::IrPayloadDrift {
                claimed: ir.canonical_ir_hash.clone(),
                computed: computed_hex,
            });
        }
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
/// verify (1 or 2).
fn verify_schema_version(m: &BundleManifest) -> Result<(), VerifyError> {
    if m.schema_version == 0 || m.schema_version > MAX_SUPPORTED_SCHEMA_VERSION {
        return Err(VerifyError::UnsupportedSchemaVersion {
            found: m.schema_version,
            supported: MAX_SUPPORTED_SCHEMA_VERSION,
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
            ir_payload: None,
        })
        .expect("build fixture bundle");
        artifact.path
    }

    fn vopts(bundle_path: std::path::PathBuf, root: &std::path::Path) -> VerifyOptions {
        VerifyOptions {
            bundle_path,
            project_root: root.to_path_buf(),
            // Existing tests target steps 1–9; leave step 10 inert. The
            // happy-path fixture is v1 (no ir_payload), so None is a
            // no-op here, not a fail-closed refusal.
            recomputed_ir_hash: None,
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
    fn verify_schema_version_accepts_v2() {
        let tmp = tempdir().unwrap();
        let path = build_minimal_bundle(tmp.path());
        let s = std::fs::read_to_string(&path).unwrap();
        let mut m = BundleManifest::parse_str(&s).unwrap();
        m.schema_version = 2;
        verify_schema_version(&m).expect("v2 must be accepted");
    }

    #[test]
    fn verify_schema_version_rejects_v99() {
        let tmp = tempdir().unwrap();
        let path = build_minimal_bundle(tmp.path());
        let s = std::fs::read_to_string(&path).unwrap();
        let mut m = BundleManifest::parse_str(&s).unwrap();
        m.schema_version = 99;
        let err = verify_schema_version(&m).unwrap_err();
        assert!(
            matches!(
                err,
                VerifyError::UnsupportedSchemaVersion {
                    found: 99,
                    supported: 2
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
            ir_payload: None,
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
            ir_payload: None,
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

    /// Parse the minimal fixture bundle and attach a synthetic, internally
    /// consistent v2 `ir_payload` (its `canonical_ir_hash` matches its
    /// bytes). Returns the manifest + the genuine IR hash.
    fn manifest_with_ir(root: &std::path::Path) -> (BundleManifest, String) {
        use crate::bundle::manifest::IrPayload;
        use sha2::{Digest, Sha256};
        let path = build_minimal_bundle(root);
        let s = std::fs::read_to_string(&path).unwrap();
        let mut m = BundleManifest::parse_str(&s).unwrap();
        let bytes: Vec<u8> = b"genuine ir bytes".to_vec();
        let mut h = Sha256::new();
        h.update(&bytes);
        let hash_bytes: [u8; 32] = h.finalize().into();
        let hash_hex = crate::tree_hash::to_hex_lower(&hash_bytes);
        m.ir_payload = Some(IrPayload {
            ir_format: "v1.0.0".to_string(),
            canonical_ir_hash: hash_hex.clone(),
            canonical_ir_bytes_hex: crate::tree_hash::to_hex_lower(&bytes),
        });
        (m, hash_hex)
    }

    #[test]
    fn ir_xcheck_rejects_divergent_source_hash() {
        let tmp = tempdir().unwrap();
        let (m, _genuine) = manifest_with_ir(tmp.path());
        let err = verify_ir_matches_source(&m, Some("deadbeefdivergent")).unwrap_err();
        match err {
            VerifyError::IrSourceDivergence {
                bundle_hash,
                source_hash,
            } => {
                assert_eq!(source_hash, "deadbeefdivergent");
                assert_eq!(bundle_hash, m.ir_payload.unwrap().canonical_ir_hash);
            }
            other => panic!("expected IrSourceDivergence, got {other:?}"),
        }
    }

    #[test]
    fn ir_xcheck_accepts_matching_source_hash() {
        let tmp = tempdir().unwrap();
        let (m, genuine) = manifest_with_ir(tmp.path());
        verify_ir_matches_source(&m, Some(&genuine)).expect("matching hash must pass");
    }

    #[test]
    fn ir_xcheck_fails_closed_when_source_unlowerable() {
        let tmp = tempdir().unwrap();
        let (m, _genuine) = manifest_with_ir(tmp.path());
        let err = verify_ir_matches_source(&m, None).unwrap_err();
        assert!(
            matches!(err, VerifyError::IrSourceUnverifiable),
            "got {err:?}"
        );
    }

    #[test]
    fn ir_xcheck_noop_for_v1_bundle() {
        let tmp = tempdir().unwrap();
        let path = build_minimal_bundle(tmp.path()); // v1 — no ir_payload
        let s = std::fs::read_to_string(&path).unwrap();
        let m = BundleManifest::parse_str(&s).unwrap();
        assert!(m.ir_payload.is_none(), "fixture must be v1 for this test");
        verify_ir_matches_source(&m, None).expect("v1 + None must pass");
        verify_ir_matches_source(&m, Some("anything")).expect("v1 + Some must pass");
    }

    /// Build a bundle with a synthetic `ir_payload` where the
    /// `canonical_ir_hash` is correct. Then corrupt one hex char of
    /// `canonical_ir_bytes_hex` and assert that `verify_ir_payload` catches
    /// the tamper and returns an error whose string contains "ir_payload".
    #[test]
    fn verify_detects_ir_payload_drift() {
        use crate::bundle::manifest::IrPayload;
        use sha2::{Digest, Sha256};

        // Build a valid manifest first (schema_version = 2, self-hash set).
        let tmp = tempdir().unwrap();
        let bundle_path = build_minimal_bundle(tmp.path());
        let s = std::fs::read_to_string(&bundle_path).unwrap();
        let mut m = BundleManifest::parse_str(&s).unwrap();

        // Attach a synthetic IrPayload with correct hash.
        let bytes: Vec<u8> = b"fake ir bytes".to_vec();
        let mut h = Sha256::new();
        h.update(&bytes);
        let hash_bytes: [u8; 32] = h.finalize().into();
        let hash_hex = crate::tree_hash::to_hex_lower(&hash_bytes);
        let bytes_hex = crate::tree_hash::to_hex_lower(&bytes);
        m.ir_payload = Some(IrPayload {
            ir_format: "v1.0.0".to_string(),
            canonical_ir_hash: hash_hex.clone(),
            canonical_ir_bytes_hex: bytes_hex.clone(),
        });

        // Correct hash → verify_ir_payload must pass.
        verify_ir_payload(&m).expect("clean ir_payload must pass");

        // Corrupt the bytes hex (flip one char) → verify_ir_payload must fail.
        if let Some(p) = m.ir_payload.as_mut() {
            // XOR the first character: '0'→'f', 'f'→'0', etc.
            let mut hex_chars: Vec<char> = p.canonical_ir_bytes_hex.chars().collect();
            hex_chars[0] = if hex_chars[0] == '0' { 'f' } else { '0' };
            p.canonical_ir_bytes_hex = hex_chars.into_iter().collect();
        }
        let err = verify_ir_payload(&m).expect_err("tampered bytes must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("ir_payload"),
            "error must mention 'ir_payload'; got: {msg}"
        );
        assert!(
            matches!(err, VerifyError::IrPayloadDrift { .. }),
            "got {err:?}"
        );
    }
}
