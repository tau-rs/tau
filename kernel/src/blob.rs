//! The content-addressed payload store.
//!
//! Payloads are not in the log (ADR-0004): an envelope carries a [`BlobRef`]
//! and the bytes live here. M0 ships the smallest store that makes the loop
//! run — in memory, SHA-256, no eviction. Persistence and crypto-shredding are
//! M3, and neither changes the ABI, because the ABI deliberately names no hash
//! function.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::abi::BlobRef;

/// SHA-256 of `bytes`.
pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// An in-memory content-addressed store.
#[derive(Debug, Default)]
pub struct BlobStore {
    blobs: BTreeMap<BlobRef, Vec<u8>>,
}

impl BlobStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The reference `bytes` would be stored under, without storing them.
    ///
    /// The empty payload maps to [`BlobRef::EMPTY`] rather than to the digest
    /// of zero bytes, because the ABI reserves that value for exactly this.
    #[must_use]
    pub fn digest(bytes: &[u8]) -> BlobRef {
        if bytes.is_empty() {
            BlobRef::EMPTY
        } else {
            BlobRef::from_bytes(sha256(bytes))
        }
    }

    /// Stores `bytes` and returns their reference. Idempotent.
    pub fn put(&mut self, bytes: &[u8]) -> BlobRef {
        let blob = Self::digest(bytes);
        self.blobs.entry(blob).or_insert_with(|| bytes.to_vec());
        blob
    }

    /// The bytes behind `blob`, if this store holds them.
    #[must_use]
    pub fn get(&self, blob: &BlobRef) -> Option<&[u8]> {
        if *blob == BlobRef::EMPTY {
            return Some(&[]);
        }
        self.blobs.get(blob).map(Vec::as_slice)
    }

    /// How many distinct payloads are stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.blobs.len()
    }

    /// Whether the store holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blobs.is_empty()
    }
}
