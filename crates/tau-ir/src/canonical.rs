//! Deterministic serialization of an `IrModule` to canonical bytes.
//!
//! Rules (per design spec D-6):
//! 1. Deserialize once, re-serialize via the canonical encoder. The
//!    canonical encoder writes fields in a fixed order, uses BTreeMap
//!    iteration (alphabetical) for every map, and omits optional
//!    fields whose value equals the type's default.
//! 2. No `SystemTime` in the bytes (i64-ms only — enforced by the type
//!    surface, not by this encoder).
//! 3. The encoder is idempotent: `decode(encode(x)) == x` and
//!    `encode(decode(encode(x))) == encode(x)`.

use alloc::vec::Vec;

use crate::module::IrModule;

/// Serialize an `IrModule` to canonical bytes.
///
/// Uses serde_json's compact (no-pretty) encoder over the IrModule's
/// derived Serialize impl. Map iteration is BTreeMap (alphabetical) by
/// the type's structure. Optional fields with default values are
/// elided via `#[serde(default, skip_serializing_if = "Default::default")]`
/// attributes on the type definitions.
pub fn to_canonical_bytes(module: &IrModule) -> Vec<u8> {
    serde_json::to_vec(module).expect("IrModule serializes cleanly to JSON")
}

/// Deserialize canonical bytes back to an `IrModule`. Pure inverse of
/// `to_canonical_bytes`.
pub fn from_canonical_bytes(bytes: &[u8]) -> Result<IrModule, serde_json::Error> {
    serde_json::from_slice(bytes)
}
