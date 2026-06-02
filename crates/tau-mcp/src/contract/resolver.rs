//! `McpContractResolver` port trait + impls.
//!
//! The resolver feeds tau-ir's `Caches::mcp_contract` closure. Two impls
//! ship in v0:
//!
//! - [`PinnedResolver`] — sync; reads `.tau/mcp/<entry>.contract.json`
//!   per `tau build --offline`.
//! - `tau_mcp_tokio::resolver::LiveMcpContractResolver` — async; performs
//!   the MCP handshake via `host_lifecycle::open` + `tools/list`.
//!
//! Both impls populate a sync cache (`BTreeMap<url, ResolvedMcpContract>`)
//! that the lowering stage reads through.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(feature = "with-std-adapters")]
use crate::contract::pinned::PinnedContract;
use crate::contract::server_contract::{ContractTool, ServerContract};
use crate::McpError;
use tau_domain::Capability;

/// Per-server-tool slice of a resolved MCP contract. Mirror of
/// `tau_ir::lower::ResolvedServerTool` — the resolver returns this type
/// and tau-cli converts to tau-ir's shape (trivial field-by-field map).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedServerTool {
    /// Server-side tool name.
    pub name: String,
    /// Capability kind names the contract declares for this tool
    /// (e.g. `"fs.read"`, `"net.http"`). v0 uses the kind string;
    /// future may carry structured caps once the MCP spec standardises
    /// per-tool capability declarations.
    pub caps: Vec<String>,
    /// JSON schema for the tool's input (opaque pass-through).
    pub input_schema: serde_json::Value,
}

/// Resolved MCP contract for one tau.toml `[tools.<entry>]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedMcpContract {
    /// SHA-256 of the canonical contract.
    pub hash: [u8; 32],
    /// All server-side tools the contract advertises.
    pub expanded_tools: Vec<ResolvedServerTool>,
    /// True iff any expanded tool's caps include `sampling.*`.
    pub requires_sampling: bool,
}

/// Resolver errors.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ResolveError {
    /// Pinned file not found.
    #[error("pinned contract file not found at {path:?}")]
    PinnedFileMissing {
        /// Path the resolver tried to read.
        path: String,
    },
    /// Pinned file present but unreadable.
    #[error("pinned contract file unreadable at {path:?}: {reason}")]
    PinnedFileUnreadable {
        /// Path attempted.
        path: String,
        /// I/O error message.
        reason: String,
    },
    /// Pinned file present but content didn't parse.
    #[error("pinned contract file parse failure at {path:?}: {reason}")]
    PinnedFileParse {
        /// Path attempted.
        path: String,
        /// serde / hashing error.
        reason: String,
    },
    /// Contract hash from the pinned file failed self-verification.
    #[error("pinned contract self-hash mismatch at {path:?}")]
    PinnedFileSelfHashMismatch {
        /// Path attempted.
        path: String,
    },
}

impl From<ResolveError> for McpError {
    fn from(e: ResolveError) -> Self {
        McpError::Protocol(alloc::format!("resolver: {e}"))
    }
}

/// The resolver port trait.
///
/// Implementors fetch a contract for one tau.toml `[tools.<entry>]`
/// (identified by `entry` name + `url`). v0 callers pre-fetch all
/// entries before lowering; the trait does not need to be cache-aware.
pub trait McpContractResolver {
    /// Resolve `(entry, url)` to a `ResolvedMcpContract`.
    ///
    /// Note: this method is **sync**. The live resolver in
    /// tau-mcp-tokio handles its own async-to-sync bridging (the
    /// async pre-fetch loop runs before lower; the cache is sync).
    fn resolve(&self, entry: &str, url: &str) -> Result<ResolvedMcpContract, ResolveError>;
}

/// Pinned-file resolver. Reads `<base>/<entry>.contract.json`.
///
/// `base` is typically `<project_root>/.tau/mcp/`.
#[cfg(feature = "with-std-adapters")]
pub struct PinnedResolver {
    base: std::path::PathBuf,
}

#[cfg(feature = "with-std-adapters")]
impl PinnedResolver {
    /// Construct with the directory holding `<entry>.contract.json` files.
    pub fn new(base: impl Into<std::path::PathBuf>) -> Self {
        Self { base: base.into() }
    }

    /// Build the on-disk path for one entry.
    pub fn pinned_path(&self, entry: &str) -> std::path::PathBuf {
        self.base.join(alloc::format!("{entry}.contract.json"))
    }
}

#[cfg(feature = "with-std-adapters")]
impl McpContractResolver for PinnedResolver {
    fn resolve(&self, entry: &str, url: &str) -> Result<ResolvedMcpContract, ResolveError> {
        let _ = url; // pinned file is keyed by entry name; URL is the cache key
        let path = self.pinned_path(entry);
        let bytes = std::fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ResolveError::PinnedFileMissing {
                    path: path.display().to_string(),
                }
            } else {
                ResolveError::PinnedFileUnreadable {
                    path: path.display().to_string(),
                    reason: alloc::format!("{e}"),
                }
            }
        })?;
        let pinned: PinnedContract = serde_json::from_slice(&bytes).map_err(|e| {
            ResolveError::PinnedFileParse {
                path: path.display().to_string(),
                reason: alloc::format!("{e}"),
            }
        })?;
        pinned.verify_self_hash().map_err(|_| {
            ResolveError::PinnedFileSelfHashMismatch {
                path: path.display().to_string(),
            }
        })?;
        let hash = pinned.decoded_hash().map_err(|e| {
            ResolveError::PinnedFileParse {
                path: path.display().to_string(),
                reason: alloc::format!("decoded_hash: {e}"),
            }
        })?;
        Ok(resolved_from_server_contract(hash, &pinned.contract))
    }
}

/// Public conversion: `ServerContract` (from a live handshake OR a
/// pinned file) + `hash` → `ResolvedMcpContract`. Used by both
/// `PinnedResolver` and `tau_mcp_tokio::resolver::LiveMcpContractResolver`.
pub fn resolved_from_server_contract(
    hash: [u8; 32],
    contract: &ServerContract,
) -> ResolvedMcpContract {
    let mut requires_sampling = false;
    let mut expanded_tools = Vec::with_capacity(contract.tools.len());
    for t in &contract.tools {
        let caps = caps_from_contract_tool(t);
        if caps.iter().any(|c| c.starts_with("sampling.")) {
            requires_sampling = true;
        }
        expanded_tools.push(ResolvedServerTool {
            name: t.name.clone(),
            caps,
            input_schema: t.input_schema.clone().0,
        });
    }
    ResolvedMcpContract {
        hash,
        expanded_tools,
        requires_sampling,
    }
}

/// Extract capability kind strings from a `ContractTool`.
///
/// `ContractTool.caps` carries server-declared [`Capability`] values.
/// v0 maps each to its wire `kind` string (e.g. `"fs.read"`, `"net.http"`).
/// Per-tool cap declarations are a tau-specific extension; most off-the-shelf
/// MCP servers ship an empty caps vec, and the author's envelope governs the
/// upper bound in that case.
fn caps_from_contract_tool(t: &ContractTool) -> Vec<String> {
    t.caps.iter().map(cap_kind_string).collect()
}

/// Return the wire `kind` string for a [`Capability`] variant.
fn cap_kind_string(cap: &Capability) -> String {
    use tau_domain::{AgentCapability, FsCapability, NetCapability, ProcessCapability, SkillCapability};
    match cap {
        Capability::Filesystem(FsCapability::Read { .. }) => "fs.read".to_string(),
        Capability::Filesystem(FsCapability::Write { .. }) => "fs.write".to_string(),
        Capability::Filesystem(FsCapability::Exec { .. }) => "fs.exec".to_string(),
        Capability::Network(NetCapability::Http { .. }) => "net.http".to_string(),
        Capability::Process(ProcessCapability::Spawn { .. }) => "process.spawn".to_string(),
        Capability::Agent(AgentCapability::Spawn { .. }) => "agent.spawn".to_string(),
        Capability::Skill(SkillCapability::Spawn { .. }) => "skill.spawn".to_string(),
        Capability::TaskList { mode } => alloc::format!("task_list.{mode}"),
        Capability::Plan { mode } => alloc::format!("plan.{mode}"),
        Capability::Custom { name, .. } => name.clone(),
        // Exhaustive match: new variants added to tau-domain will cause a
        // compile error here, prompting the implementer to add a mapping.
        _ => "unknown".to_string(),
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

    fn two_tool_contract() -> ServerContract {
        ServerContract {
            protocol_version: "2025-03-26".to_string(),
            server_info: ServerInfo {
                name: "weather".to_string(),
                version: "1.0".to_string(),
                additional: BTreeMap::new(),
            },
            tools: vec![
                ContractTool {
                    name: "get_forecast".to_string(),
                    description: Some("Get weather forecast".to_string()),
                    input_schema: McpToolInputSchema(json!({
                        "type": "object",
                        "properties": {
                            "lat": {"type": "number"},
                            "lon": {"type": "number"}
                        }
                    })),
                    caps: vec![],
                },
                ContractTool {
                    name: "get_current".to_string(),
                    description: Some("Get current conditions".to_string()),
                    input_schema: McpToolInputSchema(json!({
                        "type": "object",
                        "properties": {
                            "city": {"type": "string"}
                        }
                    })),
                    caps: vec![],
                },
            ],
        }
    }

    #[test]
    fn resolved_from_server_contract_round_trip() {
        let contract = two_tool_contract();
        let expected_hash =
            crate::contract::canonical::canonical_hash(&contract).expect("hash");
        let resolved = resolved_from_server_contract(expected_hash, &contract);

        // hash must match the canonical hash
        assert_eq!(resolved.hash, expected_hash);
        // two tools expanded
        assert_eq!(resolved.expanded_tools.len(), 2);
        // names must match in order
        assert_eq!(resolved.expanded_tools[0].name, "get_forecast");
        assert_eq!(resolved.expanded_tools[1].name, "get_current");
        // caps are empty (no caps declared on these tools)
        assert!(resolved.expanded_tools[0].caps.is_empty());
        assert!(resolved.expanded_tools[1].caps.is_empty());
        // no sampling caps → requires_sampling = false
        assert!(!resolved.requires_sampling);
    }

    #[test]
    fn resolved_from_server_contract_empty() {
        let contract = ServerContract {
            protocol_version: "2025-03-26".to_string(),
            server_info: ServerInfo {
                name: "empty".to_string(),
                version: "0.1".to_string(),
                additional: BTreeMap::new(),
            },
            tools: vec![],
        };
        let hash = crate::contract::canonical::canonical_hash(&contract).expect("hash");
        let resolved = resolved_from_server_contract(hash, &contract);
        assert_eq!(resolved.expanded_tools.len(), 0);
        assert!(!resolved.requires_sampling);
    }

    #[test]
    fn cap_kind_string_fs_read() {
        use tau_domain::Capability;
        // Capability::Custom is the only constructable variant from outside tau-domain.
        // For fs.read, deserialize from JSON which routes through the custom Deserialize impl.
        let cap: Capability = serde_json::from_str(r#"{"kind":"fs.read","paths":["/tmp/**"]}"#)
            .expect("deserialize fs.read cap");
        assert_eq!(cap_kind_string(&cap), "fs.read");
    }

    #[test]
    fn cap_kind_string_net_http() {
        use tau_domain::Capability;
        let cap: Capability =
            serde_json::from_str(r#"{"kind":"net.http","hosts":["example.com"],"methods":["GET"]}"#)
                .expect("deserialize net.http cap");
        assert_eq!(cap_kind_string(&cap), "net.http");
    }

    #[test]
    fn cap_kind_string_custom() {
        use alloc::collections::BTreeMap;
        use tau_domain::Capability;
        let cap = Capability::Custom {
            name: "my.custom.cap".to_string(),
            params: BTreeMap::new(),
        };
        assert_eq!(cap_kind_string(&cap), "my.custom.cap");
    }

    #[cfg(feature = "with-std-adapters")]
    mod pinned_resolver_tests {
        use super::*;
        use crate::contract::pinned::PinnedContract;

        fn write_pinned(dir: &std::path::Path, entry: &str, contract: &ServerContract) {
            let pinned =
                PinnedContract::from_parts("https://example.com".to_string(), contract.clone())
                    .expect("build pinned");
            let bytes = serde_json::to_vec(&pinned).expect("serialize");
            std::fs::write(dir.join(alloc::format!("{entry}.contract.json")), &bytes)
                .expect("write");
        }

        #[test]
        fn pinned_resolver_reads_contract() {
            let tmp = std::env::temp_dir().join(alloc::format!(
                "tau-mcp-pinned-test-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&tmp).expect("create tmp dir");
            let contract = two_tool_contract();
            write_pinned(&tmp, "weather", &contract);

            let resolver = PinnedResolver::new(&tmp);
            let resolved = resolver
                .resolve("weather", "https://example.com")
                .expect("resolve");
            assert_eq!(resolved.expanded_tools.len(), 2);
            assert_eq!(resolved.expanded_tools[0].name, "get_forecast");
        }

        #[test]
        fn pinned_resolver_missing_file_error() {
            let tmp = std::env::temp_dir().join("tau-mcp-pinned-missing");
            std::fs::create_dir_all(&tmp).expect("create tmp dir");
            let resolver = PinnedResolver::new(&tmp);
            let err = resolver
                .resolve("nonexistent", "https://example.com")
                .expect_err("should fail");
            assert!(matches!(err, ResolveError::PinnedFileMissing { .. }));
        }

        #[test]
        fn pinned_resolver_tampered_hash_error() {
            let tmp = std::env::temp_dir().join(alloc::format!(
                "tau-mcp-pinned-tamper-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&tmp).expect("create tmp dir");
            let contract = two_tool_contract();

            // Write a valid pinned file, then tamper with the contract field.
            let mut pinned =
                PinnedContract::from_parts("https://example.com".to_string(), contract.clone())
                    .expect("build");
            pinned.contract.tools[0].description = Some("tampered".to_string());
            let bytes = serde_json::to_vec(&pinned).expect("serialize");
            std::fs::write(tmp.join("weather.contract.json"), &bytes).expect("write");

            let resolver = PinnedResolver::new(&tmp);
            let err = resolver
                .resolve("weather", "https://example.com")
                .expect_err("should detect drift");
            assert!(matches!(
                err,
                ResolveError::PinnedFileSelfHashMismatch { .. }
            ));
        }
    }
}
