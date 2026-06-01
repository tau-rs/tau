//! Pinned contract file shape.
//!
//! Stored at `.tau/mcp/<name>.contract.json` by `tau mcp pin <name>`.
//! Read by `tau build --offline` (PR-4) and by `tau verify --bundle`
//! (PR-6). Carries the full `ServerContract` plus the URL and the
//! pre-computed `contract_hash` so callers can read-and-trust without
//! re-hashing (re-hash is still the runtime defense-in-depth check).

use alloc::string::String;
use serde::{Deserialize, Serialize};

use crate::contract::canonical::{canonical_hash, hash_to_hex, Hash256};
use crate::contract::server_contract::ServerContract;
use crate::McpError;

/// Schema version of the pinned-contract file format.
pub const PINNED_CONTRACT_SCHEMA_VERSION: u32 = 1;

/// A pinned MCP server contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PinnedContract {
    /// Schema version (for forward compat).
    pub schema_version: u32,
    /// Server URL (matches the `[tools.<name>] mcp = "..."` field).
    pub url: String,
    /// Pre-computed contract hash (lowercase hex).
    pub contract_hash_hex: String,
    /// Full server contract snapshot.
    pub contract: ServerContract,
}

impl PinnedContract {
    /// Build a `PinnedContract` from a `(url, ServerContract)` pair,
    /// computing the hash inline.
    pub fn from_parts(url: String, contract: ServerContract) -> Result<Self, McpError> {
        let h = canonical_hash(&contract)?;
        Ok(Self {
            schema_version: PINNED_CONTRACT_SCHEMA_VERSION,
            url,
            contract_hash_hex: hash_to_hex(&h),
            contract,
        })
    }

    /// Decode the `contract_hash_hex` field back to a `Hash256`.
    pub fn decoded_hash(&self) -> Result<Hash256, McpError> {
        decode_hex_hash(&self.contract_hash_hex)
    }

    /// Verify `contract_hash_hex` matches a freshly-computed hash of
    /// `contract`. Used by `tau verify --bundle` and the runtime drift
    /// check.
    pub fn verify_self_hash(&self) -> Result<(), McpError> {
        let observed = canonical_hash(&self.contract)?;
        let observed_hex = hash_to_hex(&observed);
        if observed_hex != self.contract_hash_hex {
            return Err(McpError::ContractDrift {
                observed: observed_hex,
                expected: self.contract_hash_hex.clone(),
            });
        }
        Ok(())
    }
}

fn decode_hex_hash(s: &str) -> Result<Hash256, McpError> {
    if s.len() != 64 {
        return Err(McpError::Protocol(alloc::format!(
            "contract_hash_hex must be 64 chars, got {}",
            s.len()
        )));
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let hi = hex_digit(chunk[0])?;
        let lo = hex_digit(chunk[1])?;
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_digit(b: u8) -> Result<u8, McpError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(McpError::Protocol(alloc::format!(
            "invalid hex digit: 0x{b:02x}"
        ))),
    }
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
    fn round_trips() {
        let p = PinnedContract::from_parts("https://example.com".to_string(), fixture())
            .expect("build");
        let bytes = serde_json::to_vec(&p).expect("serialize");
        let decoded: PinnedContract = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(p, decoded);
    }

    #[test]
    fn verify_self_hash_ok() {
        let p = PinnedContract::from_parts("u".to_string(), fixture()).expect("build");
        p.verify_self_hash().expect("matches");
    }

    #[test]
    fn verify_self_hash_drift() {
        let mut p = PinnedContract::from_parts("u".to_string(), fixture()).expect("build");
        // Tamper with contract; hash field now wrong.
        p.contract.tools[0].description = Some("tampered".to_string());
        let err = p.verify_self_hash().expect_err("should detect drift");
        assert!(matches!(err, McpError::ContractDrift { .. }));
    }

    #[test]
    fn decoded_hash_round_trip() {
        let p = PinnedContract::from_parts("u".to_string(), fixture()).expect("build");
        let h_decoded = p.decoded_hash().expect("decode");
        let h_recomputed = canonical_hash(&p.contract).expect("rehash");
        assert_eq!(h_decoded, h_recomputed);
    }

    #[test]
    fn invalid_hex_length_rejected() {
        let p = PinnedContract {
            schema_version: PINNED_CONTRACT_SCHEMA_VERSION,
            url: "u".to_string(),
            contract_hash_hex: "abc".to_string(),
            contract: fixture(),
        };
        let err = p.decoded_hash().expect_err("should reject short hex");
        assert!(matches!(err, McpError::Protocol(_)));
    }
}
