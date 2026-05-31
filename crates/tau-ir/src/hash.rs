//! SHA-256 over the canonical bytes of an `IrModule`.

use sha2::{Digest, Sha256};

use crate::canonical::to_canonical_bytes;
use crate::module::IrModule;

/// Compute the 32-byte content hash of an `IrModule`.
pub fn compute_hash(module: &IrModule) -> [u8; 32] {
    let bytes = to_canonical_bytes(module);
    let mut h = Sha256::new();
    h.update(&bytes);
    h.finalize().into()
}
