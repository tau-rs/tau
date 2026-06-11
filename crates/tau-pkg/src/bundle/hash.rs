//! Self-hash compute + verify for `BundleManifest`. See spec §5.

use sha2::{Digest, Sha256};

use crate::bundle::canonical::to_canonical_toml;
use crate::bundle::error::BundleIntegrityError;
use crate::bundle::manifest::BundleManifest;

/// Canonical sentinel timestamp (RFC 3339 formatted UNIX_EPOCH) used by
/// `compute_self_hash` to zero the `bundle.created_at` field before
/// canonicalizing. This sentinel ensures reproducibility across builds
/// of identical source.
const ZEROED_TIMESTAMP: &str = "1970-01-01T00:00:00Z";

/// Compute the canonical SHA-256 of a manifest with the `bundle.sha256`
/// and `bundle.created_at` fields zeroed. Does NOT mutate the input.
///
/// `created_at` is excluded from the hash domain so two builds of identical
/// source produce identical hashes (per spec §2 stable-bundles decision).
/// The field is preserved in the final on-disk manifest — only the
/// *hash domain* excludes it.
pub fn compute_self_hash(manifest: &BundleManifest) -> String {
    let mut clone = manifest.clone();
    clone.bundle.sha256 = String::new();
    clone.bundle.created_at = ZEROED_TIMESTAMP.into();
    let canonical = to_canonical_toml(&clone);
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    hex_encode(&hasher.finalize())
}

/// Verify the bundle's recorded self-hash against its canonical content.
///
/// This is an **integrity** check — it detects corruption or tampering of
/// the sealed bytes. It is **not** a signature and proves nothing about
/// *who* built the bundle or whether its source is trustworthy (see the
/// module doc on `bundle::verify` for the integrity / correspondence /
/// authenticity distinction).
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

    #[test]
    fn compute_self_hash_zeros_created_at() {
        let mut a = sample_manifest();
        let mut b = sample_manifest();
        // Same content, different `created_at` — must hash identically.
        a.bundle.created_at = "2026-01-01T00:00:00Z".into();
        b.bundle.created_at = "2026-12-31T23:59:59Z".into();
        let ha = compute_self_hash(&a);
        let hb = compute_self_hash(&b);
        assert_eq!(ha, hb, "hashes must be independent of created_at");
    }
}
