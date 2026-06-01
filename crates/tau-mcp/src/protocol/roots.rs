//! `roots/list` — server asks host which filesystem roots it may
//! read/write.
//!
//! Per the β.3 design doc §9: tau v0 returns the explicit `roots` field
//! from tau.toml, build-time-checked ⊆ the tool's `fs.read` caps.
//! Default-empty `roots` returns `[]` (server gets no fs access).

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

/// `roots/list` request — empty params.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RootsListRequest {}

/// `roots/list` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RootsListResponse {
    /// Allowed roots; empty array means the server has no host-granted
    /// filesystem visibility (it falls back to its own behavior).
    pub roots: Vec<Root>,
}

/// One root the host advertises.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Root {
    /// URI of the root (typically `"file:///path"`).
    pub uri: String,
    /// Optional human-readable name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;
    use serde_json::json;

    #[test]
    fn roots_response_round_trips() {
        let resp = RootsListResponse {
            roots: vec![Root {
                uri: "file:///tmp/mcp-cache".to_string(),
                name: Some("cache".to_string()),
            }],
        };
        let bytes = serde_json::to_vec(&resp).expect("serialize");
        let decoded: RootsListResponse = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(resp, decoded);
    }

    #[test]
    fn empty_roots_round_trips() {
        let resp = RootsListResponse { roots: vec![] };
        let bytes = serde_json::to_vec(&resp).expect("serialize");
        assert_eq!(
            serde_json::from_slice::<RootsListResponse>(&bytes).unwrap(),
            resp
        );
        // Also accept legacy `{"roots":[]}` form unchanged.
        let wire = json!({"roots":[]});
        let decoded: RootsListResponse = serde_json::from_value(wire).expect("decode");
        assert_eq!(decoded, resp);
    }
}
