//! Self-hash compute + verify for `BundleManifest`. See spec §5.

use sha2::{Digest, Sha256};

use crate::bundle::canonical::to_canonical_toml;
use crate::bundle::error::BundleIntegrityError;
use crate::bundle::manifest::BundleManifest;

/// Compute the canonical SHA-256 of a manifest with the `bundle.sha256`
/// field forced to the empty string. Does NOT mutate the input.
pub fn compute_self_hash(manifest: &BundleManifest) -> String {
    let mut clone = manifest.clone();
    clone.bundle.sha256 = String::new();
    let canonical = to_canonical_toml(&clone);
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    hex_encode(&hasher.finalize())
}

/// Verify that the manifest's `bundle.sha256` field equals the
/// recomputed self-hash.
pub fn verify_self_hash(manifest: &BundleManifest) -> Result<(), BundleIntegrityError> {
    if manifest.bundle.sha256.is_empty() {
        return Err(BundleIntegrityError::HashFieldEmpty);
    }
    let computed = compute_self_hash(manifest);
    if computed == manifest.bundle.sha256 {
        Ok(())
    } else {
        Err(BundleIntegrityError::HashMismatch {
            claimed: manifest.bundle.sha256.clone(),
            computed,
        })
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::manifest::tests_helpers::sample_manifest;

    #[test]
    fn compute_self_hash_is_deterministic() {
        let m = sample_manifest();
        let a = compute_self_hash(&m);
        let b = compute_self_hash(&m);
        assert_eq!(a, b);
    }

    #[test]
    fn compute_self_hash_does_not_mutate_input() {
        let m = sample_manifest();
        let original_sha = m.bundle.sha256.clone();
        let _ = compute_self_hash(&m);
        assert_eq!(m.bundle.sha256, original_sha);
    }

    #[test]
    fn compute_self_hash_returns_64_hex_chars() {
        let m = sample_manifest();
        let h = compute_self_hash(&m);
        assert_eq!(h.len(), 64);
        assert!(h
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn compute_self_hash_ignores_existing_sha_value() {
        let mut m = sample_manifest();
        m.bundle.sha256 = "9".repeat(64);
        let h1 = compute_self_hash(&m);
        m.bundle.sha256 = "f".repeat(64);
        let h2 = compute_self_hash(&m);
        assert_eq!(
            h1, h2,
            "existing sha value must not affect the computed hash"
        );
    }

    #[test]
    fn verify_self_hash_ok_when_hash_matches() {
        let mut m = sample_manifest();
        m.bundle.sha256 = compute_self_hash(&m);
        verify_self_hash(&m).expect("ok");
    }

    #[test]
    fn verify_self_hash_detects_tampered_package_version() {
        let mut m = sample_manifest();
        m.bundle.sha256 = compute_self_hash(&m);
        // Tamper after hash is set.
        m.packages[0].version = semver::Version::parse("0.2.2").unwrap();
        match verify_self_hash(&m) {
            Err(BundleIntegrityError::HashMismatch { claimed, computed }) => {
                assert_ne!(claimed, computed);
            }
            other => panic!("expected HashMismatch, got {other:?}"),
        }
    }

    #[test]
    fn verify_self_hash_errors_when_field_is_empty() {
        let mut m = sample_manifest();
        m.bundle.sha256.clear();
        match verify_self_hash(&m) {
            Err(BundleIntegrityError::HashFieldEmpty) => {}
            other => panic!("expected HashFieldEmpty, got {other:?}"),
        }
    }
}
