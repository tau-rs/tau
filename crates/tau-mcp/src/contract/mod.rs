//! Contract types — what `tau build` pins for an MCP server.
//!
//! A [`ServerContract`] captures the server's `tools/list` response +
//! per-tool capability declaration. The canonical-hash module produces a
//! `Hash256` over the canonical-JSON bytes (β.2 canonical-encoder rules
//! re-applied here). Pinned-contract file I/O lives in [`pinned`].
//! The [`resolver`] module provides the [`McpContractResolver`] port trait
//! and the sync [`PinnedResolver`] impl (gated on `with-std-adapters`).

pub mod canonical;
pub mod pinned;
pub mod resolver;
pub mod server_contract;

pub use canonical::{canonical_hash, hash_to_hex, Hash256};
pub use pinned::PinnedContract;
#[cfg(feature = "with-std-adapters")]
pub use resolver::PinnedResolver;
pub use resolver::{
    resolved_from_server_contract, McpContractResolver, ResolveError, ResolvedMcpContract,
    ResolvedServerTool,
};
pub use server_contract::{ContractTool, ServerContract};
// Re-export from upstream protocol modules for ergonomic contract API.
pub use crate::protocol::initialize::ServerInfo;
pub use crate::protocol::tools::McpToolInputSchema;
