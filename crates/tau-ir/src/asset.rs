//! The content-addressed asset store (D6-B).
//!
//! An [`AssetBlob`] is a file-like resource — currently only agent prompts —
//! carried in a bundle and referenced from the IR by content hash
//! (`"sha256:" + 64 lowercase hex`, see [`crate::prompt::PromptSource::Asset`]).
//! Lowering reads the bytes at build time, hashes them, and emits both the
//! `Asset` reference (into the IR) and the blob (into a side map the bundle
//! persists). The interpreter resolves the reference back to bytes at run time.
//!
//! The blob type lives in `tau-ir` (not `tau-ir-lower`) because the no_std
//! interpreter must consume the asset map without pulling in the std lowering
//! stack. The blob is NOT part of [`crate::module::IrModule`] and does not
//! enter the IR hash — assets are a bundle-level sibling of the module.

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Compute the canonical asset hash of `bytes`: `"sha256:" + 64 lowercase hex`.
///
/// This is the single source of truth for the asset-reference format that
/// [`crate::prompt::PromptSource::Asset`] carries and the bundle keys assets
/// by. Its output always satisfies [`crate::prompt::is_well_formed_asset_hash`].
pub fn asset_hash(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let digest = h.finalize();
    let mut s = String::with_capacity("sha256:".len() + 64);
    s.push_str("sha256:");
    for b in digest.iter() {
        // Lowercase hex, two chars per byte.
        s.push(char::from_digit((b >> 4) as u32, 16).expect("nibble < 16"));
        s.push(char::from_digit((b & 0x0f) as u32, 16).expect("nibble < 16"));
    }
    s
}

/// What a stored asset is. `#[non_exhaustive]`: new kinds (skill bodies,
/// templates, schemas) are additive.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum AssetKind {
    /// An agent system prompt.
    Prompt,
}

impl AssetKind {
    /// The stable wire tag used by the bundle's asset section.
    pub fn as_str(&self) -> &'static str {
        match self {
            AssetKind::Prompt => "prompt",
        }
    }
}

/// A content-addressed blob: its [`AssetKind`] plus raw bytes. Keyed elsewhere
/// by its hash (`"sha256:" + 64 lowercase hex`); the hash is not stored inline
/// so the blob cannot disagree with its key.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AssetBlob {
    /// What this asset is.
    pub kind: AssetKind,
    /// The raw content bytes.
    pub bytes: Vec<u8>,
}

impl AssetBlob {
    /// Construct a prompt asset from its bytes.
    pub fn prompt(bytes: Vec<u8>) -> Self {
        Self {
            kind: AssetKind::Prompt,
            bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::is_well_formed_asset_hash;

    #[test]
    fn asset_hash_matches_known_empty_digest_and_is_well_formed() {
        // SHA-256 of the empty input.
        let h = asset_hash(b"");
        assert_eq!(
            h,
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert!(is_well_formed_asset_hash(&h));
    }

    #[test]
    fn asset_hash_is_lowercase_and_stable() {
        let a = asset_hash(b"You are a careful assistant.");
        let b = asset_hash(b"You are a careful assistant.");
        assert_eq!(a, b);
        assert!(is_well_formed_asset_hash(&a));
        assert!(a.starts_with("sha256:"));
    }
}
