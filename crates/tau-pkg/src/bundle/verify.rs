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
pub fn verify_bundle(_opts: VerifyOptions) -> Result<VerifyReport, VerifyError> {
    unimplemented!("filled in by subsequent tasks")
}
