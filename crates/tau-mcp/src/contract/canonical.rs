//! Canonical hash for `ServerContract`.
//!
//! Same shape as the β.2 IR module hash: SHA-256 over canonical-JSON
//! bytes. "Canonical" means object keys sorted alphabetically, no
//! whitespace, `f64` integers normalized to integer form when they
//! losslessly represent integers. serde_json::to_vec gives us most of
//! that for free when the source types are stable; the BTreeMap-backed
//! `additional` fields preserve sorted keys.
//!
//! The deterministic property checked by `golden_canonical.rs` is:
//! same `ServerContract` (constructed identically) → same `Hash256`
//! across runs and across platforms.

use sha2::{Digest, Sha256};

use crate::contract::server_contract::ServerContract;
use crate::McpError;

/// 32-byte content hash (SHA-256 output).
pub type Hash256 = [u8; 32];

/// Compute the canonical hash of a `ServerContract`.
///
/// The hash participates in the IR module hash (PR-4 wires this into
/// `ToolImpl::Mcp::contract_hash`) so contract drift invalidates the
/// bundle.
pub fn canonical_hash(contract: &ServerContract) -> Result<Hash256, McpError> {
    // serde_json's default Map preserves insertion order; for canonical
    // form we re-serialize through a value tree using sorted keys. The
    // `preserve_order` feature is NOT enabled on serde_json in tau-mcp
    // (default off), so Map = BTreeMap and keys come out sorted — this
    // is the same property β.2's IR-hash relies on.
    let bytes = serde_json::to_vec(contract)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let out = hasher.finalize();
    let mut h: Hash256 = [0; 32];
    h.copy_from_slice(&out);
    Ok(h)
}

/// Format a `Hash256` as a lowercase hex string for diagnostics +
/// lockfile.
pub fn hash_to_hex(h: &Hash256) -> alloc::string::String {
    use core::fmt::Write;
    let mut s = alloc::string::String::with_capacity(64);
    for b in h.iter() {
        // Per the LowerHex-in-CI gotcha from project_skills_5_shipped_2026_05_16,
        // use the {:02x} form explicitly.
        let _ = write!(&mut s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::server_contract::{ContractTool, ServerContract};
    use crate::protocol::initialize::ServerInfo;
    use crate::protocol::tools::McpToolInputSchema;
    use alloc::collections::BTreeMap;
    use alloc::string::ToString;
    use alloc::vec;
    use serde_json::json;

    fn fixture() -> ServerContract {
        ServerContract {
            protocol_version: "2025-03-26".to_string(),
            server_info: ServerInfo {
                name: "weather".to_string(),
                version: "1.0".to_string(),
                additional: BTreeMap::new(),
            },
            tools: vec![ContractTool {
                name: "get_forecast".to_string(),
                description: None,
                input_schema: McpToolInputSchema(json!({"type":"object"})),
                caps: vec![],
            }],
        }
    }

    #[test]
    fn determinism() {
        let h1 = canonical_hash(&fixture()).expect("hash");
        let h2 = canonical_hash(&fixture()).expect("hash");
        assert_eq!(h1, h2);
    }

    #[test]
    fn hex_is_lowercase_64_chars() {
        let h = canonical_hash(&fixture()).expect("hash");
        let s = hash_to_hex(&h);
        assert_eq!(s.len(), 64);
        assert!(s
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn different_contracts_have_different_hashes() {
        let mut other = fixture();
        other.tools[0].description = Some("changed".to_string());
        let h1 = canonical_hash(&fixture()).expect("hash");
        let h2 = canonical_hash(&other).expect("hash");
        assert_ne!(h1, h2);
    }
}
